//! WMAP — binary world-map serialization (techspec §3).
//!
//! Layout is little-endian and wire-compatible with the netcode delta format
//! (techspec §17): each component is `type_id | version | data_size | data`,
//! where `type_id` is the CRC32 of the Rust component type name. Unknown
//! component types are skipped (with a warning) so newer worlds still load on
//! older runtimes.

use std::fs;
use std::io::Write;

use glam::{Quat, Vec3};

use crate::audio::{BusType, PlayMode};
use crate::ecs::{
    AngularVelocity, AudioSourceComponent, Camera, CameraMode, Collider, Color, Entity, Model,
    RigidBody, ScriptComponent, Transform3D, Velocity3D, World,
};
use crate::entity_types;

const WMAP_MAGIC: [u8; 4] = *b"WMAP";
const CURRENT_MAJOR: u16 = 1;
const CURRENT_MINOR: u16 = 0;

#[allow(dead_code)]
const FLAG_IS_PREFAB: u32 = 1 << 0;

const HEADER_SIZE: usize = 4 + 2 + 2 + 4 + 8 + 8 + 8 + 8 + 8; // 52 bytes

// --- Component type ids (CRC32 of the type name) ---------------------------

const fn crc32_byte(mut crc: u32, b: u8) -> u32 {
    crc ^= b as u32;
    let mut j = 0;
    while j < 8 {
        if crc & 1 != 0 {
            crc = (crc >> 1) ^ 0xEDB8_8320;
        } else {
            crc >>= 1;
        }
        j += 1;
    }
    crc
}

const fn crc32(s: &str) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        crc = crc32_byte(crc, bytes[i]);
        i += 1;
    }
    !crc
}

const TRANSFORM_ID: u32 = crc32("Transform3D");
const VELOCITY_ID: u32 = crc32("Velocity3D");
const ANGULAR_VEL_ID: u32 = crc32("AngularVelocity");
const RIGID_BODY_ID: u32 = crc32("RigidBody");
const COLLIDER_ID: u32 = crc32("Collider");
const COLOR_ID: u32 = crc32("Color");
const PLAYER_ID: u32 = crc32("Player");
const GROUND_ID: u32 = crc32("Ground");
const STATIC_ID: u32 = crc32("Static");
const SENSOR_ID: u32 = crc32("Sensor");
const AUDIO_LISTENER_ID: u32 = crc32("AudioListenerComponent");
const CAMERA_ID: u32 = crc32("Camera");
const MODEL_ID: u32 = crc32("Model");
const SCRIPT_ID: u32 = crc32("ScriptComponent");
const AUDIO_SOURCE_ID: u32 = crc32("AudioSourceComponent");
const ENTITY_TYPE_ID: u32 = crc32("EntityType");

const TRANSFORM_VERSION: u32 = 2; // v2 adds scale

// --- Byte helpers ----------------------------------------------------------

struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.remaining() < n {
            return Err("unexpected end of WMAP data".to_string());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, String> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, String> {
        let b = self.take(8)?;
        Ok(u64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn vec3(&mut self) -> Result<Vec3, String> {
        Ok(Vec3::new(self.f32()?, self.f32()?, self.f32()?))
    }

    fn string(&mut self) -> Result<String, String> {
        let len = self.u32()? as usize;
        let b = self.take(len)?;
        String::from_utf8(b.to_vec())
            .map_err(|_| "invalid UTF-8 in WMAP string".to_string())
    }
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_u64(out: &mut Vec<u8>, v: u64) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_f32(out: &mut Vec<u8>, v: f32) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put_vec3(out: &mut Vec<u8>, v: Vec3) {
    put_f32(out, v.x);
    put_f32(out, v.y);
    put_f32(out, v.z);
}
fn put_string(out: &mut Vec<u8>, s: &str) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s.as_bytes());
}

// --- Per-component (de)serialization ---------------------------------------

fn write_transform(out: &mut Vec<u8>, t: &Transform3D) {
    put_vec3(out, t.position);
    put_f32(out, t.rotation.x);
    put_f32(out, t.rotation.y);
    put_f32(out, t.rotation.z);
    put_f32(out, t.rotation.w);
    put_vec3(out, t.scale);
}

fn read_transform(c: &mut Cursor, version: u32) -> Result<Transform3D, String> {
    let position = c.vec3()?;
    let rotation = Quat::from_xyzw(c.f32()?, c.f32()?, c.f32()?, c.f32()?);
    // v1 had no scale; default it (techspec §3 version migration example).
    let scale = if version >= 2 {
        c.vec3()?
    } else {
        log::warn!("WMAP: Transform3D v1 -> defaulting scale to ONE");
        Vec3::ONE
    };
    Ok(Transform3D {
        position,
        rotation,
        scale,
    })
}

fn write_camera(out: &mut Vec<u8>, cam: &Camera) {
    out.push(cam.mode as u8);
    put_f32(out, cam.yaw);
    put_f32(out, cam.pitch);
    put_f32(out, cam.distance);
    put_f32(out, cam.height);
}

fn read_camera(c: &mut Cursor) -> Result<Camera, String> {
    let mode = match c.u8()? {
        0 => CameraMode::ThirdPerson,
        1 => CameraMode::FirstPerson,
        2 => CameraMode::FreeLook,
        other => {
            log::warn!("WMAP: unknown CameraMode {other}, defaulting to ThirdPerson");
            CameraMode::ThirdPerson
        }
    };
    Ok(Camera {
        mode,
        yaw: c.f32()?,
        pitch: c.f32()?,
        distance: c.f32()?,
        height: c.f32()?,
    })
}

fn write_audio_source(out: &mut Vec<u8>, a: &AudioSourceComponent) {
    put_string(out, &a.clip);
    put_f32(out, a.volume);
    out.push(a.looping as u8);
    out.push(a.bus as u8);
    out.push(a.mode as u8);
}

fn read_audio_source(c: &mut Cursor) -> Result<AudioSourceComponent, String> {
    let clip = c.string()?;
    let volume = c.f32()?;
    let looping = c.u8()? != 0;
    let bus = match c.u8()? {
        1 => BusType::Music,
        2 => BusType::Voice,
        _ => BusType::Sfx,
    };
    let mode = match c.u8()? {
        1 => PlayMode::Streaming,
        _ => PlayMode::Buffered,
    };
    Ok(AudioSourceComponent {
        clip,
        volume,
        looping,
        bus,
        mode,
    })
}

// --- Public API ------------------------------------------------------------

/// Write the entire world to a `.wmap` file.
pub fn write_world(world: &World, path: &str, seed: u64) -> Result<(), String> {
    let mut entities = world.entities();
    entities.sort_by(|a, b| (a.index, a.generation).cmp(&(b.index, b.generation)));

    // Build the name string table first so offsets are known up-front.
    let mut name_blob: Vec<u8> = Vec::new();
    let mut name_offsets: std::collections::HashMap<Entity, u64> = std::collections::HashMap::new();
    for &e in &entities {
        name_offsets.insert(e, name_blob.len() as u64);
        let name = world.get_name_or(&e, "");
        put_string(&mut name_blob, &name);
    }

    // Entity section.
    let mut entity_section: Vec<u8> = Vec::new();
    let mut component_ct: u64 = 0;

    for &e in &entities {
        let id = ((e.generation as u64) << 32) | (e.index as u64);
        let name_off = *name_offsets.get(&e).unwrap_or(&0);

        let mut comp_buf: Vec<u8> = Vec::new();
        let mut count = 0u32;

        let push = |out: &mut Vec<u8>, type_id: u32, version: u32, data: &[u8]| {
            put_u32(out, type_id);
            put_u32(out, version);
            put_u64(out, data.len() as u64);
            out.extend_from_slice(data);
        };

        // Entity type must be written first so readers apply template defaults
        // before per-component overrides.
        if let Some(t) = world.get_entity_type(e) {
            let mut d = Vec::new();
            put_string(&mut d, t);
            push(&mut comp_buf, ENTITY_TYPE_ID, 1, &d);
            count += 1;
        }

        if let Some(t) = world.get_transform(e) {
            let mut d = Vec::new();
            write_transform(&mut d, &t);
            push(&mut comp_buf, TRANSFORM_ID, TRANSFORM_VERSION, &d);
            count += 1;
        }
        if let Some(v) = world.get_velocity_3d(e) {
            let mut d = Vec::new();
            put_vec3(&mut d, v.linear);
            push(&mut comp_buf, VELOCITY_ID, 1, &d);
            count += 1;
        }
        if let Some(a) = world.get_angular_velocity(e) {
            let mut d = Vec::new();
            put_vec3(&mut d, a.radians);
            push(&mut comp_buf, ANGULAR_VEL_ID, 1, &d);
            count += 1;
        }
        if let Some(rb) = world.get_rigid_body(e) {
            let mut d = Vec::new();
            put_f32(&mut d, rb.mass);
            put_f32(&mut d, rb.restitution);
            put_f32(&mut d, rb.angular_damping);
            push(&mut comp_buf, RIGID_BODY_ID, 1, &d);
            count += 1;
        }
        if let Some(col) = world.get_collider(e) {
            let mut d = Vec::new();
            put_vec3(&mut d, col.half_extents);
            push(&mut comp_buf, COLLIDER_ID, 1, &d);
            count += 1;
        }
        if let Some(c) = world.get_color(e) {
            let mut d = Vec::new();
            for x in c.rgba.iter() {
                put_f32(&mut d, *x);
            }
            push(&mut comp_buf, COLOR_ID, 1, &d);
            count += 1;
        }
        if world.is_player(e) {
            push(&mut comp_buf, PLAYER_ID, 1, &[]);
            count += 1;
        }
        if world.is_ground(e) {
            push(&mut comp_buf, GROUND_ID, 1, &[]);
            count += 1;
        }
        if world.is_static(e) {
            push(&mut comp_buf, STATIC_ID, 1, &[]);
            count += 1;
        }
        if world.is_sensor(e) {
            push(&mut comp_buf, SENSOR_ID, 1, &[]);
            count += 1;
        }
        if world.is_audio_listener(e) {
            push(&mut comp_buf, AUDIO_LISTENER_ID, 1, &[]);
            count += 1;
        }
        if let Some(cam) = world.get_camera(e) {
            let mut d = Vec::new();
            write_camera(&mut d, &cam);
            push(&mut comp_buf, CAMERA_ID, 1, &d);
            count += 1;
        }
        if let Some(m) = world.get_model(e) {
            let mut d = Vec::new();
            put_string(&mut d, &m.path);
            push(&mut comp_buf, MODEL_ID, 1, &d);
            count += 1;
        }
        if let Some(s) = world.get_script(e) {
            let mut d = Vec::new();
            put_string(&mut d, &s.script_name);
            push(&mut comp_buf, SCRIPT_ID, 1, &d);
            count += 1;
        }
        if let Some(a) = world.get_audio_source(e) {
            let mut d = Vec::new();
            write_audio_source(&mut d, &a);
            push(&mut comp_buf, AUDIO_SOURCE_ID, 1, &d);
            count += 1;
        }

        component_ct += count as u64;

        put_u64(&mut entity_section, id);
        put_u64(&mut entity_section, name_off);
        put_u32(&mut entity_section, count);
        entity_section.extend_from_slice(&comp_buf);
    }

    let mut out: Vec<u8> =
        Vec::with_capacity(HEADER_SIZE + entity_section.len() + name_blob.len());
    out.extend_from_slice(&WMAP_MAGIC);
    put_u16(&mut out, CURRENT_MAJOR);
    put_u16(&mut out, CURRENT_MINOR);
    put_u32(&mut out, 0); // flags
    put_u64(&mut out, entities.len() as u64);
    put_u64(&mut out, component_ct);
    let name_table_off = (HEADER_SIZE + entity_section.len()) as u64;
    put_u64(&mut out, name_table_off);
    put_u64(&mut out, name_blob.len() as u64);
    put_u64(&mut out, seed);
    out.extend_from_slice(&entity_section);
    out.extend_from_slice(&name_blob);

    let mut file =
        fs::File::create(path).map_err(|e| format!("Failed to create '{path}': {e}"))?;
    file.write_all(&out)
        .map_err(|e| format!("Failed to write '{path}': {e}"))?;

    Ok(())
}

/// Read a `.wmap` file, returning the reconstructed world, the RNG seed, and
/// the entity name table.
pub fn read_world(path: &str) -> Result<(World, u64, Vec<(Entity, String)>), String> {
    let bytes = fs::read(path).map_err(|e| format!("Failed to read '{path}': {e}"))?;
    let mut c = Cursor::new(&bytes);

    let magic = c.take(4)?;
    if magic != WMAP_MAGIC {
        return Err(format!(
            "Invalid WMAP magic: expected WMAP, got {:?}",
            std::str::from_utf8(magic).unwrap_or("???")
        ));
    }
    let major = c.u16()?;
    let minor = c.u16()?;
    let _flags = c.u32()?;
    let entity_count = c.u64()?;
    let _component_ct = c.u64()?;
    let name_table_off = c.u64()?;
    let name_table_sz = c.u64()?;
    let seed = c.u64()?;

    if major > CURRENT_MAJOR {
        return Err(format!(
            "WMAP version {}.{} not supported (max {}.{}). Re-export level.",
            major, minor, CURRENT_MAJOR, CURRENT_MINOR
        ));
    }
    if major == CURRENT_MAJOR && minor > CURRENT_MINOR {
        log::warn!(
            "WMAP version {}.{} newer than runtime {}.{}; attempting load",
            major, minor, CURRENT_MAJOR, CURRENT_MINOR
        );
    }

    if name_table_off as usize > bytes.len() {
        return Err("WMAP name table offset out of range".to_string());
    }
    let name_blob =
        &bytes[name_table_off as usize..(name_table_off + name_table_sz) as usize];

    let mut world = World::new();
    let mut names: Vec<(Entity, String)> = Vec::new();

    for _ in 0..entity_count {
        let id = c.u64()?;
        let name_off = c.u64()? as usize;
        let comp_count = c.u32()?;

        let index = (id & 0xFFFF_FFFF) as u32;
        let generation = (id >> 32) as u32;
        let entity = Entity::new(index, generation);
        world.create_entity_at(entity);

            let mut nb = Cursor::new(name_blob);
        nb.pos = name_off;
        let name = nb.string().unwrap_or_default();
        if !name.is_empty() {
            world.set_name(entity, &name);
            names.push((entity, name));
        }

        for _ in 0..comp_count {
            let type_id = c.u32()?;
            let version = c.u32()?;
            let data_size = c.u64()? as usize;
            let data = c.take(data_size)?;
            apply_component(&mut world, entity, type_id, version, data)?;
        }
    }

    Ok((world, seed, names))
}

fn apply_component(
    world: &mut World,
    entity: Entity,
    type_id: u32,
    version: u32,
    data: &[u8],
) -> Result<(), String> {
    let mut c = Cursor::new(data);
    match type_id {
        TRANSFORM_ID => {
            world.add_transform(entity, read_transform(&mut c, version)?);
        }
        VELOCITY_ID => {
            world.add_velocity_3d(entity, Velocity3D { linear: c.vec3()? });
        }
        ANGULAR_VEL_ID => {
            world.add_angular_velocity(entity, AngularVelocity { radians: c.vec3()? });
        }
        RIGID_BODY_ID => {
            world.add_rigid_body(
                entity,
                RigidBody {
                    mass: c.f32()?,
                    restitution: c.f32()?,
                    angular_damping: c.f32()?,
                },
            );
        }
        COLLIDER_ID => {
            world.add_collider(entity, Collider { half_extents: c.vec3()? });
        }
        COLOR_ID => {
            let mut rgba = [0f32; 4];
            for x in rgba.iter_mut() {
                *x = c.f32()?;
            }
            world.set_color(entity, Color { rgba });
        }
        PLAYER_ID => world.add_player(entity),
        GROUND_ID => world.add_ground(entity),
        STATIC_ID => world.add_static(entity),
        SENSOR_ID => world.add_sensor(entity),
        AUDIO_LISTENER_ID => world.add_audio_listener(entity),
        CAMERA_ID => {
            world.add_camera(entity, read_camera(&mut c)?);
        }
        MODEL_ID => {
            world.add_model(entity, Model { path: c.string()? });
        }
        SCRIPT_ID => {
            world.add_script(entity, ScriptComponent { script_name: c.string()? });
        }
        AUDIO_SOURCE_ID => {
            world.add_audio_source(entity, read_audio_source(&mut c)?);
        }
        ENTITY_TYPE_ID => {
            let type_name = c.string()?;
            world.set_entity_type(entity, &type_name);
            // Apply template defaults for this entity type. Per-component data
            // in subsequent entries will override these defaults.
            entity_types::apply_type(world, entity, &type_name);
        }
        other => {
            log::warn!("WMAP: skipping unknown component type_id {:#x}", other);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::World;
    use glam::Quat;

    fn sample_world() -> World {
        let mut w = World::new();
        let p = w.spawn();
        w.set_name(p, "player");
        w.add_transform(
            p,
            Transform3D {
                position: Vec3::new(1.0, 2.0, 3.0),
                rotation: Quat::from_xyzw(0.0, 0.0, 0.0, 1.0),
                scale: Vec3::new(2.0, 2.0, 2.0),
            },
        );
        w.add_velocity_3d(p, Velocity3D { linear: Vec3::new(0.5, 0.0, -0.5) });
        w.add_rigid_body(p, RigidBody { mass: 75.0, restitution: 0.1, angular_damping: 0.9 });
        w.add_collider(p, Collider { half_extents: Vec3::new(0.5, 0.5, 0.5) });
        w.set_color(p, Color { rgba: [1.0, 0.2, 0.3, 1.0] });
        w.add_player(p);
        w.add_camera(p, Camera::default());
        w.add_audio_listener(p);

        let g = w.spawn();
        w.set_name(g, "ground");
        w.add_transform(g, Transform3D::default());
        w.add_ground(g);
        w.add_static(g);
        w.add_collider(g, Collider { half_extents: Vec3::new(5.0, 0.5, 5.0) });
        w.set_color(g, Color { rgba: [0.5, 0.45, 0.35, 1.0] });

        let s = w.spawn();
        w.add_transform(s, Transform3D::default());
        w.add_script(s, ScriptComponent { script_name: "enemy".to_string() });
        w
    }

    #[test]
    fn round_trip_preserves_world() {
        let w = sample_world();
        let path = std::env::temp_dir().join("waraner_test.wmap");
        let path_str = path.to_str().unwrap();
        write_world(&w, path_str, 0xDEAD_BEEF).expect("write");

        let (w2, seed, names) = read_world(path_str).expect("read");
        assert_eq!(seed, 0xDEAD_BEEF);
        assert_eq!(w2.entities().len(), 3);

        let p = w2
            .entities()
            .into_iter()
            .find(|e| w2.get_name_or(e, "") == "player")
            .unwrap();
        let t = w2.get_transform(p).unwrap();
        assert_eq!(t.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(t.scale, Vec3::new(2.0, 2.0, 2.0));
        assert_eq!(w2.get_velocity_3d(p).unwrap().linear, Vec3::new(0.5, 0.0, -0.5));
        assert_eq!(w2.get_rigid_body(p).unwrap().mass, 75.0);
        assert!(w2.is_player(p));
        assert!(w2.get_camera(p).is_some());
        assert!(w2.is_audio_listener(p));
        let _ = names;

        let g = w2
            .entities()
            .into_iter()
            .find(|e| w2.get_name_or(e, "") == "ground")
            .unwrap();
        assert!(w2.is_ground(g));
        assert!(w2.is_static(g));
        assert_eq!(w2.get_collider(g).unwrap().half_extents, Vec3::new(5.0, 0.5, 5.0));

        let s = w2
            .entities()
            .into_iter()
            .find(|e| {
                w2.get_script(*e)
                    .map(|x| x.script_name == "enemy")
                    .unwrap_or(false)
            })
            .unwrap();
        assert_eq!(w2.get_script(s).unwrap().script_name, "enemy");

        let _ = std::fs::remove_file(path_str);
    }

    #[test]
    fn stable_ids_across_load() {
        let w = sample_world();
        let p = w
            .entities()
            .into_iter()
            .find(|e| w.get_name_or(e, "") == "player")
            .unwrap();
        let saved_id = ((p.generation as u64) << 32) | p.index as u64;

        let path = std::env::temp_dir().join("waraner_ids.wmap");
        let path_str = path.to_str().unwrap();
        write_world(&w, path_str, 0).expect("write");
        let (w2, _, _) = read_world(path_str).expect("read");
        let p2 = w2
            .entities()
            .into_iter()
            .find(|e| w2.get_name_or(e, "") == "player")
            .unwrap();
        let loaded_id = ((p2.generation as u64) << 32) | p2.index as u64;
        assert_eq!(saved_id, loaded_id);
        let _ = std::fs::remove_file(path_str);
    }

    #[test]
    fn v1_transform_migrates_scale() {
        // Manually craft a v1 Transform (no scale) and ensure it loads as v2.
        let mut comp = Vec::new();
        put_vec3(&mut comp, Vec3::new(9.0, 8.0, 7.0));
        put_f32(&mut comp, 0.0);
        put_f32(&mut comp, 0.0);
        put_f32(&mut comp, 0.0);
        put_f32(&mut comp, 1.0);

        let mut data = Vec::new();
        put_u32(&mut data, TRANSFORM_ID);
        put_u32(&mut data, 1); // v1
        put_u64(&mut data, comp.len() as u64);
        data.extend_from_slice(&comp);

        let mut w = World::new();
        let e = w.spawn();
        w.create_entity_at(e);
        let mut c = Cursor::new(&data);
        let type_id = c.u32().unwrap();
        let version = c.u32().unwrap();
        let size = c.u64().unwrap() as usize;
        let d = c.take(size).unwrap();
        apply_component(&mut w, e, type_id, version, d).unwrap();

        let t = w.get_transform(e).unwrap();
        assert_eq!(t.position, Vec3::new(9.0, 8.0, 7.0));
        assert_eq!(t.scale, Vec3::ONE);
    }
}

