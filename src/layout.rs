//! Dock magnification geometry.
//!
//! Every icon has a slot on a "rest" axis (the dock with no magnification).
//! Magnification is a smooth stretch of that axis around the cursor: a point
//! at rest coordinate `u` is displayed at `u + D(u - cursor)`, where `D` is
//! the running integral of a raised-cosine bump. Because the stretch is one
//! continuous function, icons never overlap, never jump when the cursor
//! crosses from one icon to the next, and each icon's size is simply how much
//! its slot was stretched.

use std::f32::consts::PI;

#[derive(Clone, Copy, Debug)]
pub struct Magnifier {
    /// Extra scale at the cursor (0.9 means icons under the cursor are 1.9x).
    pub amp: f32,
    /// How far from the cursor the effect reaches, in rest pixels.
    pub radius: f32,
    /// Cursor position on the rest axis.
    pub center: f32,
}

impl Magnifier {
    /// Displacement of a point `t` pixels away from the cursor.
    pub fn disp(&self, t: f32) -> f32 {
        if self.amp == 0.0 || self.radius <= 0.0 {
            return 0.0;
        }
        let r = self.radius;
        let t = t.clamp(-r, r);
        self.amp * 0.5 * (t + (r / PI) * (PI * t / r).sin())
    }

    pub fn map(&self, u: f32) -> f32 {
        u + self.disp(u - self.center)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Slot {
    pub left: f32,
    pub right: f32,
    /// Stretch factor of this slot (1.0 at rest).
    pub scale: f32,
}

impl Slot {
    pub fn center(&self) -> f32 {
        (self.left + self.right) * 0.5
    }
}

/// Lays out slots of the given rest widths, centred on `center_x`.
/// The dock stays centred while it grows, so the output is shifted by half of
/// the difference between how far the two ends moved.
pub fn layout(widths: &[f32], center_x: f32, mag: &Magnifier, out: &mut Vec<Slot>) {
    out.clear();
    let total: f32 = widths.iter().sum();
    let u0 = center_x - total * 0.5;
    let un = u0 + total;
    let shift = -(mag.disp(u0 - mag.center) + mag.disp(un - mag.center)) * 0.5;
    let mut u = u0;
    let mut left = mag.map(u) + shift;
    for &w in widths {
        u += w;
        let right = mag.map(u) + shift;
        let scale = if w > 1e-3 { (right - left) / w } else { 1.0 + mag.amp * bump(u - mag.center, mag.radius) };
        out.push(Slot { left, right, scale });
        left = right;
    }
}

fn bump(t: f32, r: f32) -> f32 {
    if r <= 0.0 || t.abs() >= r {
        0.0
    } else {
        0.5 * (1.0 + (PI * t / r).cos())
    }
}

/// The grid of the app tray, in physical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    pub cell_w: f32,
    pub cell_h: f32,
    /// Top-left corner of the first cell.
    pub x: f32,
    pub y: f32,
}

impl Grid {
    /// As many columns and rows of cells as fit into `width` by `height` (within sensible
    /// limits), with its first cell at (`x`, `y`).
    pub fn fit(width: f32, height: f32, cell_w: f32, cell_h: f32, x: f32, y: f32) -> Grid {
        let cols = ((width / cell_w).floor() as usize).clamp(3, 9);
        let rows = ((height / cell_h).floor() as usize).clamp(2, 6);
        Grid { cols, rows, cell_w, cell_h, x, y }
    }

    pub fn width(&self) -> f32 {
        self.cols as f32 * self.cell_w
    }

    pub fn height(&self) -> f32 {
        self.rows as f32 * self.cell_h
    }

    pub fn per_page(&self) -> usize {
        self.cols * self.rows
    }

    pub fn pages(&self, items: usize) -> usize {
        items.div_ceil(self.per_page()).max(1)
    }

    /// Centre of the cell for the item at `index` within its page.
    pub fn cell_center(&self, index_in_page: usize) -> (f32, f32) {
        let (col, row) = (index_in_page % self.cols, index_in_page / self.cols);
        (self.x + (col as f32 + 0.5) * self.cell_w, self.y + (row as f32 + 0.5) * self.cell_h)
    }

    /// Which cell of the page a point is in, if any.
    pub fn cell_at(&self, x: f32, y: f32) -> Option<usize> {
        if x < self.x || y < self.y {
            return None;
        }
        let (col, row) = (((x - self.x) / self.cell_w) as usize, ((y - self.y) / self.cell_h) as usize);
        (col < self.cols && row < self.rows).then_some(row * self.cols + col)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(c: f32, amp: f32) -> Vec<Slot> {
        let widths = vec![60.0; 12];
        let mut out = Vec::new();
        layout(&widths, 1000.0, &Magnifier { amp, radius: 156.0, center: c }, &mut out);
        out
    }

    #[test]
    fn grid_fits_and_round_trips() {
        for (w, h) in [(1267.0, 670.0), (900.0, 476.0), (1690.0, 890.0), (2534.0, 1340.0), (528.0, 372.0)] {
            let g = Grid::fit(w, h, 102.0, 106.0, 22.0, 66.0);
            assert!(g.cols >= 3 && g.rows >= 2);
            assert!(g.cols <= 9 && g.rows <= 6);
            if w >= 3.0 * 102.0 {
                assert!(g.width() <= w + 0.5, "grid wider than its room at {w}x{h}");
            }
            for i in 0..g.per_page() {
                let (cx, cy) = g.cell_center(i);
                assert_eq!(g.cell_at(cx, cy), Some(i));
            }
            assert_eq!(g.cell_at(g.x - 1.0, g.y + 5.0), None);
            assert_eq!(g.cell_at(g.x + g.width() + 1.0, g.y + 5.0), None);
            assert_eq!(g.pages(0), 1);
            assert_eq!(g.pages(g.per_page()), 1);
            assert_eq!(g.pages(g.per_page() + 1), 2);
        }
    }

    #[test]
    fn rest_is_identity() {
        let s = run(1000.0, 0.0);
        assert!((s[0].left - 640.0).abs() < 1e-3);
        assert!((s[11].right - 1360.0).abs() < 1e-3);
        assert!(s.iter().all(|s| (s.scale - 1.0).abs() < 1e-5));
    }

    #[test]
    fn stays_centred_and_ordered() {
        for c in (600..1400).step_by(7) {
            let s = run(c as f32, 0.9);
            let mid = (s[0].left + s[11].right) * 0.5;
            assert!((mid - 1000.0).abs() < 1e-2, "centre drifted to {mid} at cursor {c}");
            for w in s.windows(2) {
                assert!((w[0].right - w[1].left).abs() < 1e-3);
                assert!(w[0].right > w[0].left);
            }
        }
    }

    #[test]
    fn cursor_is_fixed_point_in_the_interior() {
        // Far from both ends the point under the cursor must not move at all.
        let m = Magnifier { amp: 0.9, radius: 156.0, center: 1000.0 };
        assert!((m.map(1000.0) - 1000.0).abs() < 1e-4);
        let s = run(1000.0, 0.9);
        // Total growth is amp * radius, split evenly between both sides.
        let grow = (s[11].right - s[0].left) - 720.0;
        assert!((grow - 0.9 * 156.0).abs() < 1e-2, "grow {grow}");
    }

    #[test]
    fn peak_scale_is_near_one_plus_amp() {
        let s = run(1000.0 + 30.0, 0.9); // centre of slot 6
        let peak = s.iter().map(|s| s.scale).fold(0.0, f32::max);
        assert!(peak > 1.8 && peak <= 1.9, "peak {peak}");
    }

    #[test]
    fn continuous_as_cursor_moves() {
        let mut prev = run(700.0, 0.9);
        let mut c = 700.0;
        while c < 1300.0 {
            c += 0.5;
            let cur = run(c, 0.9);
            for (a, b) in prev.iter().zip(cur.iter()) {
                assert!((a.left - b.left).abs() < 1.0, "jump at cursor {c}");
            }
            prev = cur;
        }
    }
}
