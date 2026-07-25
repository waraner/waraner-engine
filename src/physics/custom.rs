#![allow(dead_code)]

use std::collections::HashMap;


use glam::{Quat, Vec3};

use crate::ecs::{Collider, Entity, Ground, InputState, RigidBody, World};
use crate::physics::PhysicsBackend;

const CELL_SIZE: f32 = 4.0;
const MAX_SLOPE_ANGLE: f32 = 35.0_f32.to_radians();

struct AABB {
    min: Vec3,
    max: Vec3,
}

struct SpatialGrid {
    cell_size: f32,
    cells: HashMap<(i32, i32, i32), Vec<Entity>>,
}

impl SpatialGrid {
    fn new(cell_size: f32) -> Self {
        Self { cell_size, cells: HashMap::new() }
    }

    fn clear(&mut self) {
        self.cells.clear();
    }

    fn key(x: f32, y: f32, z: f32, cell_size: f32) -> (i32, i32, i32) {
        (
            (x / cell_size).floor() as i32,
            (y / cell_size).floor() as i32,
            (z / cell_size).floor() as i32,
        )
    }

    fn insert(&mut self, entity: Entity, min: Vec3, max: Vec3) {
        let min_key = Self::key(min.x, min.y, min.z, self.cell_size);
        let max_key = Self::key(max.x, max.y, max.z, self.cell_size);
        for x in min_key.0..=max_key.0 {
            for y in min_key.1..=max_key.1 {
                for z in min_key.2..=max_key.2 {
                    self.cells.entry((x, y, z)).or_default().push(entity);
                }
            }
        }
    }

    fn query(&self, min: Vec3, max: Vec3) -> Vec<Entity> {
        let mut result = Vec::new();
        let min_key = Self::key(min.x, min.y, min.z, self.cell_size);
        let max_key = Self::key(max.x, max.y, max.z, self.cell_size);
        for x in min_key.0..=max_key.0 {
            for y in min_key.1..=max_key.1 {
                for z in min_key.2..=max_key.2 {
                    if let Some(entities) = self.cells.get(&(x, y, z)) {
                        result.extend(entities.iter().copied());
                    }
                }
            }
        }
        result
    }
}

pub struct CustomPhysics {
    spatial_grid: SpatialGrid,
    on_ground: bool,
}

impl Default for CustomPhysics {
    fn default() -> Self {
        Self {
            spatial_grid: SpatialGrid::new(CELL_SIZE),
            on_ground: false,
        }
    }
}

impl CustomPhysics {
    fn aabb_for_entity(&self, world: &World, entity: Entity) -> Option<AABB> {
        let transform = world.get_transform(entity)?;
        let collider = world.get_collider(entity)?;
        let world_half = transform.scale * collider.half_extents;
        let center = transform.position;
        Some(AABB { min: center - world_half, max: center + world_half })
    }

    fn rebuild_spatial_grid(&mut self, world: &World) {
        self.spatial_grid.clear();
        for entity in world.query().with::<Ground>().iter_entities() {
            if let Some(aabb) = self.aabb_for_entity(world, entity) {
                self.spatial_grid.insert(entity, aabb.min, aabb.max);
            }
        }
    }

    fn resolve_dynamic_collisions(&mut self, world: &mut World, dynamics: &[Entity], _dt: f32) {
        for i in 0..dynamics.len() {
            for j in (i + 1)..dynamics.len() {
                let a = dynamics[i];
                let b = dynamics[j];
                if let (Some(ta), Some(tb)) =
                    (world.get_transform(a), world.get_transform(b))
                {
                    if let (Some(ca), Some(cb), Some(rba), Some(rbb)) = (
                        world.get_collider(a),
                        world.get_collider(b),
                        world.get_rigid_body(a),
                        world.get_rigid_body(b),
                    ) {
                        let ha = ta.scale * ca.half_extents;
                        let hb = tb.scale * cb.half_extents;
                        let min_a = ta.position - ha;
                        let max_a = ta.position + ha;
                        let min_b = tb.position - hb;
                        let max_b = tb.position + hb;

                        if max_a.x > min_b.x
                            && min_a.x < max_b.x
                            && max_a.y > min_b.y
                            && min_a.y < max_b.y
                            && max_a.z > min_b.z
                            && min_a.z < max_b.z
                        {
                            let overlap_x = max_a.x.min(max_b.x) - min_a.x.max(min_b.x);
                            let overlap_y = max_a.y.min(max_b.y) - min_a.y.max(min_b.y);
                            let overlap_z = max_a.z.min(max_b.z) - min_a.z.max(min_b.z);

                            let rest = (rba.restitution + rbb.restitution) * 0.5;

                            let (axis, overlap, sign) =
                                if overlap_x <= overlap_y && overlap_x <= overlap_z {
                                    (Vec3::X, overlap_x, if ta.position.x > tb.position.x { 1.0 } else { -1.0 })
                                } else if overlap_y <= overlap_x && overlap_y <= overlap_z {
                                    (Vec3::Y, overlap_y, if ta.position.y > tb.position.y { 1.0 } else { -1.0 })
                                } else {
                                    (Vec3::Z, overlap_z, if ta.position.z > tb.position.z { 1.0 } else { -1.0 })
                                };

                            if overlap > 0.0 && rba.mass > 0.0 && rbb.mass > 0.0 {
                                let rel_vel = if let (Some(va), Some(vb)) =
                                    (world.get_velocity_3d(a), world.get_velocity_3d(b))
                                {
                                    if axis.x != 0.0 {
                                        va.linear.x - vb.linear.x
                                    } else if axis.y != 0.0 {
                                        va.linear.y - vb.linear.y
                                    } else {
                                        va.linear.z - vb.linear.z
                                    }
                                } else {
                                    0.0
                                };
                                let impulse = (1.0 + rest) * rel_vel * 0.5;

                                let push = overlap * 0.5 * sign;
                                if let Some(pos_a) = world.get_transform_mut(a) {
                                    pos_a.position += axis * push;
                                }
                                if let Some(pos_b) = world.get_transform_mut(b) {
                                    pos_b.position -= axis * push;
                                }
                                if let Some(va) = world.get_velocity_3d_mut(a) {
                                    if axis.x != 0.0 {
                                        va.linear.x -= impulse * sign / rba.mass;
                                    } else if axis.y != 0.0 {
                                        va.linear.y -= impulse * sign / rba.mass;
                                    } else {
                                        va.linear.z -= impulse * sign / rba.mass;
                                    }
                                }
                                if let Some(vb) = world.get_velocity_3d_mut(b) {
                                    if axis.x != 0.0 {
                                        vb.linear.x += impulse * sign / rbb.mass;
                                    } else if axis.y != 0.0 {
                                        vb.linear.y += impulse * sign / rbb.mass;
                                    } else {
                                        vb.linear.z += impulse * sign / rbb.mass;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

impl PhysicsBackend for CustomPhysics {
    fn tick(
        &mut self,
        world: &mut World,
        input: &InputState,
        player_entity: Entity,
        camera_yaw: f32,
        dynamic_entities: &[Entity],
        dt: f32,
    ) {
        const GRAVITY: Vec3 = Vec3::new(0.0, -20.0, 0.0);
        const MOVE_SPEED: f32 = 6.0;
        const JUMP_VEL: f32 = 8.0;

        let forward = Vec3::new(camera_yaw.sin(), 0.0, camera_yaw.cos());
        let right = Vec3::new(-camera_yaw.cos(), 0.0, camera_yaw.sin());

        let mut transform = match world.get_transform(player_entity) {
            Some(t) => t,
            None => return,
        };
        let mut vel = match world.get_velocity_3d(player_entity) {
            Some(v) => v,
            None => return,
        };
        let player_col = world.get_collider(player_entity)
            .unwrap_or(Collider { half_extents: Vec3::splat(0.5) });
        let player_rb = world.get_rigid_body(player_entity)
            .unwrap_or(RigidBody { mass: 1.0, restitution: 0.0, angular_damping: 0.95 });
        let player_half = transform.scale * player_col.half_extents;

        vel.linear += GRAVITY * dt;

        let mut input_dir = Vec3::ZERO;
        if input.forward { input_dir += forward; }
        if input.backward { input_dir -= forward; }
        if input.left { input_dir -= right; }
        if input.right { input_dir += right; }

        if input_dir.length_squared() > 0.0 {
            input_dir = input_dir.normalize() * MOVE_SPEED;
            let accel = if self.on_ground { 15.0 } else { 3.0 };
            vel.linear.x += (input_dir.x - vel.linear.x) * (accel * dt).min(1.0);
            vel.linear.z += (input_dir.z - vel.linear.z) * (accel * dt).min(1.0);
        } else {
            let damp = if self.on_ground { 0.85 } else { 0.99 };
            vel.linear.x *= damp;
            vel.linear.z *= damp;
        }

        if input.jump && self.on_ground {
            vel.linear.y = JUMP_VEL;
        }

        transform.position += vel.linear * dt;

        self.rebuild_spatial_grid(world);

        let ground_candidates = self.spatial_grid.query(
            transform.position - player_half - Vec3::Y * 0.1,
            transform.position + player_half + Vec3::Y * 0.1,
        );

        let mut on_ground = false;

        for &entity in &ground_candidates {
            if !world.is_ground(entity) {
                continue;
            }
            if let Some(gt) = world.get_transform(entity) {
                if let Some(gcol) = world.get_collider(entity) {
                    let ground_pos = gt.position;
                    let inv_rot = gt.rotation.inverse();
                    let local_player = inv_rot * (transform.position - ground_pos);
                    let local_min = local_player - player_half;
                    let local_max = local_player + player_half;
                    let local_ground_min = -gcol.half_extents;
                    let local_ground_max = gcol.half_extents;

                    let overlap_x =
                        local_max.x.min(local_ground_max.x) - local_min.x.max(local_ground_min.x);
                    let overlap_z =
                        local_max.z.min(local_ground_max.z) - local_min.z.max(local_ground_min.z);

                    if overlap_x > 0.0 && overlap_z > 0.0 {
                        let local_player_bottom = local_min.y;
                        let local_ground_top = local_ground_max.y;
                        let penetration = local_ground_top - local_player_bottom;

                        if penetration > 0.0 && penetration < 2.0 {
                            let world_normal = gt.rotation * Vec3::Y;
                            let slope_angle = world_normal.angle_between(Vec3::Y);

                            if slope_angle <= MAX_SLOPE_ANGLE {
                                let world_push = world_normal * penetration;
                                transform.position += world_push;
                                if vel.linear.y < 0.0 {
                                    vel.linear.y = -vel.linear.y * player_rb.restitution.max(0.0);
                                }
                                on_ground = true;
                            } else {
                                let tangent_x = Vec3::cross(world_normal, Vec3::Z);
                                let tangent_len = tangent_x.length();
                                if tangent_len > 0.0001 {
                                    let tangent_x = tangent_x / tangent_len;
                                    let tangent_z = Vec3::cross(world_normal, tangent_x).normalize();
                                    vel.linear.x = vel.linear.dot(tangent_x);
                                    vel.linear.z = vel.linear.dot(tangent_z);
                                }
                                on_ground = false;
                            }
                        }
                    }
                }
            }
        }

        self.on_ground = on_ground;

        transform.rotation = Quat::from_rotation_y(camera_yaw);
        world.add_transform(player_entity, transform);
        world.add_velocity_3d(player_entity, vel);

        for &entity in dynamic_entities {
            if entity == player_entity {
                continue;
            }
            if let Some(mut body_transform) = world.get_transform(entity) {
                if let Some(mut body_vel) = world.get_velocity_3d(entity) {
                    let rb = world.get_rigid_body(entity);
                    if rb.map(|r| r.mass > 0.0).unwrap_or(false) {
                        body_vel.linear += GRAVITY * dt;
                        body_transform.position += body_vel.linear * dt;

                        let col = world.get_collider(entity)
                            .unwrap_or(Collider { half_extents: Vec3::splat(0.5) });
                        let half = body_transform.scale * col.half_extents;
                        let body_aabb = AABB {
                            min: body_transform.position - half,
                            max: body_transform.position + half,
                        };
                        let body_candidates = self.spatial_grid.query(
                            body_aabb.min - Vec3::Y * 0.1,
                            body_aabb.max + Vec3::Y * 0.1,
                        );

                        let body_rest = rb.map(|r| r.restitution.max(0.0)).unwrap_or(0.0);
                        for &ground in &body_candidates {
                            if world.is_ground(ground) {
                                if let Some(gt) = world.get_transform(ground) {
                                    if let Some(gcol) = world.get_collider(ground) {
                                        let ground_min = gt.position - gcol.half_extents;
                                        let ground_max = gt.position + gcol.half_extents;

                                        if body_aabb.max.x > ground_min.x
                                            && body_aabb.min.x < ground_max.x
                                            && body_aabb.max.z > ground_min.z
                                            && body_aabb.min.z < ground_max.z
                                        {
                                            let penetration = ground_max.y - body_aabb.min.y;
                                            if penetration > 0.0 && penetration < 2.0 {
                                                body_transform.position.y += penetration;
                                                if body_vel.linear.y < 0.0 {
                                                    body_vel.linear.y = -body_vel.linear.y * body_rest;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        if let Some(body_ang) = world.get_angular_velocity_mut(entity) {
                            body_transform.rotation *=
                                Quat::from_scaled_axis(body_ang.radians * dt);
                            body_ang.radians *= rb.map(|r| r.angular_damping).unwrap_or(1.0);
                        }
                        world.add_transform(entity, body_transform);
                        world.add_velocity_3d(entity, body_vel);
                    }
                }
            }
        }

        self.resolve_dynamic_collisions(world, dynamic_entities, dt);
    }
}
