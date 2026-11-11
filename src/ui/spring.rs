#[derive(Debug, Clone)]
pub struct SpringValue {
    pub current: f32,
    pub target: f32,
    pub velocity: f32,
    pub stiffness: f32,
    pub damping: f32,
}

impl SpringValue {
    pub fn new(initial: f32) -> Self {
        Self {
            current: initial,
            target: initial,
            velocity: 0.0,
            stiffness: 320.0,
            damping: 24.0,
        }
    }

    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    pub fn update(&mut self, dt: f32) -> bool {
        // 限制 dt 防止突跳
        let dt = dt.min(0.05);
        let displacement = self.current - self.target;
        let spring_force = -self.stiffness * displacement;
        let damping_force = -self.damping * self.velocity;
        let acceleration = spring_force + damping_force;

        self.velocity += acceleration * dt;
        self.current += self.velocity * dt;

        let is_settled = (self.current - self.target).abs() < 0.2 && self.velocity.abs() < 0.2;
        if is_settled {
            self.current = self.target;
            self.velocity = 0.0;
        }

        !is_settled
    }

    #[allow(dead_code)]
    pub fn is_animating(&self) -> bool {
        (self.current - self.target).abs() >= 0.2 || self.velocity.abs() >= 0.2
    }
}
