mod audio;
mod asset_id;
mod constants;
mod asset_loader;
mod asset_system;
mod ecs;
mod entity_types;
pub mod config;
mod model_loader;
mod physics;
mod physics_thread;
mod render_frame;
mod renderer;
mod script;
mod waraner_engine;
mod wmesh;
mod wmap;
mod wpak;

use std::path::Path;
use std::sync::Arc;

use crossbeam::channel;
use glam::{Quat, Vec3};
use winit::{
    event_loop::EventLoop,
    window::WindowBuilder,
};

use crate::audio::AudioBackend;
use crate::ecs::{
    AngularVelocity, Camera, Collider, Color,
    Entity, Model, RigidBody, Transform3D, Velocity3D, World,
};
use crate::physics::PhysicsBackend;
use crate::physics_thread::ThreadedPhysics;
use crate::render_frame::{MainMessage, RenderMessage};
use crate::renderer::Renderer;
use crate::waraner_engine::WaranerEngine;

fn spawn_scene() -> (World, Entity, Vec<Entity>, Box<dyn PhysicsBackend>, Box<dyn AudioBackend>) {
    let mut world = World::new();
    let player_entity = world.spawn();

    world.add_transform(player_entity, Transform3D {
        position: Vec3::new(0.0, 2.0, 0.0),
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    });
    world.add_velocity_3d(player_entity, Velocity3D::default());
    world.add_rigid_body(player_entity, RigidBody {
        mass: 1.0,
        restitution: 0.0,
        angular_damping: 0.95,
    });
    world.add_collider(player_entity, Default::default());
    world.add_player(player_entity);
    world.add_camera(player_entity, Camera::default());
    world.add_audio_listener(player_entity);
    world.set_color(player_entity, Color { rgba: [0.3, 0.5, 1.0, 1.0] });
    
    let ground_spawns = [
        (Vec3::new(0.0, -2.5, 0.0), 10.0, 10.0),
        (Vec3::new(8.0, -1.5, 0.0), 4.0, 4.0),
        (Vec3::new(-8.0, -0.5, 0.0), 4.0, 4.0),
        (Vec3::new(0.0, 0.0, -8.0), 4.0, 4.0),
    ];

    for (pos, hw, hd) in ground_spawns {
        let e = world.spawn();
        world.add_transform(e, Transform3D {
            position: pos,
            rotation: Quat::IDENTITY,
            scale: Vec3::new(hw, 1.0, hd),
        });
        world.add_ground(e);
        world.add_collider(e, Collider { half_extents: Vec3::new(hw * 0.5, 0.5, hd * 0.5) });
        world.add_static(e);
        world.add_rigid_body(e, RigidBody {
            mass: 0.0,
            restitution: 0.2,
            angular_damping: 0.99,
        });
        world.add_velocity_3d(e, Velocity3D::default());
        world.set_color(e, Color { rgba: [0.55, 0.45, 0.35, 1.0] });
    }

    let dynamic_spawns = [
        (Vec3::new(2.0, 2.0, 0.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.5, 0.5, 0.5), 1.0, None),
        (Vec3::new(3.0, 4.0, 1.0), Vec3::new(1.5, 1.5, 1.5), Vec3::new(0.5, 0.5, 0.5), 2.0, Some("teapot")),
    ];

    let mut dynamic_entities = Vec::new();
    for (pos, visual_scale, collider_size, mass, model_name) in dynamic_spawns {
        let e = world.spawn();
        world.add_transform(e, Transform3D {
            position: pos,
            rotation: Quat::IDENTITY,
            scale: visual_scale,
        });
        world.add_velocity_3d(e, Velocity3D::default());
        world.add_angular_velocity(e, AngularVelocity {
            radians: Vec3::new(0.5, 1.0, 0.3),
        });
        world.add_rigid_body(e, RigidBody { mass, restitution: 0.2, angular_damping: 0.98 });
        world.add_collider(e, Collider { half_extents: collider_size });
        world.set_color(e, Color { rgba: [1.0, 0.3, 0.2, 1.0] });
        if let Some(name) = model_name {
            world.add_model(e, Model { path: name.to_string() });
        }
        dynamic_entities.push(e);
    }

    let physics = Box::new(ThreadedPhysics::from_world(&world, player_entity, &dynamic_entities));
    let audio: Box<dyn AudioBackend> = Box::new(crate::audio::RubiAudio::new());
    (world, player_entity, dynamic_entities, physics, audio)
}

/// Load `levels/default.wmap` if present, otherwise build the demo scene and
/// save it out so subsequent launches load from disk (WMAP round-trip).
fn load_or_create_level() -> (World, Entity, Vec<Entity>, Box<dyn PhysicsBackend>, Box<dyn AudioBackend>) {
    let level_path = std::path::Path::new("levels").join("default.wmap");

    if level_path.exists() {
        match crate::wmap::read_world(level_path.to_str().unwrap()) {
            Ok((world, _seed, _names)) => {
                let player = world
                    .entities()
                    .into_iter()
                    .find(|e| world.is_player(*e))
                    .or_else(|| world.entities().first().copied())
                    .expect("level contains no entities");
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
                let audio: Box<dyn AudioBackend> = Box::new(crate::audio::RubiAudio::new());
                log::info!("Loaded level from {}", level_path.display());
                return (world, player, dynamic, physics, audio);
            }
            Err(e) => log::warn!(
                "Failed to load '{}': {}. Building default scene.",
                level_path.display(),
                e
            ),
        }
    }

    let (world, player_entity, dynamic_entities, physics, audio) = spawn_scene();

    let _ = std::fs::create_dir_all("levels");
    match crate::wmap::write_world(&world, level_path.to_str().unwrap(), 0) {
        Ok(()) => log::info!("Saved default level to {}", level_path.display()),
        Err(e) => log::warn!("Failed to save default level: {}", e),
    }

    (world, player_entity, dynamic_entities, physics, audio)
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(
        WindowBuilder::new()
            .with_title("Waraner Engine - Cube")
            .with_inner_size(winit::dpi::LogicalSize::new(1280, 720))
            .build(&event_loop)
            .unwrap(),
    );

    let mut renderer = pollster::block_on(Renderer::new(window.clone()));

    let model_dir = Path::new("assets").join("models");
    if model_dir.exists() {
        let mut entries: Vec<_> = std::fs::read_dir(&model_dir).unwrap()
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.path());

        let mut loaded_stems: std::collections::HashSet<String> = std::collections::HashSet::new();

        for entry in &entries {
            let path = entry.path();
            let ext = match path.extension().and_then(|s| s.to_str()) {
                Some(e) => e.to_lowercase(),
                None => continue,
            };
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();

            let src_extensions = ["obj", "gltf", "glb"];

            match ext.as_str() {
                e if src_extensions.contains(&e) => {
                    let wmesh_path = path.with_extension("wmesh");
                    let path_str = path.to_string_lossy().to_string();
                    let wmesh_str = wmesh_path.to_string_lossy().to_string();

                    if wmesh_path.exists() {
                        let src_mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                        let wm_mtime = std::fs::metadata(&wmesh_path).and_then(|m| m.modified()).ok();
                        if let (Some(src), Some(wm)) = (src_mtime, wm_mtime) {
                            if wm >= src {
                                log::info!("Loading cached '{}'", wmesh_str);
                                match renderer.load_model(&wmesh_str) {
                                    Ok(_) => loaded_stems.insert(stem),
                                    Err(e) => { log::warn!("  failed: {e}"); false }
                                };
                                continue;
                            }
                        }
                    }

                    match renderer.load_model(&path_str) {
                        Ok(_) => {
                            loaded_stems.insert(stem.clone());
                            let meshes = match crate::model_loader::load_model(&path_str) {
                                Ok(m) => m,
                                Err(e) => { log::warn!("  cannot convert: {e}"); continue; }
                            };
                            if let Err(e) = crate::wmesh::write_wmesh(&wmesh_str, &meshes) {
                                log::warn!("  failed to write wmesh: {e}");
                            }
                        }
                        Err(e) => log::warn!("Failed to load '{}': {}", path_str, e),
                    }
                }
                "wmesh" => {
                    if loaded_stems.contains(&stem) {
                        continue;
                    }
                    let path_str = path.to_string_lossy().to_string();
                    match renderer.load_model(&path_str) {
                        Ok(_) => { loaded_stems.insert(stem); }
                        Err(e) => log::warn!("Failed to load '{}': {}", path_str, e),
                    }
                }
                _ => {}
            }
        }
    } else {
        log::info!("No assets/models/ directory found; using built-in meshes only");
    }

    let (render_tx, render_rx) = channel::bounded::<RenderMessage>(3);
    let render_tx_shutdown = render_tx.clone();
    let (main_tx, main_rx) = channel::unbounded::<MainMessage>();

    let render_thread = std::thread::Builder::new()
        .name("render".into())
        .spawn(move || {
            let mut renderer = renderer;
            loop {
                match render_rx.recv() {
                    Ok(RenderMessage::Frame(frame, events)) => {
                        if let Some(cmd) = renderer.render(&frame, &events) {
                            let _ = main_tx.send(MainMessage::ConsoleCommand(cmd));
                        }
                    }
                    Ok(RenderMessage::Shutdown) => break,
                    Err(_) => break,
                }
            }
        })
        .expect("failed to spawn render thread");

    let (world, player_entity, dynamic_entities, physics, audio) = load_or_create_level();
    let mut engine = WaranerEngine::new(
        config::WaranerConfig::from_env_and_args(),
        window,
        world,
        player_entity,
        dynamic_entities,
        physics,
        audio,
        std::path::PathBuf::from("scripts"),
    );

    match engine.init_scripting() {
        Ok(()) => log::info!("Lua scripting initialized"),
        Err(e) => log::warn!("Lua scripting init skipped: {}", e),
    }

    let _ = event_loop.run(move |event, elwt| {
        engine.handle_event(&event, elwt);

        match event {
            winit::event::Event::AboutToWait => {
                while let Ok(msg) = main_rx.try_recv() {
                    match msg {
                        MainMessage::ConsoleCommand(cmd) => {
                            engine.handle_console_command(&cmd);
                        }
                    }
                }

                let input_events = engine.drain_input_events();
                if let Some(frame) = engine.build_frame() {
                    let _ = render_tx.send(RenderMessage::Frame(frame, input_events));
                }

                elwt.set_control_flow(winit::event_loop::ControlFlow::Poll);
            }
            _ => {}
        }
    });

    let _ = render_tx_shutdown.send(RenderMessage::Shutdown);
    let _ = render_thread.join();
}
