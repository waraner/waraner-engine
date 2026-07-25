use std::collections::HashMap;
use std::thread;

use crossbeam::channel;

use crate::constants::USE_NOBLE_PHYSICS;
use crate::ecs::{Collider, Entity, Ground, InputState, RigidBody, Transform3D, World};
use crate::physics::Box3DPhysics;
use crate::physics::NoblePhysics;
use crate::physics::{CollisionEvent, PhysicsBackend};

// ---------------------------------------------------------------------------
// Internal commands (main thread → physics thread)
// ---------------------------------------------------------------------------

enum PhysicsCommand {
    Tick {
        input: InputState,
        camera_yaw: f32,
        dt: f32,
        response: channel::Sender<PhysicsResult>,
    },
    AddEntity {
        entity: Entity,
        transform: Transform3D,
        collider: Collider,
        rigid_body: RigidBody,
        is_sensor: bool,
        is_player: bool,
    },
    RemoveEntity(Entity),
    TeleportPlayer {
        entity: Entity,
        position: glam::Vec3,
    },
    Shutdown,
}

// ---------------------------------------------------------------------------
// Results (physics thread → main thread)
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct PhysicsResult {
    pub transforms: HashMap<Entity, Transform3D>,
    pub collisions: Vec<CollisionEvent>,
}

// ---------------------------------------------------------------------------
// Raw physics thread handle
// ---------------------------------------------------------------------------

pub struct PhysicsThread {
    cmd_tx: channel::Sender<PhysicsCommand>,
    handle: Option<thread::JoinHandle<()>>,
}

impl PhysicsThread {
    pub fn new() -> Self {
        let (cmd_tx, cmd_rx) = channel::unbounded::<PhysicsCommand>();

        let handle = thread::Builder::new()
            .name("physics".into())
            .spawn(move || physics_main(cmd_rx))
            .expect("failed to spawn physics thread");

        Self {
            cmd_tx,
            handle: Some(handle),
        }
    }

    pub fn send_tick(&self, input: &InputState, camera_yaw: f32, dt: f32) -> PhysicsResult {
        let (tx, rx) = channel::bounded(1);
        let _ = self.cmd_tx.send(PhysicsCommand::Tick {
            input: input.clone(),
            camera_yaw,
            dt,
            response: tx,
        });
        rx.recv().unwrap_or_default()
    }

    pub fn add_entity(
        &self,
        entity: Entity,
        transform: Transform3D,
        collider: Collider,
        rigid_body: RigidBody,
        is_sensor: bool,
        is_player: bool,
    ) {
        let _ = self.cmd_tx.send(PhysicsCommand::AddEntity {
            entity,
            transform,
            collider,
            rigid_body,
            is_sensor,
            is_player,
        });
    }

    pub fn remove_entity(&self, entity: Entity) {
        let _ = self.cmd_tx.send(PhysicsCommand::RemoveEntity(entity));
    }

    pub fn teleport_player(&self, entity: Entity, position: glam::Vec3) {
        let _ = self.cmd_tx.send(PhysicsCommand::TeleportPlayer { entity, position });
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(PhysicsCommand::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for PhysicsThread {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---------------------------------------------------------------------------
// PhysicsBackend implementation that delegates to the dedicated thread.
// This is the drop-in replacement for direct Box3DPhysics usage.
// ---------------------------------------------------------------------------

pub struct ThreadedPhysics {
    thread: PhysicsThread,
    collision_events: Vec<CollisionEvent>,
}

impl ThreadedPhysics {
    pub fn new() -> Self {
        Self {
            thread: PhysicsThread::new(),
            collision_events: Vec::new(),
        }
    }

    /// Bootstrap physics bodies from an existing ECS world, matching the
    /// same entity set that `Box3DPhysics::new` would register.
    pub fn from_world(
        world: &World,
        player_entity: Entity,
        dynamic_entities: &[Entity],
    ) -> Self {
        let physics = Self::new();

        // Ground entities
        for entity in world.query().with::<Ground>().iter_entities() {
            if let (Some(t), Some(c)) = (world.get_transform(entity), world.get_collider(entity)) {
                let rb = world.get_rigid_body(entity)
                    .unwrap_or(RigidBody { mass: 0.0, restitution: 0.0, angular_damping: 1.0 });
                physics.thread.add_entity(entity, t, c, rb, world.is_sensor(entity), false);
            }
        }

        // Player entity
        if let (Some(t), Some(c)) = (world.get_transform(player_entity), world.get_collider(player_entity)) {
            let rb = world.get_rigid_body(player_entity)
                .unwrap_or(RigidBody { mass: 1.0, restitution: 0.0, angular_damping: 0.95 });
            physics.thread.add_entity(player_entity, t, c, rb, world.is_sensor(player_entity), true);
        }

        // Dynamic entities
        for &entity in dynamic_entities {
            if entity == player_entity { continue; }
            if let (Some(t), Some(c)) = (world.get_transform(entity), world.get_collider(entity)) {
                let rb = world.get_rigid_body(entity)
                    .unwrap_or(RigidBody { mass: 1.0, restitution: 0.2, angular_damping: 0.98 });
                physics.thread.add_entity(entity, t, c, rb, world.is_sensor(entity), false);
            }
        }

        physics
    }

    pub fn thread(&self) -> &PhysicsThread {
        &self.thread
    }
}

impl PhysicsBackend for ThreadedPhysics {
    fn tick(
        &mut self,
        world: &mut World,
        input: &InputState,
        _player_entity: Entity,
        camera_yaw: f32,
        _dynamic_entities: &[Entity],
        dt: f32,
    ) {
        let result = self.thread.send_tick(input, camera_yaw, dt);
        for (entity, transform) in result.transforms {
            world.add_transform(entity, transform);
        }
        self.collision_events = result.collisions;
    }

    fn teleport_player(&mut self, player_entity: Entity, position: glam::Vec3) {
        self.thread.teleport_player(player_entity, position);
    }

    fn remove_entity(&mut self, entity: Entity) {
        self.thread.remove_entity(entity);
    }

    fn drain_collision_events(&mut self) -> Vec<CollisionEvent> {
        std::mem::take(&mut self.collision_events)
    }
}

impl Drop for ThreadedPhysics {
    fn drop(&mut self) {
        self.thread.shutdown();
    }
}

// ---------------------------------------------------------------------------
// Physics thread main loop
// ---------------------------------------------------------------------------

fn physics_main(cmd_rx: channel::Receiver<PhysicsCommand>) {
    let mut physics: Option<Box<dyn PhysicsBackend>> = None;
    let mut player_entity: Option<Entity> = None;

    for cmd in &cmd_rx {
        match cmd {
            PhysicsCommand::Tick { input, camera_yaw, dt, response } => {
                let result = match physics.as_mut() {
                    Some(phys) => {
                        let player = match player_entity {
                            Some(p) => p,
                            None => {
                                let _ = response.send(PhysicsResult::default());
                                continue;
                            }
                        };
                        let (transforms, collisions) = phys.tick_raw(&input, player, camera_yaw, dt);
                        PhysicsResult { transforms, collisions }
                    }
                    None => PhysicsResult::default(),
                };
                let _ = response.send(result);
            }

            PhysicsCommand::AddEntity { entity, transform, collider, rigid_body, is_sensor, is_player } => {
                if is_player {
                    player_entity = Some(entity);
                }
                match physics.as_mut() {
                    Some(phys) => {
                        phys.add_body(entity, transform, collider, rigid_body, is_sensor, is_player);
                    }
                    None => {
                        let mut new_phys: Box<dyn PhysicsBackend> = if USE_NOBLE_PHYSICS {
                            Box::new(NoblePhysics::new(&World::new(), entity, &[]))
                        } else {
                            Box::new(Box3DPhysics::new(&World::new(), entity, &[]))
                        };
                        new_phys.add_body(entity, transform, collider, rigid_body, is_sensor, is_player);
                        physics = Some(new_phys);
                    }
                }
            }

            PhysicsCommand::RemoveEntity(entity) => {
                if let Some(phys) = physics.as_mut() {
                    phys.remove_entity(entity);
                }
                if player_entity == Some(entity) {
                    player_entity = None;
                }
            }

            PhysicsCommand::TeleportPlayer { entity, position } => {
                if let Some(phys) = physics.as_mut() {
                    phys.teleport_player(entity, position);
                }
            }

            PhysicsCommand::Shutdown => break,
        }
    }
}
