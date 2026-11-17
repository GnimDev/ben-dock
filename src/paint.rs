//! Pictures painted by hand, pixel by pixel: the rings, arcs, glows and graphs of the
//! widgets, and the settings window's small pictures of them.
//!
//! The compositor has rounded bars and text; the widgets want more than that, and what
//! they want is small enough to paint here. Every shape has a soft one-pixel edge, worked
//! out from how far the middle of a pixel is from the shape (the way `shell::launcher_icon`
//! paints its tile). A picture is premultiplied BGRA, top row first, as `Scene` takes it.
//!
//! Angles are in turns, clockwise from twelve o'clock, as on a clock's face.

use std::f32::consts::TAU;

/// A colour and how solid it is: red, green, blue and alpha, each 0 to 1. The colour is not
/// yet weighted by the alpha.
pub type Rgba = [f32; 4];

/// The same colour, less solid.
pub fn fade(color: Rgba, by: f32) -> Rgba {
    [color[0], color[1], color[2], color[3] * by]
}

/// A colour part of the way from one to another (0 = the first, 1 = the second).
pub fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t, a[3] + (b[3] - a[3]) * t]
}

/// The colour with its hue turned by so many degrees round the colour wheel: as bright and
/// as strong as it was, but another colour. Grey stays grey.
pub fn turn(color: Rgba, degrees: f32) -> Rgba {
    let [r, g, b, a] = color;
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let spread = max - min;
    if spread < 1e-4 {
        return color;
    }
    let hue = if max == r {
        ((g - b) / spread).rem_euclid(6.0)
    } else if max == g {
        (b - r) / spread + 2.0
    } else {
        (r - g) / spread + 4.0
    };
    let hue = (hue * 60.0 + degrees).rem_euclid(360.0) / 60.0;
    let x = spread * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (spread, x, 0.0),
        1 => (x, spread, 0.0),
        2 => (0.0, spread, x),
        3 => (0.0, x, spread),
        4 => (x, 0.0, spread),
        _ => (spread, 0.0, x),
    };
    [r + min, g + min, b + min, a]
}

/// Where a point of a circle is: `turns` round from twelve o'clock, `radius` from the middle.
pub fn on_circle(cx: f32, cy: f32, radius: f32, turns: f32) -> (f32, f32) {
    let angle = turns * TAU;
    (cx + radius * angle.sin(), cy - radius * angle.cos())
}

pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub bgra: Vec<u8>,
}

impl Canvas {
    /// A clear picture of this many pixels.
    pub fn new(w: u32, h: u32) -> Self {
        Canvas { w, h, bgra: vec![0; (w * h * 4) as usize] }
    }

    /// Lays a colour over one pixel, `cover` of it (0 to 1).
    fn put(&mut self, x: i32, y: i32, color: Rgba, cover: f32) {
        let alpha = color[3].clamp(0.0, 1.0) * cover.clamp(0.0, 1.0);
        if alpha <= 0.0 || x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let at = ((y as u32 * self.w + x as u32) * 4) as usize;
        let under = 1.0 - alpha;
        for (offset, channel) in [color[2], color[1], color[0], 1.0].into_iter().enumerate() {
            let pixel = &mut self.bgra[at + offset];
            *pixel = (channel.clamp(0.0, 1.0) * alpha * 255.0 + *pixel as f32 * under).round().min(255.0) as u8;
        }
    }

    /// Goes over the pixels of a rectangle and lays over each the colour `shade` gives for
    /// its middle, as much of it as `shade` says.
    fn each(&mut self, left: f32, top: f32, right: f32, bottom: f32, mut shade: impl FnMut(f32, f32) -> (Rgba, f32)) {
        let (x0, y0) = (left.floor().max(0.0) as i32, top.floor().max(0.0) as i32);
        let (x1, y1) = ((right.ceil() as i32).min(self.w as i32), (bottom.ceil() as i32).min(self.h as i32));
        for y in y0..y1 {
            for x in x0..x1 {
                let (color, cover) = shade(x as f32 + 0.5, y as f32 + 0.5);
                self.put(x, y, color, cover);
            }
        }
    }

    /// A filled circle.
    pub fn disc(&mut self, cx: f32, cy: f32, radius: f32, color: Rgba) {
        self.each(cx - radius - 1.0, cy - radius - 1.0, cx + radius + 1.0, cy + radius + 1.0, |x, y| (color, radius - (x - cx).hypot(y - cy) + 0.5));
    }

    /// A light with no edge: strongest in the middle, gone at `radius`.
    pub fn glow(&mut self, cx: f32, cy: f32, radius: f32, color: Rgba) {
        self.each(cx - radius, cy - radius, cx + radius, cy + radius, |x, y| {
            let near = (1.0 - (x - cx).hypot(y - cy) / radius.max(0.01)).max(0.0);
            (color, near * near)
        });
    }

    /// A ring, or part of one: `share` of a full turn, beginning at `from`, of a band
    /// `thick` wide whose middle is `radius` from the centre. Its ends are round. The colour
    /// runs from `tail` where it begins to `head` where it ends.
    pub fn arc(&mut self, cx: f32, cy: f32, radius: f32, thick: f32, from: f32, share: f32, tail: Rgba, head: Rgba) {
        let share = share.clamp(0.0, 1.0);
        if share <= 0.0 {
            return;
        }
        let half = thick * 0.5;
        let (begin, end) = (on_circle(cx, cy, radius, from), on_circle(cx, cy, radius, from + share));
        let reach = radius + half + 1.0;
        self.each(cx - reach, cy - reach, cx + reach, cy + reach, |x, y| {
            let (dx, dy) = (x - cx, y - cy);
            let band = half - (dx.hypot(dy) - radius).abs() + 0.5;
            // How far round the pixel is, measured from where the arc begins.
            let round = (dx.atan2(-dy) / TAU - from).rem_euclid(1.0);
            if share >= 1.0 || round <= share {
                return (mix(tail, head, round / share), band);
            }
            // Past an end: only the round cap of that end is left.
            let (at_begin, at_end) = (half - (x - begin.0).hypot(y - begin.1) + 0.5, half - (x - end.0).hypot(y - end.1) + 0.5);
            if at_end >= at_begin {
                (head, at_end)
            } else {
                (tail, at_begin)
            }
        });
    }

    /// A rounded bar, its colour running from `left` to `right` along it.
    pub fn bar(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, left: Rgba, right: Rgba) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let radius = radius.min(w * 0.5).min(h * 0.5);
        let (mx, my) = (x + w * 0.5, y + h * 0.5);
        self.each(x - 1.0, y - 1.0, x + w + 1.0, y + h + 1.0, |px, py| {
            let (dx, dy) = ((px - mx).abs() - (w * 0.5 - radius), (py - my).abs() - (h * 0.5 - radius));
            let outside = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - radius;
            (mix(left, right, (px - x) / w), 0.5 - outside)
        });
    }

    /// A straight line with round ends.
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, thick: f32, color: Rgba) {
        let half = thick * 0.5;
        let (vx, vy) = (x1 - x0, y1 - y0);
        let length = (vx * vx + vy * vy).max(1e-6);
        self.each(x0.min(x1) - half - 1.0, y0.min(y1) - half - 1.0, x0.max(x1) + half + 1.0, y0.max(y1) + half + 1.0, |x, y| {
            let along = (((x - x0) * vx + (y - y0) * vy) / length).clamp(0.0, 1.0);
            (color, half - (x - x0 - vx * along).hypot(y - y0 - vy * along) + 0.5)
        });
    }

    /// A graph of values (each 0 to 1, the oldest first) in a rectangle: a smooth line
    /// through them, and under it a filling that is `fill` just below the line and thins
    /// out to nothing at the foot.
    pub fn graph(&mut self, x: f32, y: f32, w: f32, h: f32, values: &[f32], thick: f32, line: Rgba, fill: Rgba) {
        if values.len() < 2 || w <= 1.0 || h <= 1.0 {
            return;
        }
        let half = thick * 0.5;
        // Where the line is above a point of the floor: eased from each value to the next.
        let height = |px: f32| {
            let at = ((px - x) / w).clamp(0.0, 1.0) * (values.len() - 1) as f32;
            let index = (at.floor() as usize).min(values.len() - 2);
            let t = at - index as f32;
            let eased = t * t * (3.0 - 2.0 * t);
            let value = values[index] + (values[index + 1] - values[index]) * eased;
            y + h - half - value.clamp(0.0, 1.0) * (h - thick)
        };
        self.each(x, y - 1.0, x + w, y + h, |px, py| {
            let top = height(px);
            // How steep the line is here: a steep line is nearer than it is high above.
            let slope = height(px + 0.5) - height(px - 0.5);
            let off = (py - top).abs() / (1.0 + slope * slope).sqrt();
            let on_line = (half - off + 0.5).clamp(0.0, 1.0);
            let below = (py - top + 0.5).clamp(0.0, 1.0) * (1.0 - (py - top) / (y + h - top).max(1.0)).clamp(0.0, 1.0);
            // The line over the filling, as one colour for the pixel.
            let under = fill[3] * below * (1.0 - line[3] * on_line);
            let alpha = line[3] * on_line + under;
            if alpha <= 0.0 {
                return (line, 0.0);
            }
            let share = line[3] * on_line / alpha;
            ([fill[0] + (line[0] - fill[0]) * share, fill[1] + (line[1] - fill[1]) * share, fill[2] + (line[2] - fill[2]) * share, alpha], 1.0)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WHITE: Rgba = [1.0, 1.0, 1.0, 1.0];

    fn alpha(canvas: &Canvas, x: u32, y: u32) -> u8 {
        canvas.bgra[((y * canvas.w + x) * 4 + 3) as usize]
    }

    #[test]
    fn a_disc_is_solid_inside_and_clear_outside() {
        let mut canvas = Canvas::new(40, 40);
        canvas.disc(20.0, 20.0, 10.0, WHITE);
        assert_eq!(alpha(&canvas, 20, 20), 255);
        assert_eq!(alpha(&canvas, 20, 12), 255);
        assert_eq!(alpha(&canvas, 20, 5), 0);
        assert_eq!(alpha(&canvas, 2, 2), 0);
        // Its edge is soft: a pixel the edge runs through is partly covered.
        let edge = alpha(&canvas, 29, 23);
        assert!(edge > 40 && edge < 200, "{edge}");
    }

    #[test]
    fn a_colour_that_is_half_there_stays_half_there_all_over() {
        let mut canvas = Canvas::new(40, 40);
        canvas.disc(20.0, 20.0, 15.0, [1.0, 1.0, 1.0, 0.5]);
        // Deep inside it is no more solid than near its edge.
        assert_eq!(alpha(&canvas, 20, 20), 128);
        assert_eq!(alpha(&canvas, 20, 8), 128);
        // Laid over itself it gets more solid, and never more than whole.
        canvas.disc(20.0, 20.0, 15.0, [1.0, 1.0, 1.0, 0.5]);
        assert_eq!(alpha(&canvas, 20, 20), 192);
    }

    #[test]
    fn an_arc_begins_at_the_top_and_goes_clockwise() {
        let mut canvas = Canvas::new(60, 60);
        // A quarter of a ring: from twelve o'clock to three.
        canvas.arc(30.0, 30.0, 20.0, 6.0, 0.0, 0.25, WHITE, WHITE);
        assert_eq!(alpha(&canvas, 44, 16), 255, "half way along it");
        assert_eq!(alpha(&canvas, 30, 10), 255, "where it begins");
        assert_eq!(alpha(&canvas, 49, 30), 255, "where it ends");
        assert_eq!(alpha(&canvas, 30, 50), 0, "six o'clock");
        assert_eq!(alpha(&canvas, 10, 30), 0, "nine o'clock");
        assert_eq!(alpha(&canvas, 30, 30), 0, "the middle");
        // Its ends are round: just before twelve there is still some of the cap.
        assert!(alpha(&canvas, 28, 10) > 0);
        assert_eq!(alpha(&canvas, 24, 11), 0);
    }

    #[test]
    fn a_whole_ring_has_no_ends_and_no_share_is_nothing() {
        let mut ring = Canvas::new(60, 60);
        ring.arc(30.0, 30.0, 20.0, 6.0, 0.3, 1.0, WHITE, WHITE);
        for (x, y) in [(30, 10), (50, 30), (30, 50), (10, 30)] {
            assert_eq!(alpha(&ring, x, y), 255);
        }
        let mut none = Canvas::new(60, 60);
        none.arc(30.0, 30.0, 20.0, 6.0, 0.0, 0.0, WHITE, WHITE);
        assert!(none.bgra.iter().all(|&byte| byte == 0));
    }

    #[test]
    fn an_arc_changes_colour_along_its_way() {
        let mut canvas = Canvas::new(60, 60);
        canvas.arc(30.0, 30.0, 20.0, 6.0, 0.0, 0.5, [1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]);
        let pixel = |x: u32, y: u32| &canvas.bgra[((y * 60 + x) * 4) as usize..][..4];
        // Red where it begins (blue, green, red, alpha), blue where it ends.
        assert!(pixel(31, 10)[2] > 240 && pixel(31, 10)[0] < 15);
        assert!(pixel(31, 50)[0] > 240 && pixel(31, 50)[2] < 15);
    }

    #[test]
    fn a_bar_and_a_line_fill_what_they_cover() {
        let mut canvas = Canvas::new(60, 20);
        canvas.bar(5.0, 5.0, 50.0, 10.0, 5.0, WHITE, WHITE);
        assert_eq!(alpha(&canvas, 30, 10), 255);
        assert_eq!(alpha(&canvas, 5, 5), 0, "its corner is rounded off");
        assert_eq!(alpha(&canvas, 30, 2), 0);
        let mut canvas = Canvas::new(60, 20);
        canvas.line(10.0, 10.0, 50.0, 10.0, 4.0, WHITE);
        assert_eq!(alpha(&canvas, 30, 10), 255);
        assert_eq!(alpha(&canvas, 30, 4), 0);
        assert_eq!(alpha(&canvas, 55, 10), 0);
    }

    #[test]
    fn a_graph_is_higher_where_the_value_is() {
        let mut canvas = Canvas::new(100, 40);
        canvas.graph(0.0, 0.0, 100.0, 40.0, &[0.0, 1.0], 2.0, WHITE, [1.0, 1.0, 1.0, 0.5]);
        // At the low end the line is at the foot and nothing is above it; at the high end
        // it is at the top, with the filling under it.
        assert_eq!(alpha(&canvas, 1, 5), 0);
        assert!(alpha(&canvas, 1, 38) > 200);
        assert!(alpha(&canvas, 98, 1) > 200);
        let filled = alpha(&canvas, 98, 12);
        assert!(filled > 40 && filled < 200, "{filled}");
    }

    #[test]
    fn colours_mix_fade_and_turn() {
        assert_eq!(mix([0.0, 0.0, 0.0, 1.0], WHITE, 0.5), [0.5, 0.5, 0.5, 1.0]);
        assert_eq!(fade(WHITE, 0.25), [1.0, 1.0, 1.0, 0.25]);
        // Red turned a third of the way round is green; grey has no hue to turn.
        let green = turn([1.0, 0.0, 0.0, 1.0], 120.0);
        assert!(green[0] < 0.01 && green[1] > 0.99 && green[2] < 0.01);
        assert_eq!(turn([0.4, 0.4, 0.4, 1.0], 90.0), [0.4, 0.4, 0.4, 1.0]);
    }
}
