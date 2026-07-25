use std::cell::UnsafeCell;
use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam::channel;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use glam::Vec3;

use crate::audio::{AudioBackend, AudioError, AudioHandle, BusType, PlayMode};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_NULL, Decoder, DecoderOptions};
use symphonia::core::formats::{FormatReader, FormatOptions, SeekMode, SeekTo};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

// ── HRTF ────────────────────────────────────────────────────────────────
//
// Reference:  hrtf crate by mrDIMAS (github.com/mrDIMAS/hrtf).
// That crate uses pre-measured HRIR spheres + FFT overlap-save convolution.
// We adapt the same architecture but generate synthetic HRIRs on-the-fly
// using a spherical head model (Duda / Woodworth) with time-domain FIR
// convolution — simpler (no FFT deps) while preserving the per-source
// continuous-convolution pattern.

const HRIR_LEN: usize = 48; // impulse-response length (samples)
const CROSSFADE_FRAMES: usize = 32;

/// Generate a pair of synthetic HRIRs for a given direction.
///
/// `azimuth`   – radians, 0 = front, positive = right, [-π, π]
/// `elevation` – radians, 0 = horizontal, positive = up,   [-π/2, π/2]
fn synth_hrir(
    azimuth: f32,
    elevation: f32,
    sample_rate: u32,
) -> ([f32; HRIR_LEN], [f32; HRIR_LEN]) {
    let sr = sample_rate as f32;
    const R: f32 = 0.09; // head radius (m)
    const C: f32 = 343.0; // speed of sound (m/s)

    // Normalise azimuth to [-π, π]
    let az = azimuth - (2.0 * PI) * (azimuth / (2.0 * PI)).floor();
    let az = if az > PI { az - 2.0 * PI } else { az };

    // Angle from source to each ear.
    // Left ear sits at -90°, right ear at +90° in the horizontal plane.
    let ear_angle = |rel: f32| -> f32 {
        let mut a = rel;
        a = a - (2.0 * PI) * (a / (2.0 * PI)).floor();
        if a > PI { a -= 2.0 * PI };
        a.clamp(-PI / 2.0, PI / 2.0)
    };
    let l_angle = ear_angle(az + PI / 2.0);
    let r_angle = ear_angle(az - PI / 2.0);

    // Woodworth ITD:  Δt(θ) = (R / C) * (θ + sin θ)
    let itd = |angle: f32| -> f32 {
        let a = angle.abs();
        (R / C) * (a + a.sin()) * angle.signum()
    };
    let itd_l = itd(l_angle);
    let itd_r = itd(r_angle);

    // Fractional-sample delays
    let delay_l = itd_l * sr;
    let delay_r = itd_r * sr;

    // ILD / head-shadow gain.
    let gain = |angle: f32| -> f32 { 0.25 + 0.75 * (angle.abs() / (PI / 2.0)).cos().max(0.0) };
    let gain_l = gain(l_angle) * (1.0 - elevation.sin().abs() * 0.15);
    let gain_r = gain(r_angle) * (1.0 - elevation.sin().abs() * 0.15);

    // Head-shadow filter (one-pole low-pass applied offline to the IR).
    // Cutoff depends on how far the source is from the ear axis.
    let head_shadow = |ir: &mut [f32], angle: f32| {
        let shadow_deg = angle.abs().to_degrees();
        let cutoff_hz = 8000.0 - (shadow_deg / 90.0).min(1.0) * 7600.0;
        let dt = 1.0 / sr;
        let rc = 1.0 / (2.0 * PI * cutoff_hz.max(50.0));
        let alpha = dt / (rc + dt);
        let mut y = 0.0f32;
        for x in ir.iter_mut() {
            y += alpha * (*x - y);
            *x = y;
        }
    };

    // Build raw HRIR as a delayed raised-cosine pulse.
    let pulse = |hrir: &mut [f32; HRIR_LEN], delay: f32, ampl: f32| {
        let half = HRIR_LEN as f32 / 2.0;
        let center = half + delay;
        for i in 0..HRIR_LEN {
            let n = i as f32;
            let d = (n - center).abs();
            if d < half {
                let t = d / half;
                hrir[i] = ampl * 0.5 * (1.0 + (t * PI).cos());
            }
        }
    };

    let mut hrir_l = [0.0f32; HRIR_LEN];
    let mut hrir_r = [0.0f32; HRIR_LEN];
    pulse(&mut hrir_l, delay_l, gain_l);
    pulse(&mut hrir_r, delay_r, gain_r);

    head_shadow(&mut hrir_l, l_angle);
    head_shadow(&mut hrir_r, r_angle);

    let energy: f32 = hrir_l.iter().map(|x| x * x).sum::<f32>()
        + hrir_r.iter().map(|x| x * x).sum::<f32>();
    if energy > 0.0 {
        let scale = 1.0 / (energy / 2.0).sqrt();
        for i in 0..HRIR_LEN {
            hrir_l[i] *= scale * 0.5;
            hrir_r[i] *= scale * 0.5;
        }
    }

    (hrir_l, hrir_r)
}

/// Per-source HRTF convolution engine using direct FIR + ring buffer.
///
/// Follows the same continuous-convolution design as [HrtfProcessor] in the
/// reference crate but operates in the time domain (no FFT dependency) and
/// generates its impulse responses synthetically instead of relying on
/// pre-measured HRIR spheres.
struct HrtfProcessor {
    left_ir: [f32; HRIR_LEN],
    right_ir: [f32; HRIR_LEN],
    delay_line: Vec<f32>,
    cursor: usize,

    // Crossfade state for click-free IR transitions
    fade_left: Option<[f32; HRIR_LEN]>,
    fade_right: Option<[f32; HRIR_LEN]>,
    fade_counter: usize,
}

impl HrtfProcessor {
    fn new(sample_rate: u32) -> Self {
        let (l, r) = synth_hrir(0.0, 0.0, sample_rate);
        Self {
            left_ir: l,
            right_ir: r,
            delay_line: vec![0.0; HRIR_LEN],
            cursor: 0,
            fade_left: None,
            fade_right: None,
            fade_counter: 0,
        }
    }

    fn set_ir(&mut self, new_l: [f32; HRIR_LEN], new_r: [f32; HRIR_LEN]) {
        self.fade_left = Some(self.left_ir);
        self.fade_right = Some(self.right_ir);
        self.left_ir = new_l;
        self.right_ir = new_r;
        self.fade_counter = CROSSFADE_FRAMES;
    }

    fn process(&mut self, input: f32) -> (f32, f32) {
        self.delay_line[self.cursor] = input;
        let read = self.cursor + 1;

        let mut out_l = 0.0f32;
        let mut out_r = 0.0f32;
        for i in 0..HRIR_LEN {
            let idx = (read + i) % HRIR_LEN;
            let s = self.delay_line[idx];
            out_l += s * self.left_ir[i];
            out_r += s * self.right_ir[i];
        }

        if let (Some(ref fade_l), Some(ref fade_r)) = (self.fade_left, self.fade_right) {
            let mut fl = 0.0;
            let mut fr = 0.0;
            for i in 0..HRIR_LEN {
                let idx = (read + i) % HRIR_LEN;
                let s = self.delay_line[idx];
                fl += s * fade_l[i];
                fr += s * fade_r[i];
            }
            let t = self.fade_counter as f32 / CROSSFADE_FRAMES as f32;
            out_l = out_l * (1.0 - t) + fl * t;
            out_r = out_r * (1.0 - t) + fr * t;
            self.fade_counter -= 1;
            if self.fade_counter == 0 {
                self.fade_left = None;
                self.fade_right = None;
            }
        }

        self.cursor = read % HRIR_LEN;
        (out_l, out_r)
    }
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

struct AudioBuffer {
    samples: Vec<f32>,
    channels: u16,
    sample_rate: u32,
}

struct AudioClip {
    buffer: Arc<AudioBuffer>,
}

// ---------------------------------------------------------------------------
// Spatial audio types
// ---------------------------------------------------------------------------

struct ListenerParams {
    position: Vec3,
    forward: Vec3,
    up: Vec3,
    velocity: Vec3,
}

#[derive(Clone)]
struct SpatialParams {
    position: Vec3,
    velocity: Vec3,
}

struct SpatialState {
    params: SpatialParams,
    volume_attenuation: f32,
}

// ---------------------------------------------------------------------------
// Source
// ---------------------------------------------------------------------------

struct Source {
    handle: AudioHandle,
    bus: BusType,
    buffer: Option<Arc<AudioBuffer>>,
    read_position: f64,
    streaming: Option<StreamingDecoder>,
    volume: f32,
    pan: f32,
    looping: bool,
    active: bool,
    spatial: Option<SpatialState>,
    paused: bool,
    doppler_pitch: f32,
    hrtf: Option<HrtfProcessor>,
}

impl Source {
    fn next_sample(&mut self, pitch: f32) -> (f32, f32) {
        if let Some(ref mut sd) = self.streaming {
            if sd.eof {
                if self.looping {
                    sd.seek_to_start();
                } else {
                    self.active = false;
                    return (0.0, 0.0);
                }
            }
            return sd.read_frame(pitch);
        }

        let buffer = match &self.buffer {
            Some(b) => b,
            None => return (0.0, 0.0),
        };

        let frame_count = (buffer.samples.len() / buffer.channels as usize) as f64;
        if self.read_position >= frame_count {
            if self.looping {
                self.read_position = 0.0;
            } else {
                self.active = false;
                return (0.0, 0.0);
            }
        }

        let ch = buffer.channels as usize;
        let frame_pos = self.read_position;
        let i = (frame_pos as usize) * ch;
        let next_i = ((frame_pos as usize + 1).min(buffer.samples.len() / ch - 1)) * ch;
        let frac = frame_pos.fract() as f32;

        let (l, r) = if buffer.channels == 1 {
            let s = buffer.samples[i] * (1.0 - frac) + buffer.samples[next_i] * frac;
            (s, s)
        } else {
            let l = buffer.samples[i] * (1.0 - frac) + buffer.samples[next_i] * frac;
            let r = buffer.samples[i + 1] * (1.0 - frac) + buffer.samples[next_i + 1] * frac;
            (l, r)
        };

        self.read_position += pitch as f64;
        (l, r)
    }

    fn mix(&mut self, output: &mut [f32]) {
        if !self.active || self.paused || (self.buffer.is_none() && self.streaming.is_none()) {
            return;
        }
        let frames = output.len() / 2;
        let pitch = self.doppler_pitch;
        let spatial_vol = self.spatial.as_ref().map_or(1.0, |s| s.volume_attenuation);
        let effective_vol = self.volume * spatial_vol;

        // Temporarily move hrtf out to avoid borrow conflicts with next_sample
        let mut hrtf = self.hrtf.take();

        if let Some(ref mut h) = hrtf {
            for frame in 0..frames {
                let (left, right) = self.next_sample(pitch);
                if !self.active {
                    break;
                }
                let mono = (left + right) * 0.5;
                let (out_l, out_r) = h.process(mono);
                let out_idx = frame * 2;
                output[out_idx + 0] += out_l * effective_vol;
                output[out_idx + 1] += out_r * effective_vol;
            }
        } else {
            for frame in 0..frames {
                let (left, right) = self.next_sample(pitch);
                if !self.active {
                    break;
                }
                let out_idx = frame * 2;
                output[out_idx + 0] += left * effective_vol * (1.0 - self.pan.max(0.0));
                output[out_idx + 1] += right * effective_vol * (1.0 + self.pan.min(0.0));
            }
        }

        self.hrtf = hrtf;
    }
}

// ---------------------------------------------------------------------------
// Clamp mode
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
#[allow(dead_code)]
enum ClampMode {
    Hard,
    Tanh,
}

// ---------------------------------------------------------------------------
// Streaming decoder
// ---------------------------------------------------------------------------

struct StreamingDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track_id: u32,
    src_sample_rate: u32,
    dst_sample_rate: u32,
    channels: u16,
    buf: Vec<f32>,
    buf_pos: f64,
    eof: bool,
}

impl StreamingDecoder {
    fn new(path: &str, dst_sample_rate: u32) -> Result<Self, AudioError> {
        let file = std::fs::File::open(path)
            .map_err(|e| AudioError::FileNotFound(format!("{}: {}", path, e)))?;

        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("");
        let mut hint = Hint::new();
        hint.with_extension(ext);

        let format_opts = FormatOptions::default();
        let metadata_opts = MetadataOptions::default();

        let probed = symphonia::default::get_probe()
            .format(&hint, mss, &format_opts, &metadata_opts)
            .map_err(|e| AudioError::DecodeFailed(format!("{}: {}", path, e)))?;

        let format = probed.format;
        let codecs = symphonia::default::get_codecs();

        let track = format
            .tracks()
            .iter()
            .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
            .ok_or_else(|| AudioError::DecodeFailed(format!("{}: no audio track", path)))?;

        let codec_params = track.codec_params.clone();
        let track_id = track.id;

        let decoder = codecs
            .make(&codec_params, &DecoderOptions::default())
            .map_err(|e| AudioError::DecodeFailed(format!("{}: {}", path, e)))?;

        let src_sample_rate = codec_params.sample_rate.unwrap_or(44100);
        let channels = codec_params
            .channels
            .map(|c| c.count() as u16)
            .unwrap_or(2);

        Ok(Self {
            format,
            decoder,
            track_id,
            src_sample_rate,
            dst_sample_rate,
            channels,
            buf: Vec::new(),
            buf_pos: 0.0,
            eof: false,
        })
    }

    fn fill_buffer(&mut self) {
        let target_output_frames = 4096;
        let ratio = self.src_sample_rate as f64 / self.dst_sample_rate as f64;
        let target_input_frames =
            (target_output_frames as f64 * ratio).ceil() as usize + 1;

        let mut raw: Vec<f32> = Vec::new();
        while raw.len() / (self.channels as usize) < target_input_frames {
            if self.eof {
                break;
            }
            match self.format.next_packet() {
                Ok(packet) => {
                    if packet.track_id() != self.track_id {
                        continue;
                    }
                    match self.decoder.decode(&packet) {
                        Ok(audio_buf) => {
                            let spec = *audio_buf.spec();
                            let num_frames = audio_buf.frames();
                            if num_frames == 0 {
                                continue;
                            }
                            let mut sb =
                                SampleBuffer::<f32>::new(audio_buf.frames() as u64, spec);
                            sb.copy_interleaved_ref(audio_buf);
                            raw.extend_from_slice(sb.samples());
                        }
                        Err(_) => continue,
                    }
                }
                Err(symphonia::core::errors::Error::IoError(_)) => {
                    self.eof = true;
                    break;
                }
                Err(_) => {
                    self.eof = true;
                    break;
                }
            }
        }

        if raw.is_empty() {
            return;
        }

        self.buf = if self.src_sample_rate != self.dst_sample_rate {
            let ch = self.channels as usize;
            let raw_frames = raw.len() / ch;
            if raw_frames == 0 {
                raw
            } else {
                let ratio = self.src_sample_rate as f64 / self.dst_sample_rate as f64;
                let dst_frames = (raw_frames as f64 / ratio).ceil() as usize;
                let mut out = Vec::with_capacity(dst_frames * ch);
                let mut src_frame_pos = 0.0f64;
                while (src_frame_pos as usize) < raw_frames.saturating_sub(1) {
                    let i = src_frame_pos as usize;
                    let frac = src_frame_pos.fract() as f32;
                    let base = i * ch;
                    let next = (i + 1) * ch;
                    for c in 0..ch {
                        let s = raw[base + c] * (1.0 - frac) + raw[next + c] * frac;
                        out.push(s);
                    }
                    src_frame_pos += ratio;
                }
                out
            }
        } else {
            raw
        };
        self.buf_pos = 0.0;
    }

    fn read_frame(&mut self, pitch: f32) -> (f32, f32) {
        let ch = self.channels as usize;
        let frames = self.buf.len() / ch;
        if self.buf_pos as usize >= frames {
            if self.eof {
                return (0.0, 0.0);
            }
            self.fill_buffer();
            if self.buf.is_empty() {
                self.eof = true;
                return (0.0, 0.0);
            }
        }

        let frame_pos = self.buf_pos;
        let i = (frame_pos as usize) * ch;
        let next = ((frame_pos as usize + 1).min(self.buf.len() / ch - 1)) * ch;
        let frac = frame_pos.fract() as f32;

        let (l, r) = if self.channels == 1 {
            let s = self.buf[i] * (1.0 - frac) + self.buf[next] * frac;
            (s, s)
        } else {
            let l = self.buf[i] * (1.0 - frac) + self.buf[next] * frac;
            let r = self.buf[i + 1] * (1.0 - frac) + self.buf[next + 1] * frac;
            (l, r)
        };

        self.buf_pos += pitch as f64;
        (l, r)
    }

    fn seek_to_start(&mut self) {
        let _ = self
            .format
            .seek(SeekMode::Accurate, SeekTo::TimeStamp { ts: 0, track_id: self.track_id });
        self.decoder.reset();
        self.buf.clear();
        self.buf_pos = 0.0;
        self.eof = false;
    }
}

// ---------------------------------------------------------------------------
// Bus
// ---------------------------------------------------------------------------

struct Bus {
    volume: f32,
    gain: f32,
}

impl Bus {
    fn new() -> Self {
        Self {
            volume: 1.0,
            gain: 1.0,
        }
    }
}

// ---------------------------------------------------------------------------
// FDN Reverb
// ---------------------------------------------------------------------------

struct DelayLine {
    buffer: Vec<f32>,
    pos: usize,
}

impl DelayLine {
    fn new(length: usize) -> Self {
        Self {
            buffer: vec![0.0; length],
            pos: 0,
        }
    }

    fn read(&self) -> f32 {
        self.buffer[self.pos]
    }

    fn write(&mut self, sample: f32) {
        self.buffer[self.pos] = sample;
        self.pos = (self.pos + 1) % self.buffer.len();
    }
}

struct FdnReverb {
    delays: [DelayLine; 4],
    damping: f32,
    feedback: f32,
    lp_state: [f32; 4],
    wet: f32,
    dry: f32,
}

impl FdnReverb {
    fn new(sample_rate: u32) -> Self {
        let sr = sample_rate as f32;
        let delay_samples = |ms: f32| (ms * sr / 1000.0).round() as usize;
        Self {
            delays: [
                DelayLine::new(delay_samples(31.0)),
                DelayLine::new(delay_samples(37.0)),
                DelayLine::new(delay_samples(43.0)),
                DelayLine::new(delay_samples(53.0)),
            ],
            damping: 0.2,
            feedback: 0.8,
            lp_state: [0.0; 4],
            wet: 0.0,
            dry: 1.0,
        }
    }

    fn process(&mut self, input_l: f32, input_r: f32) -> (f32, f32) {
        let input = (input_l + input_r) * 0.5;

        let mut taps = [0.0f32; 4];
        for i in 0..4 {
            taps[i] = self.delays[i].read();
        }

        for i in 0..4 {
            taps[i] = taps[i] * (1.0 - self.damping) + self.lp_state[i] * self.damping;
            self.lp_state[i] = taps[i];
        }

        let fba = self.feedback * 0.5;
        let fb = [
            ( taps[0] + taps[1] + taps[2] + taps[3]) * fba,
            ( taps[0] - taps[1] + taps[2] - taps[3]) * fba,
            ( taps[0] + taps[1] - taps[2] - taps[3]) * fba,
            ( taps[0] - taps[1] - taps[2] + taps[3]) * fba,
        ];

        for i in 0..4 {
            self.delays[i].write(input + fb[i]);
        }

        let wet_l = taps[0] + taps[2];
        let wet_r = taps[1] + taps[3];
        let wet_scale = 0.3;
        (
            input * self.dry + wet_l * self.wet * wet_scale,
            input * self.dry + wet_r * self.wet * wet_scale,
        )
    }
}

// ---------------------------------------------------------------------------
// Mixer with slot map
// ---------------------------------------------------------------------------

struct Mixer {
    sources: Vec<Option<Source>>,
    generations: Vec<u32>,
    free_slots: Vec<usize>,
    master_volume: f32,
    clamp_mode: ClampMode,
    buses: [Bus; 3],
    bus_scratch: Vec<f32>,
    reverb: FdnReverb,
    sample_rate: u32,
}

impl Mixer {
    fn new(sample_rate: u32) -> Self {
        Self {
            sources: Vec::new(),
            generations: Vec::new(),
            free_slots: Vec::new(),
            master_volume: 1.0,
            clamp_mode: ClampMode::Hard,
            buses: [Bus::new(), Bus::new(), Bus::new()],
            bus_scratch: Vec::new(),
            reverb: FdnReverb::new(sample_rate),
            sample_rate,
        }
    }

    fn add_source(&mut self, source: Source) -> AudioHandle {
        let handle = source.handle;
        if let Some(slot) = self.free_slots.pop() {
            self.generations[slot] += 1;
            self.sources[slot] = Some(source);
        } else {
            self.generations.push(1);
            self.sources.push(Some(source));
        }
        handle
    }

    fn remove_source(&mut self, handle: AudioHandle) {
        for (i, slot) in self.sources.iter_mut().enumerate() {
            if let Some(source) = slot {
                if source.handle == handle {
                    *slot = None;
                    self.generations[i] += 1;
                    self.free_slots.push(i);
                    return;
                }
            }
        }
    }

    fn get_mut(&mut self, handle: AudioHandle) -> Option<&mut Source> {
        for slot in self.sources.iter_mut() {
            if let Some(source) = slot {
                if source.handle == handle {
                    return Some(source);
                }
            }
        }
        None
    }

    fn cleanup_dead_sources(&mut self) {
        for (i, slot) in self.sources.iter_mut().enumerate() {
            if let Some(source) = slot {
                if !source.active && !source.looping {
                    *slot = None;
                    self.generations[i] += 1;
                    self.free_slots.push(i);
                }
            }
        }
    }

    fn mix(&mut self, output: &mut [f32]) {
        for sample in output.iter_mut() {
            *sample = 0.0;
        }

        self.cleanup_dead_sources();

        let bus_types = [BusType::Sfx, BusType::Music, BusType::Voice];
        self.bus_scratch.resize(output.len(), 0.0);
        let mut any_active = false;

        for bus_type in &bus_types {
            for s in self.bus_scratch.iter_mut() {
                *s = 0.0;
            }

            let mut bus_active = false;
            for slot in &mut self.sources {
                if let Some(source) = slot {
                    if source.active && source.bus == *bus_type {
                        bus_active = true;
                        source.mix(&mut self.bus_scratch);
                    }
                }
            }

            if bus_active {
                any_active = true;
                let idx = *bus_type as usize;
                let bus_gain = self.buses[idx].gain;
                let bus_vol = self.buses[idx].volume;
                for s in self.bus_scratch.iter_mut() {
                    *s *= bus_gain;
                }
                for i in 0..output.len() {
                    output[i] += self.bus_scratch[i] * bus_vol;
                }
            }
        }

        if self.reverb.wet > 0.0 {
            for frame in 0..output.len() / 2 {
                let (l, r) = self.reverb.process(output[frame * 2], output[frame * 2 + 1]);
                output[frame * 2] = l;
                output[frame * 2 + 1] = r;
            }
        }

        if any_active {
            for sample in output.iter_mut() {
                if !sample.is_finite() {
                    *sample = 0.0;
                }
                *sample = match self.clamp_mode {
                    ClampMode::Hard => sample.clamp(-1.0, 1.0),
                    ClampMode::Tanh => sample.tanh(),
                };
            }
        }
        for sample in output.iter_mut() {
            *sample *= self.master_volume;
        }
    }

    fn clear(&mut self) {
        for (i, slot) in self.sources.iter_mut().enumerate() {
            if slot.is_some() {
                *slot = None;
                self.generations[i] += 1;
                self.free_slots.push(i);
            }
        }
    }

    fn doppler_factor(v_source: f32, v_listener: f32) -> f32 {
        const SPEED_OF_SOUND: f32 = 343.0;
        let factor = (SPEED_OF_SOUND + v_listener) / (SPEED_OF_SOUND + v_source);
        factor.clamp(0.5, 2.0)
    }

    fn update_spatial(&mut self, listener: &ListenerParams) {
        for slot in &mut self.sources {
            if let Some(source) = slot {
                if let Some(spatial) = &mut source.spatial {
                    let direction = spatial.params.position - listener.position;
                    let distance = direction.length();
                    let effective_distance = distance.max(0.1);
                    spatial.volume_attenuation = 1.0 / (1.0 + effective_distance);

                    if distance > 0.001 {
                        let dir_norm = direction / distance;
                        let angle = (listener.forward.cross(dir_norm).y)
                            .atan2(listener.forward.dot(dir_norm));

                        // Update pan (still needed for fallback path)
                        source.pan = angle.sin();

                        // Update HRIR if the source has an HRTF processor
                        if let Some(ref mut hrtf) = source.hrtf {
                            // Azimuth in radians, 0 = front, positive = right
                            let azimuth = angle;
                            // Elevation: angle above the listener's forward-right plane
                            let forward = listener.forward;
                            let up = listener.up;
                            let right = forward.cross(up).normalize();
                            let local_up = right.cross(forward).normalize();
                            let elevation = (dir_norm.dot(local_up))
                                .asin()
                                .clamp(-PI / 2.0, PI / 2.0);

                            let (l, r) = synth_hrir(azimuth, elevation, self.sample_rate);
                            hrtf.set_ir(l, r);
                        }

                        let v_source = spatial.params.velocity.dot(dir_norm);
                        let v_listener = -listener.velocity.dot(dir_norm);
                        source.doppler_pitch =
                            Self::doppler_factor(v_source, v_listener);
                    } else {
                        source.pan = 0.0;
                        source.doppler_pitch = 1.0;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Lock-free SPSC ring buffer
// ---------------------------------------------------------------------------

struct AudioRingBuffer {
    buffer: UnsafeCell<Vec<f32>>,
    capacity: usize,
    write_pos: AtomicU64,
    read_pos: AtomicU64,
}

unsafe impl Sync for AudioRingBuffer {}

impl AudioRingBuffer {
    fn new(capacity_frames: usize) -> Self {
        let len = capacity_frames.next_power_of_two() * 2;
        Self {
            buffer: UnsafeCell::new(vec![0.0; len]),
            capacity: len,
            write_pos: AtomicU64::new(0),
            read_pos: AtomicU64::new(0),
        }
    }

    fn write_available(&self) -> usize {
        let write = self.write_pos.load(Ordering::Relaxed) as usize;
        let read = self.read_pos.load(Ordering::Acquire) as usize;
        self.capacity - (write - read)
    }

    fn write_frames(&self, data: &[f32]) -> usize {
        let write = self.write_pos.load(Ordering::Relaxed) as usize;
        let read = self.read_pos.load(Ordering::Acquire) as usize;
        let avail = self.capacity - (write - read);
        let to_write = data.len().min(avail);

        let buf = unsafe { &mut *self.buffer.get() };
        let mask = self.capacity - 1;
        for i in 0..to_write {
            buf[(write + i) & mask] = data[i];
        }
        self.write_pos
            .store((write + to_write) as u64, Ordering::Release);
        to_write
    }

    fn read_frames(&self, output: &mut [f32]) -> usize {
        let write = self.write_pos.load(Ordering::Acquire) as usize;
        let read = self.read_pos.load(Ordering::Relaxed) as usize;
        let avail = (write - read).min(output.len());

        let buf = unsafe { &*self.buffer.get() };
        let mask = self.capacity - 1;
        for i in 0..avail {
            output[i] = buf[(read + i) & mask];
        }
        for i in avail..output.len() {
            output[i] = 0.0;
        }
        self.read_pos
            .store((read + avail) as u64, Ordering::Release);
        avail
    }
}

// ---------------------------------------------------------------------------
// Clip cache & audio loader
// ---------------------------------------------------------------------------

struct ClipCache {
    cache: HashMap<String, Arc<AudioBuffer>>,
}

impl ClipCache {
    fn new() -> Self {
        Self { cache: HashMap::new() }
    }

    fn get_or_load(&mut self, path: &str) -> Result<Arc<AudioBuffer>, AudioError> {
        if let Some(buf) = self.cache.get(path) {
            return Ok(Arc::clone(buf));
        }
        let clip = load_audio(path).map_err(|e| {
            log::error!("[rubi] failed to load audio clip '{}': {}", path, e);
            e
        })?;
        let buf = Arc::clone(&clip.buffer);
        self.cache.insert(path.to_string(), Arc::clone(&buf));
        Ok(buf)
    }
}

fn load_audio(path: &str) -> Result<AudioClip, AudioError> {
    let lower = path.to_lowercase();
    if lower.ends_with(".wav") {
        load_wav(path)
    } else {
        load_symphonia(path)
    }
}

fn load_wav(path: &str) -> Result<AudioClip, AudioError> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| AudioError::FileNotFound(format!("{}: {}", path, e)))?;
    let spec = reader.spec();
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / i16::MAX as f32)
            .collect(),
    };
    if samples.is_empty() {
        return Err(AudioError::DecodeFailed(format!("{}: no samples", path)));
    }
    Ok(AudioClip {
        buffer: Arc::new(AudioBuffer {
            samples,
            channels: spec.channels,
            sample_rate: spec.sample_rate,
        }),
    })
}

fn load_symphonia(path: &str) -> Result<AudioClip, AudioError> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::CODEC_TYPE_NULL;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path)
        .map_err(|e| AudioError::FileNotFound(format!("{}: {}", path, e)))?;

    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let ext = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let mut hint = Hint::new();
    hint.with_extension(ext);

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| AudioError::DecodeFailed(format!("{}: {}", path, e)))?;

    let mut format = probed.format;
    let codecs = symphonia::default::get_codecs();

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| AudioError::DecodeFailed(format!("{}: no audio track", path)))?;

    let codec_params = track.codec_params.clone();
    let track_id = track.id;

    let mut decoder = codecs
        .make(&codec_params, &DecoderOptions::default())
        .map_err(|e| AudioError::DecodeFailed(format!("{}: {}", path, e)))?;

    let sample_rate = codec_params.sample_rate.unwrap_or(44100);
    let channels = codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(2);

    let mut all_samples: Vec<f32> = Vec::new();

    loop {
        match format.next_packet() {
            Ok(packet) => {
                if packet.track_id() != track_id {
                    continue;
                }
                match decoder.decode(&packet) {
                    Ok(audio_buf) => {
                        let spec = *audio_buf.spec();
                        let num_frames = audio_buf.frames();
                        if num_frames == 0 {
                            continue;
                        }
                        let mut sample_buf =
                            SampleBuffer::<f32>::new(audio_buf.frames() as u64, spec);
                        sample_buf.copy_interleaved_ref(audio_buf);
                        all_samples.extend_from_slice(sample_buf.samples());
                    }
                    Err(_) => continue,
                }
            }
            Err(symphonia::core::errors::Error::IoError(_)) => break,
            Err(_) => break,
        }
    }

    if all_samples.is_empty() {
        return Err(AudioError::DecodeFailed(format!("{}: no samples decoded", path)));
    }

    Ok(AudioClip {
        buffer: Arc::new(AudioBuffer {
            samples: all_samples,
            channels,
            sample_rate,
        }),
    })
}

fn resample(source: &[f32], channels: u16, src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src_rate == dst_rate {
        return source.to_vec();
    }
    let ch = channels as usize;
    let frames = source.len() / ch;
    let ratio = src_rate as f64 / dst_rate as f64;
    let dst_frames = (frames as f64 / ratio).ceil() as usize;
    let mut out = Vec::with_capacity(dst_frames * ch);
    let mut src_frame_pos = 0.0f64;
    while (src_frame_pos as usize) < frames.saturating_sub(1) {
        let i = src_frame_pos as usize;
        let frac = src_frame_pos.fract() as f32;
        let base = i * ch;
        let next = base + ch;
        for c in 0..ch {
            let sample = source[base + c] * (1.0 - frac) + source[next + c] * frac;
            out.push(sample);
        }
        src_frame_pos += ratio;
    }
    out
}

// ---------------------------------------------------------------------------
// Command queue
// ---------------------------------------------------------------------------

#[allow(dead_code)]
enum AudioCommand {
    Play {
        path: String,
        volume: f32,
        looping: bool,
        spatial: Option<SpatialParams>,
        handle: AudioHandle,
        bus: BusType,
        mode: PlayMode,
    },
    Stop(AudioHandle),
    SetVolume(AudioHandle, f32),
    SetLooping(AudioHandle, bool),
    SetSpatial(AudioHandle, SpatialParams),
    SetListener(ListenerParams),
    Pause(AudioHandle),
    Resume(AudioHandle),
    StopAll,
    SetBusVolume(BusType, f32),
    SetBusGain(BusType, f32),
    SetReverb { wet: f32, dry: f32, feedback: f32, damping: f32 },
    Shutdown,
}

// ---------------------------------------------------------------------------
// Audio thread
// ---------------------------------------------------------------------------

fn process_command(
    cmd: AudioCommand,
    mixer: &mut Mixer,
    clip_cache: &mut ClipCache,
    sample_rate: u32,
    last_listener: &mut Option<ListenerParams>,
) {
    match cmd {
        AudioCommand::Play { path, volume, looping, spatial, handle, bus, mode } => {
            let (buffer, streaming_decoder) = if mode == PlayMode::Streaming {
                (None, StreamingDecoder::new(&path, sample_rate).ok())
            } else {
                let buf = match clip_cache.get_or_load(&path) {
                    Ok(buf) => {
                        if buf.sample_rate != sample_rate {
                            let resampled =
                                resample(&buf.samples, buf.channels, buf.sample_rate, sample_rate);
                            Some(Arc::new(AudioBuffer {
                                samples: resampled,
                                channels: buf.channels,
                                sample_rate,
                            }))
                        } else {
                            Some(buf)
                        }
                    }
                    Err(_) => None,
                };
                (buf, None)
            };
            let hrtf = if spatial.is_some() {
                Some(HrtfProcessor::new(sample_rate))
            } else {
                None
            };
            mixer.add_source(Source {
                handle,
                bus,
                buffer,
                read_position: 0.0,
                streaming: streaming_decoder,
                volume,
                pan: 0.0,
                looping,
                active: true,
                doppler_pitch: 1.0,
                spatial: spatial.map(|p| SpatialState {
                    params: p,
                    volume_attenuation: 1.0,
                }),
                paused: false,
                hrtf,
            });
            if let Some(ref listener) = *last_listener {
                mixer.update_spatial(listener);
            }
        }
        AudioCommand::Stop(handle) => {
            mixer.remove_source(handle);
        }
        AudioCommand::SetVolume(handle, vol) => {
            if let Some(s) = mixer.get_mut(handle) {
                s.volume = vol;
            }
        }
        AudioCommand::SetLooping(handle, looping) => {
            if let Some(s) = mixer.get_mut(handle) {
                s.looping = looping;
            }
        }
        AudioCommand::SetSpatial(handle, params) => {
            if let Some(s) = mixer.get_mut(handle) {
                s.spatial = Some(SpatialState {
                    params,
                    volume_attenuation: 1.0,
                });
            }
        }
        AudioCommand::SetListener(params) => {
            *last_listener = Some(ListenerParams {
                position: params.position,
                forward: params.forward,
                up: params.up,
                velocity: params.velocity,
            });
            mixer.update_spatial(last_listener.as_ref().unwrap());
        }
        AudioCommand::Pause(handle) => {
            if let Some(s) = mixer.get_mut(handle) {
                s.paused = true;
            }
        }
        AudioCommand::Resume(handle) => {
            if let Some(s) = mixer.get_mut(handle) {
                s.paused = false;
            }
        }
        AudioCommand::StopAll => {
            mixer.clear();
        }
        AudioCommand::SetBusVolume(bus, vol) => {
            let idx = bus as usize;
            mixer.buses[idx].volume = vol;
        }
        AudioCommand::SetBusGain(bus, gain) => {
            let idx = bus as usize;
            mixer.buses[idx].gain = gain;
        }
        AudioCommand::SetReverb { wet, dry, feedback, damping } => {
            mixer.reverb.wet = wet;
            mixer.reverb.dry = dry;
            mixer.reverb.feedback = feedback;
            mixer.reverb.damping = damping;
        }
        AudioCommand::Shutdown => {}
    }
}

fn audio_thread_main(command_rx: channel::Receiver<AudioCommand>) {
    let host = cpal::default_host();
    let device = match host.default_output_device() {
        Some(d) => d,
        None => {
            log::error!("[rubi] no audio output device available");
            for cmd in command_rx {
                if matches!(cmd, AudioCommand::Shutdown) {
                    break;
                }
            }
            return;
        }
    };
    let config = match device.default_output_config() {
        Ok(c) => c,
        Err(e) => {
            log::error!("[rubi] failed to get default audio config: {}", e);
            for cmd in command_rx {
                if matches!(cmd, AudioCommand::Shutdown) {
                    break;
                }
            }
            return;
        }
    };

    let sample_rate = config.sample_rate().0;
    let channels = config.channels();
    let stream_config = cpal::StreamConfig {
        channels,
        sample_rate: cpal::SampleRate(sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let ring = Arc::new(AudioRingBuffer::new(4096));
    let ring_cb = Arc::clone(&ring);

    let _stream = match device.build_output_stream(
        &stream_config,
        move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
            ring_cb.read_frames(data);
        },
        |err| log::error!("[rubi] audio stream error: {}", err),
        None,
    ) {
        Ok(s) => {
            if let Err(e) = s.play() {
                log::error!("[rubi] failed to start audio stream: {}", e);
                for cmd in command_rx {
                    if matches!(cmd, AudioCommand::Shutdown) {
                        break;
                    }
                }
                return;
            }
            s
        }
        Err(e) => {
            log::error!("[rubi] failed to build audio stream: {}", e);
            for cmd in command_rx {
                if matches!(cmd, AudioCommand::Shutdown) {
                    break;
                }
            }
            return;
        }
    };

    let mut mixer = Mixer::new(sample_rate);
    let mut clip_cache = ClipCache::new();
    let mut last_listener: Option<ListenerParams> = None;
    let mut mix_buf = Vec::with_capacity(1024);

    loop {
        // Mix into ring buffer if space is available
        let mix_frames = ring.write_available() / 2;
        if mix_frames >= 256 {
            let samples = (mix_frames.min(512) * 2) as usize;
            mix_buf.resize(samples, 0.0);
            mix_buf.fill(0.0);
            mixer.mix(&mut mix_buf);
            ring.write_frames(&mix_buf);
        }

        // Wait for next command (up to 2ms)
        match command_rx.recv_timeout(Duration::from_millis(2)) {
            Ok(AudioCommand::Shutdown) | Err(channel::RecvTimeoutError::Disconnected) => return,
            Ok(cmd) => {
                process_command(cmd, &mut mixer, &mut clip_cache, sample_rate, &mut last_listener);
                while let Ok(cmd) = command_rx.try_recv() {
                    if matches!(cmd, AudioCommand::Shutdown) {
                        return;
                    }
                    process_command(
                        cmd,
                        &mut mixer,
                        &mut clip_cache,
                        sample_rate,
                        &mut last_listener,
                    );
                }
            }
            Err(channel::RecvTimeoutError::Timeout) => {}
        }
    }
}

// ---------------------------------------------------------------------------
// RubiAudio (public API, game-facing)
// ---------------------------------------------------------------------------

pub struct RubiAudio {
    command_tx: channel::Sender<AudioCommand>,
    handle_counter: u64,
    _audio_thread: Option<thread::JoinHandle<()>>,
}

impl RubiAudio {
    pub fn new() -> Self {
        let (command_tx, command_rx) = channel::unbounded::<AudioCommand>();

        let handle = thread::Builder::new()
            .name("rubi-audio".into())
            .spawn(move || {
                audio_thread_main(command_rx);
            })
            .expect("failed to spawn rubi audio thread");

        Self {
            command_tx,
            handle_counter: 0,
            _audio_thread: Some(handle),
        }
    }
}

impl Drop for RubiAudio {
    fn drop(&mut self) {
        let _ = self.command_tx.send(AudioCommand::Shutdown);
    }
}

impl AudioBackend for RubiAudio {
    fn play(&mut self, path: &str, volume: f32, looping: bool) -> AudioHandle {
        self.handle_counter += 1;
        let handle = AudioHandle { index: self.handle_counter as u32, generation: 0 };
        let cmd = AudioCommand::Play {
            path: path.to_string(),
            volume,
            looping,
            spatial: None,
            handle,
            bus: BusType::Sfx,
            mode: PlayMode::Buffered,
        };
        let _ = self.command_tx.send(cmd);
        handle
    }

    fn play_on_bus(&mut self, path: &str, volume: f32, looping: bool, bus: BusType) -> AudioHandle {
        self.handle_counter += 1;
        let handle = AudioHandle { index: self.handle_counter as u32, generation: 0 };
        let cmd = AudioCommand::Play {
            path: path.to_string(),
            volume,
            looping,
            spatial: None,
            handle,
            bus,
            mode: PlayMode::Buffered,
        };
        let _ = self.command_tx.send(cmd);
        handle
    }

    fn play_streaming(&mut self, path: &str, volume: f32, looping: bool) -> AudioHandle {
        self.handle_counter += 1;
        let handle = AudioHandle { index: self.handle_counter as u32, generation: 0 };
        let cmd = AudioCommand::Play {
            path: path.to_string(),
            volume,
            looping,
            spatial: None,
            handle,
            bus: BusType::Music,
            mode: PlayMode::Streaming,
        };
        let _ = self.command_tx.send(cmd);
        handle
    }

    fn play_streaming_on_bus(&mut self, path: &str, volume: f32, looping: bool, bus: BusType) -> AudioHandle {
        self.handle_counter += 1;
        let handle = AudioHandle { index: self.handle_counter as u32, generation: 0 };
        let cmd = AudioCommand::Play {
            path: path.to_string(),
            volume,
            looping,
            spatial: None,
            handle,
            bus,
            mode: PlayMode::Streaming,
        };
        let _ = self.command_tx.send(cmd);
        handle
    }

    fn stop(&mut self, handle: AudioHandle) {
        let _ = self.command_tx.send(AudioCommand::Stop(handle));
    }

    fn set_volume(&mut self, handle: AudioHandle, volume: f32) {
        let _ = self.command_tx.send(AudioCommand::SetVolume(handle, volume));
    }

    fn set_looping(&mut self, handle: AudioHandle, looping: bool) {
        let _ = self.command_tx.send(AudioCommand::SetLooping(handle, looping));
    }

    fn set_spatial(&mut self, handle: AudioHandle, position: Vec3) {
        let _ = self.command_tx.send(AudioCommand::SetSpatial(
            handle,
            SpatialParams { position, velocity: Vec3::ZERO },
        ));
    }

    fn set_listener(&mut self, position: Vec3, forward: Vec3, up: Vec3) {
        let _ = self.command_tx.send(AudioCommand::SetListener(ListenerParams {
            position,
            forward,
            up,
            velocity: Vec3::ZERO,
        }));
    }

    fn pause(&mut self, handle: AudioHandle) {
        let _ = self.command_tx.send(AudioCommand::Pause(handle));
    }

    fn resume(&mut self, handle: AudioHandle) {
        let _ = self.command_tx.send(AudioCommand::Resume(handle));
    }

    fn stop_all(&mut self) {
        let _ = self.command_tx.send(AudioCommand::StopAll);
    }

    fn set_bus_volume(&mut self, bus: BusType, volume: f32) {
        let _ = self.command_tx.send(AudioCommand::SetBusVolume(bus, volume));
    }

    fn set_bus_gain(&mut self, bus: BusType, gain: f32) {
        let _ = self.command_tx.send(AudioCommand::SetBusGain(bus, gain));
    }

    fn set_spatial_full(&mut self, handle: AudioHandle, position: Vec3, velocity: Vec3) {
        let _ = self.command_tx.send(AudioCommand::SetSpatial(
            handle,
            SpatialParams { position, velocity },
        ));
    }

    fn set_listener_full(&mut self, position: Vec3, forward: Vec3, up: Vec3, velocity: Vec3) {
        let _ = self.command_tx.send(AudioCommand::SetListener(ListenerParams {
            position,
            forward,
            up,
            velocity,
        }));
    }

    fn set_reverb(&mut self, wet: f32, dry: f32) {
        let _ = self.command_tx.send(AudioCommand::SetReverb {
            wet,
            dry,
            feedback: 0.8,
            damping: 0.2,
        });
    }
}
