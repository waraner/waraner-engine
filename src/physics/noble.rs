use std::collections::HashMap;

use noble_physics::core::BodyId;
use noble_physics::dynamics::{self, BodyType};
use noble_physics::math::{Mat33, Quat, Vec3};
use noble_physics::shapes::{BoxShape, Shape, ShapeData};

use crate::ecs::{Collider, Entity, Ground, InputState, RigidBody, Transform3D, World as EcsWorld};
use crate::physics::{CollisionEvent, PhysicsBackend, RayHit};
use crate::physics::player_ctl::PlayerController;

const STEP_UP_HEIGHT: f32 = 0.3;

fn v3(g: glam::Vec3) -> Vec3 {
    Vec3::new(g.x, g.y, g.z)
}

fn v3g(n: Vec3) -> glam::Vec3 {
    glam::Vec3::new(n.x, n.y, n.z)
}

fn quatg(q: Quat) -> glam::Quat {
    glam::Quat::from_xyzw(q.v.x, q.v.y, q.v.z, q.s)
}

fn raw_from_entity(e: Entity) -> u32 {
    (e.index & 0x00FF_FFFF) | ((e.generation as u32) << 24)
}

pub struct NoblePhysics {
    world: noble_physics::world::World,
    body_map: HashMap<Entity, BodyId>,
    entity_by_body: HashMap<u32, Entity>,
    scales: HashMap<Entity, glam::Vec3>,
    half_extents: HashMap<Entity, glam::Vec3>,
    player_entity: Entity,
    player_half_y: f32,
    on_ground: bool,
    collision_events: Vec<CollisionEvent>,
    player_ctl: PlayerController,
}

impl NoblePhysics {
    pub fn new(
        ecs_world: &EcsWorld,
        player_entity: Entity,
        dynamic_entities: &[Entity],
    ) -> Self {
        let mut nw = noble_physics::world::World::new_with_gravity(Vec3::new(0.0, -20.0, 0.0));
        // Sleeping is disabled: it is buggy and must not be used. A resting
        // crate's tiny solver residual is handled by the wake-on-contact /
        // contact-solver behaviour instead; bodies never go to sleep.
        nw.allow_sleep = false;

        let mut body_map = HashMap::new();
        let mut entity_by_body = HashMap::new();
        let mut scales = HashMap::new();
        let mut half_extents = HashMap::new();

        for entity in ecs_world.query().with::<Ground>().iter_entities() {
            if let Some(transform) = ecs_world.get_transform(entity) {
                if let Some(collider) = ecs_world.get_collider(entity) {
                    let half = collider.half_extents;
                    Self::create_box_body(
                        &mut nw,
                        &mut body_map,
                        &mut entity_by_body,
                        &mut scales,
                        &mut half_extents,
                        entity,
                        BodyType::Static,
                        transform.position,
                        half,
                        transform.scale,
                        RigidBody {
                            mass: 0.0,
                            restitution: 0.2,
                            angular_damping: 0.98,
                        },
                        false,
                        false,
                    );
                }
            }
        }

        let player_half = {
            let collider = ecs_world
                .get_collider(player_entity)
                .unwrap_or(Collider {
                    half_extents: Default::default(),
                });
            collider.half_extents
        };

        // Only pre-create the player here when the ECS world actually holds its
        // data. When bootstrapped from an empty world (the threaded physics
        // path), the player is added later via `add_body`, so we must not
        // create a stray duplicate body here.
        if let (Some(transform), Some(collider)) = (
            ecs_world.get_transform(player_entity),
            ecs_world.get_collider(player_entity),
        ) {
            let half = collider.half_extents;
            let is_sensor = ecs_world.is_sensor(player_entity);
            Self::create_box_body(
                &mut nw,
                &mut body_map,
                &mut entity_by_body,
                &mut scales,
                &mut half_extents,
                player_entity,
                BodyType::Dynamic,
                transform.position,
                half,
                transform.scale,
                RigidBody {
                    mass: 1.0,
                    restitution: 0.0,
                    angular_damping: 0.98,
                },
                is_sensor,
                true,
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
                    let rb = ecs_world
                        .get_rigid_body(entity)
                        .unwrap_or(RigidBody {
                            mass: 1.0,
                            restitution: 0.2,
                            angular_damping: 0.98,
                        });
                    let is_sensor = ecs_world.is_sensor(entity);
                    Self::create_box_body(
                        &mut nw,
                        &mut body_map,
                        &mut entity_by_body,
                        &mut scales,
                        &mut half_extents,
                        entity,
                        BodyType::Dynamic,
                        transform.position,
                        half,
                        transform.scale,
                        rb,
                        is_sensor,
                        false,
                    );
                }
            }
        }

        Self {
            world: nw,
            body_map,
            entity_by_body,
            scales,
            half_extents,
            player_entity,
            player_half_y: player_half.y,
            on_ground: false,
            collision_events: Vec::new(),
            player_ctl: PlayerController::new(),
        }
    }

    fn create_box_body(
        nw: &mut noble_physics::world::World,
        body_map: &mut HashMap<Entity, BodyId>,
        entity_by_body: &mut HashMap<u32, Entity>,
        scales: &mut HashMap<Entity, glam::Vec3>,
        half_extents_map: &mut HashMap<Entity, glam::Vec3>,
        entity: Entity,
        body_type: BodyType,
        position: glam::Vec3,
        half_extents: glam::Vec3,
        scale: glam::Vec3,
        rigid_body: RigidBody,
        is_sensor: bool,
        _is_player: bool,
    ) {
        let mass = if body_type == BodyType::Dynamic {
            rigid_body.mass
        } else {
            0.0
        };
        let inertia = Mat33::box_inertia(mass, -v3(half_extents), v3(half_extents));

        let body = if body_type == BodyType::Dynamic {
            let mut b = dynamics::Body::new_dynamic(v3(position), Quat::IDENTITY, mass, inertia);
            b.user_data = raw_from_entity(entity);
            b.is_sensor = is_sensor;
            b.angular_damping = rigid_body.angular_damping;
            b.is_sleeping = false;
            // CCD only for free dynamic bodies (e.g. a cube dropped from
            // height). The player is velocity-controlled and CCD's swept
            // time-of-impact fights that control, launching it when it slides
            // along a surface. The player is instead kept on the ground by an
            // explicit post-step clamp (see tick / tick_raw).
            if !_is_player {
                b.ccd_enabled = true;
                b.ccd_motion_threshold = 0.0;
                // Crates rotate naturally when shoved or nudged off an edge,
                // and tip/fall under gravity. A modest angular damping lets
                // them settle after a turn instead of jittering forever,
                // without preventing tipping.
                b.angular_damping = 2.0;
            }
            // The player is a character controller: rotation stays locked so it
            // never tumbles; its orientation is driven by the camera yaw.
            b.lock_rotation = _is_player;
            nw.create_body(b)
        } else {
            let mut b = dynamics::Body::new(v3(position), Quat::IDENTITY, BodyType::Static);
            b.user_data = raw_from_entity(entity);
            b.is_sensor = is_sensor;
            nw.create_body(b)
        };

        let density = if mass > 0.0 { 1.0 } else { 0.0 };
        let shape = Shape::with_material(
            ShapeData::Box(BoxShape::new(v3(half_extents))),
            0.6,
            rigid_body.restitution,
            density,
        );
        let shape_id = nw.create_shape(shape);
        nw.attach_shape(body, shape_id);

        // Restore the ECS-authored mass/inertia (attach_shape overwrites it from
        // shape density, discarding the intended RigidBody.mass).
        if body_type == BodyType::Dynamic {
            if let Some(b) = nw.bodies.get_mut(body) {
                b.set_mass_properties(mass, inertia);
            }
        }

        body_map.insert(entity, body);
        entity_by_body.insert(body.raw(), entity);
        scales.insert(entity, scale);
        half_extents_map.insert(entity, half_extents);
    }

    fn collect_collision_events(&mut self) {
        self.collision_events.clear();
        for event in &self.world.events.contacts {
            use noble_physics::world::ContactEventType;
            if event.event_type != ContactEventType::Begin {
                continue;
            }
            let entity_a = self.entity_by_body.get(&event.body_a.raw()).copied();
            let entity_b = self.entity_by_body.get(&event.body_b.raw()).copied();
            if let (Some(a), Some(b)) = (entity_a, entity_b) {
                self.collision_events.push(CollisionEvent {
                    entity_a: a,
                    entity_b: b,
                    point: glam::Vec3::ZERO,
                    normal: glam::Vec3::ZERO,
                    approach_speed: event.impulse,
                });
            }
        }
    }

    fn sync_to_ecs(&self, ecs_world: &mut EcsWorld, entities: &[Entity]) {
        for &entity in entities {
            if let Some(&body_id) = self.body_map.get(&entity) {
                if let Some(body) = self.world.bodies.get(body_id) {
                    let scale = self.scales.get(&entity).copied().unwrap_or(glam::Vec3::ONE);
                    let t = Transform3D {
                        position: v3g(body.transform.p),
                        rotation: quatg(body.transform.q),
                        scale,
                    };
                    ecs_world.add_transform(entity, t);
                }
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
                    let rb = ecs_world
                        .get_rigid_body(entity)
                        .unwrap_or(RigidBody {
                            mass: 1.0,
                            restitution: 0.2,
                            angular_damping: 0.98,
                        });
                    let is_sensor = ecs_world.is_sensor(entity);
                    Self::create_box_body(
                        &mut self.world,
                        &mut self.body_map,
                        &mut self.entity_by_body,
                        &mut self.scales,
                        &mut self.half_extents,
                        entity,
                        BodyType::Dynamic,
                        transform.position,
                        half,
                        transform.scale,
                        rb,
                        is_sensor,
                        false,
                    );
                }
            }
        }
    }

    fn player_on_ground(&self, player_id: BodyId) -> bool {
        let Some(body) = self.world.bodies.get(player_id) else {
            return false;
        };
        let half_y = self
            .half_extents
            .get(&self.player_entity)
            .map(|h| h.y)
            .unwrap_or(self.player_half_y);
        let origin = body.transform.p + Vec3::new(0.0, 0.05, 0.0);
        let max_frac = half_y + 0.15;
        let input = noble_physics::world::RayCastInput {
            origin,
            direction: Vec3::new(0.0, -1.0, 0.0),
            max_frac,
        };
        // Exclude the player's own shape: the ray starts inside the player's
        // box and would otherwise hit its own bottom face, making it report
        // "grounded" every frame (infinite jump).
        self.world
            .ray_cast_all(&input)
            .into_iter()
            .any(|hit| hit.body_id != player_id)
    }

    // Keep the velocity-controlled player resting on whatever solid is directly
    // beneath it. The dynamic solver alone is unstable for this body: it sinks,
    // tunnels, and teleports when moving. An explicit ground clamp makes the
    // player behave like a character controller — its bottom is snapped to the
    // surface it stands on and downward velocity is cancelled — while still
    // letting it push dynamic bodies (cubes) through the contact solver.
    fn clamp_player_to_ground(&mut self, player_id: BodyId, _dt: f32) {
        let Some(body) = self.world.bodies.get(player_id) else {
            return;
        };
        let half_y = self
            .half_extents
            .get(&self.player_entity)
            .map(|h| h.y)
            .unwrap_or(self.player_half_y);
        // Cast from just above the player's top face straight down. Starting
        // above (not inside) the box means the ray cannot hit the player's own
        // bottom face; we also exclude the player's own body id. The length is
        // enough to reach the ground even when the body is resting on it.
        let origin = body.transform.p + Vec3::new(0.0, half_y + 0.05, 0.0);
        let max_frac = 2.0 * half_y + 0.5;
        let input = noble_physics::world::RayCastInput {
            origin,
            direction: Vec3::new(0.0, -1.0, 0.0),
            max_frac,
        };
        let hit = self
            .world
            .ray_cast_all(&input)
            .into_iter()
            .find(|h| h.body_id != player_id);

        let Some(hit) = hit else {
            return;
        };
        // hit.frac is the world-space distance travelled from `origin` down to
        // the surface (the ray direction is unit length `(0,-1,0)`).
        let surface_y = origin.y - hit.frac;
        let target_y = surface_y + half_y;
        if let Some(body) = self.world.bodies.get_mut(player_id) {
            if body.transform.p.y < target_y {
                // Penetrating the surface: snap the bottom back onto it and
                // cancel only the downward velocity so the player rests instead
                // of sinking/tunneling. Upward (jump) velocity is left intact.
                body.transform.p.y = target_y;
                if body.linear_velocity.y < 0.0 {
                    body.linear_velocity.y = 0.0;
                }
            }
            body.synchronize_transform();
            body.wake();
        }
    }
}

impl NoblePhysics {
    // ------------------------------------------------------------------
    // Step-up — Source-like stair/ledge climbing
    // ------------------------------------------------------------------
    fn try_step_up(&mut self, player_id: BodyId, velocity: &glam::Vec3, _dt: f32) {
        let body = match self.world.bodies.get(player_id) {
            Some(b) => b,
            None => return,
        };
        let half_y = self.half_extents.get(&self.player_entity)
            .map(|h| h.y)
            .unwrap_or(self.player_half_y);

        let horiz = glam::Vec3::new(velocity.x, 0.0, velocity.z);
        let dist = horiz.length() * _dt;
        if dist < 0.001 {
            return;
        }
        let dir = horiz / horiz.length();
        let ndir = v3(dir);

        // Short forward ray at foot level — if it hits, there's a step/obstacle
        let foot_origin = body.transform.p + Vec3::new(0.0, -half_y + 0.05, 0.0);
        let forward_input = noble_physics::world::RayCastInput {
            origin: foot_origin,
            direction: ndir,
            max_frac: dist + 0.1,
        };
        let has_obstacle = self.world.ray_cast_all(&forward_input)
            .iter()
            .any(|h| h.body_id != player_id);
        if !has_obstacle {
            return;
        }

        // Obstacle exists at foot level.  If it also exists at
        // foot + STEP_UP_HEIGHT the obstacle is a tall wall — do not step.
        let step_top_origin = body.transform.p + Vec3::new(0.0, -half_y + STEP_UP_HEIGHT, 0.0);
        let top_input = noble_physics::world::RayCastInput {
            origin: step_top_origin,
            direction: ndir,
            max_frac: dist + 0.1,
        };
        let tall_obstacle = self.world.ray_cast_all(&top_input)
            .iter()
            .any(|h| h.body_id != player_id);
        if tall_obstacle {
            return;
        }

        // Check overhead clearance before lifting
        let up_input = noble_physics::world::RayCastInput {
            origin: body.transform.p,
            direction: Vec3::new(0.0, 1.0, 0.0),
            max_frac: STEP_UP_HEIGHT + 0.1,
        };
        let blocked = self.world.ray_cast_all(&up_input)
            .iter()
            .any(|h| h.body_id != player_id);
        if blocked {
            return;
        }

        if let Some(body) = self.world.bodies.get_mut(player_id) {
            body.transform.p.y += STEP_UP_HEIGHT;
            body.synchronize_transform();
        }
    }
}

impl PhysicsBackend for NoblePhysics {
    fn remove_entity(&mut self, entity: Entity) {
        if let Some(&body_id) = self.body_map.get(&entity) {
            self.world.remove_body(body_id);
            self.body_map.remove(&entity);
            self.entity_by_body.remove(&body_id.raw());
        }
    }

    fn teleport_player(&mut self, player_entity: Entity, position: glam::Vec3) {
        if let Some(&body_id) = self.body_map.get(&player_entity) {
            if let Some(body) = self.world.bodies.get_mut(body_id) {
                body.transform.p = v3(position);
            }
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

        let current_vel = self
            .world
            .bodies
            .get(player_id)
            .map(|b| b.linear_velocity)
            .unwrap_or(Vec3::ZERO);
        let mut vel = v3g(current_vel);

        let wish_dir = self.player_ctl.compute_velocity(
            input, camera_yaw, &mut vel, self.on_ground, dt,
        );

        let has_move_input = wish_dir.length_squared() > 0.0;
        let start_xz = self
            .world
            .bodies
            .get(player_id)
            .map(|b| b.transform.p)
            .unwrap_or(Vec3::ZERO);

        // Step-up: lift the player before moving so they climb stairs/ledges
        if has_move_input && self.on_ground {
            self.try_step_up(player_id, &vel, dt);
        }

        if let Some(body) = self.world.bodies.get_mut(player_id) {
            body.linear_velocity = v3(vel);
            body.wake();
        }

        self.world.step(dt);

        self.clamp_player_to_ground(player_id, dt);

        // When idle, friction handles deceleration. The solver may still inject
        // tiny horizontal drift — zero it out so the player stays put.
        if !has_move_input {
            if let Some(body) = self.world.bodies.get_mut(player_id) {
                body.linear_velocity.x = 0.0;
                body.linear_velocity.z = 0.0;
                body.transform.p.x = start_xz.x;
                body.transform.p.z = start_xz.z;
                body.synchronize_transform();
            }
        }

        self.on_ground = self.player_on_ground(player_id);

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
        let player_body_id = match self.body_map.get(&player_entity) {
            Some(&id) => id,
            None => return (HashMap::new(), Vec::new()),
        };

        let current_vel = self
            .world
            .bodies
            .get(player_body_id)
            .map(|b| b.linear_velocity)
            .unwrap_or(Vec3::ZERO);
        let mut vel = v3g(current_vel);

        let wish_dir = self.player_ctl.compute_velocity(
            input, camera_yaw, &mut vel, self.on_ground, dt,
        );

        let has_move_input = wish_dir.length_squared() > 0.0;
        let start_xz = self
            .world
            .bodies
            .get(player_body_id)
            .map(|b| b.transform.p)
            .unwrap_or(Vec3::ZERO);

        if has_move_input && self.on_ground {
            self.try_step_up(player_body_id, &vel, dt);
        }

        if let Some(body) = self.world.bodies.get_mut(player_body_id) {
            body.linear_velocity = v3(vel);
            body.wake();
        }

        self.world.step(dt);

        self.clamp_player_to_ground(player_body_id, dt);

        if !has_move_input {
            if let Some(body) = self.world.bodies.get_mut(player_body_id) {
                body.linear_velocity.x = 0.0;
                body.linear_velocity.z = 0.0;
                body.transform.p.x = start_xz.x;
                body.transform.p.z = start_xz.z;
                body.synchronize_transform();
            }
        }

        self.on_ground = self.player_on_ground(player_body_id);

        self.collect_collision_events();

        let all_entities: Vec<Entity> = self.body_map.keys().copied().collect();
        let mut transforms = HashMap::new();
        for &entity in &all_entities {
            if let Some(&body_id) = self.body_map.get(&entity) {
                if let Some(body) = self.world.bodies.get(body_id) {
                    let scale = self.scales.get(&entity).copied().unwrap_or(glam::Vec3::ONE);
                    let t = Transform3D {
                        position: v3g(body.transform.p),
                        rotation: quatg(body.transform.q),
                        scale,
                    };
                    transforms.insert(entity, t);
                }
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
            BodyType::Dynamic
        } else {
            BodyType::Static
        };
        Self::create_box_body(
            &mut self.world,
            &mut self.body_map,
            &mut self.entity_by_body,
            &mut self.scales,
            &mut self.half_extents,
            entity,
            body_type,
            transform.position,
            collider.half_extents,
            transform.scale,
            rigid_body,
            is_sensor,
            is_player,
        );
        if is_player {
            // The initial `NoblePhysics::new` in the threaded path may have
            // been constructed with the first non-player entity (e.g. a ground)
            // because entities are streamed in one at a time. Re-point the
            // player at the actual player entity so velocity control, the
            // ground clamp, and the on-ground check target the right body.
            self.player_entity = entity;
            self.player_half_y = collider.half_extents.y;
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
        let input = noble_physics::world::RayCastInput {
            origin: v3(origin),
            direction: v3(direction),
            max_frac: max_distance,
        };
        let hit = self.world.ray_cast_closest(&input)?;

        let entity = self.entity_by_body.get(&hit.body_id.raw()).copied();
        Some(RayHit {
            entity,
            point: v3g(hit.point),
            normal: v3g(hit.normal),
            fraction: hit.frac,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecs::{Collider, RigidBody, Transform3D, World};

    fn build_world() -> (World, Entity, Vec<Entity>) {
        let mut world = World::new();

        // Ground plane at y = 0, half extents (5, 0.5, 5) -> top at y = 0.5.
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

        // Player standing on the ground (spawned slightly above so the
        // broad-phase generates the player/ground pair as it settles, matching
        // the real game where the player spawns at y = 2.0).
        let player = world.spawn();
        world.add_player(player);
        world.add_transform(
            player,
            Transform3D {
                position: glam::Vec3::new(0.0, 1.5, 0.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.add_collider(player, Collider { half_extents: glam::Vec3::new(0.5, 0.5, 0.5) });
        world.add_rigid_body(
            player,
            RigidBody {
                mass: 1.0,
                restitution: 0.0,
                angular_damping: 0.98,
            },
        );

        // A cube dropped above the ground.
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
        world.add_rigid_body(
            cube,
            RigidBody {
                mass: 1.0,
                restitution: 0.0,
                angular_damping: 0.98,
            },
        );

        (world, player, vec![cube])
    }

    #[test]
    fn cube_falls_and_rests_on_ground() {
        let (mut world, player, dynamics) = build_world();
        let mut phys: Box<dyn PhysicsBackend> =
            Box::new(NoblePhysics::new(&world, player, &dynamics));

        let input = InputState {
            forward: false,
            backward: false,
            left: false,
            right: false,
            jump: false,
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            pointer_locked: false,
            keys: std::collections::HashMap::new(),
        };
        for _ in 0..240 {
            let (transforms, _collisions) = phys.tick_raw(&input, player, 0.0, 1.0 / 60.0);
            for (e, t) in transforms {
                if let Some(mut tr) = world.get_transform_mut(e) {
                    *tr = t;
                }
            }
        }

        let cube_t = world.get_transform(dynamics[0]).unwrap();
        // Cube started at y = 3.0 and must have fallen to rest near the ground top (0.5 + 0.5 = 1.0).
        assert!(
            cube_t.position.y < 2.0,
            "cube did not fall, y = {}",
            cube_t.position.y
        );
        assert!(
            cube_t.position.y > 0.8 && cube_t.position.y < 1.15,
            "cube did not rest on ground, y = {}",
            cube_t.position.y
        );
        // A cube resting on flat ground must stay axis-aligned; it must not
        // spin (roll) in place.
        let q = cube_t.rotation;
        let angle = 2.0 * (q.w.max(-1.0).min(1.0)).acos();
        assert!(
            angle < 0.2,
            "cube rolled while resting, rotation = {:?}",
            q
        );
    }

    // Regression test for the player sinking/falling-through and teleporting
    // while moving. The player is velocity-controlled and (without an explicit
    // ground clamp) the dynamic solver lets it tunnel through the ground when
    // it slides along the surface. With the clamp the player must stay resting
    // at y ~= 1.0 even while walking across a large floor.
    #[test]
    fn player_stays_grounded_while_moving() {
        let mut world = World::new();
        let ground = world.spawn();
        world.add_transform(
            ground,
            Transform3D {
                position: glam::Vec3::new(0.0, 0.0, 0.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.add_ground(ground);
        world.add_collider(ground, Collider { half_extents: glam::Vec3::new(50.0, 0.5, 50.0) });

        let player = world.spawn();
        world.add_player(player);
        world.add_transform(
            player,
            Transform3D {
                position: glam::Vec3::new(0.0, 1.5, 0.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.add_collider(player, Collider { half_extents: glam::Vec3::new(0.5, 0.5, 0.5) });
        world.add_rigid_body(
            player,
            RigidBody {
                mass: 1.0,
                restitution: 0.0,
                angular_damping: 0.98,
            },
        );

        let mut phys: Box<dyn PhysicsBackend> =
            Box::new(NoblePhysics::new(&world, player, &[]));

        let input = InputState {
            forward: true,
            backward: false,
            left: false,
            right: false,
            jump: false,
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            pointer_locked: false,
            keys: std::collections::HashMap::new(),
        };
        // yaw = PI/2 -> forward is +X. Walk across the floor for a while.
        let mut min_y = f32::MAX;
        let mut max_y = f32::MIN;
        for _ in 0..240 {
            let (transforms, _c) = phys.tick_raw(&input, player, std::f32::consts::FRAC_PI_2, 1.0 / 60.0);
            for (e, t) in transforms {
                if let Some(mut tr) = world.get_transform_mut(e) {
                    *tr = t;
                }
            }
            let y = world.get_transform(player).unwrap().position.y;
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        // Player must stay near rest height (1.0) with only small jitter.
        assert!(
            min_y > 0.5 && max_y < 1.8,
            "player left the ground while moving: min_y={} max_y={}",
            min_y,
            max_y
        );
    }

    // Regression test: pushing a cube must slide it (translate) without making
    // it roll/tumble. Locks rotation on dynamic boxes.
    #[test]
    fn pushing_cube_does_not_roll_it() {
        let mut world = World::new();
        let ground = world.spawn();
        world.add_transform(
            ground,
            Transform3D {
                position: glam::Vec3::new(0.0, 0.0, 0.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.add_ground(ground);
        world.add_collider(ground, Collider { half_extents: glam::Vec3::new(50.0, 0.5, 50.0) });

        let player = world.spawn();
        world.add_player(player);
        world.add_transform(
            player,
            Transform3D {
                position: glam::Vec3::new(0.0, 1.0, -2.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.add_collider(player, Collider { half_extents: glam::Vec3::new(0.5, 0.5, 0.5) });
        world.add_rigid_body(
            player,
            RigidBody { mass: 1.0, restitution: 0.0, angular_damping: 0.98 },
        );

        let cube = world.spawn();
        world.add_transform(
            cube,
            Transform3D {
                position: glam::Vec3::new(0.0, 1.0, 0.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.add_collider(cube, Collider { half_extents: glam::Vec3::new(0.5, 0.5, 0.5) });
        world.add_rigid_body(
            cube,
            RigidBody { mass: 1.0, restitution: 0.0, angular_damping: 0.98 },
        );

        let mut phys: Box<dyn PhysicsBackend> =
            Box::new(NoblePhysics::new(&world, player, &[cube]));

        let input = InputState {
            forward: true,
            backward: false,
            left: false,
            right: false,
            jump: false,
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            pointer_locked: false,
            keys: std::collections::HashMap::new(),
        };
        // yaw = 0 -> forward is +Z (towards the cube, which sits at z = 0).
        let idle = InputState {
            forward: false,
            backward: false,
            left: false,
            right: false,
            jump: false,
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            pointer_locked: false,
            keys: std::collections::HashMap::new(),
        };
        // Let the cube fall and go to sleep first, then confirm a sleeping
        // crate still wakes and moves when the player shoves it (regression
        // for the "sleeping cubes act static" bug).
        for _ in 0..120 {
            let (transforms, _c) = phys.tick_raw(&idle, player, 0.0, 1.0 / 60.0);
            for (e, t) in transforms {
                if let Some(mut tr) = world.get_transform_mut(e) {
                    *tr = t;
                }
            }
        }
        let slept_z = world.get_transform(cube).unwrap().position.z;
        for ti in 0..90 {
            let (transforms, _c) = phys.tick_raw(&input, player, 0.0, 1.0 / 60.0);
            for (e, t) in transforms {
                if let Some(mut tr) = world.get_transform_mut(e) {
                    *tr = t;
                }
            }
        }
        for _ in 0..150 {
            let (transforms, _c) = phys.tick_raw(&idle, player, 0.0, 1.0 / 60.0);
            for (e, t) in transforms {
                if let Some(mut tr) = world.get_transform_mut(e) {
                    *tr = t;
                }
            }
        }
        let cube_t = world.get_transform(cube).unwrap();
        // The cube should have been shoved in +Z (away from the player start),
        // even though it had gone to sleep beforehand.
        assert!(
            cube_t.position.z > slept_z + 0.3,
            "sleeping cube was not pushed (wake-on-contact broken), z = {} (slept {})",
            cube_t.position.z,
            slept_z
        );
        // A straight shove should mostly translate the crate; any spin stays
        // small (it must not tumble/balance on an edge).
        let q = cube_t.rotation;
        let w = q.w.abs();
        let angle = 2.0 * w.clamp(-1.0, 1.0).acos();
        assert!(
            angle < 0.35,
            "cube tumbled when shoved straight, rotation = {:?} (angle {:.3} rad)",
            q,
            angle
        );
    }

    #[test]
    fn player_is_grounded_after_resting() {
        let (mut world, player, dynamics) = build_world();
        let mut phys: Box<dyn PhysicsBackend> =
            Box::new(NoblePhysics::new(&world, player, &dynamics));

        let input = InputState {
            forward: false,
            backward: false,
            left: false,
            right: false,
            jump: false,
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            pointer_locked: false,
            keys: std::collections::HashMap::new(),
        };
        for _ in 0..240 {
            let (transforms, _collisions) = phys.tick_raw(&input, player, 0.0, 1.0 / 60.0);
            for (e, t) in transforms {
                if let Some(mut tr) = world.get_transform_mut(e) {
                    *tr = t;
                }
            }
        }

        // Player should have settled on the ground (top at 0.5 + half 0.5 = 1.0).
        let player_t = world.get_transform(player).unwrap();
        assert!(
            player_t.position.y > 0.8 && player_t.position.y < 1.2,
            "player did not rest on ground, y = {}",
            player_t.position.y
        );
    }

    #[test]
    fn jump_is_grounded_only() {
        let (mut world, player, dynamics) = build_world();
        let mut phys: Box<dyn PhysicsBackend> =
            Box::new(NoblePhysics::new(&world, player, &dynamics));

        let mut input = InputState {
            forward: false,
            backward: false,
            left: false,
            right: false,
            jump: false,
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            pointer_locked: false,
            keys: std::collections::HashMap::new(),
        };

        // Let the player settle on the ground.
        for _ in 0..60 {
            let (transforms, _c) = phys.tick_raw(&input, player, 0.0, 1.0 / 60.0);
            for (e, t) in transforms {
                if let Some(mut tr) = world.get_transform_mut(e) {
                    *tr = t;
                }
            }
        }
        assert!(
            world.get_transform(player).unwrap().position.y < 1.2,
            "player did not settle before jump test"
        );

        // Hold jump continuously. With correct (grounded-only) jumping the
        // player launches once, lands, and cannot re-jump until it is actually
        // on the ground again — so it must not keep climbing indefinitely.
        input.jump = true;
        let mut max_y = 0.0f32;
        for _ in 0..180 {
            let (transforms, _c) = phys.tick_raw(&input, player, 0.0, 1.0 / 60.0);
            for (e, t) in transforms {
                if let Some(mut tr) = world.get_transform_mut(e) {
                    *tr = t;
                }
            }
            max_y = max_y.max(world.get_transform(player).unwrap().position.y);
        }
        // A single jump rises ~JUMP_VEL^2/(2*g) = 64/40 = 1.6 above the ground,
        // so the apex should be well under ~3.0. Continuous (infinite) jumping
        // would send the player arbitrarily high.
        assert!(
            max_y < 3.0,
            "infinite jump: player kept climbing, max_y = {}",
            max_y
        );
    }

    // Regression test for the threaded bootstrap path. In the real game
    // `ThreadedPhysics` streams entities in one at a time; grounds are added
    // before the player, so `NoblePhysics` is first constructed with a ground
    // as its `player_entity`. If `add_body` does not re-point `player_entity`
    // at the real player, velocity control and the ground clamp target the
    // ground (a static body) and the player falls through and cannot jump.
    #[test]
    fn threaded_bootstrap_targets_player() {
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
        world.add_collider(ground, Collider { half_extents: glam::Vec3::new(50.0, 0.5, 50.0) });

        let player = world.spawn();
        world.add_player(player);
        world.add_transform(
            player,
            Transform3D {
                position: glam::Vec3::new(0.0, 1.5, 0.0),
                rotation: glam::Quat::IDENTITY,
                scale: glam::Vec3::ONE,
            },
        );
        world.add_collider(player, Collider { half_extents: glam::Vec3::new(0.5, 0.5, 0.5) });
        world.add_rigid_body(
            player,
            RigidBody {
                mass: 1.0,
                restitution: 0.0,
                angular_damping: 0.98,
            },
        );

        // Mimic physics_main: the first entity (a ground) constructs the
        // NoblePhysics instance with itself as the initial player_entity.
        let mut phys = NoblePhysics::new(&World::new(), ground, &[]);
        phys.add_body(
            ground,
            world.get_transform(ground).unwrap(),
            world.get_collider(ground).unwrap(),
            RigidBody { mass: 0.0, restitution: 0.2, angular_damping: 1.0 },
            false,
            false,
        );
        phys.add_body(
            player,
            world.get_transform(player).unwrap(),
            world.get_collider(player).unwrap(),
            world.get_rigid_body(player).unwrap(),
            false,
            true,
        );

        let idle = InputState {
            forward: false,
            backward: false,
            left: false,
            right: false,
            jump: false,
            mouse_dx: 0.0,
            mouse_dy: 0.0,
            pointer_locked: false,
            keys: std::collections::HashMap::new(),
        };
        let mut jump_in = idle.clone();
        jump_in.jump = true;

        // Settle so the player falls onto the ground and on_ground becomes
        // true (proving velocity control + clamp target the player, not the
        // ground). The player spawns at y = 1.5 and must fall to rest at ~1.0.
        for _ in 0..90 {
            let (transforms, _c) = phys.tick_raw(&idle, player, 0.0, 1.0 / 60.0);
            for (e, t) in transforms {
                if let Some(mut tr) = world.get_transform_mut(e) {
                    *tr = t;
                }
            }
        }
        let settled_y = world.get_transform(player).unwrap().position.y;
        // The player must come to rest on the ground (top at 0.5 + half 0.5 =
        // 1.0), proving velocity control + the clamp target the player (not the
        // static ground) and that it does not fall through.
        assert!(
            settled_y > 0.8 && settled_y < 1.2,
            "player did not rest on ground (bootstrap bug), y = {}",
            settled_y
        );

        // Now jump: the PLAYER must launch, not the static ground.
        let (transforms, _c) = phys.tick_raw(&jump_in, player, 0.0, 1.0 / 60.0);
        for (e, t) in transforms {
            if let Some(mut tr) = world.get_transform_mut(e) {
                *tr = t;
            }
        }
        let py = world.get_transform(player).unwrap().position.y;
        assert!(
            py > settled_y,
            "player was not targeted by jump (bootstrap bug), y = {} (settled {})",
            py,
            settled_y
        );
    }
}
