//! Motion: the few springs every moving thing shares.
//!
//! Nothing in the interface moves on a timer or along an easing curve. Every
//! moving value is a damped spring (see `spring.rs`), and the springs come in
//! four sizes, so that things of the same kind move alike:
//!
//!   tiny    a hover, a press: over in about a tenth of a second
//!   small   a tooltip, a menu, the drop of light under the cursor
//!   medium  a surface changing shape: the island, the dock's panel
//!   large   a whole sheet arriving: the app tray
//!
//! Two settings bend all of them at once: `speed` scales how fast they are and
//! `elastic` how far they overshoot before settling. When Windows' "animation
//! effects" are switched off, nothing overshoots and everything is quick.

use crate::spring::Spring;

/// How a spring is tuned.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    /// Natural frequency in rad/s.
    pub omega: f32,
    /// Damping ratio: 1 = no overshoot.
    pub zeta: f32,
}

impl Spring {
    pub fn with(pos: f32, tone: Tone) -> Self {
        Spring::new(pos, tone.omega, tone.zeta)
    }

    pub fn tune(&mut self, tone: Tone) {
        self.omega = tone.omega;
        self.zeta = tone.zeta;
    }

    /// Jumps to a value and stays there.
    pub fn snap(&mut self, value: f32) {
        self.pos = value;
        self.target = value;
        self.vel = 0.0;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    speed: f32,
    elastic: f32,
    reduce: bool,
}

impl Motion {
    pub fn new(speed: f32, elastic: f32, reduce: bool) -> Self {
        Motion { speed: speed.clamp(0.25, 4.0), elastic: elastic.clamp(0.0, 1.0), reduce }
    }

    /// `bounce`: how much of the damping the `elastic` setting may take away.
    ///
    /// The speeds follow what the published motion systems agree on (see
    /// docs/nghien-cuu-fluid.md): a spring feels as long as one period, 2π/ω, so everything
    /// here is perceived as lasting between a tenth and a third of a second. At the default
    /// elasticity geometry overshoots by a few percent; below a damping of 0.5 motion reads
    /// as a toy, so that is where it stops.
    fn tone(&self, omega: f32, bounce: f32) -> Tone {
        if self.reduce {
            return Tone { omega: omega * self.speed * 1.6, zeta: 1.0 };
        }
        Tone { omega: omega * self.speed, zeta: (1.0 - bounce * self.elastic).clamp(0.5, 1.0) }
    }

    pub fn tiny(&self) -> Tone {
        self.tone(56.0, 0.20)
    }

    pub fn small(&self) -> Tone {
        self.tone(38.0, 0.30)
    }

    pub fn medium(&self) -> Tone {
        self.tone(24.0, 0.56)
    }

    pub fn large(&self) -> Tone {
        self.tone(18.0, 0.26)
    }

    /// For opacity and colour, which must never overshoot.
    pub fn fade(&self) -> Tone {
        self.tone(40.0, 0.0)
    }

    /// For a surface closing or shrinking: as quick as `medium`, without the bounce.
    pub fn calm(&self) -> Tone {
        self.tone(26.0, 0.0)
    }

    /// For things that follow the cursor: quick, with a little give.
    pub fn follow(&self) -> Tone {
        self.tone(44.0, 0.44)
    }

    /// For something soft that was given a push: it wobbles once and comes to rest.
    pub fn jelly(&self) -> Tone {
        self.tone(30.0, 0.9)
    }

    /// How much moving things stretch along their path and flatten on landing, 0 to 1.
    pub fn squash(&self) -> f32 {
        if self.reduce {
            0.0
        } else {
            self.elastic
        }
    }

    pub fn reduced(&self) -> bool {
        self.reduce
    }
}

/// A rectangle as shown this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Shape {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub alpha: f32,
}

/// A rectangle that glides after a target instead of jumping to it: the drop
/// of light under the cursor in the dock, the mark behind a menu row. While it
/// travels it stretches along its path and thins across it, the way a drop of
/// liquid does, and it rounds off again as it arrives.
#[derive(Clone, Copy, Debug)]
pub struct Glide {
    x: Spring,
    y: Spring,
    w: Spring,
    h: Spring,
    alpha: Spring,
    /// Whether it has a place yet. Until then, the first target is where it appears.
    placed: bool,
}

impl Glide {
    pub fn new(m: &Motion) -> Self {
        let t = m.follow();
        Glide { x: Spring::with(0.0, t), y: Spring::with(0.0, t), w: Spring::with(0.0, t), h: Spring::with(0.0, t), alpha: Spring::with(0.0, m.fade()), placed: false }
    }

    pub fn tune(&mut self, m: &Motion) {
        let t = m.follow();
        for s in [&mut self.x, &mut self.y, &mut self.w, &mut self.h] {
            s.tune(t);
        }
        self.alpha.tune(m.fade());
    }

    /// Sends it to a rectangle (left, top, width, height), or away with `None`.
    pub fn aim(&mut self, target: Option<(f32, f32, f32, f32)>) {
        match target {
            Some((x, y, w, h)) => {
                if !self.placed || self.alpha.pos < 0.02 {
                    // Not on screen: appear in place instead of flying in from where it last was.
                    self.x.snap(x);
                    self.y.snap(y);
                    self.w.snap(w);
                    self.h.snap(h);
                    self.placed = true;
                }
                self.x.set(x);
                self.y.set(y);
                self.w.set(w);
                self.h.set(h);
                self.alpha.set(1.0);
            }
            None => self.alpha.set(0.0),
        }
    }

    pub fn step(&mut self, dt: f32) {
        for s in [&mut self.x, &mut self.y, &mut self.w, &mut self.h, &mut self.alpha] {
            s.step(dt);
        }
    }

    pub fn settled(&self) -> bool {
        self.alpha.settled(1e-3) && (self.alpha.pos < 0.01 || [self.x, self.y, self.w, self.h].iter().all(|s| s.settled(0.05)))
    }

    pub fn visible(&self) -> bool {
        self.alpha.pos > 0.004
    }

    /// The rectangle to draw. `squash` (0..1) is how liquid it is.
    pub fn shape(&self, squash: f32) -> Shape {
        let alpha = self.alpha.pos.clamp(0.0, 1.0);
        let (w, h) = (self.w.pos.max(0.0), self.h.pos.max(0.0));
        // The centre's speed, in its own sizes per second, so a small drop and a big one stretch alike.
        let vx = (self.x.vel + self.w.vel * 0.5) / w.max(1.0);
        let vy = (self.y.vel + self.h.vel * 0.5) / h.max(1.0);
        let (ax, ay) = stretch(vx, vy, squash);
        // It also swells into place as it appears.
        let grow = 0.72 + 0.28 * alpha;
        let (sw, sh) = (w * ax * grow, h * ay * grow);
        Shape { x: self.x.pos + (w - sw) * 0.5, y: self.y.pos + (h - sh) * 0.5, w: sw, h: sh, alpha }
    }
}

/// How much something moving at (`vx`, `vy`) body-lengths per second is stretched along x and
/// y. Stretching along the motion is paid for by thinning across it, so the area stays about
/// the same and the thing reads as soft rather than as growing.
pub fn stretch(vx: f32, vy: f32, squash: f32) -> (f32, f32) {
    const GAIN: f32 = 0.028;
    const MOST: f32 = 0.34;
    let sx = (vx.abs() * GAIN).min(MOST) * squash;
    let sy = (vy.abs() * GAIN).min(MOST) * squash;
    ((1.0 + sx) * (1.0 - 0.45 * sy), (1.0 + sy) * (1.0 - 0.45 * sx))
}

/// Squash and stretch of something hopping: `f` is where it is in the hop (0 = just left the
/// ground, 0.5 = the top, 1 = landing). Returns (width, height) factors: flattened against
/// the ground at both ends, a little long and thin while it moves fastest.
pub fn hop_shape(f: f32, squash: f32) -> (f32, f32) {
    let f = f.clamp(0.0, 1.0);
    const GROUND: f32 = 0.14;
    let near = (1.0 - f.min(1.0 - f) / GROUND).max(0.0);
    let flat = near * near * 0.16 * squash;
    let speed = (1.0 - 2.0 * f).abs();
    let long = speed * (1.0 - near) * 0.07 * squash;
    (1.0 + flat - long * 0.6, 1.0 - flat + long)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(g: &mut Glide, seconds: f32) {
        for _ in 0..(seconds * 240.0) as usize {
            g.step(1.0 / 240.0);
        }
    }

    #[test]
    fn tones_follow_the_settings() {
        let calm = Motion::new(1.0, 0.0, false);
        let lively = Motion::new(1.0, 1.0, false);
        assert_eq!(calm.medium().zeta, 1.0, "no elasticity means no overshoot");
        assert!(lively.medium().zeta < 0.6);
        assert!(Motion::new(2.0, 0.5, false).small().omega > Motion::new(1.0, 0.5, false).small().omega * 1.9);
        let reduced = Motion::new(1.0, 1.0, true);
        assert_eq!(reduced.medium().zeta, 1.0);
        assert_eq!(reduced.squash(), 0.0);
        assert_eq!(lively.fade().zeta, 1.0, "opacity never overshoots");
    }

    #[test]
    fn glide_appears_in_place_then_travels() {
        let m = Motion::new(1.0, 0.6, false);
        let mut g = Glide::new(&m);
        assert!(!g.visible());
        g.aim(Some((100.0, 10.0, 40.0, 30.0)));
        let first = g.shape(m.squash());
        assert!((first.x + first.w * 0.5 - 120.0).abs() < 1e-3, "appears centred on its first target");
        run(&mut g, 0.6);
        assert!(g.settled());
        let s = g.shape(m.squash());
        assert!((s.x - 100.0).abs() < 0.1 && (s.w - 40.0).abs() < 0.1 && (s.alpha - 1.0).abs() < 1e-3);

        // Sent somewhere else it passes through the points in between.
        g.aim(Some((300.0, 10.0, 40.0, 30.0)));
        g.step(1.0 / 60.0);
        let moving = g.shape(m.squash());
        assert!(moving.x > 100.0 && moving.x < 300.0);
        assert!(moving.w > 40.0, "stretched along its path: {}", moving.w);
        assert!(moving.h < 30.0, "and thinner across it: {}", moving.h);
        run(&mut g, 0.8);
        let s = g.shape(m.squash());
        assert!((s.x - 300.0).abs() < 0.1 && (s.w - 40.0).abs() < 0.1 && (s.h - 30.0).abs() < 0.1);

        g.aim(None);
        run(&mut g, 0.5);
        assert!(!g.visible() && g.settled());
    }

    #[test]
    fn stretch_keeps_the_area_roughly() {
        for v in [0.0f32, 3.0, 8.0, 40.0] {
            let (sx, sy) = stretch(v, 0.0, 1.0);
            assert!(sx >= 1.0 && sy <= 1.0);
            assert!((sx * sy - 1.0).abs() < 0.2, "area changed by {}", sx * sy);
        }
        assert_eq!(stretch(25.0, 25.0, 0.0), (1.0, 1.0));
    }

    #[test]
    fn hop_is_flat_on_the_ground_and_round_at_the_top() {
        let (w0, h0) = hop_shape(0.0, 1.0);
        assert!(w0 > 1.0 && h0 < 1.0);
        let (wt, ht) = hop_shape(0.5, 1.0);
        assert!((wt - 1.0).abs() < 1e-4 && (ht - 1.0).abs() < 1e-4);
        let (wm, hm) = hop_shape(0.25, 1.0);
        assert!(wm < 1.0 && hm > 1.0);
        assert_eq!(hop_shape(0.3, 0.0), (1.0, 1.0));
    }
}
