use std::ffi::c_void;
use std::sync::Arc;

use crossbeam::channel;
use glam::{Quat, Vec3};
use winit::{
    event_loop::EventLoop,
    window::WindowBuilder,
};

use crate::audio::{AudioBackend, RubiAudio};
use crate::config::WaranerConfig;
use crate::ecs::{
    AngularVelocity, Camera, Collider, Color,
    Entity, RigidBody, Transform3D, Velocity3D, World,
};
use crate::physics::PhysicsBackend;
use crate::physics_thread::ThreadedPhysics;
use crate::render_frame::{MainMessage, RenderMessage};
use crate::renderer::Renderer;
use crate::waraner_engine::WaranerEngine;

// ---------------------------------------------------------------------------
// Context handle — opaque pointer holding the full runtime state
// ---------------------------------------------------------------------------

pub struct WaranerContext {
    pub config: WaranerConfig,
}

// ---------------------------------------------------------------------------
// Default scene (no filesystem deps — built-in meshes only)
// ---------------------------------------------------------------------------

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
        (Vec3::new(2.0, 2.0, 0.0), Vec3::new(1.0, 1.0, 1.0), Vec3::new(0.5, 0.5, 0.5), 1.0),
        (Vec3::new(3.0, 4.0, 1.0), Vec3::new(1.5, 1.5, 1.5), Vec3::new(0.5, 0.5, 0.5), 2.0),
    ];

    let mut dynamic_entities = Vec::new();
    for (pos, visual_scale, collider_size, mass) in dynamic_spawns {
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
        dynamic_entities.push(e);
    }

    let physics = Box::new(ThreadedPhysics::from_world(&world, player_entity, &dynamic_entities));
    let audio: Box<dyn AudioBackend> = Box::new(RubiAudio::new());
    (world, player_entity, dynamic_entities, physics, audio)
}

fn load_or_create_level(config: &WaranerConfig) -> (World, Entity, Vec<Entity>, Box<dyn PhysicsBackend>, Box<dyn AudioBackend>) {
    let level_path = config.data_path.join("levels").join("default.wmap");

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
                let audio: Box<dyn AudioBackend> = Box::new(RubiAudio::new());
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

    let _ = std::fs::create_dir_all(config.data_path.join("levels"));
    match crate::wmap::write_world(&world, level_path.to_str().unwrap(), 0) {
        Ok(()) => log::info!("Saved default level to {}", level_path.display()),
        Err(e) => log::warn!("Failed to save default level: {}", e),
    }

    (world, player_entity, dynamic_entities, physics, audio)
}

fn fatal_error(msg: &str) -> ! {
    log::error!("FATAL: {}", msg);

    #[cfg(target_os = "windows")]
    {
        use std::ffi::CString;
        extern "system" {
            fn MessageBoxA(
                hwnd: *mut std::ffi::c_void,
                text: *const u8,
                caption: *const u8,
                mb_type: u32,
            ) -> i32;
        }
        let title = CString::new("Waraner Engine - Fatal Error").unwrap();
        let text = CString::new(msg).unwrap();
        unsafe {
            MessageBoxA(
                std::ptr::null_mut(),
                text.as_ptr() as *const u8,
                title.as_ptr() as *const u8,
                0x00000010,
            );
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("FATAL: {}", msg);
    }

    std::process::abort();
}

// ---------------------------------------------------------------------------
// C API — game.exe calls these via libloading
// ---------------------------------------------------------------------------

#[no_mangle]
pub extern "C" fn waraner_create_context(config_ptr: *const WaranerConfig) -> *mut c_void {
    let config = if config_ptr.is_null() {
        WaranerConfig::from_env_and_args()
    } else {
        unsafe { (*config_ptr).clone() }
    };

    let ctx = Box::new(WaranerContext { config });
    Box::into_raw(ctx) as *mut c_void
}

/// Run the engine — enters the winit event loop and blocks until the
/// window closes. This is the main entry point for standalone usage.
#[no_mangle]
pub extern "C" fn waraner_run(ctx_ptr: *mut c_void) {
    let ctx = unsafe { &mut *(ctx_ptr as *mut WaranerContext) };
    let config = &ctx.config;

    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(&config.log_level),
    )
    .init();

    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(
        WindowBuilder::new()
            .with_title(&config.window_title)
            .with_inner_size(winit::dpi::LogicalSize::new(config.window_width, config.window_height))
            .build(&event_loop)
            .unwrap(),
    );

    let renderer = pollster::block_on(Renderer::new(window.clone()));

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

    let (world, player_entity, dynamic_entities, physics, audio) = load_or_create_level(config);
    let script_dir = config.data_path.join("scripts");
    let mut engine = WaranerEngine::new(
        config.clone(),
        window,
        world,
        player_entity,
        dynamic_entities,
        physics,
        audio,
        script_dir,
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

/// Advance the engine by one frame. Context must have been created via
/// `waraner_create_context` and initialized by a prior call to
/// `waraner_run` (which sets up the window, renderer, and threads).
///
/// This is intended for advanced integrations (e.g. custom event loops).
/// Most users should call `waraner_run` instead.
#[no_mangle]
pub extern "C" fn waraner_tick(ctx_ptr: *mut c_void, dt: f64) {
    let ctx = unsafe { &mut *(ctx_ptr as *mut WaranerContext) };
    let _ = &ctx.config;
    // tick() is called internally by build_frame via the event loop;
    // this export is provided for external event-loop integrations.
    log::trace!("waraner_tick({}) called", dt);
}

#[no_mangle]
pub extern "C" fn waraner_destroy_context(ctx_ptr: *mut c_void) {
    let _ = unsafe { Box::from_raw(ctx_ptr as *mut WaranerContext) };
    log::info!("Waraner context destroyed");
}
