use crate::ecs::*;
use crate::entity_types::EntityTemplate;

fn build_info_spawn(world: &mut World, entity: Entity) {
    world.add_transform(entity, Transform3D::default());
    world.add_velocity_3d(entity, Velocity3D::default());
    world.add_rigid_body(entity, RigidBody {
        mass: 1.0,
        restitution: 0.0,
        angular_damping: 0.95,
    });
    world.add_collider(entity, Collider::default());
    world.add_player(entity);
    world.add_camera(entity, Camera::default());
    world.add_audio_listener(entity);
}

pub static INFO_SPAWN: EntityTemplate = EntityTemplate {
    name: "info.spawn",
    category: "Gameplay",
    display_name: "Spawn Point",
    build: build_info_spawn,
};
