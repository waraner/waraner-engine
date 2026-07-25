use crate::ecs::*;
use crate::entity_types::EntityTemplate;

fn build_prop_physics(world: &mut World, entity: Entity) {
    world.add_transform(entity, Transform3D::default());
    world.add_velocity_3d(entity, Velocity3D::default());
    world.add_angular_velocity(entity, AngularVelocity::default());
    world.add_rigid_body(entity, RigidBody {
        mass: 1.0,
        restitution: 0.3,
        angular_damping: 0.95,
    });
    world.add_collider(entity, Collider::default());
}

pub static PROP_PHYSICS: EntityTemplate = EntityTemplate {
    name: "prop.physics",
    category: "Props",
    display_name: "Physics Prop",
    build: build_prop_physics,
};
