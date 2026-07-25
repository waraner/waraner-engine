use crate::ecs::InputState;
use glam::Vec3;

pub struct PlayerController {
    pub ground_accel: f32,
    pub ground_max_speed: f32,
    pub air_accel: f32,
    pub air_max_speed: f32,
    pub friction: f32,
    pub stop_speed: f32,
    pub jump_velocity: f32,
    prev_jump: bool,
}

impl PlayerController {
    pub fn new() -> Self {
        Self {
            ground_accel: 10.0,
            ground_max_speed: 6.0,
            air_accel: 1.0,
            air_max_speed: 6.0,
            friction: 4.0,
            stop_speed: 1.0,
            jump_velocity: 8.0,
            prev_jump: false,
        }
    }

    pub fn compute_velocity(
        &mut self,
        input: &InputState,
        camera_yaw: f32,
        velocity: &mut Vec3,
        on_ground: bool,
        dt: f32,
    ) -> Vec3 {
        let wish_dir = Self::build_wish_dir(input, camera_yaw);

        if on_ground {
            self.apply_friction(velocity, dt);
            self.ground_move(velocity, wish_dir, dt);
        } else {
            self.air_move(velocity, wish_dir, dt);
        }

        let jump_edge = input.jump && !self.prev_jump;
        self.prev_jump = input.jump;
        if jump_edge && on_ground {
            velocity.y = self.jump_velocity;
        }

        wish_dir
    }

    fn apply_friction(&self, velocity: &mut Vec3, dt: f32) {
        let speed = Vec3::new(velocity.x, 0.0, velocity.z).length();
        if speed < 0.01 {
            return;
        }

        let drop = if speed < self.stop_speed {
            self.stop_speed * self.friction * dt
        } else {
            speed * self.friction * dt
        };

        let new_speed = (speed - drop).max(0.0) / speed;
        velocity.x *= new_speed;
        velocity.z *= new_speed;
    }

    fn ground_move(&self, velocity: &mut Vec3, wish_dir: Vec3, dt: f32) {
        let len = wish_dir.length();
        if len < 0.001 {
            return;
        }
        let dir = wish_dir / len;
        self.accelerate(velocity, dir, self.ground_max_speed, self.ground_accel, dt);
    }

    fn air_move(&self, velocity: &mut Vec3, wish_dir: Vec3, dt: f32) {
        let len = wish_dir.length();
        if len < 0.001 {
            return;
        }
        let dir = wish_dir / len;
        self.accelerate(velocity, dir, self.air_max_speed, self.air_accel, dt);
    }

    fn accelerate(&self, velocity: &mut Vec3, wish_dir: Vec3, wish_speed: f32, accel: f32, dt: f32) {
        let current_speed = velocity.dot(wish_dir);
        let add_speed = wish_speed - current_speed;
        if add_speed <= 0.0 {
            return;
        }

        let accel_amount = (accel * dt * wish_speed).min(add_speed);
        *velocity += wish_dir * accel_amount;
    }

    pub fn build_wish_dir(input: &InputState, camera_yaw: f32) -> Vec3 {
        let forward = Vec3::new(camera_yaw.sin(), 0.0, camera_yaw.cos());
        let right = Vec3::new(-camera_yaw.cos(), 0.0, camera_yaw.sin());

        let mut dir = Vec3::ZERO;
        if input.forward {
            dir += forward;
        }
        if input.backward {
            dir -= forward;
        }
        if input.left {
            dir -= right;
        }
        if input.right {
            dir += right;
        }
        dir
    }
}

impl Default for PlayerController {
    fn default() -> Self {
        Self::new()
    }
}
