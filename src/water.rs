//! The water of the live wallpaper, seen from above.
//!
//! It is a sheet of cells, each a few screen pixels wide, that holds how high
//! the water stands there. Every tick a cell moves towards the mean of its
//! four neighbours and past it, which is all a wave is; and it loses a little
//! of its height, so everything comes to rest by itself. A drop that falls in
//! pumps its spot up and down a couple of times, and rings run out from it.
//! Something that brushes the surface, the way the cursor does, only dents it.
//!
//! The sheet is painted as light and shade, not as water: where a slope
//! faces the light it is bright, where it faces away it is dark, and level
//! water is clear. Laid over the wallpaper's colour that is what rings look
//! like. Nothing here draws to the screen (see `wallpaper.rs`).

use std::f32::consts::TAU;

/// The water moves on in steps of this length, however long a frame takes.
const TICK: f32 = 1.0 / 60.0;
/// What is left of a wave after one tick, and one tick more. (A wave swings between two
/// ticks, so its height falls by the square root of this each tick: to a half in about
/// a second and a half.)
const KEEP: f32 = 0.984;
/// For how many ticks a drop pumps the water under it, and how long one swing takes.
const PUMP_TICKS: u32 = 20;
const PUMP_SWING: f32 = 10.0;
/// Below this height (and this speed) the water counts as level.
const STILL: f32 = 0.01;
/// Slopes gentler than this are not painted.
const FAINT: f32 = 0.012;
/// The way to the light: up and to the left.
const LIGHT: (f32, f32) = (-0.55, -0.83);
/// The colour of the light on a slope, as red, green and blue.
const GLINT: (f32, f32, f32) = (0.93, 0.98, 1.0);

/// A drop that is still pumping: where (in cells), how wide, how strong, and for how long.
struct Drop {
    x: f32,
    y: f32,
    radius: f32,
    strength: f32,
    age: u32,
}

pub struct Water {
    w: usize,
    h: usize,
    /// How high each cell stands now, and one tick ago.
    now: Vec<f32>,
    before: Vec<f32>,
    drops: Vec<Drop>,
    /// Time not played yet (less than one tick).
    carry: f32,
    /// Nothing is moving and nothing is about to.
    level: bool,
}

impl Water {
    /// A sheet of `w` by `h` cells, level.
    pub fn new(w: usize, h: usize) -> Self {
        let (w, h) = (w.max(3), h.max(3));
        Water { w, h, now: vec![0.0; w * h], before: vec![0.0; w * h], drops: Vec::new(), carry: 0.0, level: true }
    }

    pub fn size(&self) -> (usize, usize) {
        (self.w, self.h)
    }

    /// A drop falls in at (`x`, `y`), in cells: rings spread from there. `radius`: how wide
    /// it is, in cells. `strength`: about 3 for a click of the mouse.
    pub fn drop(&mut self, x: f32, y: f32, radius: f32, strength: f32) {
        if self.drops.len() < 48 {
            self.drops.push(Drop { x, y, radius: radius.max(0.6), strength, age: 0 });
            self.level = false;
        }
    }

    /// Something brushes the surface at (`x`, `y`): a dent, there and then.
    pub fn touch(&mut self, x: f32, y: f32, radius: f32, strength: f32) {
        self.dent(x, y, radius.max(0.6), strength);
        self.level = false;
    }

    fn dent(&mut self, x: f32, y: f32, radius: f32, strength: f32) {
        let reach = radius * 3.0;
        let (x0, x1) = ((x - reach).floor().max(1.0) as usize, ((x + reach).ceil() as usize).min(self.w - 2));
        let (y0, y1) = ((y - reach).floor().max(1.0) as usize, ((y + reach).ceil() as usize).min(self.h - 2));
        if x - reach > (self.w - 1) as f32 || y - reach > (self.h - 1) as f32 || x + reach < 0.0 || y + reach < 0.0 {
            return;
        }
        for cy in y0..=y1 {
            for cx in x0..=x1 {
                let d2 = (cx as f32 - x).powi(2) + (cy as f32 - y).powi(2);
                self.now[cy * self.w + cx] -= strength * (-d2 / (radius * radius)).exp();
            }
        }
    }

    /// True when the water is level and nothing is falling in.
    pub fn calm(&self) -> bool {
        self.level
    }

    /// Moves the water on by `dt` seconds. Returns true if it moved.
    pub fn advance(&mut self, dt: f32) -> bool {
        if self.level {
            self.carry = 0.0;
            return false;
        }
        // After a hitch, do not try to catch up on everything that was missed.
        self.carry = (self.carry + dt.max(0.0)).min(TICK * 4.0);
        let mut moved = false;
        while self.carry >= TICK {
            self.carry -= TICK;
            self.tick();
            moved = true;
        }
        moved
    }

    fn tick(&mut self) {
        // The drops still falling in pump the water under them: down, up, and less each time.
        let mut drops = std::mem::take(&mut self.drops);
        for drop in &mut drops {
            let swing = (TAU * drop.age as f32 / PUMP_SWING).sin() * (1.0 - drop.age as f32 / PUMP_TICKS as f32);
            self.dent(drop.x, drop.y, drop.radius, drop.strength * swing * 0.42);
            drop.age += 1;
        }
        drops.retain(|drop| drop.age < PUMP_TICKS);
        self.drops = drops;

        let (w, h) = (self.w, self.h);
        let mut most: f32 = 0.0;
        for y in 1..h - 1 {
            let row = y * w;
            for x in 1..w - 1 {
                let i = row + x;
                // Towards the mean of the four neighbours and as far past it again. The cell's
                // own height of a tick ago is all that is needed of the old sheet, so the new
                // one is written over it.
                let next = ((self.now[i - 1] + self.now[i + 1] + self.now[i - w] + self.now[i + w]) * 0.5 - self.before[i]) * KEEP;
                most = most.max(next.abs()).max((next - self.now[i]).abs());
                self.before[i] = next;
            }
        }
        std::mem::swap(&mut self.now, &mut self.before);
        if most < STILL && self.drops.is_empty() {
            // Level again, to the last fraction.
            self.now.iter_mut().for_each(|v| *v = 0.0);
            self.before.iter_mut().for_each(|v| *v = 0.0);
            self.level = true;
        }
    }

    /// Paints the surface as light and shade over whatever is under it: `w * h` pixels of
    /// premultiplied BGRA, top row first. Level water is clear.
    pub fn paint(&self, out: &mut Vec<u8>) {
        let (w, h) = (self.w, self.h);
        out.clear();
        out.resize(w * h * 4, 0);
        if self.level {
            return;
        }
        for y in 1..h - 1 {
            let row = y * w;
            for x in 1..w - 1 {
                let i = row + x;
                let slope = -((self.now[i + 1] - self.now[i - 1]) * LIGHT.0 + (self.now[i + w] - self.now[i - w]) * LIGHT.1);
                // The faintest slopes are left out: a sheet of cells always trembles a little
                // from one cell to the next, and painted, that shows as stripes.
                if slope.abs() < FAINT {
                    continue;
                }
                let slope = slope - FAINT.copysign(slope);
                // Facing the light: a soft glow, and a glint on the steepest of it.
                let glow = (slope * 2.4).clamp(0.0, 1.0);
                let glint = (slope * 1.1 - 0.35).clamp(0.0, 1.0);
                let light = (glow * glow * (3.0 - 2.0 * glow) * 0.8 + glint * glint * 0.35).min(1.0);
                // Facing away: shade.
                let shade = (-slope * 2.0).clamp(0.0, 1.0) * 0.5;
                let alpha = light + shade * (1.0 - light);
                let o = i * 4;
                out[o] = (GLINT.2 * light * 255.0) as u8;
                out[o + 1] = (GLINT.1 * light * 255.0) as u8;
                out[o + 2] = (GLINT.0 * light * 255.0) as u8;
                out[o + 3] = (alpha * 255.0) as u8;
            }
        }
    }
}

/// The light that plays on the bottom of shallow water, as a square picture that can be
/// laid side by side with itself without a seam: `size * size` values from 0 (dark) to 1.
/// (The pattern is the one David Hoskins published for shaders; here it is worked out once.)
pub fn caustics(size: usize) -> Vec<f32> {
    const ROUNDS: usize = 5;
    const MOMENT: f32 = 3.7;
    const THIN: f32 = 0.005;
    let mut out = vec![0.0; size * size];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 / size as f32 * TAU - 250.0, y as f32 / size as f32 * TAU - 250.0);
            let (mut ix, mut iy) = (px, py);
            let mut sum = 1.0;
            for round in 0..ROUNDS {
                let t = MOMENT * (1.0 - 3.5 / (round + 1) as f32);
                (ix, iy) = (px + (t - ix).cos() + (t + iy).sin(), py + (t - iy).sin() + (t + ix).cos());
                let (a, b) = (px / ((ix + t).sin() / THIN), py / ((iy + t).cos() / THIN));
                sum += 1.0 / (a * a + b * b).sqrt();
            }
            let c = 1.17 - (sum / ROUNDS as f32).powf(1.4);
            out[y * size + x] = c.abs().powi(8).clamp(0.0, 1.0);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(water: &mut Water, seconds: f32, rate: f32) {
        for _ in 0..(seconds * rate).round() as usize {
            water.advance(1.0 / rate);
        }
    }

    fn lit(water: &Water) -> usize {
        let mut pixels = Vec::new();
        water.paint(&mut pixels);
        pixels.chunks_exact(4).filter(|p| p[3] > 8).count()
    }

    #[test]
    fn level_water_is_calm_and_clear() {
        let mut water = Water::new(120, 80);
        assert!(water.calm());
        assert!(!water.advance(0.1));
        assert_eq!(lit(&water), 0);
        let mut pixels = Vec::new();
        water.paint(&mut pixels);
        assert_eq!(pixels.len(), 120 * 80 * 4);
    }

    #[test]
    fn a_drop_sends_rings_out_and_the_water_comes_to_rest() {
        let mut water = Water::new(160, 100);
        water.drop(80.0, 50.0, 3.2, 3.2);
        assert!(!water.calm());
        run(&mut water, 0.5, 60.0);
        let near = lit(&water);
        assert!(near > 100, "rings near the drop: {near} lit pixels");
        // Half a second on, the rings have run about twenty cells out and not much further.
        let at = |x: usize, y: usize| water.now[y * 160 + x].abs();
        assert!((60..70).chain(92..102).any(|x| at(x, 50) > 0.01), "nothing twenty cells out");
        assert!(at(140, 50) < 0.001 && at(10, 50) < 0.001, "already at the edges");
        run(&mut water, 12.0, 60.0);
        assert!(water.calm(), "still moving after twelve seconds");
        assert_eq!(lit(&water), 0);
    }

    #[test]
    fn light_and_shade_lie_on_opposite_slopes() {
        let mut water = Water::new(80, 80);
        water.touch(40.0, 40.0, 4.0, 1.5);
        let mut pixels = Vec::new();
        water.paint(&mut pixels);
        let at = |x: usize, y: usize| {
            let o = (y * 80 + x) * 4;
            (pixels[o + 2], pixels[o + 3])
        };
        // A dent: its lower right wall faces the light from the upper left, its upper left
        // wall is turned away from it.
        let (lit_red, lit_alpha) = at(43, 44);
        let (shaded_red, shaded_alpha) = at(37, 36);
        assert!(lit_red > 60 && lit_alpha > 60, "{lit_red} {lit_alpha}");
        assert!(shaded_red < 10 && shaded_alpha > 30, "{shaded_red} {shaded_alpha}");
        // Premultiplied: no colour is brighter than its own cover.
        assert!(pixels.chunks_exact(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]));
        // The rim of the sheet is never painted.
        assert_eq!(at(0, 40).1, 0);
        assert_eq!(at(79, 40).1, 0);
    }

    #[test]
    fn the_same_at_any_refresh_rate() {
        let (mut slow, mut fast) = (Water::new(100, 60), Water::new(100, 60));
        slow.drop(50.0, 30.0, 3.0, 3.0);
        fast.drop(50.0, 30.0, 3.0, 3.0);
        run(&mut slow, 0.5, 60.0);
        run(&mut fast, 0.5, 240.0);
        let most = slow.now.iter().zip(&fast.now).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(most < 0.02, "the sheets differ by {most}");
    }

    #[test]
    fn drops_outside_the_sheet_do_no_harm() {
        let mut water = Water::new(40, 30);
        water.drop(-50.0, -50.0, 3.0, 3.0);
        water.drop(500.0, 500.0, 3.0, 3.0);
        water.touch(39.5, 29.5, 2.0, 1.0);
        run(&mut water, 1.0, 60.0);
        assert!(water.now.iter().all(|v| v.is_finite()));
        for _ in 0..200 {
            water.drop(20.0, 15.0, 2.0, 1.0);
        }
        run(&mut water, 20.0, 60.0);
        assert!(water.calm());
    }

    #[test]
    fn the_light_pattern_joins_up_with_itself() {
        let size = 64;
        let tile = caustics(size);
        assert_eq!(tile.len(), size * size);
        assert!(tile.iter().all(|v| (0.0..=1.0).contains(v)));
        let mean = tile.iter().sum::<f32>() / tile.len() as f32;
        assert!(mean > 0.02 && mean < 0.5, "mean {mean}");
        // The step from the last column back to the first is no rougher than the steps inside.
        let step = |a: usize, b: usize| (0..size).map(|y| (tile[y * size + a] - tile[y * size + b]).abs()).sum::<f32>() / size as f32;
        let inside = (1..size).map(|x| step(x - 1, x)).fold(0.0, f32::max);
        assert!(step(size - 1, 0) <= inside * 1.5 + 0.01, "seam {} against {}", step(size - 1, 0), inside);
    }
}
