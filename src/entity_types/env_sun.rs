use crate::ecs::*;
use crate::entity_types::EntityTemplate;

fn build_env_sun(world: &mut World, entity: Entity) {
    world.add_transform(entity, Transform3D::default());
    world.add_sun_light(entity, SunLight::default());
}

pub static ENV_SUN: EntityTemplate = EntityTemplate {
    name: "env.sun",
    category: "Environment",
    display_name: "Sun",
    build: build_env_sun,
};
