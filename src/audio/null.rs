#![allow(dead_code)]

use crate::audio::{AudioBackend, AudioHandle, BusType};
use glam::Vec3;

pub struct NullAudioBackend;

impl NullAudioBackend {
    pub fn new() -> Self {
        Self
    }
}

impl AudioBackend for NullAudioBackend {
    fn play(&mut self, _path: &str, _volume: f32, _looping: bool) -> AudioHandle {
        AudioHandle { index: 0, generation: 0 }
    }

    fn stop(&mut self, _handle: AudioHandle) {}

    fn set_volume(&mut self, _handle: AudioHandle, _volume: f32) {}

    fn set_looping(&mut self, _handle: AudioHandle, _looping: bool) {}

    fn set_spatial(&mut self, _handle: AudioHandle, _position: Vec3) {}

    fn set_listener(&mut self, _position: Vec3, _forward: Vec3, _up: Vec3) {}

    fn pause(&mut self, _handle: AudioHandle) {}

    fn resume(&mut self, _handle: AudioHandle) {}

    fn stop_all(&mut self) {}

    fn play_on_bus(&mut self, _path: &str, _volume: f32, _looping: bool, _bus: BusType) -> AudioHandle {
        AudioHandle { index: 0, generation: 0 }
    }

    fn play_streaming(&mut self, _path: &str, _volume: f32, _looping: bool) -> AudioHandle {
        AudioHandle { index: 0, generation: 0 }
    }

    fn play_streaming_on_bus(&mut self, _path: &str, _volume: f32, _looping: bool, _bus: BusType) -> AudioHandle {
        AudioHandle { index: 0, generation: 0 }
    }

    fn set_bus_volume(&mut self, _bus: BusType, _volume: f32) {}

    fn set_bus_gain(&mut self, _bus: BusType, _gain: f32) {}

    fn set_spatial_full(&mut self, _handle: AudioHandle, _position: Vec3, _velocity: Vec3) {}

    fn set_listener_full(&mut self, _position: Vec3, _forward: Vec3, _up: Vec3, _velocity: Vec3) {}

    fn set_reverb(&mut self, _wet: f32, _dry: f32) {}
}
