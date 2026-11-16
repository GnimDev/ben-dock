//! The top edge of the dock's glass as the surface of a drop of water.
//!
//! The edge is a row of points. Each is held to its two neighbours, the way a
//! stretched skin is, and pulled back to level. Press on it and it dents
//! under the weight while the rest of it swells, because the water has to go
//! somewhere; let go and the dent springs back up past level, and what is
//! left of the push runs along the surface as waves, to both ends and back,
//! until it has died away.
//!
//! Nothing here draws. `dock.rs` asks where the surface is and shapes the
//! glass after it (see `scene::Skin`).

/// The surface is moved on in steps of this length, however long a frame takes, so it
/// behaves the same at every refresh rate.
const TICK: f32 = 1.0 / 240.0;
/// How strongly a point is held to its neighbours, how strongly it is pulled back to level,
/// and how much it gives to the rest when it is pushed down (the water under it has to go
/// somewhere). Per second squared; with points the same distance apart whatever the display
/// scale, waves cross an icon in about a tenth of a second.
const TENSION: f32 = 10_000.0;
const LEVEL: f32 = 330.0;
const VOLUME: f32 = 990.0;
/// How quickly neighbouring points come to move alike, which keeps the surface smooth.
const SMOOTH: f32 = 14.0;
/// The ends of the surface run into the round ends of the glass over this many points.
const TAPER: f32 = 3.0;

/// Something that presses on the surface from above: where its lowest point is, how far
/// below the resting surface that is, and the radius of its round underside. In pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Weight {
    pub x: f32,
    pub depth: f32,
    pub radius: f32,
}

#[derive(Default)]
pub struct Surface {
    /// Distance between two points, in pixels. The first point is at x = 0.
    step: f32,
    /// The ends of the surface.
    left: f32,
    right: f32,
    /// The points that can move: `first..=last`. The rest are level.
    first: usize,
    last: usize,
    /// Height of each point above rest (pixels, up is positive), and its speed.
    h: Vec<f32>,
    v: Vec<f32>,
    pull: Vec<f32>,
    /// Time not played yet (less than one tick).
    carry: f32,
}

impl Surface {
    /// Lays the surface between `left` and `right`, on a row of points `step` pixels apart
    /// that reaches from 0 to `width`. Points that end up outside go level at once.
    pub fn span(&mut self, left: f32, right: f32, step: f32, width: f32) {
        let step = step.max(0.5);
        let count = (width.max(0.0) / step).ceil() as usize + 2;
        if step != self.step || count != self.h.len() {
            self.step = step;
            self.h = vec![0.0; count];
            self.v = vec![0.0; count];
            self.pull = vec![0.0; count];
            self.carry = 0.0;
        }
        self.left = left.max(0.0);
        self.right = right.min(width).max(self.left);
        // Only points clearly between the ends move.
        let first = ((self.left / step + 0.25).floor() as usize + 1).min(count - 1);
        let last = ((self.right / step - 0.25).ceil() as usize).saturating_sub(1).min(count - 2);
        (self.first, self.last) = (first, last);
        for i in 0..count {
            if i < first || i > last {
                self.h[i] = 0.0;
                self.v[i] = 0.0;
            }
        }
    }

    fn moving(&self) -> std::ops::RangeInclusive<usize> {
        self.first..=self.last
    }

    /// Moves the surface on by `dt` seconds. `damping`: how quickly it comes to rest (per
    /// second). `weight`: what is pressing on it, if anything.
    pub fn advance(&mut self, dt: f32, damping: f32, weight: Option<Weight>) {
        if self.h.is_empty() || self.first > self.last {
            return;
        }
        self.carry += dt.max(0.0);
        // After a hitch, do not try to catch up on everything that was missed.
        self.carry = self.carry.min(TICK * 16.0);
        while self.carry >= TICK {
            self.carry -= TICK;
            self.tick(damping, weight);
        }
    }

    fn tick(&mut self, damping: f32, weight: Option<Weight>) {
        let range = self.moving();
        let count = (self.last + 1 - self.first) as f32;
        let mean = self.h[range.clone()].iter().sum::<f32>() / count;
        for i in range.clone() {
            // The points beyond the ends are level and still, which is what holds the ends down.
            let (h, v) = (self.h[i], self.v[i]);
            let (hl, hr) = (self.h[i - 1], self.h[i + 1]);
            let (vl, vr) = (self.v[i - 1], self.v[i + 1]);
            self.pull[i] = TENSION * (hl - 2.0 * h + hr) - LEVEL * h - VOLUME * mean - damping * v + SMOOTH * (vl - 2.0 * v + vr);
        }
        for i in range.clone() {
            self.v[i] += self.pull[i] * TICK;
            self.h[i] += self.v[i] * TICK;
        }
        if let Some(weight) = weight.filter(|w| w.depth > 0.0 && w.radius > 0.0) {
            for i in range {
                let d = (i as f32 * self.step - weight.x).abs();
                if d >= weight.radius {
                    continue;
                }
                // The round underside of what is pressing: nothing of the surface is above it.
                let floor = weight.radius - (weight.radius * weight.radius - d * d).sqrt() - weight.depth;
                if floor < 0.0 && self.h[i] > floor {
                    self.h[i] = floor;
                    self.v[i] = self.v[i].min(0.0);
                }
            }
        }
    }

    /// Gives the surface a push around `x`: `speed` pixels per second upwards (downwards
    /// below zero) in the middle, fading out `reach` pixels to either side.
    pub fn push(&mut self, x: f32, reach: f32, speed: f32) {
        if self.h.is_empty() || self.first > self.last || reach <= 0.0 {
            return;
        }
        for i in self.moving() {
            let d = (i as f32 * self.step - x).abs() / reach;
            if d < 1.0 {
                let bump = (d * std::f32::consts::FRAC_PI_2).cos();
                self.v[i] += speed * bump * bump;
            }
        }
    }

    /// How level the surface is drawn `x` pixels along: fully away from the ends, and not at
    /// all right at them, so that it meets the round ends of the glass without a kink.
    fn taper(&self, x: f32) -> f32 {
        let ease = |t: f32| {
            let t = t.clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        let reach = TAPER * self.step;
        ease((x - self.left) / reach) * ease((self.right - x) / reach)
    }

    /// Height of the surface above rest at `x`, in pixels.
    pub fn height(&self, x: f32) -> f32 {
        if self.h.is_empty() || self.first > self.last || x <= self.left || x >= self.right {
            return 0.0;
        }
        let at = (x / self.step).max(0.0);
        let i = (at.floor() as usize).min(self.h.len() - 2);
        let f = at - i as f32;
        (self.h[i] * (1.0 - f) + self.h[i + 1] * f) * self.taper(x)
    }

    /// The surface from end to end, as (x, height above rest), for drawing. The two ends
    /// are always level.
    pub fn points(&self, out: &mut Vec<(f32, f32)>) {
        out.clear();
        out.push((self.left, 0.0));
        if !self.h.is_empty() && self.first <= self.last {
            for i in self.moving() {
                let x = i as f32 * self.step;
                out.push((x, self.h[i] * self.taper(x)));
            }
        }
        out.push((self.right, 0.0));
    }

    /// How far the highest point stands above rest (never below zero).
    pub fn crest(&self) -> f32 {
        if self.h.is_empty() || self.first > self.last {
            return 0.0;
        }
        self.moving().map(|i| self.h[i] * self.taper(i as f32 * self.step)).fold(0.0, f32::max)
    }

    /// True when the surface is level and still.
    pub fn calm(&self) -> bool {
        self.h.is_empty() || self.first > self.last || self.moving().all(|i| self.h[i].abs() < 0.06 && self.v[i].abs() < 1.5)
    }

    /// Makes the surface level and still at once.
    pub fn still(&mut self) {
        self.h.iter_mut().for_each(|h| *h = 0.0);
        self.v.iter_mut().for_each(|v| *v = 0.0);
        self.carry = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAMPING: f32 = 4.5;

    fn water() -> Surface {
        let mut s = Surface::default();
        s.span(40.0, 340.0, 3.0, 400.0);
        s
    }

    fn run(s: &mut Surface, seconds: f32, rate: f32, weight: Option<Weight>) {
        for _ in 0..(seconds * rate).round() as usize {
            s.advance(1.0 / rate, DAMPING, weight);
        }
    }

    #[test]
    fn a_weight_dents_the_surface_and_the_rest_swells() {
        let mut s = water();
        assert!(s.calm());
        let weight = Weight { x: 190.0, depth: 20.0, radius: 22.0 };
        run(&mut s, 0.4, 60.0, Some(weight));
        assert!(!s.calm());
        assert!((s.height(190.0) + 20.0).abs() < 0.5, "under the weight: {}", s.height(190.0));
        // The skin comes down with it on both sides, less the further away...
        assert!(s.height(165.0) < -1.0 && s.height(165.0) > -19.0, "beside it: {}", s.height(165.0));
        // ...and far away the water that was pushed aside lifts it.
        assert!(s.height(80.0) > 0.5 && s.height(300.0) > 0.5, "far away: {} and {}", s.height(80.0), s.height(300.0));
        assert!(s.crest() > 0.5 && s.crest() < 8.0, "crest {}", s.crest());
    }

    #[test]
    fn let_go_it_springs_past_level_and_comes_to_rest() {
        let mut s = water();
        run(&mut s, 0.4, 60.0, Some(Weight { x: 190.0, depth: 20.0, radius: 22.0 }));
        let mut highest: f32 = 0.0;
        let mut reached_the_end = false;
        for _ in 0..60 {
            s.advance(1.0 / 60.0, DAMPING, None);
            highest = highest.max(s.height(190.0));
            reached_the_end |= s.height(70.0).abs() > 1.0;
        }
        assert!(highest > 4.0 && highest < 20.0, "where the dent was it stands {highest} px above level");
        assert!(reached_the_end, "the wave runs along the surface");
        run(&mut s, 3.0, 60.0, None);
        assert!(s.calm(), "still moving after three seconds: crest {}", s.crest());
    }

    #[test]
    fn the_ends_stay_level_and_nothing_moves_outside_them() {
        let mut s = water();
        let weight = Weight { x: 46.0, depth: 18.0, radius: 24.0 };
        for _ in 0..90 {
            s.advance(1.0 / 60.0, DAMPING, Some(weight));
            assert_eq!(s.height(40.0), 0.0);
            assert_eq!(s.height(340.0), 0.0);
            assert_eq!(s.height(20.0), 0.0);
            assert_eq!(s.height(380.0), 0.0);
        }
        let mut points = Vec::new();
        s.points(&mut points);
        assert_eq!(points.first(), Some(&(40.0, 0.0)));
        assert_eq!(points.last(), Some(&(340.0, 0.0)));
        assert!(points.windows(2).all(|p| p[1].0 > p[0].0), "left to right");
        // Next to the end the surface is drawn almost level, however hard it is pressed there.
        assert!(points[1].1.abs() < 3.0, "{:?}", points[1]);
        // Narrowed, what is now outside is level at once.
        s.span(200.0, 340.0, 3.0, 400.0);
        assert_eq!(s.height(100.0), 0.0);
    }

    #[test]
    fn the_same_at_any_refresh_rate() {
        let weight = Weight { x: 190.0, depth: 16.0, radius: 22.0 };
        let (mut slow, mut fast) = (water(), water());
        run(&mut slow, 0.3, 60.0, Some(weight));
        run(&mut fast, 0.3, 144.0, Some(weight));
        run(&mut slow, 0.25, 60.0, None);
        run(&mut fast, 0.25, 144.0, None);
        for x in [90.0, 150.0, 190.0, 260.0] {
            assert!((slow.height(x) - fast.height(x)).abs() < 1.2, "at {x}: {} and {}", slow.height(x), fast.height(x));
        }
    }

    #[test]
    fn a_push_makes_a_ripple_and_it_never_runs_away() {
        let mut s = water();
        s.push(190.0, 20.0, 300.0);
        let mut most: f32 = 0.0;
        for _ in 0..240 {
            s.advance(1.0 / 60.0, DAMPING, None);
            most = most.max(s.crest());
        }
        assert!(most > 2.0 && most < 30.0, "crest {most}");
        // A surface with no room for a single point does nothing, and says it is calm.
        let mut none = Surface::default();
        none.span(10.0, 11.0, 3.0, 400.0);
        none.push(10.0, 20.0, 300.0);
        none.advance(0.1, DAMPING, Some(Weight { x: 10.0, depth: 5.0, radius: 9.0 }));
        assert!(none.calm());
        assert_eq!(none.crest(), 0.0);
    }
}
