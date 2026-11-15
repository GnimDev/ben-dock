//! Geometry of the "window flies into its dock icon" effects.
//!
//! The window's picture is cut into thin horizontal slices. For the funnel
//! effect each slice is squeezed to the width a funnel has at that height: a
//! shape that is as wide as the window at the top and as narrow as the icon at
//! the bottom. First the funnel forms (the bottom of the window narrows), then
//! the picture slides down through it into the icon.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct R {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl R {
    pub fn w(&self) -> f32 {
        self.r - self.l
    }
    pub fn h(&self) -> f32 {
        self.b - self.t
    }
}

/// One horizontal slice: which part of the window (0 = top, 1 = bottom) goes where on screen.
#[derive(Clone, Copy, Debug)]
pub struct Strip {
    pub src_top: f32,
    pub src_bottom: f32,
    pub dst: R,
}

fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn smooth(v: f32) -> f32 {
    let v = clamp01(v);
    v * v * (3.0 - 2.0 * v)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Opacity of the flying picture at progress `p` (it only fades right at the end, as the
/// last sliver disappears into the icon).
pub fn opacity(p: f32) -> f32 {
    1.0 - smooth((p - 0.88) / 0.12)
}

/// The funnel at progress `p`: where the picture's top and bottom edges are, and how far the
/// funnel shape has formed.
struct Funnel {
    win: R,
    icon: R,
    top: f32,
    bottom: f32,
    form: f32,
    end_y: f32,
}

impl Funnel {
    fn at(win: R, icon: R, p: f32) -> Self {
        let p = clamp01(p);
        // The picture ends at the middle of the icon.
        let end_y = (icon.t + icon.b) * 0.5;
        // First 40%: the bottom edge stretches down to the icon while the sides curve in.
        let reach = smooth(p / 0.4);
        // From 30% on: the top edge is pulled down through the funnel, faster and faster.
        let pull = clamp01((p - 0.3) / 0.7).powf(1.8);
        let top = lerp(win.t, end_y, pull);
        let bottom = lerp(win.b, end_y, reach).max(top).min(win.b.max(end_y));
        Funnel { win, icon, top, bottom, form: reach, end_y }
    }

    /// Left and right edge of the funnel at screen height `y`.
    fn edges(&self, y: f32) -> (f32, f32) {
        let span = (self.end_y - self.win.t).abs().max(1.0);
        let e = smooth((y - self.win.t) / span);
        let l = lerp(self.win.l, lerp(self.win.l, self.icon.l, e), self.form);
        let r = lerp(self.win.r, lerp(self.win.r, self.icon.r, e), self.form);
        (l, r.max(l))
    }
}

/// Funnel effect. `p` runs from 0 (the window where it is) to 1 (inside the icon).
///
/// The picture is cut into horizontal slices on whole screen rows. Slices are tall where the
/// funnel's sides are nearly vertical and only a row or two high where the sides sweep
/// inwards, so the edge never steps sideways by more than a couple of pixels from one slice
/// to the next. At most `max_slices` are produced.
pub fn funnel(win: R, icon: R, p: f32, max_slices: usize, out: &mut Vec<Strip>) {
    let f = Funnel::at(win, icon, p);
    let max_slices = max_slices.max(1);
    // Sideways step allowed between neighbouring slices; relaxed until the slices fit the budget.
    let mut step = 1.5;
    for _ in 0..6 {
        cut(&f, step, out);
        if out.len() <= max_slices {
            return;
        }
        step *= (out.len() as f32 / max_slices as f32).max(1.15);
    }
    out.truncate(max_slices);
}

fn cut(f: &Funnel, step: f32, out: &mut Vec<Strip>) {
    out.clear();
    let (top, bottom) = (f.top.round(), f.bottom.round());
    let height = f.bottom - f.top;
    if bottom - top < 1.0 || height <= 0.0 {
        return;
    }
    const TALLEST: f32 = 24.0;
    let mut y = top;
    while y < bottom {
        // The tallest slice (in whole rows) whose ends differ sideways by no more than `step`.
        let (l0, r0) = f.edges(y);
        let fits = |h: f32| {
            let (l1, r1) = f.edges(y + h);
            (l1 - l0).abs() <= step && (r1 - r0).abs() <= step
        };
        let limit = TALLEST.min(bottom - y);
        let mut h = limit;
        if !fits(h) {
            let (mut lo, mut hi) = (1.0, limit);
            while hi - lo > 1.0 {
                let mid = ((lo + hi) * 0.5).floor();
                if fits(mid) {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            h = lo;
        }
        let h = h.max(1.0);
        let (l, r) = f.edges(y + h * 0.5);
        out.push(Strip { src_top: clamp01((y - f.top) / height), src_bottom: clamp01((y + h - f.top) / height), dst: R { l, t: y, r, b: y + h } });
        y += h;
    }
}

/// Plain shrink effect: the whole picture scales down into the icon.
pub fn shrink(win: R, icon: R, p: f32) -> R {
    let e = smooth(clamp01(p));
    // Ease a little more at the end so it lands softly.
    let e = lerp(e, e * e, 0.35);
    R { l: lerp(win.l, icon.l, e), t: lerp(win.t, icon.t, e), r: lerp(win.r, icon.r, e), b: lerp(win.b, icon.b, e) }
}

/// Drop effect: the picture shrinks a little and slides straight down towards the icon.
pub fn drop_down(win: R, icon: R, p: f32) -> R {
    let p = clamp01(p);
    let scale = lerp(1.0, 0.3, smooth(p));
    let travel = p * p;
    let cx = lerp((win.l + win.r) * 0.5, (icon.l + icon.r) * 0.5, smooth(p));
    let cy = lerp((win.t + win.b) * 0.5, (icon.t + icon.b) * 0.5, travel);
    let (w, h) = (win.w() * scale, win.h() * scale);
    R { l: cx - w * 0.5, t: cy - h * 0.5, r: cx + w * 0.5, b: cy + h * 0.5 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: R = R { l: 300.0, t: 120.0, r: 1500.0, b: 900.0 };
    const ICON: R = R { l: 940.0, t: 1030.0, r: 973.0, b: 1063.0 };

    #[test]
    fn starts_as_the_window() {
        let mut s = Vec::new();
        funnel(WIN, ICON, 0.0, 240, &mut s);
        assert!((s[0].dst.t - WIN.t).abs() < 1e-3 && (s.last().unwrap().dst.b - WIN.b).abs() < 1e-3);
        assert!((s[0].src_top).abs() < 1e-6 && (s.last().unwrap().src_bottom - 1.0).abs() < 1e-6);
        assert!(s.iter().all(|x| (x.dst.l - WIN.l).abs() < 1e-3 && (x.dst.r - WIN.r).abs() < 1e-3));
        assert_eq!(shrink(WIN, ICON, 0.0), WIN);
    }

    #[test]
    fn ends_inside_the_icon() {
        let mut s = Vec::new();
        funnel(WIN, ICON, 1.0, 240, &mut s);
        assert!(s.is_empty(), "nothing is left to show once the picture is inside the icon");
        // Just before the end, what is left sits inside the icon.
        funnel(WIN, ICON, 0.999, 240, &mut s);
        for x in &s {
            assert!(x.dst.t >= ICON.t - 1.0 && x.dst.b <= ICON.b + 1.0);
            assert!(x.dst.l >= ICON.l - 1.0 && x.dst.r <= ICON.r + 1.0);
        }
        let end = shrink(WIN, ICON, 1.0);
        assert!((end.l - ICON.l).abs() < 1e-3 && (end.b - ICON.b).abs() < 1e-3);
        assert!(opacity(1.0) < 1e-3 && (opacity(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn slices_stay_stacked_and_inside_the_path() {
        let mut s = Vec::new();
        for step in 0..=200 {
            let p = step as f32 / 200.0;
            funnel(WIN, ICON, p, 240, &mut s);
            assert!(s.len() <= 240);
            for pair in s.windows(2) {
                assert!((pair[0].dst.b - pair[1].dst.t).abs() < 1e-3, "gap between slices at p={p}");
                assert!((pair[0].src_bottom - pair[1].src_top).abs() < 1e-5, "picture rows skipped at p={p}");
                // The edge steps sideways by a few pixels at most from one slice to the next.
                assert!((pair[0].dst.l - pair[1].dst.l).abs() <= 6.0, "edge jumps {} px at p={p}", (pair[0].dst.l - pair[1].dst.l).abs());
            }
            for x in &s {
                assert!(x.dst.b >= x.dst.t - 1e-3 && x.dst.r >= x.dst.l);
                assert!(x.dst.l >= WIN.l - 1e-3 && x.dst.r <= WIN.r + 1e-3, "slice left the funnel at p={p}");
                assert!(x.dst.t >= WIN.t - 0.51 && x.dst.b <= ICON.b + 1e-3);
                assert!(x.dst.t.fract() == 0.0 && x.dst.b.fract() == 0.0, "slices sit on whole rows");
            }
        }
    }

    #[test]
    fn moves_steadily_towards_the_icon() {
        // The top edge never moves back up, and the picture never grows wider at the bottom.
        let mut s = Vec::new();
        let mut last_top = f32::MIN;
        let mut last_width = f32::MAX;
        for step in 0..100 {
            funnel(WIN, ICON, step as f32 / 100.0, 240, &mut s);
            if s.is_empty() {
                continue;
            }
            assert!(s[0].dst.t >= last_top - 1e-3);
            last_top = s[0].dst.t;
            let w = s.last().unwrap().dst.w();
            assert!(w <= last_width + 1.0, "bottom got wider at step {step}");
            last_width = w;
        }
    }

    #[test]
    fn drop_starts_as_the_window_and_ends_on_the_icon() {
        let start = drop_down(WIN, ICON, 0.0);
        assert!((start.l - WIN.l).abs() < 1e-3 && (start.b - WIN.b).abs() < 1e-3);
        let end = drop_down(WIN, ICON, 1.0);
        let (cx, cy) = ((end.l + end.r) * 0.5, (end.t + end.b) * 0.5);
        assert!((cx - (ICON.l + ICON.r) * 0.5).abs() < 1e-2 && (cy - (ICON.t + ICON.b) * 0.5).abs() < 1e-2);
        assert!(end.w() < WIN.w() * 0.31);
    }

    #[test]
    fn works_when_the_window_reaches_below_the_icon() {
        // A window that overlaps the dock area.
        let low = R { l: 100.0, t: 500.0, r: 900.0, b: 1075.0 };
        let mut s = Vec::new();
        for step in 0..=50 {
            funnel(low, ICON, step as f32 / 50.0, 240, &mut s);
            assert!(s.iter().all(|x| x.dst.b >= x.dst.t - 1e-3 && x.dst.l.is_finite() && x.dst.r.is_finite()));
        }
    }
}
