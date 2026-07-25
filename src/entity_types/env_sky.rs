use crate::ecs::*;
use crate::entity_types::EntityTemplate;

fn build_env_sky(world: &mut World, entity: Entity) {
    world.add_transform(entity, Transform3D::default());
    world.add_sky_settings(entity, SkySettings::default());
}

pub static ENV_SKY: EntityTemplate = EntityTemplate {
    name: "env.sky",
    category: "Environment",
    display_name: "Sky",
    build: build_env_sky,
};
