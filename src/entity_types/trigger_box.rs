use crate::ecs::*;
use crate::entity_types::EntityTemplate;

fn build_trigger_box(world: &mut World, entity: Entity) {
    world.add_transform(entity, Transform3D::default());
    world.add_collider(entity, Collider::default());
    world.add_sensor(entity);
}

pub static TRIGGER_BOX: EntityTemplate = EntityTemplate {
    name: "trigger.box",
    category: "Gameplay",
    display_name: "Trigger Volume",
    build: build_trigger_box,
};
