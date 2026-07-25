use crate::ecs::*;
use crate::entity_types::EntityTemplate;

fn build_env_fog(world: &mut World, entity: Entity) {
    world.add_transform(entity, Transform3D::default());
}

pub static ENV_FOG: EntityTemplate = EntityTemplate {
    name: "env.fog",
    category: "Environment",
    display_name: "Fog",
    build: build_env_fog,
};
