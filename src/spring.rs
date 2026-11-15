//! Damped springs with closed-form stepping.
//!
//! The position after `dt` is computed from the exact solution of the spring
//! equation, not by adding up small steps, so an animation looks the same at
//! 60, 120 or 144 Hz and can be retargeted mid-flight without a jump in speed.

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub pos: f32,
    pub vel: f32,
    pub target: f32,
    /// Natural frequency in rad/s. Higher is faster.
    pub omega: f32,
    /// Damping ratio. 1.0 = no overshoot, below 1.0 = bouncy.
    pub zeta: f32,
}

impl Spring {
    pub const fn new(pos: f32, omega: f32, zeta: f32) -> Self {
        Self { pos, vel: 0.0, target: pos, omega, zeta }
    }

    pub fn set(&mut self, target: f32) {
        self.target = target;
    }

    pub fn step(&mut self, dt: f32) {
        if dt <= 0.0 {
            return;
        }
        let w = self.omega;
        let z = self.zeta;
        let x0 = self.pos - self.target;
        let v0 = self.vel;
        let (x, v);
        if z >= 1.0 {
            // Critically damped: x(t) = (x0 + c t) e^(-w t)
            let e = (-w * dt).exp();
            let c = v0 + w * x0;
            x = (x0 + c * dt) * e;
            v = (c - w * (x0 + c * dt)) * e;
        } else {
            // Under-damped: x(t) = e^(-z w t) (a cos(wd t) + b sin(wd t))
            let wd = w * (1.0 - z * z).sqrt();
            let e = (-z * w * dt).exp();
            let a = x0;
            let b = (v0 + z * w * x0) / wd;
            let (s, c) = (wd * dt).sin_cos();
            x = e * (a * c + b * s);
            v = e * ((b * wd - z * w * a) * c - (a * wd + z * w * b) * s);
        }
        self.pos = self.target + x;
        self.vel = v;
        if self.settled_within(1e-4) {
            self.pos = self.target;
            self.vel = 0.0;
        }
    }

    fn settled_within(&self, eps: f32) -> bool {
        (self.pos - self.target).abs() < eps && self.vel.abs() < eps * 10.0
    }

    /// True when the spring has stopped moving (to within `eps`).
    pub fn settled(&self, eps: f32) -> bool {
        self.settled_within(eps)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rate_independent() {
        for zeta in [1.0f32, 0.55] {
            let mut a = Spring::new(0.0, 24.0, zeta);
            let mut b = a;
            a.set(1.0);
            b.set(1.0);
            for _ in 0..30 {
                a.step(1.0 / 60.0);
            }
            for _ in 0..72 {
                b.step(1.0 / 144.0);
            }
            assert!((a.pos - b.pos).abs() < 1e-3, "{} vs {}", a.pos, b.pos);
            assert!((a.vel - b.vel).abs() < 2e-2, "{} vs {}", a.vel, b.vel);
        }
    }

    #[test]
    fn converges() {
        let mut s = Spring::new(0.0, 30.0, 1.0);
        s.set(1.0);
        for _ in 0..120 {
            s.step(1.0 / 60.0);
        }
        assert!(s.settled(1e-3));
        assert_eq!(s.pos, 1.0);
    }
}
