use glam::Mat4;

use crate::ecs::{Color, Transform3D};

#[derive(Copy, Clone, Debug)]
pub struct SkyFrame {
    pub color: glam::Vec3,
    pub brightness: f32,
    pub indirect_light_multiplier: f32,
    pub sky_color: glam::Vec3,
    pub sky_intensity: f32,
    pub sky_ibl_scale: f32,
    pub skybox_bounce_multiplier: f32,
}

#[derive(Clone, Debug)]
pub struct DrawInstance {
    pub transform: Transform3D,
    pub tint: Color,
    pub mesh_name: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DirectionalLight {
    pub direction: glam::Vec3,
    pub color: glam::Vec3,
    pub intensity: f32,
}

#[derive(Clone, Debug)]
pub struct ConsoleState {
    pub open: bool,
    pub log: Vec<String>,
    pub history: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RenderFrame {
    pub ground_instances: Vec<DrawInstance>,
    pub player_instance: Option<DrawInstance>,
    pub dynamic_instances: Vec<DrawInstance>,
    pub view_proj: Mat4,
    pub width: u32,
    pub height: u32,
    pub console: ConsoleState,
    pub ui_labels: Vec<String>,
    /// Directional sun light for shadow mapping + shading.
    pub sun_light: DirectionalLight,
    /// View-projection matrix from the sun's perspective (shadow map).
    pub sun_view_proj: Mat4,
    /// Camera world-space position.
    pub camera_pos: glam::Vec3,
    /// Ambient light color (Lua-configurable).
    pub ambient: glam::Vec3,
    /// Tonemap exposure (Lua-configurable).
    pub exposure: f32,
    /// Bloom intensity (Lua-configurable).
    pub bloom_intensity: f32,
    /// MSAA sample count (0=off, 2, 4).
    pub msaa_samples: u32,
    /// Sky settings.
    pub sky: SkyFrame,
}

#[derive(Clone, Debug)]
pub enum RenderInputEvent {
    MousePos(f32, f32),
    MouseButton(imgui::MouseButton, bool),
    Key(imgui::Key, bool),
    Char(char),
    Resize { width: u32, height: u32 },
}

#[derive(Clone, Debug)]
pub enum RenderMessage {
    Frame(RenderFrame, Vec<RenderInputEvent>),
    Shutdown,
}

#[derive(Clone, Debug)]
pub enum MainMessage {
    ConsoleCommand(String),
}
