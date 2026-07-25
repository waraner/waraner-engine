use crate::ecs::*;
use crate::entity_types::EntityTemplate;

fn build_light_point(world: &mut World, entity: Entity) {
    world.add_transform(entity, Transform3D::default());
}

pub static LIGHT_POINT: EntityTemplate = EntityTemplate {
    name: "light.point",
    category: "Environment",
    display_name: "Point Light",
    build: build_light_point,
};
