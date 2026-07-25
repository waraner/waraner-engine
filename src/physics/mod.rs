use crate::ecs::{Entity, InputState, World};
use glam::Vec3;
use std::collections::HashMap;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct CollisionEvent {
    pub entity_a: Entity,
    pub entity_b: Entity,
    pub point: Vec3,
    pub normal: Vec3,
    pub approach_speed: f32,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct RayHit {
    pub entity: Option<Entity>,
    pub point: Vec3,
    pub normal: Vec3,
    pub fraction: f32,
}

pub trait PhysicsBackend {
    fn tick(
        &mut self,
        world: &mut World,
        input: &InputState,
        player_entity: Entity,
        camera_yaw: f32,
        dynamic_entities: &[Entity],
        dt: f32,
    );

    /// Raw tick used by the dedicated physics thread. Returns the transforms
    /// and collision events produced this step.
    fn tick_raw(
        &mut self,
        _input: &InputState,
        _player_entity: Entity,
        _camera_yaw: f32,
        _dt: f32,
    ) -> (HashMap<Entity, crate::ecs::Transform3D>, Vec<CollisionEvent>) {
        (std::collections::HashMap::new(), Vec::new())
    }

    /// Register a body with the backend (used by the threaded backing store).
    fn add_body(
        &mut self,
        _entity: Entity,
        _transform: crate::ecs::Transform3D,
        _collider: crate::ecs::Collider,
        _rigid_body: crate::ecs::RigidBody,
        _is_sensor: bool,
        _is_player: bool,
    ) {
    }

    fn teleport_player(&mut self, _player_entity: Entity, _position: glam::Vec3) {}
    fn remove_entity(&mut self, _entity: Entity) {}
    #[allow(dead_code)]
    fn drain_collision_events(&mut self) -> Vec<CollisionEvent> { Vec::new() }
    #[allow(dead_code)]
    fn ray_cast(&self, _origin: Vec3, _direction: Vec3, _max_distance: f32) -> Option<RayHit> { None }
}

pub mod player_ctl;

mod custom;
mod box3d;
mod noble;

pub use box3d::Box3DPhysics;
pub use noble::NoblePhysics;
