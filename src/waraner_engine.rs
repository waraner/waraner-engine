use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::f32::consts::PI;
use std::time::SystemTime;

use glam::{Mat4, Vec3};
use winit::{
    event::{DeviceEvent, ElementState, MouseButton, WindowEvent},
    event_loop::EventLoopWindowTarget,
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorGrabMode, Window},
};

use crate::config::WaranerConfig;

fn grab_cursor(window: &Window) -> bool {
    match window.set_cursor_grab(CursorGrabMode::Confined) {
        Ok(()) => {}
        Err(_) => {
            if window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
                log::warn!("cursor grab not supported");
                return false;
            }
        }
    }
    window.set_cursor_visible(false);
    true
}

fn release_cursor(window: &Window) {
    let _ = window.set_cursor_grab(CursorGrabMode::None);
    window.set_cursor_visible(true);
}

use crate::audio::{AudioBackend, AudioHandle, PlayMode};
use crate::ecs::{
    AudioListenerComponent, AudioSourceComponent, CameraMode, Color,
    Entity, Ground, InputState, ScriptComponent, SkySettings, SunLight, Transform3D, World,
};
use crate::physics::PhysicsBackend;
use crate::physics_thread::ThreadedPhysics;
use crate::render_frame::*;
use crate::script::LuaEngine;

pub struct HotReloadState {
    pub script_dir: PathBuf,
    pub last_modified: HashMap<String, SystemTime>,
}

pub struct WaranerEngine {
    pub config: WaranerConfig,
    window: Arc<Window>,
    world: World,
    player_entity: Entity,
    keys: InputState,
    view_proj: Mat4,
    physics: Box<dyn PhysicsBackend>,
    audio: Box<dyn AudioBackend>,
    audio_handles: HashMap<Entity, AudioHandle>,
    dynamic_entities: Vec<Entity>,
    console_log: Vec<String>,
    console_history: Vec<String>,
    console_history_index: usize,
    console_open: bool,
    input_events: Vec<RenderInputEvent>,
    physics_accumulator: f32,
    last_frame_instant: std::time::Instant,
    script: LuaEngine,
    script_dir: PathBuf,
    world_seed: u64,
    hot_reload: Option<HotReloadState>,
}

impl WaranerEngine {
    pub fn new(
        config: WaranerConfig,
        window: Arc<Window>,
        world: World,
        player_entity: Entity,
        dynamic_entities: Vec<Entity>,
        physics: Box<dyn PhysicsBackend>,
        audio: Box<dyn AudioBackend>,
        script_dir: PathBuf,
    ) -> Self {
        let script = LuaEngine::new(script_dir.clone());
        let hot_reload = if cfg!(debug_assertions) {
            Some(HotReloadState {
                last_modified: HashMap::new(),
                script_dir: script_dir.clone(),
            })
        } else {
            None
        };
        Self {
            config,
            window,
            world,
            player_entity,
            keys: InputState::default(),
            view_proj: Mat4::IDENTITY,
            physics,
            audio,
            audio_handles: HashMap::new(),
            dynamic_entities,
            console_log: vec!["Console ready. Type /help for commands.".to_string()],
            console_history: Vec::new(),
            console_history_index: 0,
            console_open: false,
            input_events: Vec::new(),
            physics_accumulator: 0.0,
            last_frame_instant: std::time::Instant::now(),
            script,
            script_dir,
            world_seed: 0,
            hot_reload,
        }
    }

    pub fn handle_event(&mut self, event: &winit::event::Event<()>, _elwt: &EventLoopWindowTarget<()>) {
        match event {
            winit::event::Event::WindowEvent { event, window_id } if *window_id == self.window.id() => {
                match event {
                    WindowEvent::CloseRequested => std::process::exit(0),
                    WindowEvent::Resized(size) => {
                        self.input_events.push(RenderInputEvent::Resize {
                            width: size.width,
                            height: size.height,
                        });
                    }
                    WindowEvent::MouseInput { state, button, .. } => {
                        let pressed = matches!(state, ElementState::Pressed);
                        let btn = match button {
                            MouseButton::Left => imgui::MouseButton::Left,
                            MouseButton::Right => imgui::MouseButton::Right,
                            MouseButton::Middle => imgui::MouseButton::Middle,
                            _ => imgui::MouseButton::Left,
                        };
                        self.input_events.push(RenderInputEvent::MouseButton(btn, pressed));

                        if matches!(button, MouseButton::Left) && !self.keys.pointer_locked && !self.console_open {
                            self.keys.pointer_locked = grab_cursor(&self.window);
                        }
                    }
                    WindowEvent::CursorMoved { position, .. } => {
                        self.input_events.push(RenderInputEvent::MousePos(
                            position.x as f32,
                            position.y as f32,
                        ));
                    }
                    WindowEvent::KeyboardInput { event, .. } => {
                        if let PhysicalKey::Code(code) = event.physical_key {
                            let pressed = matches!(event.state, ElementState::Pressed);

                            if let Some(c) = &event.text {
                                for ch in c.chars() {
                                    self.input_events.push(RenderInputEvent::Char(ch));
                                }
                            }

                            if let Some(key) = Self::map_key(code) {
                                self.input_events.push(RenderInputEvent::Key(key, pressed));
                            }

                            let key_name = Self::key_name(code);
                            self.keys.keys.insert(key_name, pressed);

                            if !self.console_open {
                                if pressed {
                                    match code {
                                        KeyCode::KeyW => self.keys.forward = true,
                                        KeyCode::KeyA => self.keys.left = true,
                                        KeyCode::KeyS => self.keys.backward = true,
                                        KeyCode::KeyD => self.keys.right = true,
                                        KeyCode::Space => self.keys.jump = true,
                                        KeyCode::Escape => {
                                            if self.keys.pointer_locked {
                                                release_cursor(&self.window);
                                                self.keys.pointer_locked = false;
                                            }
                                        }
                                        KeyCode::KeyV | KeyCode::Tab => {
                                            if let Some(cam) = self.world.get_camera_mut(self.player_entity) {
                                                cam.mode = match cam.mode {
                                                    CameraMode::ThirdPerson => CameraMode::FirstPerson,
                                                    CameraMode::FirstPerson => CameraMode::FreeLook,
                                                    CameraMode::FreeLook => CameraMode::ThirdPerson,
                                                };
                                            }
                                        }
                                        KeyCode::Backquote => {
                                            self.console_open = !self.console_open;
                                            if self.console_open && self.keys.pointer_locked {
                                                release_cursor(&self.window);
                                                self.keys.pointer_locked = false;
                                            }
                                        }
                                        _ => {}
                                    }
                                } else {
                                    match code {
                                        KeyCode::KeyW => self.keys.forward = false,
                                        KeyCode::KeyA => self.keys.left = false,
                                        KeyCode::KeyS => self.keys.backward = false,
                                        KeyCode::KeyD => self.keys.right = false,
                                        KeyCode::Space => self.keys.jump = false,
                                        _ => {}
                                    }
                                }
                            } else {
                                if pressed {
                                    match code {
                                        KeyCode::Escape => {
                                            if self.keys.pointer_locked {
                                                release_cursor(&self.window);
                                                self.keys.pointer_locked = false;
                                            }
                                        }
                                        KeyCode::Backquote => {
                                            self.console_open = false;
                                            if !self.keys.pointer_locked {
                                                self.keys.pointer_locked = grab_cursor(&self.window);
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            winit::event::Event::DeviceEvent {
                event: DeviceEvent::MouseMotion { delta },
                ..
            } => {
                if self.keys.pointer_locked {
                    self.keys.mouse_dx += delta.0;
                    self.keys.mouse_dy += delta.1;
                }
            }
            winit::event::Event::AboutToWait => {}
            _ => {}
        }
    }

    fn key_name(code: KeyCode) -> String {
        let s = format!("{:?}", code);
        if let Some(rest) = s.strip_prefix("Key") {
            rest.to_string()
        } else if let Some(rest) = s.strip_prefix("Digit") {
            rest.to_string()
        } else {
            s.to_uppercase()
        }
    }

    fn map_key(code: KeyCode) -> Option<imgui::Key> {
        match code {
            KeyCode::KeyA => Some(imgui::Key::A),
            KeyCode::KeyB => Some(imgui::Key::B),
            KeyCode::KeyC => Some(imgui::Key::C),
            KeyCode::KeyD => Some(imgui::Key::D),
            KeyCode::KeyE => Some(imgui::Key::E),
            KeyCode::KeyF => Some(imgui::Key::F),
            KeyCode::KeyG => Some(imgui::Key::G),
            KeyCode::KeyH => Some(imgui::Key::H),
            KeyCode::KeyI => Some(imgui::Key::I),
            KeyCode::KeyJ => Some(imgui::Key::J),
            KeyCode::KeyK => Some(imgui::Key::K),
            KeyCode::KeyL => Some(imgui::Key::L),
            KeyCode::KeyM => Some(imgui::Key::M),
            KeyCode::KeyN => Some(imgui::Key::N),
            KeyCode::KeyO => Some(imgui::Key::O),
            KeyCode::KeyP => Some(imgui::Key::P),
            KeyCode::KeyQ => Some(imgui::Key::Q),
            KeyCode::KeyR => Some(imgui::Key::R),
            KeyCode::KeyS => Some(imgui::Key::S),
            KeyCode::KeyT => Some(imgui::Key::T),
            KeyCode::KeyU => Some(imgui::Key::U),
            KeyCode::KeyV => Some(imgui::Key::V),
            KeyCode::KeyW => Some(imgui::Key::W),
            KeyCode::KeyX => Some(imgui::Key::X),
            KeyCode::KeyY => Some(imgui::Key::Y),
            KeyCode::KeyZ => Some(imgui::Key::Z),
            KeyCode::Digit0 => Some(imgui::Key::Alpha0),
            KeyCode::Digit1 => Some(imgui::Key::Alpha1),
            KeyCode::Digit2 => Some(imgui::Key::Alpha2),
            KeyCode::Digit3 => Some(imgui::Key::Alpha3),
            KeyCode::Digit4 => Some(imgui::Key::Alpha4),
            KeyCode::Digit5 => Some(imgui::Key::Alpha5),
            KeyCode::Digit6 => Some(imgui::Key::Alpha6),
            KeyCode::Digit7 => Some(imgui::Key::Alpha7),
            KeyCode::Digit8 => Some(imgui::Key::Alpha8),
            KeyCode::Digit9 => Some(imgui::Key::Alpha9),
            KeyCode::Space => Some(imgui::Key::Space),
            KeyCode::Enter => Some(imgui::Key::Enter),
            KeyCode::Escape => Some(imgui::Key::Escape),
            KeyCode::Tab => Some(imgui::Key::Tab),
            KeyCode::ShiftLeft | KeyCode::ShiftRight => Some(imgui::Key::LeftShift),
            KeyCode::ControlLeft | KeyCode::ControlRight => Some(imgui::Key::LeftCtrl),
            KeyCode::AltLeft | KeyCode::AltRight => Some(imgui::Key::LeftAlt),
            KeyCode::ArrowUp => Some(imgui::Key::UpArrow),
            KeyCode::ArrowDown => Some(imgui::Key::DownArrow),
            KeyCode::ArrowLeft => Some(imgui::Key::LeftArrow),
            KeyCode::ArrowRight => Some(imgui::Key::RightArrow),
            KeyCode::Backspace => Some(imgui::Key::Backspace),
            KeyCode::Delete => Some(imgui::Key::Delete),
            KeyCode::Home => Some(imgui::Key::Home),
            KeyCode::End => Some(imgui::Key::End),
            KeyCode::PageUp => Some(imgui::Key::PageUp),
            KeyCode::PageDown => Some(imgui::Key::PageDown),
            _ => None,
        }
    }

    fn update_camera(&mut self) {
        let Some(camera) = self.world.get_camera(self.player_entity) else { return };
        let Some(player_transform) = self.world.get_transform(self.player_entity) else { return };
        let pos = player_transform.position;

        let forward = Vec3::new(
            camera.yaw.sin() * camera.pitch.cos(),
            camera.pitch.sin(),
            camera.yaw.cos() * camera.pitch.cos(),
        )
        .normalize();

        let target = pos + Vec3::Y * 1.5;
        let eye = match camera.mode {
            CameraMode::ThirdPerson => {
                let offset = -forward * camera.distance + Vec3::Y * camera.height;
                target + offset
            }
            CameraMode::FirstPerson => pos + Vec3::Y * 1.6,
            CameraMode::FreeLook => {
                let offset = -forward * 3.0 + Vec3::Y * 2.0;
                target + offset
            }
        };

        let aspect = self.config_width_height().0 as f32 / self.config_width_height().1 as f32;
        let projection = Mat4::perspective_rh(PI / 4.0, aspect, 0.1, 200.0);
        let view = Mat4::look_at_rh(eye, target, Vec3::Y);
        self.view_proj = projection * view;
    }

    fn config_width_height(&self) -> (u32, u32) {
        let size = self.window.inner_size();
        (size.width.max(1), size.height.max(1))
    }

    fn tick_physics(&mut self, dt: f32) {
        let camera_yaw = self.world.get_camera(self.player_entity).map(|c| c.yaw).unwrap_or(0.0);
        self.physics.tick(
            &mut self.world,
            &self.keys,
            self.player_entity,
            camera_yaw,
            &self.dynamic_entities,
            dt,
        );
    }

    fn update_audio(&mut self) {
        let current_audio_entities: Vec<Entity> = self.world.query()
            .with::<AudioSourceComponent>()
            .with::<Transform3D>()
            .iter_entities();

        self.audio_handles.retain(|&entity, &mut handle| {
            if current_audio_entities.contains(&entity) {
                true
            } else {
                self.audio.stop(handle);
                false
            }
        });

        // Update source positions FIRST so SetListener computes spatial with fresh data
        for &entity in &current_audio_entities {
            let handle = match self.audio_handles.get(&entity) {
                Some(&h) => h,
                None => {
            if let Some(comp) = self.world.get_audio_source(entity) {
                let h = match comp.mode {
                    PlayMode::Streaming => self.audio.play_streaming_on_bus(&comp.clip, comp.volume, comp.looping, comp.bus),
                    PlayMode::Buffered => self.audio.play_on_bus(&comp.clip, comp.volume, comp.looping, comp.bus),
                };
                        self.audio_handles.insert(entity, h);
                        h
                    } else {
                        continue;
                    }
                }
            };

            if let Some(comp) = self.world.get_audio_source(entity) {
                self.audio.set_volume(handle, comp.volume);
                self.audio.set_looping(handle, comp.looping);
            }

            if let (Some(transform), vel) = (
                self.world.get_transform(entity),
                self.world.get_velocity_3d(entity),
            ) {
                let velocity = vel.map(|v| v.linear).unwrap_or(Vec3::ZERO);
                self.audio.set_spatial_full(handle, transform.position, velocity);
            }
        }

        // SetListener LAST — triggers update_spatial with all positions already applied
        for entity in self.world.query().with::<AudioListenerComponent>().with::<Transform3D>().iter_entities() {
            if let (Some(transform), vel) = (
                self.world.get_transform(entity),
                self.world.get_velocity_3d(entity),
            ) {
                let forward = transform.rotation * Vec3::new(0.0, 0.0, 1.0);
                let up = transform.rotation * Vec3::Y;
                let velocity = vel.map(|v| v.linear).unwrap_or(Vec3::ZERO);
                self.audio.set_listener_full(transform.position, forward, up, velocity);
            }
        }
    }

    fn cmd_help(&mut self, _args: &[&str]) {
        self.console_log.push("Commands:".to_string());
        self.console_log.push("  /help            - show this help".to_string());
        self.console_log.push("  /clear           - clear console".to_string());
        self.console_log.push("  /spawn <type> <x> <y> <z> - spawn entity of type".to_string());
        self.console_log.push("  /types           - list available entity types".to_string());
        self.console_log.push("  /despawn <index> - despawn dynamic cube by index".to_string());
        self.console_log.push("  /list            - list dynamic entities".to_string());
        self.console_log.push("  /teleport <x> <y> <z> - teleport player".to_string());
        self.console_log.push("  /jump            - make player jump".to_string());
        self.console_log.push("  /save [path]     - save world to .wmap (default levels/default.wmap)".to_string());
        self.console_log.push("  /load [path]     - load world from .wmap".to_string());
    }

    fn cmd_clear(&mut self, _args: &[&str]) {
        self.console_log.clear();
    }

    fn cmd_spawn(&mut self, args: &[&str]) {
        if args.len() < 4 {
            self.console_log.push("Usage: /spawn <type> <x> <y> <z>".to_string());
            self.console_log.push("Types: prop.mesh, prop.physics, env.sun, env.fog, info.spawn, trigger.box, light.point".to_string());
            return;
        }
        let type_name = args[0];
        let x: f32 = args[1].parse().unwrap_or(0.0);
        let y: f32 = args[2].parse().unwrap_or(0.0);
        let z: f32 = args[3].parse().unwrap_or(0.0);

        let e = match crate::entity_types::spawn_type(&mut self.world, type_name) {
            Some(e) => e,
            None => {
                self.console_log.push(format!("Unknown entity type '{}'", type_name));
                return;
            }
        };
        // Override position from command args.
        if let Some(t) = self.world.get_transform_mut(e) {
            t.position = Vec3::new(x, y, z);
        }
        self.dynamic_entities.push(e);
        self.console_log.push(format!("Spawned {} at ({}, {}, {})", type_name, x, y, z));
    }

    fn cmd_types(&mut self, _args: &[&str]) {
        self.console_log.push("Entity types:".to_string());
        let reg = crate::entity_types::default_registry();
        for cat in reg.categories() {
            self.console_log.push(format!("  [{}]", cat));
            for tmpl in reg.templates_in_category(cat) {
                self.console_log.push(format!("    {}  ({})", tmpl.name, tmpl.display_name));
            }
        }
    }

    fn cmd_despawn(&mut self, args: &[&str]) {
        let index: usize = match args.first().and_then(|s| s.parse().ok()) {
            Some(i) => i,
            None => {
                self.console_log.push("Usage: /despawn <index>".to_string());
                return;
            }
        };
        let entities: Vec<Entity> = self.dynamic_entities.iter()
            .filter(|&&e| e != self.player_entity)
            .copied()
            .collect();
        if index >= entities.len() {
            self.console_log.push(format!("Index {} out of range (0-{})", index, entities.len().saturating_sub(1)));
            return;
        }
        let target = entities[index];
        if let Some(&handle) = self.audio_handles.get(&target) {
            self.audio.stop(handle);
            self.audio_handles.remove(&target);
        }
        self.script.destroy_entity(target);
        self.world.despawn(target);
        self.physics.remove_entity(target);
        self.dynamic_entities.retain(|e| *e != target);
        self.console_log.push(format!("Despawned entity #{} ({:?})", index, target));
    }

    fn cmd_list(&mut self, _args: &[&str]) {
        let dynamic: Vec<Entity> = self.dynamic_entities.iter()
            .filter(|&&e| e != self.player_entity)
            .copied()
            .collect();
        if dynamic.is_empty() {
            self.console_log.push("No dynamic entities.".to_string());
            return;
        }
        for (i, e) in dynamic.iter().enumerate() {
            let pos = self.world.get_transform(*e)
                .map(|t| format!("({:.1}, {:.1}, {:.1})", t.position.x, t.position.y, t.position.z))
                .unwrap_or_default();
            self.console_log.push(format!("  [{}] {:?} at {}", i, e, pos));
        }
    }

    fn cmd_teleport(&mut self, args: &[&str]) {
        if args.len() < 3 {
            self.console_log.push("Usage: /teleport <x> <y> <z>".to_string());
            return;
        }
        let x: f32 = args[0].parse().unwrap_or(0.0);
        let y: f32 = args[1].parse().unwrap_or(0.0);
        let z: f32 = args[2].parse().unwrap_or(0.0);
        let new_pos = Vec3::new(x, y, z);
        if let Some(pos) = self.world.get_transform_mut(self.player_entity) {
            pos.position = new_pos;
        }
        self.physics.teleport_player(self.player_entity, new_pos);
        self.console_log.push(format!("Teleported to ({}, {}, {})", x, y, z));
    }

    fn cmd_jump(&mut self, _args: &[&str]) {
        if let Some(vel) = self.world.get_velocity_3d_mut(self.player_entity) {
            vel.linear.y += 8.0;
            self.console_log.push("Jumped!".to_string());
        }
    }

    fn cmd_save(&mut self, args: &[&str]) {
        let path = args.first().copied().unwrap_or("levels/default.wmap");
        if let Err(e) = self.save_level(path) {
            self.console_log.push(format!("Save failed: {}", e));
        }
    }

    fn cmd_load(&mut self, args: &[&str]) {
        let path = args.first().copied().unwrap_or("levels/default.wmap");
        if let Err(e) = self.load_level(path) {
            self.console_log.push(format!("Load failed: {}", e));
        }
    }

    pub fn check_hot_reload(&mut self) {
        let should_reload = {
            let Some(reload) = self.hot_reload.as_mut() else { return };
            let Ok(entries) = std::fs::read_dir(&reload.script_dir) else { return };
            let mut changed = false;
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()).map(|e| e == "lua" || e == "luac").unwrap_or(false) {
                    if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                        let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
                        let prev = reload.last_modified.insert(name.clone(), modified);
                        if prev.is_some_and(|t| t != modified) {
                            log::info!("[HotReload] '{}' changed, reloading scripts", path.display());
                            changed = true;
                        }
                    }
                }
            }
            changed
        };

        if should_reload {
            let script_dir = self.script_dir.clone();
            let old_script = std::mem::replace(&mut self.script, LuaEngine::new(script_dir));
            let _ = old_script;
            if let Err(e) = self.init_scripting() {
                log::warn!("[HotReload] script reload failed: {}", e);
            } else {
                self.console_log.push("Hot-reloaded scripts".to_string());
            }
        }
    }

    /// Get a user-configurable value as a string (for Lua).
    pub fn config_get(&self, key: &str) -> Option<String> {
        match key {
            "window_width" => Some(self.config.window_width.to_string()),
            "window_height" => Some(self.config.window_height.to_string()),
            "fullscreen" => Some(self.config.fullscreen.to_string()),
            "vsync" => Some(self.config.vsync.to_string()),
            _ => None,
        }
    }

    /// Set a user-configurable value from a string.
    pub fn config_set(&mut self, key: &str, value: &str) -> Result<(), String> {
        match key {
            "window_width" => {
                let v = value.parse::<u32>().map_err(|e| format!("invalid window_width: {e}"))?;
                self.config.window_width = v;
            }
            "window_height" => {
                let v = value.parse::<u32>().map_err(|e| format!("invalid window_height: {e}"))?;
                self.config.window_height = v;
            }
            "fullscreen" => {
                self.config.fullscreen = value.eq_ignore_ascii_case("true") || value == "1";
            }
            "vsync" => {
                self.config.vsync = value.eq_ignore_ascii_case("true") || value == "1";
            }
            _ => return Err(format!("unknown user config key: {key}")),
        }
        Ok(())
    }

    /// Save user-configurable settings to disk.
    pub fn config_save(&self) -> Result<(), String> {
        self.config.save_user_config().map(|_| ())
    }

    pub fn init_scripting(&mut self) -> Result<(), String> {
        self.script.init(
            &mut self.world,
            &mut self.keys,
            &mut self.audio,
            &mut self.player_entity,
            &mut self.dynamic_entities,
            &mut self.config,
        )?;
        self.script.load_main("main.lua")?;

        for entity in self.world.query().with::<ScriptComponent>().iter_entities() {
            if let Some(script_comp) = self.world.get_script(entity) {
                if !script_comp.script_name.is_empty() {
                    self.script.attach_script(entity, &script_comp.script_name)?;
                }
            }
        }

        Ok(())
    }

    /// Replace the active world by loading a `.wmap` level file. Rebuilds
    /// physics, clears stale audio handles, and re-initializes scripting.
    pub fn load_level(&mut self, path: &str) -> Result<(), String> {
        let (world, seed, _names) = crate::wmap::read_world(path)?;
        self.world_seed = seed;

        let player = world
            .entities()
            .into_iter()
            .find(|e| world.is_player(*e))
            .or_else(|| world.entities().first().copied())
            .ok_or_else(|| "level contains no entities".to_string())?;

        let dynamic: Vec<Entity> = world
            .entities()
            .into_iter()
            .filter(|e| {
                !world.is_static(*e)
                    && !world.is_ground(*e)
                    && !world.is_player(*e)
                    && world.get_rigid_body(*e).is_some()
            })
            .collect();

        let physics = Box::new(ThreadedPhysics::from_world(&world, player, &dynamic));

        self.audio_handles.clear();
        self.world = world;
        self.player_entity = player;
        self.dynamic_entities = dynamic;
        self.physics = physics;
        self.script = LuaEngine::new(self.script_dir.clone());
        self.init_scripting()?;

        self.console_log.push(format!("Loaded level '{}'", path));
        Ok(())
    }

    /// Serialize the current world to a `.wmap` file.
    pub fn save_level(&mut self, path: &str) -> Result<(), String> {
        match crate::wmap::write_world(&self.world, path, self.world_seed) {
            Ok(()) => {
                self.console_log.push(format!("Saved level '{}'", path));
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub fn handle_console_command(&mut self, line: &str) {
        if line.trim().is_empty() {
            return;
        }
        self.console_log.push(format!("> {}", line));
        self.console_history.push(line.to_string());
        self.console_history_index = self.console_history.len();

        let parts: Vec<&str> = line.split_whitespace().collect();
        let cmd = parts[0];
        let args = &parts[1..];

        match cmd {
            "help" | "/help" => self.cmd_help(args),
            "clear" | "/clear" => self.cmd_clear(args),
            "spawn" | "/spawn" => self.cmd_spawn(args),
            "types" | "/types" => self.cmd_types(args),
            "despawn" | "/despawn" => self.cmd_despawn(args),
            "list" | "/list" => self.cmd_list(args),
            "teleport" | "/teleport" => self.cmd_teleport(args),
            "jump" | "/jump" => self.cmd_jump(args),
            "save" | "/save" => self.cmd_save(args),
            "load" | "/load" => self.cmd_load(args),
            _ => self.console_log.push(format!("Unknown command: {}", cmd)),
        }
    }

    pub fn drain_input_events(&mut self) -> Vec<RenderInputEvent> {
        std::mem::take(&mut self.input_events)
    }

    pub fn tick(&mut self, dt: f32) {
        self.check_hot_reload();

        let tick_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.physics_accumulator += dt;
            const FIXED_DT: f32 = 1.0 / 60.0;
            while self.physics_accumulator >= FIXED_DT {
                self.tick_physics(FIXED_DT);
                self.physics_accumulator -= FIXED_DT;
            }

            if let Some(cam) = self.world.get_camera_mut(self.player_entity) {
                cam.yaw -= self.keys.mouse_dx as f32 * 0.002;
                cam.pitch -= self.keys.mouse_dy as f32 * 0.002;
                cam.pitch = cam.pitch.clamp(-1.5, 1.5);
            }
            self.keys.mouse_dx = 0.0;
            self.keys.mouse_dy = 0.0;

            self.update_camera();
            self.update_audio();

            let collisions = self.physics.drain_collision_events();
            self.script.update(&mut self.world, dt, collisions);
        }));

        if tick_result.is_err() {
            log::error!("[Engine] panic caught in tick — attempting to continue cleanly");
            self.console_log.push("⚠ Engine panic during tick — see log for details.".to_string());
        }
    }

    pub fn build_frame(&mut self) -> Option<RenderFrame> {
        let now = std::time::Instant::now();
        let frame_dt = (now.duration_since(self.last_frame_instant).as_secs_f32()).min(0.05);
        self.last_frame_instant = now;
        let fps = if frame_dt > 0.0 { 1.0 / frame_dt } else { 0.0 };
        self.script.set_fps(fps);

        self.tick(frame_dt);

        let (width, height) = self.config_width_height();

        let mut ground_instances = Vec::new();
        for entity in self.world.query().with::<Ground>().with::<Transform3D>().with::<Color>().iter_entities() {
            if let (Some(transform), Some(color)) = (self.world.get_transform(entity), self.world.get_color(entity)) {
                let mesh_name = self.world.get_model(entity).map(|m| m.path);
                ground_instances.push(DrawInstance { transform, tint: color, mesh_name });
            }
        }

        let player_instance = self.world.get_transform(self.player_entity).map(|transform| {
            let tint = self.world.get_color(self.player_entity)
                .unwrap_or(Color { rgba: [1.0, 1.0, 1.0, 1.0] });
            let mesh_name = self.world.get_model(self.player_entity).map(|m| m.path);
            DrawInstance { transform, tint, mesh_name }
        });

        let mut dynamic_instances = Vec::new();
        for &entity in &self.dynamic_entities {
            if entity == self.player_entity {
                continue;
            }
            if let Some(transform) = self.world.get_transform(entity) {
                let tint = self.world.get_color(entity)
                    .unwrap_or(Color { rgba: [1.0, 1.0, 1.0, 1.0] });
                let mesh_name = self.world.get_model(entity).map(|m| m.path);
                dynamic_instances.push(DrawInstance { transform, tint, mesh_name });
            }
        }

        let console = ConsoleState {
            open: self.console_open,
            log: self.console_log.clone(),
            history: self.console_history.clone(),
        };

        let mut ui_labels = self.script.take_ui_labels();
        if self.script.is_debug_menu() {
            ui_labels.push(format!(
                "FPS: {:.1}  |  Frame: {:.1}ms",
                fps,
                frame_dt * 1000.0
            ));
            ui_labels.push(format!("Entities: {}", self.dynamic_entities.len()));
            ui_labels.push(format!("Scripts: {}", self.script.script_count()));
        }

        // Directional sun light — driven by the first env.sun entity, if any.
        let (sun_dir, sun_light) = {
            let sun_entities = self.world.query().with::<SunLight>().with::<Transform3D>().iter_entities();
            if let Some(&sun_entity) = sun_entities.first() {
                let t = self.world.get_transform(sun_entity).unwrap_or_default();
                let sl = self.world.get_sun_light(sun_entity).unwrap_or_default();
                let dir = t.position.normalize_or_zero();
                let dir = if dir.length_squared() < 0.001 {
                    glam::Vec3::new(0.5, 0.7, 1.0).normalize()
                } else {
                    dir
                };
                (dir, DirectionalLight {
                    direction: dir,
                    color: glam::Vec3::from(sl.color),
                    intensity: sl.intensity,
                })
            } else {
                let dir = glam::Vec3::new(0.5, 0.7, 1.0).normalize();
                (dir, DirectionalLight {
                    direction: dir,
                    color: glam::Vec3::new(1.0, 0.95, 0.85),
                    intensity: 1.0,
                })
            }
        };

        // Camera world-space position (for lighting and shadow following).
        let camera_pos = match self.world.get_camera(self.player_entity) {
            Some(cam) => {
                let pt = self.world.get_transform(self.player_entity).unwrap();
                let fwd = glam::Vec3::new(
                    cam.yaw.sin() * cam.pitch.cos(),
                    cam.pitch.sin(),
                    cam.yaw.cos() * cam.pitch.cos(),
                );
                pt.position + glam::Vec3::Y * 1.5 + (-fwd * cam.distance + glam::Vec3::Y * cam.height)
            }
            None => glam::Vec3::ZERO,
        };

        // Sun view-projection for shadow mapping — follows the camera (Source-like).
        let shadow_target = camera_pos;
        let shadow_dist = 30.0;
        let shadow_size = 30.0;
        let sun_eye = shadow_target + sun_dir * shadow_dist;
        let sun_view = Mat4::look_at_rh(sun_eye, shadow_target, glam::Vec3::Y);
        let sun_proj = Mat4::orthographic_rh(
            -shadow_size, shadow_size,
            -shadow_size, shadow_size,
            0.1, shadow_dist * 2.0,
        );
        let sun_view_proj = sun_proj * sun_view;

        let ambient = self.script.get_ambient();
        let exposure = self.script.get_exposure();
        let bloom_intensity = self.script.get_bloom_intensity();
        let msaa_samples = self.script.get_msaa();

        // Sky settings — from the first env.sky entity, if any.
        let sky = {
            let sky_entities = self.world.query().with::<SkySettings>().iter_entities();
            if let Some(&sky_entity) = sky_entities.first() {
                let s = self.world.get_sky_settings(sky_entity).unwrap_or_default();
                SkyFrame {
                    color: glam::Vec3::from(s.color),
                    brightness: s.brightness,
                    indirect_light_multiplier: s.indirect_light_multiplier,
                    sky_color: glam::Vec3::from(s.sky_color),
                    sky_intensity: s.sky_intensity,
                    sky_ibl_scale: s.sky_ibl_scale,
                    skybox_bounce_multiplier: s.skybox_bounce_multiplier,
                }
            } else {
                SkyFrame {
                    color: glam::Vec3::new(0.4, 0.6, 0.9),
                    brightness: 1.0,
                    indirect_light_multiplier: 1.0,
                    sky_color: glam::Vec3::new(0.4, 0.6, 0.9),
                    sky_intensity: 1.0,
                    sky_ibl_scale: 1.0,
                    skybox_bounce_multiplier: 1.0,
                }
            }
        };

        Some(RenderFrame {
            ground_instances,
            player_instance,
            dynamic_instances,
            view_proj: self.view_proj,
            width,
            height,
            console,
            ui_labels,
            sun_light,
            sun_view_proj,
            camera_pos,
            ambient,
            exposure,
            bloom_intensity,
            msaa_samples,
            sky,
        })
    }
}
