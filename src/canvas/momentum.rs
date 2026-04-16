use crate::canvas::geometry::Vec2;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Momentum {
    velocity: Vec2,
    friction: f64,
    stop_threshold: f64,
}

impl Momentum {
    pub fn new(friction: f64, stop_threshold: f64) -> Self {
        assert!(friction >= 0.0, "friction must be non-negative");
        assert!(stop_threshold >= 0.0, "stop_threshold must be non-negative");
        Self {
            velocity: Vec2::new(0.0, 0.0),
            friction,
            stop_threshold,
        }
    }

    pub fn velocity(&self) -> Vec2 {
        self.velocity
    }

    pub fn set_velocity(&mut self, velocity: Vec2) {
        self.velocity = velocity;
    }

    pub fn is_stopped(&self) -> bool {
        self.velocity.length() <= self.stop_threshold
    }

    pub fn step(&mut self, dt_seconds: f64) -> Vec2 {
        assert!(dt_seconds >= 0.0, "dt_seconds must be non-negative");

        if self.is_stopped() {
            self.velocity = Vec2::new(0.0, 0.0);
            return self.velocity;
        }

        let displacement = if self.friction <= f64::EPSILON {
            self.velocity * dt_seconds
        } else {
            let damping = (-self.friction * dt_seconds).exp();
            let distance_scale = (1.0 - damping) / self.friction;
            self.velocity * distance_scale
        };

        let damping = (-self.friction * dt_seconds).exp();
        self.velocity = self.velocity * damping;

        if self.is_stopped() {
            self.velocity = Vec2::new(0.0, 0.0);
        }

        displacement
    }
}
