use std::collections::HashMap;
use std::sync::OnceLock;

use crate::ecs::*;

mod env_fog;
mod env_sky;
mod env_sun;
mod info_spawn;
mod light_point;
mod prop_mesh;
mod prop_physics;
mod trigger_box;

pub use self::env_fog::ENV_FOG;
pub use self::env_sky::ENV_SKY;
pub use self::env_sun::ENV_SUN;
pub use self::info_spawn::INFO_SPAWN;
pub use self::light_point::LIGHT_POINT;
pub use self::prop_mesh::PROP_MESH;
pub use self::prop_physics::PROP_PHYSICS;
pub use self::trigger_box::TRIGGER_BOX;

/// A recipe for creating an entity with a predefined set of components.
pub struct EntityTemplate {
    /// Dotted name, e.g. "prop.physics"
    pub name: &'static str,
    /// Editor category, e.g. "Props", "Environment", "Gameplay"
    pub category: &'static str,
    /// Human-readable label, e.g. "Physics Prop"
    pub display_name: &'static str,
    /// Applies component defaults to a newly spawned entity.
    pub build: fn(&mut World, Entity),
}

pub struct EntityTypeRegistry {
    by_name: HashMap<&'static str, &'static EntityTemplate>,
    by_category: HashMap<&'static str, Vec<&'static EntityTemplate>>,
}

impl EntityTypeRegistry {
    pub fn new() -> Self {
        Self {
            by_name: HashMap::new(),
            by_category: HashMap::new(),
        }
    }

    pub fn register(&mut self, tmpl: &'static EntityTemplate) {
        self.by_name.insert(tmpl.name, tmpl);
        self.by_category
            .entry(tmpl.category)
            .or_default()
            .push(tmpl);
    }

    pub fn get(&self, name: &str) -> Option<&EntityTemplate> {
        self.by_name.get(name).copied()
    }

    pub fn categories(&self) -> Vec<&&str> {
        let mut cats: Vec<&&str> = self.by_category.keys().collect();
        cats.sort();
        cats
    }

    pub fn templates_in_category(&self, category: &str) -> &[&'static EntityTemplate] {
        self.by_category
            .get(category)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Spawn a new entity with all components defined by the named template.
    pub fn spawn(&self, world: &mut World, name: &str) -> Option<Entity> {
        let tmpl = self.by_name.get(name)?;
        let entity = world.spawn();
        world.set_entity_type(entity, name);
        (tmpl.build)(world, entity);
        Some(entity)
    }
}

/// Return the singleton default entity type registry.
pub fn default_registry() -> &'static EntityTypeRegistry {
    static REGISTRY: OnceLock<EntityTypeRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut reg = EntityTypeRegistry::new();
        reg.register(&PROP_MESH);
        reg.register(&PROP_PHYSICS);
        reg.register(&ENV_SUN);
        reg.register(&ENV_SKY);
        reg.register(&ENV_FOG);
        reg.register(&INFO_SPAWN);
        reg.register(&TRIGGER_BOX);
        reg.register(&LIGHT_POINT);
        reg
    })
}

/// Look up a template by dotted name from the default registry.
pub fn get_template(name: &str) -> Option<&'static EntityTemplate> {
    default_registry().get(name)
}

/// Spawn an entity from a named template in the default registry.
pub fn spawn_type(world: &mut World, name: &str) -> Option<Entity> {
    default_registry().spawn(world, name)
}

/// Apply template defaults to an existing entity (used when loading from WMAP).
/// Returns `true` if the type was found and applied.
pub fn apply_type(world: &mut World, entity: Entity, name: &str) -> bool {
    match default_registry().get(name) {
        Some(tmpl) => {
            (tmpl.build)(world, entity);
            true
        }
        None => {
            log::warn!("Unknown entity type '{}' — no template applied", name);
            false
        }
    }
}
