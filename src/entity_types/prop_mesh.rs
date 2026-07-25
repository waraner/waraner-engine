use crate::ecs::*;
use crate::entity_types::EntityTemplate;

fn build_prop_mesh(world: &mut World, entity: Entity) {
    world.add_transform(entity, Transform3D::default());
}

pub static PROP_MESH: EntityTemplate = EntityTemplate {
    name: "prop.mesh",
    category: "Props",
    display_name: "Static Mesh",
    build: build_prop_mesh,
};
