use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::{Quat, Vec3};
use mlua::{Function, Lua, RegistryKey, Table, Thread, Value};

use crate::physics::CollisionEvent;

use crate::audio::{AudioBackend, AudioHandle, BusType};
use crate::config::WaranerConfig;
use crate::ecs::{
    Color, Entity, InputState, Model, ScriptComponent, Transform3D, Velocity3D, World,
};

#[allow(dead_code)]
struct EngineRef {
    world: *mut World,
    input: *mut InputState,
    audio: *mut Box<dyn AudioBackend>,
    player: *mut Entity,
    dynamic: *mut Vec<Entity>,
    config: *mut WaranerConfig,
    fps: f32,
    dt: f32,
}
unsafe impl Send for EngineRef {}
unsafe impl Sync for EngineRef {}

fn decode_entity(table: &Table) -> Option<Entity> {
    let index: u32 = table.get("__entity_index").ok()?;
    let gen: u32 = table.get("__entity_gen").ok()?;
    Some(Entity::new(index, gen))
}

fn make_entity_table(lua: &Lua, entity: Entity, methods: &Table) -> Table {
    let t = lua.create_table().unwrap();
    let _ = t.set("__entity_index", entity.index);
    let _ = t.set("__entity_gen", entity.generation);
    let mt = lua.create_table().unwrap();
    let _ = mt.set("__index", methods.clone());
    let _ = t.set_metatable(Some(mt));
    t
}

fn audio_handle_from_table(t: &Table) -> Option<AudioHandle> {
    let h: Table = t.get("__handle").ok()?;
    let index: u32 = h.get("index").ok()?;
    let generation: u32 = h.get("generation").ok()?;
    Some(AudioHandle { index, generation })
}

fn set_debug_menu(lua: &Lua, value: bool) {
    let _ = lua.set_named_registry_value("__debug_menu", value);
}

fn is_debug_menu(lua: &Lua) -> bool {
    lua.named_registry_value("__debug_menu")
        .unwrap_or(false)
}

pub struct LuaEngine {
    lua: Lua,
    script_envs: HashMap<Entity, RegistryKey>,
    script_states: HashMap<Entity, RegistryKey>,
    threads: HashMap<Entity, RegistryKey>,
    pending_create: Vec<Entity>,
    ui_labels: Vec<String>,
    script_dir: PathBuf,
    methods_key: RegistryKey,
    fps: f32,
    dt: f32,
}

impl LuaEngine {
    pub fn new(script_dir: PathBuf) -> Self {
        let lua = Lua::new();
        let methods_key = lua
            .create_registry_value(lua.create_table().unwrap())
            .unwrap();
        Self {
            lua,
            script_envs: HashMap::new(),
            script_states: HashMap::new(),
            threads: HashMap::new(),
            pending_create: Vec::new(),
            ui_labels: Vec::new(),
            script_dir,
            methods_key,
            fps: 0.0,
            dt: 0.0,
        }
    }

    pub fn init(
        &mut self,
        world: &mut World,
        input: &mut InputState,
        audio: &mut Box<dyn AudioBackend>,
        player: &mut Entity,
        dynamic: &mut Vec<Entity>,
        config: &mut WaranerConfig,
    ) -> Result<(), String> {
        let engine_ref = EngineRef {
            world: world as *mut World,
            input: input as *mut InputState,
            audio: audio as *mut Box<dyn AudioBackend>,
            player: player as *mut Entity,
            dynamic: dynamic as *mut Vec<Entity>,
            config: config as *mut WaranerConfig,
            fps: 0.0,
            dt: 0.0,
        };
        self.lua.set_app_data(engine_ref);
        self.sandbox();
        self.register_engine_api()?;
        let _ = self.lua.set_memory_limit(64 * 1024 * 1024);
        Ok(())
    }

    fn sandbox(&self) {
        let globals = self.lua.globals();
        let dangerous = ["dofile", "loadfile", "require", "module", "os", "io", "debug"];
        for name in &dangerous {
            let _ = globals.raw_remove(*name);
        }

        // Restore debug.traceback but remove everything else from debug
        if let Ok(debug_tbl) = self.lua.globals().get::<Table>("debug") {
            if let Ok(traceback) = debug_tbl.get::<Function>("traceback") {
                let safe_debug = self.lua.create_table().unwrap();
                let _ = safe_debug.set("traceback", traceback);
                self.lua.globals().set("debug", safe_debug).ok();
            }
        }

        let _ = self
            .lua
            .set_named_registry_value("__instruction_count", 0u64);
        let _ = self
            .lua
            .set_named_registry_value("__instruction_limit", 10_000_000u64);

        let hook_fn = move |lua: &Lua, _debug: mlua::Debug| -> mlua::Result<mlua::VmState> {
            let limit: u64 = lua
                .named_registry_value("__instruction_limit")
                .unwrap_or(10_000_000);
            let count: u64 = lua
                .named_registry_value("__instruction_count")
                .unwrap_or(0);
            if count >= limit {
                return Err(mlua::Error::external(
                    "instruction limit exceeded per frame",
                ));
            }
            let _ = lua.set_named_registry_value("__instruction_count", count + 1);
            Ok(mlua::VmState::Continue)
        };
        let _ = self.lua.set_hook(mlua::HookTriggers::new().every_line(), hook_fn);
    }

    fn load_script_source(&self, path: &Path, name: &str) -> Result<Value, String> {
        let luac_path = path.with_extension("luac");
        if luac_path.exists() {
            let bytes = std::fs::read(&luac_path)
                .map_err(|e| format!("cannot read {}: {}", luac_path.display(), e))?;
            self.lua
                .load(&bytes[..])
                .set_name(name)
                .eval()
                .map_err(|e| format!("lua error in {}: {}", luac_path.display(), e))
        } else {
            let code = std::fs::read_to_string(path)
                .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            self.lua
                .load(&code)
                .set_name(name)
                .eval()
                .map_err(|e| format!("lua error in {}: {}", path.display(), e))
        }
    }

    pub fn load_main(&mut self, path: &str) -> Result<(), String> {
        let mut full_path = self.script_dir.join(path);
        let luac_path = full_path.with_extension("luac");
        if luac_path.exists() {
            full_path = luac_path;
        }
        self.load_script_source(&full_path, "main")?;
        Ok(())
    }

    pub fn attach_script(&mut self, entity: Entity, script_name: &str) -> Result<(), String> {
        if self.script_envs.contains_key(&entity) {
            return Ok(());
        }

        let script_path = self.script_dir.join(format!("{}.lua", script_name));
        let luac_path = script_path.with_extension("luac");
        let source_path = if luac_path.exists() { luac_path } else { script_path };
        let callback_value = self.load_script_source(&source_path, script_name)?;
        let callback_table = match callback_value {
            Value::Table(t) => t,
            _ => return Err(format!("script '{}' did not return a table", script_name)),
        };

        let env_key = self
            .lua
            .create_registry_value(callback_table)
            .map_err(|e| format!("registry error: {}", e))?;

        let methods: Table = self
            .lua
            .registry_value(&self.methods_key)
            .map_err(|e| format!("methods error: {}", e))?;
        let state = make_entity_table(&self.lua, entity, &methods);

        let state_key = self
            .lua
            .create_registry_value(state)
            .map_err(|e| format!("registry error: {}", e))?;

        self.script_envs.insert(entity, env_key);
        self.script_states.insert(entity, state_key);
        self.pending_create.push(entity);
        Ok(())
    }

    pub fn detach_script(&mut self, entity: Entity) {
        if let Some(key) = self.script_envs.remove(&entity) {
            let _ = self.lua.remove_registry_value(key);
        }
        if let Some(key) = self.script_states.remove(&entity) {
            let _ = self.lua.remove_registry_value(key);
        }
        if let Some(key) = self.threads.remove(&entity) {
            let _ = self.lua.remove_registry_value(key);
        }
        self.pending_create.retain(|e| *e != entity);
    }

    fn load_callback(&self, entity: Entity, name: &str) -> Option<Function> {
        let key = self.script_envs.get(&entity)?;
        let tbl: Table = self.lua.registry_value(key).ok()?;
        tbl.get::<Function>(name).ok()
    }

    fn self_table(&self, entity: Entity) -> Option<Table> {
        let key = self.script_states.get(&entity)?;
        self.lua.registry_value(key).ok()
    }

    fn call_on_create(&mut self, entity: Entity) {
        if let Some(on_create) = self.load_callback(entity, "on_create") {
            if let Some(self_table) = self.self_table(entity) {
                let _ = on_create.call::<()>(self_table);
            }
        }
    }

    fn call_on_update(&mut self, entity: Entity, dt: f32) {
        if let Some(on_update) = self.load_callback(entity, "on_update") {
            if let Some(self_table) = self.self_table(entity) {
                let _ = on_update.call::<()>((self_table, dt));
            }
        }
    }

    fn call_on_destroy(&mut self, entity: Entity) {
        if let Some(on_destroy) = self.load_callback(entity, "on_destroy") {
            if let Some(self_table) = self.self_table(entity) {
                let _ = on_destroy.call::<()>(self_table);
            }
        }
    }

    pub fn sync_scripts(&mut self, world: &mut World) {
        for entity in world.query().with::<ScriptComponent>().iter_entities() {
            if !self.script_envs.contains_key(&entity) {
                if let Some(script) = world.get_script(entity) {
                    if !script.script_name.is_empty() {
                        if let Err(e) = self.attach_script(entity, &script.script_name) {
                            log::warn!("[Lua] failed to attach script '{}': {}", script.script_name, e);
                        }
                    }
                }
            }
        }
    }

    pub fn update(&mut self, world: &mut World, dt: f32, collisions: Vec<CollisionEvent>) {
        self.dt = dt;

        self.sync_scripts(world);

        let alive_in_world: Vec<Entity> = world.query().with::<ScriptComponent>().iter_entities();
        let dead: Vec<Entity> = self
            .script_envs
            .keys()
            .filter(|e| !alive_in_world.contains(e))
            .copied()
            .collect();
        for entity in dead {
            self.call_on_destroy(entity);
            self.detach_script(entity);
        }

        if let Some(mut engine_ref) = self.lua.app_data_mut::<EngineRef>() {
            engine_ref.fps = self.fps;
            engine_ref.dt = dt;
        }

        let creates = std::mem::take(&mut self.pending_create);
        for entity in &creates {
            self.call_on_create(*entity);
        }

        // on_update with coroutine yield support
        let entities: Vec<Entity> = self.script_envs.keys().copied().collect();
        for entity in entities {
            if let Some(self_table) = self.self_table(entity) {
                if let Some(thread_key) = self.threads.get(&entity) {
                    // Resume existing coroutine
                    if let Ok(thread) = self.lua.registry_value::<Thread>(thread_key) {
                        match thread.resume::<()>((dt,)) {
                            Ok(()) => {
                                match thread.status() {
                                    mlua::ThreadStatus::Finished => {
                                        if let Some(key) = self.threads.remove(&entity) {
                                            let _ = self.lua.remove_registry_value(key);
                                        }
                                    }
                                    _ => {} // Resumable — keep for next frame
                                }
                            }
                            Err(e) => {
                                log::warn!("[Lua] coroutine error for entity {}: {}", entity.index, e);
                                if let Some(key) = self.threads.remove(&entity) {
                                    let _ = self.lua.remove_registry_value(key);
                                }
                            }
                        }
                    }
                } else {
                    // No coroutine yet — try on_update
                    if let Some(on_update) = self.load_callback(entity, "on_update") {
                        match self.lua.create_thread(on_update) {
                            Ok(thread) => {
                                match thread.resume::<()>((self_table, dt)) {
                                    Ok(()) => {
                                        match thread.status() {
                                            mlua::ThreadStatus::Resumable => {
                                                // Coroutine yielded — store for next frame
                                                if let Ok(key) = self.lua.create_registry_value(thread) {
                                                    self.threads.insert(entity, key);
                                                }
                                            }
                                            _ => {} // Completed normally, no thread to store
                                        }
                                    }
                                    Err(e) => {
                                        log::warn!("[Lua] on_update error for entity {}: {}", entity.index, e);
                                    }
                                }
                            }
                            Err(e) => {
                                log::warn!("[Lua] failed to create thread for entity {}: {}", entity.index, e);
                            }
                        }
                    }
                }
            }
        }

        // Process collision events — invoke on_collision on both participants
        for event in &collisions {
            for &(entity, other) in &[(event.entity_a, event.entity_b), (event.entity_b, event.entity_a)] {
                if let Some(on_collision) = self.load_callback(entity, "on_collision") {
                    if let Some(self_table) = self.self_table(entity) {
                        let methods: Table = self
                            .lua
                            .named_registry_value("__entity_methods")
                            .ok()
                            .unwrap();
                        let other_table = make_entity_table(&self.lua, other, &methods);
                        let _ = on_collision.call::<()>((self_table, other_table));
                    }
                }
            }
        }

        self.collect_ui_labels();

        let _ = self
            .lua
            .set_named_registry_value("__instruction_count", 0u64);
    }

    fn collect_ui_labels(&mut self) {
        self.ui_labels.clear();
        if let Ok(labels) = self.lua.named_registry_value::<Table>("__ui_labels") {
            for entry in labels.sequence_values::<String>() {
                if let Ok(text) = entry {
                    self.ui_labels.push(text);
                }
            }
            let _ = self.lua.set_named_registry_value("__ui_labels", self.lua.create_table().unwrap());
        }
    }

    pub fn destroy_entity(&mut self, entity: Entity) {
        self.call_on_destroy(entity);
        self.detach_script(entity);
    }

    pub fn set_fps(&mut self, fps: f32) {
        self.fps = fps;
    }

    pub fn take_ui_labels(&mut self) -> Vec<String> {
        std::mem::take(&mut self.ui_labels)
    }

    pub fn is_debug_menu(&self) -> bool {
        is_debug_menu(&self.lua)
    }

    pub fn get_ambient(&self) -> glam::Vec3 {
        let r: f32 = self.lua.named_registry_value("__ambient_r").unwrap_or(0.05);
        let g: f32 = self.lua.named_registry_value("__ambient_g").unwrap_or(0.05);
        let b: f32 = self.lua.named_registry_value("__ambient_b").unwrap_or(0.08);
        glam::Vec3::new(r, g, b)
    }

    pub fn get_exposure(&self) -> f32 {
        self.lua.named_registry_value("__exposure").unwrap_or(0.7)
    }

    pub fn get_msaa(&self) -> u32 {
        self.lua.named_registry_value("__msaa").unwrap_or(4)
    }

    pub fn get_bloom_intensity(&self) -> f32 {
        self.lua.named_registry_value("__bloom_intensity").unwrap_or(0.2)
    }

    pub fn script_count(&self) -> usize {
        self.script_envs.len()
    }

    fn register_engine_api(&self) -> Result<(), String> {
        let globals = self.lua.globals();
        let engine = self.lua.create_table().unwrap();
        let input_table = self.lua.create_table().unwrap();
        let audio_table = self.lua.create_table().unwrap();

        let methods: Table = self
            .lua
            .registry_value::<Table>(&self.methods_key)
            .map_err(|e| format!("{}", e))?;

        let _ = self
            .lua
            .set_named_registry_value("__actions", self.lua.create_table().unwrap());
        let _ = self
            .lua
            .set_named_registry_value("__ui_labels", self.lua.create_table().unwrap());
        let _ = self.lua.set_named_registry_value("__ambient_r", 0.05f32);
        let _ = self.lua.set_named_registry_value("__ambient_g", 0.05f32);
        let _ = self.lua.set_named_registry_value("__ambient_b", 0.08f32);
        let _ = self.lua.set_named_registry_value("__exposure", 0.7f32);

        // engine.spawn(name)
        engine
            .set(
                "spawn",
                self.lua
                    .create_function(|lua, name: String| {
                        let engine_ref = lua
                            .app_data_ref::<EngineRef>()
                            .expect("EngineRef not set");
                        let world = unsafe { &mut *engine_ref.world };

                        let entity = match crate::entity_types::spawn_type(world, &name) {
                            Some(e) => e,
                            None => {
                                // Unknown type: fall back to a plain dynamic entity.
                                let e = world.spawn();
                                world.add_transform(e, Transform3D::default());
                                world.add_velocity_3d(e, Velocity3D::default());
                                e
                            }
                        };

                        let dynamic = unsafe { &mut *engine_ref.dynamic };
                        dynamic.push(entity);

                        let methods: Table = lua
                            .named_registry_value("__entity_methods")
                            .ok()
                            .unwrap();
                        let t = make_entity_table(lua, entity, &methods);

                        log::info!("[Lua] spawned entity {} ({})", entity.index, name);
                        Ok(t)
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.set_position_batch({{entity, x, y, z}, ...})
        engine
            .set(
                "set_position_batch",
                self.lua
                    .create_function(|lua, batch: Table| {
                        let engine_ref = lua
                            .app_data_ref::<EngineRef>()
                            .expect("EngineRef not set");
                        let world = unsafe { &mut *engine_ref.world };
                        for pair in batch.sequence_values::<Table>() {
                            if let Ok(entry) = pair {
                                let entity_table: Table = match entry.get(1) {
                                    Ok(t) => t,
                                    Err(_) => continue,
                                };
                                let x: f32 = entry.get(2).unwrap_or(0.0);
                                let y: f32 = entry.get(3).unwrap_or(0.0);
                                let z: f32 = entry.get(4).unwrap_or(0.0);
                                if let Some(entity) = decode_entity(&entity_table) {
                                    if let Some(t) = world.get_transform_mut(entity) {
                                        t.position = Vec3::new(x, y, z);
                                    }
                                }
                            }
                        }
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.despawn(entity)
        engine
            .set(
                "despawn",
                self.lua
                    .create_function(|lua, entity_table: Table| {
                        let engine_ref = lua
                            .app_data_ref::<EngineRef>()
                            .expect("EngineRef not set");
                        let world = unsafe { &mut *engine_ref.world };
                        if let Some(entity) = decode_entity(&entity_table) {
                            world.despawn(entity);
                            log::info!("[Lua] despawned entity {}", entity.index);
                        }
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        // Entity methods
        let reg_method = |_lua: &Lua, name: &str, f: Function| {
            let _ = methods.set(name, f);
        };

        reg_method(
            &self.lua,
            "set_position",
            self.lua
                .create_function(|lua, (entity_table, x, y, z): (Table, f32, f32, f32)| {
                    let engine_ref = lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                    let world = unsafe { &mut *engine_ref.world };
                    if let Some(entity) = decode_entity(&entity_table) {
                        if let Some(t) = world.get_transform_mut(entity) {
                            t.position = Vec3::new(x, y, z);
                        }
                    }
                    Ok(())
                })
                .unwrap(),
        );

        reg_method(
            &self.lua,
            "get_position",
            self.lua
                .create_function(|lua, entity_table: Table| {
                    let engine_ref = lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                    let world = unsafe { &*engine_ref.world };
                    if let Some(entity) = decode_entity(&entity_table) {
                        if let Some(t) = world.get_transform(entity) {
                            return Ok((t.position.x, t.position.y, t.position.z));
                        }
                    }
                    Ok((0.0f32, 0.0, 0.0))
                })
                .unwrap(),
        );

        reg_method(
            &self.lua,
            "set_rotation",
            self.lua
                .create_function(|lua, (entity_table, x, y, z): (Table, f32, f32, f32)| {
                    let engine_ref = lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                    let world = unsafe { &mut *engine_ref.world };
                    if let Some(entity) = decode_entity(&entity_table) {
                        if let Some(t) = world.get_transform_mut(entity) {
                            let qx = Quat::from_axis_angle(Vec3::X, x.to_radians());
                            let qy = Quat::from_axis_angle(Vec3::Y, y.to_radians());
                            let qz = Quat::from_axis_angle(Vec3::Z, z.to_radians());
                            t.rotation = qx * qy * qz;
                        }
                    }
                    Ok(())
                })
                .unwrap(),
        );

        reg_method(
            &self.lua,
            "set_scale",
            self.lua
                .create_function(|lua, (entity_table, x, y, z): (Table, f32, f32, f32)| {
                    let engine_ref = lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                    let world = unsafe { &mut *engine_ref.world };
                    if let Some(entity) = decode_entity(&entity_table) {
                        if let Some(t) = world.get_transform_mut(entity) {
                            t.scale = Vec3::new(x, y, z);
                        }
                    }
                    Ok(())
                })
                .unwrap(),
        );

        reg_method(
            &self.lua,
            "set_color",
            self.lua
                .create_function(|lua, (entity_table, r, g, b, a): (Table, f32, f32, f32, f32)| {
                    let engine_ref = lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                    let world = unsafe { &mut *engine_ref.world };
                    if let Some(entity) = decode_entity(&entity_table) {
                        world.set_color(entity, Color { rgba: [r, g, b, a] });
                    }
                    Ok(())
                })
                .unwrap(),
        );

        reg_method(
            &self.lua,
            "set_model",
            self.lua
                .create_function(|lua, (entity_table, path): (Table, String)| {
                    let engine_ref = lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                    let world = unsafe { &mut *engine_ref.world };
                    if let Some(entity) = decode_entity(&entity_table) {
                        world.add_model(entity, Model { path });
                    }
                    Ok(())
                })
                .unwrap(),
        );

        reg_method(
            &self.lua,
            "set_velocity",
            self.lua
                .create_function(|lua, (entity_table, x, y, z): (Table, f32, f32, f32)| {
                    let engine_ref = lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                    let world = unsafe { &mut *engine_ref.world };
                    if let Some(entity) = decode_entity(&entity_table) {
                        world.add_velocity_3d(
                            entity,
                            Velocity3D {
                                linear: Vec3::new(x, y, z),
                            },
                        );
                    }
                    Ok(())
                })
                .unwrap(),
        );

        reg_method(
            &self.lua,
            "add_script",
            self.lua
                .create_function(|lua, (entity_table, script_name): (Table, String)| {
                    let engine_ref = lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                    let world = unsafe { &mut *engine_ref.world };
                    if let Some(entity) = decode_entity(&entity_table) {
                        world.add_script(entity, ScriptComponent { script_name });
                    }
                    Ok(())
                })
                .unwrap(),
        );

        // engine.input:is_key_down(key) — colon syntax passes input table as first arg
        input_table
            .set(
                "is_key_down",
                self.lua
                    .create_function(|lua, (_self, key): (Table, String)| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let input = unsafe { &*engine_ref.input };
                        let result = match key.to_uppercase().as_str() {
                            "W" => input.forward,
                            "A" => input.left,
                            "S" => input.backward,
                            "D" => input.right,
                            "SPACE" => input.jump,
                            _ => false,
                        };
                        Ok(result)
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.input:get_mouse_delta() — colon syntax passes input table as first arg
        input_table
            .set(
                "get_mouse_delta",
                self.lua
                    .create_function(|lua, _self: Table| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let input = unsafe { &*engine_ref.input };
                        Ok((input.mouse_dx as f32, input.mouse_dy as f32))
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.input:define_action(name, binding)
        input_table
            .set(
                "define_action",
                self.lua
                    .create_function(|lua, (name, binding): (String, Table)| {
                        if let Ok(actions) = lua.named_registry_value::<Table>("__actions") {
                            let _ = actions.set(name, binding);
                        }
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.input:is_action_pressed(name)
        input_table
            .set(
                "is_action_pressed",
                self.lua
                    .create_function(|lua, name: String| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let input = unsafe { &*engine_ref.input };
                        let pressed = if let Ok(actions) =
                            lua.named_registry_value::<Table>("__actions")
                        {
                            if let Ok(binding) = actions.get::<Table>(name.as_str()) {
                                if let Ok(key) = binding.get::<String>("key") {
                                    input.keys.get(&key).copied().unwrap_or(false)
                                } else {
                                    false
                                }
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        Ok(pressed)
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.input:get_action_axis(name) — returns -1..1
        input_table
            .set(
                "get_action_axis",
                self.lua
                    .create_function(|lua, name: String| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let input = unsafe { &*engine_ref.input };
                        let mut axis = 0.0f32;
                        if let Ok(actions) = lua.named_registry_value::<Table>("__actions") {
                            if let Ok(binding) = actions.get::<Table>(name.as_str()) {
                                let check = |key: Option<String>| -> f32 {
                                    match key {
                                        Some(k) => {
                                            if input.keys.get(&k).copied().unwrap_or(false) {
                                                1.0
                                            } else {
                                                0.0
                                            }
                                        }
                                        None => 0.0,
                                    }
                                };
                                axis += check(binding.get::<Option<String>>("pos_key").ok().flatten());
                                axis -= check(binding.get::<Option<String>>("neg_key").ok().flatten());
                                if axis == 0.0 {
                                    axis += check(binding.get::<Option<String>>("key").ok().flatten());
                                }
                            }
                        }
                        Ok(axis)
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.audio:play_sound(path, x, y, z) — colon syntax passes audio table as first arg
        audio_table
            .set(
                "play_sound",
                self.lua
                    .create_function(|lua, (_self, path, x, y, z): (Table, String, f32, f32, f32)| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let audio = unsafe { &mut **engine_ref.audio };
                        let handle = audio.play(&path, 1.0, false);
                        audio.set_spatial(handle, Vec3::new(x, y, z));
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.audio:set_bus_volume(bus, volume)
        audio_table
            .set(
                "set_bus_volume",
                self.lua
                    .create_function(|lua, (_self, bus, volume): (Table, String, f32)| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let audio = unsafe { &mut **engine_ref.audio };
                        let bus_type = match bus.to_lowercase().as_str() {
                            "music" => BusType::Music,
                            "voice" => BusType::Voice,
                            _ => BusType::Sfx,
                        };
                        audio.set_bus_volume(bus_type, volume);
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        // Audio source objects: create_source(path) -> source with set_looping/set_volume/play/stop/pause/resume
        let source_methods = self.lua.create_table().unwrap();

        source_methods
            .set(
                "play",
                self.lua
                    .create_function(|lua, self_tbl: Table| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let audio = unsafe { &mut **engine_ref.audio };
                        let path: String = self_tbl.get("__path").unwrap_or_default();
                        let looping: bool = self_tbl.get("__looping").unwrap_or(false);
                        let volume: f32 = self_tbl.get("__volume").unwrap_or(1.0);
                        let handle = audio.play_streaming(&path, volume, looping);
                        let ht = lua.create_table().unwrap();
                        let _ = ht.set("index", handle.index);
                        let _ = ht.set("generation", handle.generation);
                        let _ = self_tbl.set("__handle", ht);
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        source_methods
            .set(
                "set_looping",
                self.lua
                    .create_function(|lua, (self_tbl, looping): (Table, bool)| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let audio = unsafe { &mut **engine_ref.audio };
                        let _ = self_tbl.set("__looping", looping);
                        if let Some(handle) = audio_handle_from_table(&self_tbl) {
                            audio.set_looping(handle, looping);
                        }
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        source_methods
            .set(
                "set_volume",
                self.lua
                    .create_function(|lua, (self_tbl, volume): (Table, f32)| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let audio = unsafe { &mut **engine_ref.audio };
                        let _ = self_tbl.set("__volume", volume);
                        if let Some(handle) = audio_handle_from_table(&self_tbl) {
                            audio.set_volume(handle, volume);
                        }
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        source_methods
            .set(
                "stop",
                self.lua
                    .create_function(|lua, self_tbl: Table| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let audio = unsafe { &mut **engine_ref.audio };
                        if let Some(handle) = audio_handle_from_table(&self_tbl) {
                            audio.stop(handle);
                        }
                        let _ = self_tbl.set("__handle", Value::Nil);
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        source_methods
            .set(
                "pause",
                self.lua
                    .create_function(|lua, self_tbl: Table| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let audio = unsafe { &mut **engine_ref.audio };
                        if let Some(handle) = audio_handle_from_table(&self_tbl) {
                            audio.pause(handle);
                        }
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        source_methods
            .set(
                "resume",
                self.lua
                    .create_function(|lua, self_tbl: Table| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        let audio = unsafe { &mut **engine_ref.audio };
                        if let Some(handle) = audio_handle_from_table(&self_tbl) {
                            audio.resume(handle);
                        }
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        audio_table
            .set(
                "create_source",
                self.lua
                    .create_function(move |lua, path: String| {
                        let t = lua.create_table().unwrap();
                        let _ = t.set("__path", path);
                        let _ = t.set("__handle", Value::Nil);
                        let _ = t.set("__looping", false);
                        let _ = t.set("__volume", 1.0f32);
                        let mt = lua.create_table().unwrap();
                        let _ = mt.set("__index", source_methods.clone());
                        let _ = t.set_metatable(Some(mt));
                        Ok(t)
                    })
                    .unwrap(),
            )
            .unwrap();

        engine.set("input", input_table).unwrap();
        engine.set("audio", audio_table).unwrap();

        // --- engine.config table ---
        let config_table = self.lua.create_table().unwrap();

        // engine.config:get(key)
        config_table
            .set(
                "get",
                self.lua
                    .create_function(|lua, (_self, key): (Table, String)| {
                        let engine_ref = lua.app_data_ref::<EngineRef>()
                            .expect("EngineRef not set");
                        let config = unsafe { &*engine_ref.config };
                        let val = match key.as_str() {
                            "window_width" => Some(config.window_width.to_string()),
                            "window_height" => Some(config.window_height.to_string()),
                            "fullscreen" => Some(config.fullscreen.to_string()),
                            "vsync" => Some(config.vsync.to_string()),
                            _ => None,
                        };
                        Ok(val)
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.config:set(key, value)
        config_table
            .set(
                "set",
                self.lua
                    .create_function(|lua, (_self, key, value): (Table, String, String)| {
                        let mut engine_ref = lua.app_data_mut::<EngineRef>()
                            .expect("EngineRef not set");
                        let config = unsafe { &mut *engine_ref.config };
                        let result = match key.as_str() {
                            "window_width" => value
                                .parse::<u32>()
                                .map(|v| config.window_width = v)
                                .map_err(|e| format!("invalid window_width: {e}")),
                            "window_height" => value
                                .parse::<u32>()
                                .map(|v| config.window_height = v)
                                .map_err(|e| format!("invalid window_height: {e}")),
                            "fullscreen" => {
                                config.fullscreen =
                                    value.eq_ignore_ascii_case("true") || value == "1";
                                Ok(())
                            }
                            "vsync" => {
                                config.vsync =
                                    value.eq_ignore_ascii_case("true") || value == "1";
                                Ok(())
                            }
                            _ => Err(format!("unknown user config key: {key}")),
                        };
                        match result {
                            Ok(()) => Ok(mlua::Value::Nil),
                            Err(e) => Err(mlua::Error::external(e)),
                        }
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.config:save()
        config_table
            .set(
                "save",
                self.lua
                    .create_function(|lua, _self: Table| {
                        let engine_ref = lua.app_data_ref::<EngineRef>()
                            .expect("EngineRef not set");
                        let config = unsafe { &*engine_ref.config };
                        config.save_user_config().map_err(mlua::Error::external)?;
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        engine.set("config", config_table).unwrap();

        // engine:log(msg) — colon syntax passes engine table as first arg
        engine
            .set(
                "log",
                self.lua
                    .create_function(|_lua, (_self, msg): (Table, String)| {
                        log::info!("[Lua] {}", msg);
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine:get_fps() — colon syntax passes engine table as first arg
        engine
            .set(
                "get_fps",
                self.lua
                    .create_function(|lua, _self: Table| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        Ok(engine_ref.fps)
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine:get_dt() — colon syntax passes engine table as first arg
        engine
            .set(
                "get_dt",
                self.lua
                    .create_function(|lua, _self: Table| {
                        let engine_ref =
                            lua.app_data_ref::<EngineRef>().expect("EngineRef not set");
                        Ok(engine_ref.dt)
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine:set_ambient(r, g, b)
        engine
            .set(
                "set_ambient",
                self.lua
                    .create_function(|lua, (_self, r, g, b): (Table, f32, f32, f32)| {
                        let _ = lua.set_named_registry_value("__ambient_r", r);
                        let _ = lua.set_named_registry_value("__ambient_g", g);
                        let _ = lua.set_named_registry_value("__ambient_b", b);
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine:set_exposure(val)
        engine
            .set(
                "set_exposure",
                self.lua
                    .create_function(|lua, (_self, val): (Table, f32)| {
                        let _ = lua.set_named_registry_value("__exposure", val);
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine:set_msaa(samples) — 0 = off, 2, 4
        engine
            .set(
                "set_msaa",
                self.lua
                    .create_function(|lua, (_self, val): (Table, u32)| {
                        let _ = lua.set_named_registry_value("__msaa", val);
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine:get_msaa()
        engine
            .set(
                "get_msaa",
                self.lua
                    .create_function(|lua, _self: Table| {
                        let v: u32 = lua.named_registry_value("__msaa").unwrap_or(4);
                        Ok(v)
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine:set_bloom(val)
        engine
            .set(
                "set_bloom",
                self.lua
                    .create_function(|lua, (_self, val): (Table, f32)| {
                        let _ = lua.set_named_registry_value("__bloom_intensity", val);
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine:get_bloom()
        engine
            .set(
                "get_bloom",
                self.lua
                    .create_function(|lua, _self: Table| {
                        let v: f32 = lua.named_registry_value("__bloom_intensity").unwrap_or(0.2);
                        Ok(v)
                    })
                    .unwrap(),
            )
            .unwrap();

        // engine.ui:label(text) — pushed to an overlay drawn each frame
        let ui_table = self.lua.create_table().unwrap();
        ui_table
            .set(
                "label",
                self.lua
                    .create_function(|lua, (_self, text): (Table, String)| {
                        if let Ok(labels) = lua.named_registry_value::<Table>("__ui_labels") {
                            let _ = labels.push(text);
                        }
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();
        engine.set("ui", ui_table).unwrap();

        // engine:show_debug_menu() — toggles the diagnostic overlay
        engine
            .set(
                "show_debug_menu",
                self.lua
                    .create_function(|lua, _self: Table| {
                        let current = is_debug_menu(lua);
                        set_debug_menu(lua, !current);
                        Ok(())
                    })
                    .unwrap(),
            )
            .unwrap();

        globals.set("engine", engine).unwrap();
        let _ = self
            .lua
            .set_named_registry_value("__entity_methods", methods);

        Ok(())
    }
}
