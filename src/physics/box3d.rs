use std::collections::HashMap;

use box3d_rust as b3;

use crate::ecs::{Collider, Entity, Ground, InputState, RigidBody, Transform3D, World as EcsWorld};
use crate::physics::player_ctl::PlayerController;
use crate::physics::{CollisionEvent, PhysicsBackend, RayHit};

const STEP_UP_HEIGHT: f32 = 0.3;
const GRAVITY: f32 = -20.0;

pub struct Box3DPhysics {
    b3_world: b3::world::World,
    body_map: HashMap<Entity, b3::BodyId>,
    entity_by_body: HashMap<i32, Entity>,
    scales: HashMap<Entity, glam::Vec3>,
    player_entity: Entity,
    player_half_y: f32,
    on_ground: bool,
    collision_events: Vec<CollisionEvent>,
    player_ctl: PlayerController,
}

impl Box3DPhysics {
    pub fn new(
        ecs_world: &EcsWorld,
        player_entity: Entity,
        dynamic_entities: &[Entity],
    ) -> Self {
        let world_def = b3::types::default_world_def();
        let mut b3_world = b3::world::World::new(&world_def);

        b3::world::world_set_gravity(
            &mut b3_world,
            b3::math_functions::Vec3::new(0.0, -20.0, 0.0),
        );

        let mut body_map = HashMap::new();
        let mut entity_by_body = HashMap::new();
        let mut scales = HashMap::new();

        for entity in ecs_world.query().with::<Ground>().iter_entities() {
            if let Some(transform) = ecs_world.get_transform(entity) {
                if let Some(collider) = ecs_world.get_collider(entity) {
                    let half = collider.half_extents;
                    let pos = transform.position;
                    Self::create_box_body(
                        &mut b3_world,
                        &mut body_map,
                        &mut entity_by_body,
                        &mut scales,
                        entity,
                        b3::types::BodyType::Static,
                        pos,
                        half,
                        transform.scale,
                        0.0,
                        false,
                        0.0,
                    );
                }
            }
        }

        // Only pre-create the player here when the ECS world actually holds its
        // data. When bootstrapped from an empty world (the threaded physics
        // path), the player is added later via `add_body`.
        if let (Some(transform), Some(collider)) = (
            ecs_world.get_transform(player_entity),
            ecs_world.get_collider(player_entity),
        ) {
            let half = collider.half_extents;
            let is_sensor = ecs_world.is_sensor(player_entity);
            Self::create_box_body(
                &mut b3_world,
                &mut body_map,
                &mut entity_by_body,
                &mut scales,
                player_entity,
                b3::types::BodyType::Dynamic,
                transform.position,
                half,
                transform.scale,
                1.0,
                is_sensor,
                0.0,
            );

            let player_id = body_map[&player_entity];
            b3::body::body_set_linear_damping(&mut b3_world, player_id, 0.0);
            b3::body::body_set_motion_locks(
                &mut b3_world,
                player_id,
                b3::types::MotionLocks {
                    angular_x: true,
                    angular_y: true,
                    angular_z: true,
                    ..b3::types::MotionLocks::default()
                },
            );
        }

        for &entity in dynamic_entities {
            if entity == player_entity {
                continue;
            }
            if body_map.contains_key(&entity) {
                continue;
            }
            if let Some(transform) = ecs_world.get_transform(entity) {
                if let Some(collider) = ecs_world.get_collider(entity) {
                    let half = collider.half_extents;
                    let rb = ecs_world.get_rigid_body(entity)
                        .unwrap_or(RigidBody { mass: 1.0, restitution: 0.2, angular_damping: 0.98 });
                    let density = if rb.mass > 0.0 { 1.0 } else { 0.0 };
                    let is_sensor = ecs_world.is_sensor(entity);
                    Self::create_box_body(
                        &mut b3_world,
                        &mut body_map,
                        &mut entity_by_body,
                        &mut scales,
                        entity,
                        b3::types::BodyType::Dynamic,
                        transform.position,
                        half,
                        transform.scale,
                        density,
                        is_sensor,
                        rb.restitution,
                    );
                }
            }
        }

        Self {
            b3_world,
            body_map,
            entity_by_body,
            scales,
            player_entity,
            player_half_y: ecs_world
                .get_collider(player_entity)
                .map(|c| c.half_extents.y)
                .unwrap_or(0.5),
            on_ground: false,
            collision_events: Vec::new(),
            player_ctl: PlayerController::new(),
        }
    }

    fn create_box_body(
        b3_world: &mut b3::world::World,
        body_map: &mut HashMap<Entity, b3::BodyId>,
        entity_by_body: &mut HashMap<i32, Entity>,
        scales: &mut HashMap<Entity, glam::Vec3>,
        entity: Entity,
        body_type: b3::types::BodyType,
        position: glam::Vec3,
        half_extents: glam::Vec3,
        scale: glam::Vec3,
        density: f32,
        is_sensor: bool,
        restitution: f32,
    ) {
        let mut body_def = b3::types::default_body_def();
        body_def.type_ = body_type;
        body_def.position = b3::math_functions::Vec3::new(position.x, position.y, position.z);
        body_def.rotation = b3::math_functions::Quat::new(
            b3::math_functions::Vec3::new(0.0, 0.0, 0.0),
            1.0,
        );

        let body_id = b3::body::create_body(b3_world, &body_def);

        let hull = b3::hull::make_box_hull(half_extents.x, half_extents.y, half_extents.z);
        let mut shape_def = b3::types::default_shape_def();
        shape_def.density = density;
        shape_def.base_material.restitution = restitution;
        if is_sensor {
            shape_def.is_sensor = true;
        }
        let shape_id = b3::shape::create_hull_shape(b3_world, body_id, &shape_def, &hull.base);
        let _ = shape_id;

        if density > 0.0 {
            b3::body::body_apply_mass_from_shapes(b3_world, body_id);
        }

        b3::body::body_set_user_data(b3_world, body_id, entity.index as u64);

        body_map.insert(entity, body_id);
        entity_by_body.insert(body_id.index1, entity);
        scales.insert(entity, scale);
    }

    fn collect_collision_events(&mut self) {
        self.collision_events.clear();
        let events = b3::world::world_get_contact_events(&self.b3_world);

        for hit in events.hit_events {
            let body_a = b3::shape::shape_get_body(&self.b3_world, hit.shape_id_a);
            let body_b = b3::shape::shape_get_body(&self.b3_world, hit.shape_id_b);
            let entity_a = self.entity_by_body.get(&body_a.index1).copied();
            let entity_b = self.entity_by_body.get(&body_b.index1).copied();
            if let (Some(a), Some(b)) = (entity_a, entity_b) {
                self.collision_events.push(CollisionEvent {
                    entity_a: a,
                    entity_b: b,
                    point: glam::Vec3::new(hit.point.x, hit.point.y, hit.point.z),
                    normal: glam::Vec3::new(hit.normal.x, hit.normal.y, hit.normal.z),
                    approach_speed: hit.approach_speed,
                });
            }
        }
    }

    fn sync_to_ecs(&self, ecs_world: &mut EcsWorld, entities: &[Entity]) {
        for &entity in entities {
            if let Some(&body_id) = self.body_map.get(&entity) {
                let b3_transform = b3::body::body_get_transform(&self.b3_world, body_id);
                let original_scale = self.scales.get(&entity).copied().unwrap_or(glam::Vec3::ONE);
                let t = Transform3D {
                    position: glam::Vec3::new(
                        b3_transform.p.x,
                        b3_transform.p.y,
                        b3_transform.p.z,
                    ),
                    rotation: glam::Quat::from_xyzw(
                        b3_transform.q.v.x,
                        b3_transform.q.v.y,
                        b3_transform.q.v.z,
                        b3_transform.q.s,
                    ),
                    scale: original_scale,
                };
                ecs_world.add_transform(entity, t);
            }
        }
    }

    fn ensure_bodies_exist(
        &mut self,
        ecs_world: &EcsWorld,
        dynamic_entities: &[Entity],
    ) {
        for &entity in dynamic_entities {
            if entity == self.player_entity {
                continue;
            }
            if self.body_map.contains_key(&entity) {
                continue;
            }
            if let Some(transform) = ecs_world.get_transform(entity) {
                if let Some(collider) = ecs_world.get_collider(entity) {
                    let half = collider.half_extents;
                    let rb = ecs_world.get_rigid_body(entity)
                        .unwrap_or(RigidBody { mass: 1.0, restitution: 0.2, angular_damping: 0.98 });
                    let density = if rb.mass > 0.0 { 1.0 } else { 0.0 };
                    let is_sensor = ecs_world.is_sensor(entity);
                    Self::create_box_body(
                        &mut self.b3_world,
                        &mut self.body_map,
                        &mut self.entity_by_body,
                        &mut self.scales,
                        entity,
                        b3::types::BodyType::Dynamic,
                        transform.position,
                        half,
                        transform.scale,
                        density,
                        is_sensor,
                        rb.restitution,
                    );
                }
            }
        }
    }

    fn clamp_player_to_ground(&mut self, player_id: b3::BodyId) {
        let pos = b3::body::body_get_position(&self.b3_world, player_id);
        let origin = b3::math_functions::Pos::new(
            pos.x, pos.y - self.player_half_y + 0.05, pos.z,
        );
        let filter = b3::types::default_query_filter();
        let hit = b3::world::world_cast_ray_closest(
            &self.b3_world,
            origin,
            b3::math_functions::Vec3::new(0.0, -0.15, 0.0),
            &filter,
        );
        if hit.hit && hit.shape_id.index1 != player_id.index1 {
            let target_y = hit.point.y + self.player_half_y;
            if pos.y < target_y {
                let q = b3::body::body_get_transform(&self.b3_world, player_id).q;
                b3::body::body_set_transform(
                    &mut self.b3_world,
                    player_id,
                    b3::math_functions::Pos::new(pos.x, target_y, pos.z),
                    q,
                );
                let v = b3::body::body_get_linear_velocity(&self.b3_world, player_id);
                if v.y < 0.0 {
                    b3::body::body_set_linear_velocity(
                        &mut self.b3_world,
                        player_id,
                        b3::math_functions::Vec3::new(v.x, 0.0, v.z),
                    );
                }
            }
        }
    }

    fn check_on_ground(&self, player_id: b3::BodyId) -> bool {
        let pos = b3::body::body_get_position(&self.b3_world, player_id);
        let origin = b3::math_functions::Pos::new(
            pos.x, pos.y - self.player_half_y + 0.05, pos.z,
        );
        let filter = b3::types::default_query_filter();
        let hit = b3::world::world_cast_ray_closest(
            &self.b3_world,
            origin,
            b3::math_functions::Vec3::new(0.0, -0.15, 0.0),
            &filter,
        );
        hit.hit && hit.shape_id.index1 != player_id.index1
    }

    fn try_step_up(&mut self, player_id: b3::BodyId, velocity: &glam::Vec3, dt: f32) {
        let half_y = self.player_half_y;

        let horiz = glam::Vec3::new(velocity.x, 0.0, velocity.z);
        let dist = horiz.length() * dt;
        if dist < 0.001 {
            return;
        }
        let dir = horiz / horiz.length();

        let pos = b3::body::body_get_position(&self.b3_world, player_id);
        let filter = b3::types::default_query_filter();

        let foot_origin = glam::Vec3::new(pos.x, pos.y - half_y + 0.05, pos.z);
        let foot_translation = dir * (dist + 0.1);
        let foot_hit = b3::world::world_cast_ray_closest(
            &self.b3_world,
            b3::math_functions::Pos::new(foot_origin.x, foot_origin.y, foot_origin.z),
            v3(foot_translation),
            &filter,
        );
        if !foot_hit.hit || foot_hit.shape_id.index1 == player_id.index1 {
            return;
        }

        let mid_origin = glam::Vec3::new(pos.x, pos.y - half_y + STEP_UP_HEIGHT, pos.z);
        let mid_hit = b3::world::world_cast_ray_closest(
            &self.b3_world,
            b3::math_functions::Pos::new(mid_origin.x, mid_origin.y, mid_origin.z),
            v3(foot_translation),
            &filter,
        );
        if mid_hit.hit && mid_hit.shape_id.index1 != player_id.index1 {
            return;
        }

        let up_hit = b3::world::world_cast_ray_closest(
            &self.b3_world,
            b3::math_functions::Pos::new(pos.x, pos.y, pos.z),
            b3::math_functions::Vec3::new(0.0, STEP_UP_HEIGHT + 0.1, 0.0),
            &filter,
        );
        if up_hit.hit && up_hit.shape_id.index1 != player_id.index1 {
            return;
        }

        let q = b3::body::body_get_transform(&self.b3_world, player_id).q;
        b3::body::body_set_transform(
            &mut self.b3_world,
            player_id,
            b3::math_functions::Pos::new(pos.x, pos.y + STEP_UP_HEIGHT, pos.z),
            q,
        );
    }
}

fn v3(g: glam::Vec3) -> b3::math_functions::Vec3 {
    b3::math_functions::Vec3::new(g.x, g.y, g.z)
}

fn v3g(b: b3::math_functions::Vec3) -> glam::Vec3 {
    glam::Vec3::new(b.x, b.y, b.z)
}

impl PhysicsBackend for Box3DPhysics {
    fn remove_entity(&mut self, entity: Entity) {
        if let Some(&body_id) = self.body_map.get(&entity) {
            b3::body::destroy_body(&mut self.b3_world, body_id);
            self.body_map.remove(&entity);
            self.entity_by_body.remove(&body_id.index1);
        }
    }

    fn teleport_player(&mut self, player_entity: Entity, position: glam::Vec3) {
        if let Some(&body_id) = self.body_map.get(&player_entity) {
            let current = b3::body::body_get_transform(&self.b3_world, body_id);
            let new_pos = b3::math_functions::Pos::new(position.x, position.y, position.z);
            b3::body::body_set_transform(&mut self.b3_world, body_id, new_pos, current.q);
        }
    }

    fn tick(
        &mut self,
        ecs_world: &mut EcsWorld,
        input: &InputState,
        player_entity: Entity,
        camera_yaw: f32,
        dynamic_entities: &[Entity],
        dt: f32,
    ) {
        self.ensure_bodies_exist(ecs_world, dynamic_entities);

        let player_id = match self.body_map.get(&player_entity) {
            Some(&id) => id,
            None => return,
        };

        let current_vel = b3::body::body_get_linear_velocity(&self.b3_world, player_id);
        let mut vel = v3g(current_vel);

        let wish_dir = self.player_ctl.compute_velocity(
            input, camera_yaw, &mut vel, self.on_ground, dt,
        );

        let has_move_input = wish_dir.length_squared() > 0.0;

        if has_move_input && self.on_ground {
            self.try_step_up(player_id, &vel, dt);
        }

        b3::body::body_set_linear_velocity(
            &mut self.b3_world,
            player_id,
            v3(vel),
        );

        let half_yaw = camera_yaw * 0.5;
        let player_rot = b3::math_functions::Quat::new(
            b3::math_functions::Vec3::new(0.0, half_yaw.sin(), 0.0),
            half_yaw.cos(),
        );
        let current_pos = b3::body::body_get_position(&self.b3_world, player_id);
        b3::body::body_set_transform(&mut self.b3_world, player_id, current_pos, player_rot);

        b3::world::World::step(&mut self.b3_world, dt, 4);

        self.clamp_player_to_ground(player_id);
        self.on_ground = self.check_on_ground(player_id);

        self.collect_collision_events();

        let all_entities: Vec<Entity> = self.body_map.keys().copied().collect();
        self.sync_to_ecs(ecs_world, &all_entities);

        if let Some(t) = ecs_world.get_transform_mut(player_entity) {
            t.rotation = glam::Quat::from_rotation_y(camera_yaw);
        }
    }

    fn tick_raw(
        &mut self,
        input: &InputState,
        player_entity: Entity,
        camera_yaw: f32,
        dt: f32,
    ) -> (HashMap<Entity, Transform3D>, Vec<CollisionEvent>) {
        let player_id = match self.body_map.get(&player_entity) {
            Some(&id) => id,
            None => return (HashMap::new(), Vec::new()),
        };

        let current_vel = b3::body::body_get_linear_velocity(&self.b3_world, player_id);
        let mut vel = v3g(current_vel);

        let wish_dir = self.player_ctl.compute_velocity(
            input, camera_yaw, &mut vel, self.on_ground, dt,
        );

        let has_move_input = wish_dir.length_squared() > 0.0;

        if has_move_input && self.on_ground {
            self.try_step_up(player_id, &vel, dt);
        }

        b3::body::body_set_linear_velocity(
            &mut self.b3_world,
            player_id,
            v3(vel),
        );

        let half_yaw = camera_yaw * 0.5;
        let player_rot = b3::math_functions::Quat::new(
            b3::math_functions::Vec3::new(0.0, half_yaw.sin(), 0.0),
            half_yaw.cos(),
        );
        let current_pos = b3::body::body_get_position(&self.b3_world, player_id);
        b3::body::body_set_transform(&mut self.b3_world, player_id, current_pos, player_rot);

        b3::world::World::step(&mut self.b3_world, dt, 4);

        self.clamp_player_to_ground(player_id);
        self.on_ground = self.check_on_ground(player_id);

        self.collect_collision_events();

        let all_entities: Vec<Entity> = self.body_map.keys().copied().collect();
        let mut transforms = HashMap::new();
        for &entity in &all_entities {
            if let Some(&body_id) = self.body_map.get(&entity) {
                let b3_transform = b3::body::body_get_transform(&self.b3_world, body_id);
                let scale = self.scales.get(&entity).copied().unwrap_or(glam::Vec3::ONE);
                let t = Transform3D {
                    position: glam::Vec3::new(
                        b3_transform.p.x,
                        b3_transform.p.y,
                        b3_transform.p.z,
                    ),
                    rotation: glam::Quat::from_xyzw(
                        b3_transform.q.v.x,
                        b3_transform.q.v.y,
                        b3_transform.q.v.z,
                        b3_transform.q.s,
                    ),
                    scale,
                };
                transforms.insert(entity, t);
            }
        }

        let collisions = std::mem::take(&mut self.collision_events);
        (transforms, collisions)
    }

    fn add_body(
        &mut self,
        entity: Entity,
        transform: Transform3D,
        collider: Collider,
        rigid_body: RigidBody,
        is_sensor: bool,
        is_player: bool,
    ) {
        let body_type = if is_player || rigid_body.mass > 0.0 {
            b3::types::BodyType::Dynamic
        } else {
            b3::types::BodyType::Static
        };
        let density = if rigid_body.mass > 0.0 { 1.0 } else { 0.0 };
        Self::create_box_body(
            &mut self.b3_world,
            &mut self.body_map,
            &mut self.entity_by_body,
            &mut self.scales,
            entity,
            body_type,
            transform.position,
            collider.half_extents,
            transform.scale,
            density,
            is_sensor,
            rigid_body.restitution,
        );
        if is_player {
            self.player_half_y = collider.half_extents.y;
            let player_id = self.body_map[&entity];
            b3::body::body_set_linear_damping(&mut self.b3_world, player_id, 0.0);
            b3::body::body_set_motion_locks(
                &mut self.b3_world,
                player_id,
                b3::types::MotionLocks {
                    angular_x: true,
                    angular_y: true,
                    angular_z: true,
                    ..b3::types::MotionLocks::default()
                },
            );
        }
    }

    fn drain_collision_events(&mut self) -> Vec<CollisionEvent> {
        std::mem::take(&mut self.collision_events)
    }

    fn ray_cast(
        &self,
        origin: glam::Vec3,
        direction: glam::Vec3,
        max_distance: f32,
    ) -> Option<RayHit> {
        let filter = b3::types::default_query_filter();
        let translation = b3::math_functions::Vec3::new(
            direction.x * max_distance,
            direction.y * max_distance,
            direction.z * max_distance,
        );
        let result = b3::world::world_cast_ray_closest(
            &self.b3_world,
            b3::math_functions::Pos::new(origin.x, origin.y, origin.z),
            translation,
            &filter,
        );
        if result.hit {
            let entity = self.entity_by_body
                .get(&result.shape_id.index1)
                .copied();
            Some(RayHit {
                entity,
                point: glam::Vec3::new(result.point.x, result.point.y, result.point.z),
                normal: glam::Vec3::new(result.normal.x, result.normal.y, result.normal.z),
                fraction: result.fraction,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::{Collider, InputState, RigidBody, Transform3D, World};

    fn input() -> InputState {
        InputState {
            forward: false,
            backward: false,
            left: false,
            right: false,
            jump: false,
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            pointer_locked: false,
            keys: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn cube_falls_and_rests_on_ground() {
        let mut world = World::new();

        let ground = world.spawn();
        world.add_ground(ground);
        world.add_transform(
            ground,
            Transform3D {
                position: glam::Vec3::new(0.0, 0.0, 0.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.add_collider(ground, Collider { half_extents: glam::Vec3::new(5.0, 0.5, 5.0) });

        let player = world.spawn();
        world.add_player(player);
        world.add_transform(
            player,
            Transform3D {
                position: glam::Vec3::new(0.0, 1.0, 0.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.add_collider(player, Collider { half_extents: glam::Vec3::new(0.5, 0.5, 0.5) });
        world.add_rigid_body(player, RigidBody { mass: 1.0, restitution: 0.0, angular_damping: 0.98 });

        let cube = world.spawn();
        world.add_transform(
            cube,
            Transform3D {
                position: glam::Vec3::new(2.0, 3.0, 0.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.add_collider(cube, Collider { half_extents: glam::Vec3::new(0.5, 0.5, 0.5) });
        world.add_rigid_body(cube, RigidBody { mass: 1.0, restitution: 0.0, angular_damping: 0.98 });

        let mut phys: Box<dyn PhysicsBackend> =
            Box::new(Box3DPhysics::new(&world, player, &[cube]));
        let input = input();
        for _ in 0..240 {
            let (transforms, _c) = phys.tick_raw(&input, player, 0.0, 1.0 / 60.0);
            for (e, t) in transforms {
                if let Some(mut tr) = world.get_transform_mut(e) {
                    *tr = t;
                }
            }
        }

        let cube_t = world.get_transform(cube).unwrap();
        assert!(
            cube_t.position.y > 0.8 && cube_t.position.y < 1.2,
            "box3d cube did not rest on ground, y = {}",
            cube_t.position.y
        );
    }
}
