use std::any::{Any, TypeId};
use std::collections::HashMap;

use crate::audio::{BusType, PlayMode};
use glam::{Quat, Vec3};

// ============================================================================
// InputState
// ============================================================================

#[derive(Default, Clone)]
pub struct InputState {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub mouse_dx: f64,
    pub mouse_dy: f64,
    pub pointer_locked: bool,
    pub keys: HashMap<String, bool>,
}

// ============================================================================
// Entity — 64-bit generational handle
// ============================================================================
// Layout: 48-bit slab index + 16-bit generation (matches techspec §6).
// Public fields index/generation are kept for backward compatibility.

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Entity {
    pub index: u32,
    pub generation: u32,
}

impl Entity {
    #[inline]
    pub fn new(index: u32, generation: u32) -> Self {
        Self { index, generation }
    }

    #[inline]
    pub fn to_u64(self) -> u64 {
        (self.generation as u64) << 48 | self.index as u64
    }

    #[inline]
    pub fn from_u64(raw: u64) -> Self {
        Self {
            index: raw as u32,
            generation: (raw >> 48) as u32,
        }
    }
}

// ============================================================================
// EntityIndex — slab with generation tracking (techspec §6)
// ============================================================================

#[derive(Clone, Debug)]
struct EntitySlot {
    generation: u32,
    alive: bool,
}

#[derive(Clone, Debug)]
struct EntityIndex {
    slots: Vec<EntitySlot>,
    free_list: Vec<u32>,
    next_id: u32,
}

impl EntityIndex {
    fn new() -> Self {
        Self {
            slots: Vec::new(),
            free_list: Vec::new(),
            next_id: 0,
        }
    }

    fn alloc(&mut self) -> Entity {
        if let Some(id) = self.free_list.pop() {
            let slot = &mut self.slots[id as usize];
            slot.alive = true;
            slot.generation += 1;
            Entity::new(id, slot.generation)
        } else {
            let id = self.next_id;
            self.next_id += 1;
            let generation = 1;
            self.slots.push(EntitySlot {
                generation,
                alive: true,
            });
            Entity::new(id, generation)
        }
    }

    fn alloc_at(&mut self, entity: Entity) {
        let id = entity.index as usize;
        if id >= self.slots.len() {
            self.slots.resize_with(id + 1, || EntitySlot {
                generation: 0,
                alive: false,
            });
        }
        let slot = &mut self.slots[id];
        slot.alive = true;
        slot.generation = entity.generation;
        if id >= self.next_id as usize {
            self.next_id = id as u32 + 1;
        }
        self.free_list.retain(|&f| f != id as u32);
    }

    fn free(&mut self, entity: Entity) -> bool {
        let id = entity.index as usize;
        if let Some(slot) = self.slots.get_mut(id) {
            if slot.alive && slot.generation == entity.generation {
                slot.alive = false;
                self.free_list.push(entity.index);
                return true;
            }
        }
        false
    }

    fn is_alive(&self, entity: Entity) -> bool {
        self.slots
            .get(entity.index as usize)
            .map(|s| s.alive && s.generation == entity.generation)
            .unwrap_or(false)
    }

    fn generation(&self, index: u32) -> u32 {
        self.slots
            .get(index as usize)
            .map(|s| s.generation)
            .unwrap_or(0)
    }
}

// ============================================================================
// ArchetypeId — sorted component signature
// ============================================================================

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArchetypeId {
    components: Vec<TypeId>,
}

impl ArchetypeId {
    #[inline]
    pub fn new(mut components: Vec<TypeId>) -> Self {
        components.sort();
        components.dedup();
        Self { components }
    }

    fn with(&self, type_id: TypeId) -> Self {
        let mut c = self.components.clone();
        if !c.contains(&type_id) {
            c.push(type_id);
            c.sort();
        }
        Self { components: c }
    }

    fn without(&self, type_id: &TypeId) -> Self {
        let c: Vec<TypeId> = self
            .components
            .iter()
            .filter(|t| *t != type_id)
            .copied()
            .collect();
        Self { components: c }
    }

    fn contains(&self, type_id: &TypeId) -> bool {
        self.components.contains(type_id)
    }

    fn is_superset_of(&self, required: &[TypeId]) -> bool {
        required.iter().all(|t| self.components.contains(t))
    }
}

// ============================================================================
// ClonableAny — type-erased cloning for component storage
// ============================================================================

trait ClonableAny: Send {
    fn clone_box(&self) -> Box<dyn ClonableAny>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Clone + Send + 'static> ClonableAny for T {
    fn clone_box(&self) -> Box<dyn ClonableAny> {
        Box::new(self.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ============================================================================
// ComponentColumn — type-erased SoA storage (techspec §7)
// ============================================================================

struct ComponentColumn {
    values: Vec<Box<dyn ClonableAny>>,
}

impl ComponentColumn {
    fn new<T: 'static>() -> Self {
        Self {
            values: Vec::new(),
        }
    }

    fn push_default<T: Default + Clone + Send + 'static>(&mut self) {
        self.values.push(Box::new(T::default()));
    }

    fn push_value<T: Clone + Send + 'static>(&mut self, val: &T) {
        self.values.push(Box::new(val.clone()));
    }

    fn copy_from(&mut self, other: &ComponentColumn, row: usize) {
        if let Some(val) = other.values.get(row) {
            self.values.push(val.clone_box());
        }
    }

    fn get<T: 'static>(&self, row: usize) -> Option<&T> {
        self.values
            .get(row)
            .and_then(|v| v.as_any().downcast_ref::<T>())
    }

    fn get_mut<T: 'static>(&mut self, row: usize) -> Option<&mut T> {
        self.values
            .get_mut(row)
            .and_then(|v| v.as_any_mut().downcast_mut::<T>())
    }

    fn write_value<T: Clone + Send + 'static>(&mut self, row: usize, val: &T) {
        if row < self.values.len() {
            self.values[row] = Box::new(val.clone());
        }
    }

    fn swap_remove(&mut self, row: usize) {
        let last = self.values.len() - 1;
        if row != last {
            self.values.swap(row, last);
        }
        self.values.pop();
    }

    fn len(&self) -> usize {
        self.values.len()
    }
}

impl Clone for ComponentColumn {
    fn clone(&self) -> Self {
        ComponentColumn {
            values: self.values.iter().map(|v| v.clone_box()).collect(),
        }
    }
}

impl std::fmt::Debug for ComponentColumn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComponentColumn")
            .field("len", &self.values.len())
            .finish()
    }
}

// ============================================================================
// Archetype — entities sharing the same component signature (techspec §7)
// ============================================================================

#[derive(Clone, Debug)]
struct Archetype {
    id: ArchetypeId,
    entities: Vec<Entity>,
    columns: HashMap<TypeId, ComponentColumn>,
}

impl Archetype {
    fn new(id: ArchetypeId) -> Self {
        Self {
            id,
            entities: Vec::new(),
            columns: HashMap::new(),
        }
    }

    fn ensure_column<T: Default + Clone + Send + 'static>(&mut self) {
        let tid = TypeId::of::<T>();
        if !self.columns.contains_key(&tid) {
            self.columns.insert(tid, ComponentColumn::new::<T>());
        }
    }

    fn push_entity<T: Default + Clone + Send + 'static>(&mut self, entity: Entity, source: &Archetype, src_row: usize) {
        self.ensure_column::<T>();
        self.entities.push(entity);
        for (tid, col) in &mut self.columns {
            if tid == &TypeId::of::<T>() {
                col.push_default::<T>();
            } else if let Some(src_col) = source.columns.get(tid) {
                col.copy_from(src_col, src_row);
            }
        }
    }

    fn swap_remove(&mut self, row: usize) -> Option<Entity> {
        if row >= self.entities.len() {
            return None;
        }
        let last = self.entities.len() - 1;
        let moved = if row != last {
            self.entities.swap(row, last);
            for col in self.columns.values_mut() {
                col.swap_remove(row);
            }
            Some(self.entities[row])
        } else {
            for col in self.columns.values_mut() {
                col.swap_remove(row);
            }
            None
        };
        self.entities.pop();
        moved
    }

    fn row_of(&self, entity: Entity) -> Option<usize> {
        self.entities.iter().position(|e| *e == entity)
    }

    fn entity_count(&self) -> usize {
        self.entities.len()
    }
}

// ============================================================================
// System trait + SystemGraph (techspec §7)
// ============================================================================

pub trait System: Send + 'static {
    fn name(&self) -> &'static str;
    fn run(&mut self, world: &mut World, dt: f32);
}

pub struct SystemGraph {
    systems: Vec<Box<dyn System>>,
}

impl SystemGraph {
    pub fn new() -> Self {
        Self {
            systems: Vec::new(),
        }
    }

    pub fn add<S: System>(&mut self, system: S) {
        self.systems.push(Box::new(system));
    }

    pub fn run(&mut self, world: &mut World, dt: f32) {
        for system in &mut self.systems {
            system.run(world, dt);
        }
    }

    pub fn system_count(&self) -> usize {
        self.systems.len()
    }

    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }
}

impl Default for SystemGraph {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// World — archetype-based ECS (techspec §7)
// ============================================================================

pub struct World {
    entity_index: EntityIndex,
    archetypes: HashMap<ArchetypeId, Archetype>,
    names: HashMap<Entity, String>,
    entity_types: HashMap<Entity, String>,
}

impl World {
    #[inline]
    pub fn new() -> Self {
        Self {
            entity_index: EntityIndex::new(),
            archetypes: HashMap::new(),
            names: HashMap::new(),
            entity_types: HashMap::new(),
        }
    }

    // ------------------------------------------------------------------
    // Entity lifecycle
    // ------------------------------------------------------------------

    pub fn spawn(&mut self) -> Entity {
        let entity = self.entity_index.alloc();
        let empty_id = ArchetypeId::new(vec![]);
        let arch = self
            .archetypes
            .entry(empty_id.clone())
            .or_insert_with(|| Archetype::new(empty_id));
        arch.entities.push(entity);
        entity
    }

    pub fn despawn(&mut self, entity: Entity) {
        if !self.entity_index.is_alive(entity) {
            return;
        }
        self.names.remove(&entity);
        self.entity_types.remove(&entity);
        for arch in self.archetypes.values_mut() {
            if let Some(row) = arch.row_of(entity) {
                let moved = arch.swap_remove(row);
                if let Some(moved_entity) = moved {
                    // The moved entity's row didn't change conceptually, but
                    // its position in the entities vec changed. No action needed
                    // because we don't store row in EntityIndex.
                    let _ = moved_entity;
                }
                break;
            }
        }
        self.entity_index.free(entity);
    }

    pub fn entities(&self) -> Vec<Entity> {
        let mut result = Vec::new();
        for arch in self.archetypes.values() {
            result.extend(arch.entities.iter().copied());
        }
        result
    }

    // Create entity at a specific index/generation (for WMAP loading)
    pub fn create_entity_at(&mut self, entity: Entity) {
        if !self.entity_index.is_alive(entity) {
            self.entity_index.alloc_at(entity);
            let empty_id = ArchetypeId::new(vec![]);
            let arch = self
                .archetypes
                .entry(empty_id.clone())
                .or_insert_with(|| Archetype::new(empty_id));
            if !arch.entities.contains(&entity) {
                arch.entities.push(entity);
            }
        }
    }

    // ------------------------------------------------------------------
    // Names
    // ------------------------------------------------------------------

    pub fn set_name(&mut self, e: Entity, name: &str) {
        if name.is_empty() {
            self.names.remove(&e);
        } else {
            self.names.insert(e, name.to_string());
        }
    }

    pub fn get_name(&self, e: &Entity) -> Option<&String> {
        self.names.get(e)
    }

    pub fn get_name_or(&self, e: &Entity, default: &str) -> String {
        self.names
            .get(e)
            .map(|s| s.clone())
            .unwrap_or_else(|| default.to_string())
    }

    // ------------------------------------------------------------------
    // Entity types (prefab / template name)
    // ------------------------------------------------------------------

    pub fn set_entity_type(&mut self, e: Entity, type_name: &str) {
        if type_name.is_empty() {
            self.entity_types.remove(&e);
        } else {
            self.entity_types.insert(e, type_name.to_string());
        }
    }

    pub fn get_entity_type(&self, e: Entity) -> Option<&String> {
        self.entity_types.get(&e)
    }

    // ------------------------------------------------------------------
    // Core generic component API
    // ------------------------------------------------------------------

    fn add_component_internal<T: Default + Clone + Send + 'static>(&mut self, entity: Entity, val: Option<T>) {
        if !self.entity_index.is_alive(entity) {
            return;
        }

        let tid = TypeId::of::<T>();

        // Find current archetype ID (immutable borrow only).
        let old_id = match self.find_archetype_id_for_entity(entity) {
            Some(id) => id,
            None => return,
        };

        // Already has this component? Just update in place.
        if old_id.contains(&tid) {
            if let Some(v) = val {
                if let Some(arch) = self.archetypes.get_mut(&old_id) {
                    if let Some(row) = arch.row_of(entity) {
                        if let Some(col) = arch.columns.get_mut(&tid) {
                            col.write_value(row, &v);
                        }
                    }
                }
            }
            return;
        }

        // Clone the old archetype so we can read entity data without
        // holding a borrow across the HashMap insertion (techspec §7 migration).
        let old_arch = match self.archetypes.get(&old_id) {
            Some(a) => a.clone(),
            None => return,
        };
        let src_row = match old_arch.row_of(entity) {
            Some(r) => r,
            None => return,
        };

        // Build the new archetype signature.
        let new_id = old_id.with(tid);

        // Insert (or get existing) new archetype.
        let new_arch = self
            .archetypes
            .entry(new_id.clone())
            .or_insert_with(|| Archetype::new(new_id));

        // Ensure ALL columns that the new archetype signature requires.
        // These are: every column from the old archetype + the new T column.
        for (tid, _) in &old_arch.columns {
            if !new_arch.columns.contains_key(tid) {
                new_arch.columns.insert(*tid, ComponentColumn::new::<T>());
            }
        }
        if !new_arch.columns.contains_key(&tid) {
            new_arch.columns.insert(tid, ComponentColumn::new::<T>());
        }

        new_arch.entities.push(entity);

        for (col_tid, col) in &mut new_arch.columns {
            if col_tid == &tid {
                // Push the value (or default) for the new component.
                if let Some(ref v) = val {
                    col.push_value(v);
                } else {
                    col.push_default::<T>();
                }
            } else if let Some(old_col) = old_arch.columns.get(col_tid) {
                // Copy existing component data from the old archetype.
                col.copy_from(old_col, src_row);
            }
        }

        // Remove entity from the old archetype.
        if let Some(old_arch) = self.archetypes.get_mut(&old_id) {
            if let Some(row) = old_arch.row_of(entity) {
                old_arch.swap_remove(row);
            }
        }
    }

    pub fn add_component<T: Default + Clone + Send + 'static>(&mut self, entity: Entity, component: T) {
        self.add_component_internal::<T>(entity, Some(component));
    }

    pub fn get_component<T: 'static>(&self, entity: Entity) -> Option<&T> {
        if !self.entity_index.is_alive(entity) {
            return None;
        }
        let tid = TypeId::of::<T>();
        for arch in self.archetypes.values() {
            if let Some(row) = arch.row_of(entity) {
                if let Some(col) = arch.columns.get(&tid) {
                    return col.get::<T>(row);
                }
                return None;
            }
        }
        None
    }

    pub fn get_component_mut<T: 'static>(&mut self, entity: Entity) -> Option<&mut T> {
        if !self.entity_index.is_alive(entity) {
            return None;
        }
        let tid = TypeId::of::<T>();
        for arch in self.archetypes.values_mut() {
            if let Some(row) = arch.row_of(entity) {
                if let Some(col) = arch.columns.get_mut(&tid) {
                    return col.get_mut::<T>(row);
                }
                return None;
            }
        }
        None
    }

    pub fn has_component<T: 'static>(&self, entity: Entity) -> bool {
        self.get_component::<T>(entity).is_some()
    }

    pub fn remove_component<T: 'static + Default>(&mut self, entity: Entity) {
        if !self.entity_index.is_alive(entity) {
            return;
        }
        let tid = TypeId::of::<T>();
        let old_id = match self.find_archetype_id_for_entity(entity) {
            Some(id) => id,
            None => return,
        };
        if !old_id.contains(&tid) {
            return;
        }

        let new_id = old_id.without(&tid);
        if new_id.components.len() == old_id.components.len() {
            return;
        }

        // Clone old archetype data before any mutable access.
        let old_arch = match self.archetypes.get(&old_id) {
            Some(a) => a.clone(),
            None => return,
        };
        let src_row = match old_arch.row_of(entity) {
            Some(r) => r,
            None => return,
        };

        let new_arch = self
            .archetypes
            .entry(new_id.clone())
            .or_insert_with(|| Archetype::new(new_id.clone()));

        // Ensure columns exist for all types in the reduced signature
        // (all old columns except T).
        for (tid, _) in &old_arch.columns {
            if !new_id.contains(tid) {
                continue;
            }
            if !new_arch.columns.contains_key(tid) {
                new_arch.columns.insert(*tid, ComponentColumn::new::<T>());
            }
        }

        new_arch.entities.push(entity);
        for (col_tid, col) in &mut new_arch.columns {
            if let Some(old_col) = old_arch.columns.get(col_tid) {
                col.copy_from(old_col, src_row);
            }
        }

        // Remove from old archetype.
        if let Some(old_arch) = self.archetypes.get_mut(&old_id) {
            if let Some(row) = old_arch.row_of(entity) {
                old_arch.swap_remove(row);
            }
        }
    }

    // ------------------------------------------------------------------
    // Query
    // ------------------------------------------------------------------

    pub fn query(&self) -> Query<'_> {
        Query::new(self)
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    fn find_archetype_id_for_entity(&self, entity: Entity) -> Option<ArchetypeId> {
        for (id, arch) in &self.archetypes {
            if arch.entities.contains(&entity) {
                return Some(id.clone());
            }
        }
        None
    }

    // ------------------------------------------------------------------
    // Component registration (for introspection)
    // ------------------------------------------------------------------

    pub fn archetype_count(&self) -> usize {
        self.archetypes.len()
    }

    pub fn entity_count(&self) -> usize {
        let mut count = 0usize;
        for arch in self.archetypes.values() {
            count += arch.entity_count();
        }
        count
    }
}

impl Default for World {
    fn default() -> Self {
        Self::new()
    }
}



// ============================================================================
// Query (techspec §7)
// ============================================================================

pub struct Query<'w> {
    world: &'w World,
    required: Vec<TypeId>,
}

impl<'w> Query<'w> {
    pub fn new(world: &'w World) -> Self {
        Self {
            world,
            required: Vec::new(),
        }
    }

    pub fn with<T: 'static>(mut self) -> Self {
        let tid = TypeId::of::<T>();
        if !self.required.contains(&tid) {
            self.required.push(tid);
        }
        self
    }

    pub fn iter_entities(&self) -> Vec<Entity> {
        let mut results = Vec::new();
        'outer: for arch in self.world.archetypes.values() {
            if !arch.id.is_superset_of(&self.required) {
                continue 'outer;
            }
            results.extend(arch.entities.iter().copied());
        }
        results
    }
}

// ============================================================================
// Convenience wrapper methods for concrete component types
// ============================================================================

macro_rules! impl_get_set {
    ($get_name:ident, $get_mut_name:ident, $add_name:ident, $ty:ty) => {
        impl World {
            pub fn $get_name(&self, e: Entity) -> Option<$ty> {
                self.get_component::<$ty>(e).copied()
            }

            pub fn $get_mut_name(&mut self, e: Entity) -> Option<&mut $ty> {
                self.get_component_mut::<$ty>(e)
            }

            pub fn $add_name(&mut self, e: Entity, val: $ty) {
                self.add_component::<$ty>(e, val);
            }
        }
    };
}

macro_rules! impl_get_set_clone {
    ($get_name:ident, $get_mut_name:ident, $add_name:ident, $ty:ty) => {
        impl World {
            pub fn $get_name(&self, e: Entity) -> Option<$ty> {
                self.get_component::<$ty>(e).cloned()
            }

            pub fn $get_mut_name(&mut self, e: Entity) -> Option<&mut $ty> {
                self.get_component_mut::<$ty>(e)
            }

            pub fn $add_name(&mut self, e: Entity, val: $ty) {
                self.add_component::<$ty>(e, val);
            }
        }
    };
}

macro_rules! impl_tag_get_set {
    ($get_name:ident, $add_name:ident, $is_name:ident, $ty:ty) => {
        impl World {
            pub fn $add_name(&mut self, e: Entity) {
                self.add_component::<$ty>(e, <$ty>::default());
            }

            pub fn $is_name(&self, e: Entity) -> bool {
                self.has_component::<$ty>(e)
            }
        }
    };
}

// --- Copy components ---
impl_get_set!(get_transform, get_transform_mut, add_transform, Transform3D);
impl_get_set!(get_velocity_3d, get_velocity_3d_mut, add_velocity_3d, Velocity3D);
impl_get_set!(get_angular_velocity, get_angular_velocity_mut, add_angular_velocity, AngularVelocity);
impl_get_set!(get_color, get_color_mut, set_color, Color);
impl_get_set!(get_rigid_body, get_rigid_body_mut, add_rigid_body, RigidBody);
impl_get_set!(get_collider, get_collider_mut, add_collider, Collider);

// --- Clone components ---
impl_get_set_clone!(get_camera, get_camera_mut, add_camera, Camera);
impl_get_set_clone!(get_model, get_model_mut, add_model, Model);
impl_get_set_clone!(get_script, get_script_mut, add_script, ScriptComponent);
impl_get_set_clone!(get_audio_source, get_audio_source_mut, add_audio_source, AudioSourceComponent);
impl_get_set_clone!(get_sun_light, get_sun_light_mut, add_sun_light, SunLight);
impl_get_set!(get_sky_settings, get_sky_settings_mut, add_sky_settings, SkySettings);

// --- Tag components ---
impl_tag_get_set!(is_player, add_player, is_player, Player);
impl_tag_get_set!(is_ground, add_ground, is_ground, Ground);
impl_tag_get_set!(is_static, add_static, is_static, Static);
impl_tag_get_set!(is_sensor, add_sensor, is_sensor, Sensor);
impl_tag_get_set!(is_audio_listener, add_audio_listener, is_audio_listener, AudioListenerComponent);

// ============================================================================
// Component type definitions
// ============================================================================

#[derive(Copy, Clone, Debug)]
pub struct Transform3D {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Default for Transform3D {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Velocity3D {
    pub linear: Vec3,
}

impl Default for Velocity3D {
    fn default() -> Self {
        Self {
            linear: Vec3::ZERO,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct AngularVelocity {
    pub radians: Vec3,
}

impl Default for AngularVelocity {
    fn default() -> Self {
        Self {
            radians: Vec3::ZERO,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Color {
    pub rgba: [f32; 4],
}

impl Default for Color {
    fn default() -> Self {
        Self {
            rgba: [1.0, 1.0, 1.0, 1.0],
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct RigidBody {
    pub mass: f32,
    pub restitution: f32,
    pub angular_damping: f32,
}

impl Default for RigidBody {
    fn default() -> Self {
        Self {
            mass: 1.0,
            restitution: 0.3,
            angular_damping: 0.95,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct Collider {
    pub half_extents: Vec3,
}

impl Default for Collider {
    fn default() -> Self {
        Self {
            half_extents: Vec3::new(0.5, 0.5, 0.5),
        }
    }
}

#[derive(Default, Copy, Clone, Debug)]
pub struct Player;

#[derive(Default, Copy, Clone, Debug)]
pub struct Ground;

#[derive(Default, Copy, Clone, Debug)]
pub struct Static;

#[derive(Default, Copy, Clone, Debug)]
pub struct Sensor;

#[derive(Clone, Debug)]
pub struct AudioSourceComponent {
    pub clip: String,
    pub volume: f32,
    pub looping: bool,
    pub bus: BusType,
    pub mode: PlayMode,
}

impl Default for AudioSourceComponent {
    fn default() -> Self {
        Self {
            clip: String::new(),
            volume: 1.0,
            looping: false,
            bus: BusType::Sfx,
            mode: PlayMode::Buffered,
        }
    }
}

#[derive(Default, Copy, Clone, Debug)]
pub struct AudioListenerComponent;

#[derive(Clone, Debug, Default)]
pub struct Model {
    pub path: String,
}

#[derive(Clone, Debug, Default)]
pub struct ScriptComponent {
    pub script_name: String,
}

#[derive(Copy, Clone, Debug)]
pub enum CameraMode {
    ThirdPerson,
    FirstPerson,
    FreeLook,
}

#[derive(Copy, Clone, Debug)]
pub struct Camera {
    pub mode: CameraMode,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub height: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            mode: CameraMode::ThirdPerson,
            yaw: 0.0,
            pitch: -0.3,
            distance: 8.0,
            height: 4.0,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct SunLight {
    pub color: [f32; 3],
    pub intensity: f32,
}

impl Default for SunLight {
    fn default() -> Self {
        Self {
            color: [1.0, 0.95, 0.85],
            intensity: 1.0,
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub struct SkySettings {
    pub color: [f32; 3],
    pub brightness: f32,
    pub indirect_light_multiplier: f32,
    pub sky_color: [f32; 3],
    pub sky_intensity: f32,
    pub sky_ibl_scale: f32,
    pub skybox_bounce_multiplier: f32,
}

impl Default for SkySettings {
    fn default() -> Self {
        Self {
            color: [0.4, 0.6, 0.9],
            brightness: 1.0,
            indirect_light_multiplier: 1.0,
            sky_color: [0.4, 0.6, 0.9],
            sky_intensity: 1.0,
            sky_ibl_scale: 1.0,
            skybox_bounce_multiplier: 1.0,
        }
    }
}

// :dsm delegate macro for the remaining getters that return mutable refs
// (already handled by impl_get_set and impl_get_set_clone macros above)

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_and_get_transform() {
        let mut world = World::new();
        let e = world.spawn();
        world.add_transform(
            e,
            Transform3D {
                position: Vec3::new(1.0, 2.0, 3.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        );
        let t = world.get_transform(e).unwrap();
        assert_eq!(t.position, Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn test_spawn_and_get_velocity() {
        let mut world = World::new();
        let e = world.spawn();
        world.add_velocity_3d(
            e,
            Velocity3D {
                linear: Vec3::new(5.0, -3.0, 2.0),
            },
        );
        let v = world.get_velocity_3d(e).unwrap();
        assert_eq!(v.linear, Vec3::new(5.0, -3.0, 2.0));
    }

    #[test]
    fn test_migrate_builds_archetype() {
        let mut world = World::new();
        let e = world.spawn();
        world.add_player(e);
        world.add_transform(e, Transform3D::default());
        world.add_rigid_body(e, RigidBody::default());
        world.add_collider(e, Collider::default());
        world.set_color(e, Color::default());

        assert!(world.get_transform(e).is_some());
        assert!(world.get_rigid_body(e).is_some());
    }

    #[test]
    fn test_query_with() {
        let mut world = World::new();
        let e = world.spawn();
        world.add_ground(e);
        world.add_transform(e, Transform3D::default());

        let grounds = world.query().with::<Ground>().iter_entities();
        assert_eq!(grounds.len(), 1);
        assert_eq!(grounds[0], e);
    }

    #[test]
    fn test_query_multi_component() {
        let mut world = World::new();
        let e = world.spawn();
        world.add_ground(e);
        world.add_transform(
            e,
            Transform3D {
                position: Vec3::new(0.0, -1.0, 0.0),
                rotation: Quat::IDENTITY,
                scale: Vec3::ONE,
            },
        );
        world.set_color(e, Color {
            rgba: [0.5, 0.5, 0.5, 1.0],
        });

        let entities = world
            .query()
            .with::<Ground>()
            .with::<Transform3D>()
            .with::<Color>()
            .iter_entities();
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0], e);

        let t = world.get_transform(e).unwrap();
        assert_eq!(t.position, Vec3::new(0.0, -1.0, 0.0));
        let c = world.get_color(e).unwrap();
        assert_eq!(c.rgba, [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn test_multiple_entities_distinct_archetypes() {
        let mut world = World::new();
        let a = world.spawn();
        let b = world.spawn();

        world.add_transform(a, Transform3D::default());
        world.add_velocity_3d(a, Velocity3D::default());

        world.add_transform(b, Transform3D::default());
        world.add_rigid_body(b, RigidBody::default());

        assert!(world.get_transform(a).is_some());
        assert!(world.get_velocity_3d(a).is_some());
        assert!(world.get_rigid_body(a).is_none());

        assert!(world.get_transform(b).is_some());
        assert!(world.get_rigid_body(b).is_some());
        assert!(world.get_velocity_3d(b).is_none());
    }

    #[test]
    fn test_create_entity_at_stable_id() {
        let mut world = World::new();
        let entity = Entity::new(42, 1);
        world.create_entity_at(entity);
        assert!(world.entity_index.is_alive(entity));

        world.add_transform(entity, Transform3D::default());
        assert!(world.get_transform(entity).is_some());
        assert_eq!(world.entities().len(), 1);
    }

    #[test]
    fn test_despawn() {
        let mut world = World::new();
        let e = world.spawn();
        world.add_transform(e, Transform3D::default());
        assert!(world.get_transform(e).is_some());

        world.despawn(e);
        assert!(world.get_transform(e).is_none());
        assert_eq!(world.entities().len(), 0);
    }

    #[test]
    fn test_entity_generation() {
        let mut world = World::new();
        let e = world.spawn();
        let gen1 = e.generation;

        world.despawn(e);
        let e2 = world.spawn();
        // The new entity using the same slot should have bumped generation
        assert!(e2.generation > gen1 || e2.index != e.index);
    }

    #[test]
    fn test_generic_component_api() {
        #[derive(Copy, Clone, Debug, Default, PartialEq)]
        struct Health(f32);

        let mut world = World::new();
        let e = world.spawn();

        world.add_component(e, Health(100.0));
        assert_eq!(world.get_component::<Health>(e), Some(&Health(100.0)));

        if let Some(h) = world.get_component_mut::<Health>(e) {
            h.0 = 75.0;
        }
        assert_eq!(world.get_component::<Health>(e), Some(&Health(75.0)));

        assert!(world.has_component::<Health>(e));
        world.remove_component::<Health>(e);
        assert!(!world.has_component::<Health>(e));
    }

    #[test]
    fn test_system_graph() {
        struct CounterSystem {
            count: u32,
        }
        impl System for CounterSystem {
            fn name(&self) -> &'static str {
                "counter"
            }
            fn run(&mut self, _world: &mut World, _dt: f32) {
                self.count += 1;
            }
        }

        let mut world = World::new();
        let mut graph = SystemGraph::new();
        graph.add(CounterSystem { count: 0 });
        graph.run(&mut world, 1.0);
        graph.run(&mut world, 1.0);
        assert_eq!(graph.system_count(), 1);
    }

    #[test]
    fn test_entity_to_u64_roundtrip() {
        let e = Entity::new(12345, 3);
        let raw = e.to_u64();
        let e2 = Entity::from_u64(raw);
        assert_eq!(e, e2);
    }

    #[test]
    fn test_remove_component_migration() {
        let mut world = World::new();
        let e = world.spawn();
        world.add_transform(e, Transform3D::default());
        world.add_rigid_body(e, RigidBody::default());

        assert!(world.has_component::<Transform3D>(e));
        assert!(world.has_component::<RigidBody>(e));

        world.remove_component::<RigidBody>(e);

        assert!(world.has_component::<Transform3D>(e));
        assert!(!world.has_component::<RigidBody>(e));
    }
}
