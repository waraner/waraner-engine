use std::fmt;

use glam::Vec3;

#[derive(Debug)]
#[allow(dead_code)]
pub enum AudioError {
    FileNotFound(String),
    DecodeFailed(String),
    DeviceUnavailable(String),
    StreamError(String),
}

impl fmt::Display for AudioError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AudioError::FileNotFound(p) => write!(f, "audio file not found: {}", p),
            AudioError::DecodeFailed(e) => write!(f, "audio decode failed: {}", e),
            AudioError::DeviceUnavailable(e) => write!(f, "audio device unavailable: {}", e),
            AudioError::StreamError(e) => write!(f, "audio stream error: {}", e),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct AudioHandle {
    pub index: u32,
    pub generation: u32,
}

impl AudioHandle {
    #[allow(dead_code)]
    pub const INVALID: Self = Self { index: u32::MAX, generation: 0 };
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum BusType {
    Sfx,
    Music,
    Voice,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub enum BusEffect {
    Gain(f32),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PlayMode {
    Buffered,
    Streaming,
}

pub trait AudioBackend {
    fn play(&mut self, path: &str, volume: f32, looping: bool) -> AudioHandle;
    fn stop(&mut self, handle: AudioHandle);
    fn set_volume(&mut self, handle: AudioHandle, volume: f32);
    fn set_looping(&mut self, handle: AudioHandle, looping: bool);
    fn set_spatial(&mut self, handle: AudioHandle, position: Vec3);
    fn set_listener(&mut self, position: Vec3, forward: Vec3, up: Vec3);
    #[allow(dead_code)]
    fn pause(&mut self, handle: AudioHandle);
    #[allow(dead_code)]
    fn resume(&mut self, handle: AudioHandle);
    #[allow(dead_code)]
    fn stop_all(&mut self);

    fn play_on_bus(&mut self, path: &str, volume: f32, looping: bool, _bus: BusType) -> AudioHandle {
        self.play(path, volume, looping)
    }
    fn play_streaming(&mut self, path: &str, volume: f32, looping: bool) -> AudioHandle {
        self.play(path, volume, looping)
    }
    fn play_streaming_on_bus(&mut self, path: &str, volume: f32, looping: bool, _bus: BusType) -> AudioHandle {
        self.play_streaming(path, volume, looping)
    }
    #[allow(dead_code)]
    fn set_bus_volume(&mut self, _bus: BusType, _volume: f32) {}
    #[allow(dead_code)]
    fn set_bus_gain(&mut self, _bus: BusType, _gain: f32) {}

    fn set_spatial_full(&mut self, handle: AudioHandle, position: Vec3, _velocity: Vec3) {
        self.set_spatial(handle, position)
    }
    fn set_listener_full(&mut self, position: Vec3, forward: Vec3, up: Vec3, _velocity: Vec3) {
        self.set_listener(position, forward, up)
    }
    #[allow(dead_code)]
    fn set_reverb(&mut self, _wet: f32, _dry: f32) {}
}

mod rubi;
mod null;

pub use rubi::RubiAudio;

#[allow(unused_imports)]
pub use null::NullAudioBackend;
