//! Themes: what the glass is made of and what is written on it.
//!
//! One function turns the settings into every colour the interface uses, so
//! nothing else in the app holds a colour of its own. A theme is a material
//! (liquid, frosted, ink, solid), a mode (dark or light), and its colours:
//! the Windows accent colour on plain glass, or one of the app's own sets
//! of colours (`SCHEMES`).
//!
//!   liquid   clear glass: little tint, strong colours behind, a bright rim
//!   frosted  more blur and more tint: calmer, the easiest to read
//!   ink      smoked black glass whatever the mode, like a phone's island
//!   solid    no blur at all: the lightest on the machine
//!
//! Each of the app's own sets is named after a water it was taken from: the
//! wharf by day, at dawn and at dusk, a flooded forest, mist, a river under
//! lanterns, a sheet of paper, a pond of lotus. It has an accent colour, glass
//! of its own colour, a tint for the glass's bright edge, the two waters of
//! the live wallpaper, and the material it was made for.

use crate::config::Config;
use crate::gfx::color;
use crate::paint::{fade, Canvas, Rgba};
use crate::scene::{rgba, Material};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::UI::Color;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Liquid,
    Frosted,
    Ink,
    Solid,
}

impl Style {
    pub fn of(cfg: &Config) -> Style {
        match cfg.style.round() as i32 {
            1 => Style::Frosted,
            2 => Style::Ink,
            3 => Style::Solid,
            _ => Style::Liquid,
        }
    }
}

/// The water of the live wallpaper: its colour at the top and at the bottom, the colour of
/// the light that plays on it, and how strong each of the two layers of that light is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Water {
    pub top: (f32, f32, f32),
    pub bottom: (f32, f32, f32),
    pub light: (f32, f32, f32),
    pub layers: [f32; 2],
}

/// A set of colours that belong together, and the glass they were chosen for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scheme {
    /// The accent colour. `None`: the one Windows has.
    pub accent: Option<(f32, f32, f32)>,
    /// The glass's own colour where it is dark, and where it is light. `None`: plain glass.
    pub glass: Option<((f32, f32, f32), (f32, f32, f32))>,
    /// The colour of the glass's bright edge.
    pub rim: (f32, f32, f32),
    /// The wallpaper's water: the dark one and the bright one.
    pub water: [Water; 2],
    /// What choosing it sets besides (see `choose`): the material, light or dark (as the
    /// setting "theme" says it), how dense the glass is, how much it blurs what is behind
    /// and takes of the accent colour, and how round the dock is.
    pub style: f32,
    pub mode: f32,
    pub tint_opacity: f32,
    pub blur_radius: f32,
    pub accent_tint: f32,
    pub corner_radius: f32,
}

/// The sets of colours, in the order of the setting "scheme" (whose choices are their
/// names). The first is no set of the app's own: Windows' accent colour on plain glass, a
/// pool at night and the sea on a clear day.
pub const SCHEMES: [Scheme; 9] = [
    Scheme {
        accent: None,
        glass: None,
        rim: (1.0, 1.0, 1.0),
        water: [
            Water { top: (0.020, 0.043, 0.066), bottom: (0.008, 0.020, 0.034), light: (0.50, 0.78, 0.86), layers: [0.16, 0.11] },
            Water { top: (0.035, 0.300, 0.540), bottom: (0.016, 0.140, 0.330), light: (0.72, 0.93, 1.00), layers: [0.30, 0.20] },
        ],
        style: 0.0,
        mode: 0.0,
        tint_opacity: 0.5,
        blur_radius: 26.0,
        accent_tint: 0.10,
        corner_radius: 50.0,
    },
    // The wharf's own water: the blue of the logo on deep glass.
    Scheme {
        accent: Some((0.100, 0.500, 0.940)),
        glass: Some(((0.035, 0.075, 0.150), (0.930, 0.960, 0.995))),
        rim: (0.80, 0.92, 1.00),
        water: [
            Water { top: (0.028, 0.125, 0.290), bottom: (0.010, 0.045, 0.125), light: (0.45, 0.78, 1.00), layers: [0.20, 0.13] },
            Water { top: (0.035, 0.290, 0.640), bottom: (0.016, 0.130, 0.380), light: (0.72, 0.93, 1.00), layers: [0.30, 0.20] },
        ],
        style: 0.0,
        mode: 1.0,
        tint_opacity: 0.46,
        blur_radius: 28.0,
        accent_tint: 0.20,
        corner_radius: 50.0,
    },
    // Dawn: the first orange of the sun on cream.
    Scheme {
        accent: Some((0.890, 0.440, 0.140)),
        glass: Some(((0.130, 0.090, 0.070), (0.995, 0.968, 0.930))),
        rim: (1.00, 0.95, 0.86),
        water: [
            Water { top: (0.270, 0.135, 0.215), bottom: (0.095, 0.048, 0.110), light: (1.00, 0.72, 0.54), layers: [0.20, 0.13] },
            Water { top: (0.800, 0.440, 0.250), bottom: (0.430, 0.165, 0.250), light: (1.00, 0.90, 0.70), layers: [0.32, 0.21] },
        ],
        style: 0.0,
        mode: 2.0,
        tint_opacity: 0.40,
        blur_radius: 30.0,
        accent_tint: 0.12,
        corner_radius: 50.0,
    },
    // Dusk: violet over the river, the last light still warm on it.
    Scheme {
        accent: Some((0.560, 0.330, 0.860)),
        glass: Some(((0.085, 0.055, 0.140), (0.975, 0.960, 0.990))),
        rim: (0.94, 0.86, 1.00),
        water: [
            Water { top: (0.225, 0.090, 0.330), bottom: (0.070, 0.030, 0.145), light: (1.00, 0.62, 0.48), layers: [0.22, 0.15] },
            Water { top: (0.470, 0.200, 0.480), bottom: (0.200, 0.085, 0.320), light: (1.00, 0.76, 0.56), layers: [0.32, 0.20] },
        ],
        style: 0.0,
        mode: 1.0,
        tint_opacity: 0.52,
        blur_radius: 30.0,
        accent_tint: 0.16,
        corner_radius: 50.0,
    },
    // The flooded forest: jade under the trees, the glass frosted and a little squarer.
    Scheme {
        accent: Some((0.130, 0.620, 0.420)),
        glass: Some(((0.045, 0.095, 0.075), (0.945, 0.975, 0.950))),
        rim: (0.86, 1.00, 0.90),
        water: [
            Water { top: (0.030, 0.175, 0.125), bottom: (0.010, 0.068, 0.056), light: (0.70, 0.95, 0.66), layers: [0.18, 0.12] },
            Water { top: (0.060, 0.380, 0.300), bottom: (0.020, 0.180, 0.170), light: (0.86, 1.00, 0.76), layers: [0.28, 0.18] },
        ],
        style: 1.0,
        mode: 1.0,
        tint_opacity: 0.42,
        blur_radius: 26.0,
        accent_tint: 0.14,
        corner_radius: 38.0,
    },
    // Early mist: grey-green on cool white, everything behind it softened.
    Scheme {
        accent: Some((0.300, 0.520, 0.560)),
        glass: Some(((0.105, 0.125, 0.145), (0.955, 0.968, 0.980))),
        rim: (1.0, 1.0, 1.0),
        water: [
            Water { top: (0.205, 0.255, 0.305), bottom: (0.090, 0.120, 0.158), light: (0.80, 0.90, 0.96), layers: [0.15, 0.10] },
            Water { top: (0.420, 0.500, 0.570), bottom: (0.250, 0.320, 0.400), light: (1.00, 1.00, 1.00), layers: [0.26, 0.18] },
        ],
        style: 1.0,
        mode: 2.0,
        tint_opacity: 0.58,
        blur_radius: 36.0,
        accent_tint: 0.05,
        corner_radius: 50.0,
    },
    // Lanterns on the river at night: red on black glass, its edge gold.
    Scheme {
        accent: Some((0.880, 0.250, 0.180)),
        glass: Some(((0.110, 0.050, 0.035), (0.990, 0.960, 0.915))),
        rim: (1.00, 0.80, 0.46),
        water: [
            Water { top: (0.150, 0.045, 0.045), bottom: (0.036, 0.012, 0.022), light: (1.00, 0.66, 0.30), layers: [0.26, 0.18] },
            Water { top: (0.520, 0.130, 0.080), bottom: (0.200, 0.040, 0.060), light: (1.00, 0.80, 0.42), layers: [0.34, 0.22] },
        ],
        style: 2.0,
        mode: 1.0,
        tint_opacity: 0.40,
        blur_radius: 24.0,
        accent_tint: 0.08,
        corner_radius: 50.0,
    },
    // A sheet of dó paper: indigo ink, nothing blurred, the corners of a page.
    Scheme {
        accent: Some((0.220, 0.300, 0.600)),
        glass: Some(((0.150, 0.138, 0.120), (0.962, 0.945, 0.905))),
        rim: (1.00, 0.98, 0.93),
        water: [
            Water { top: (0.215, 0.200, 0.172), bottom: (0.085, 0.079, 0.070), light: (0.92, 0.88, 0.78), layers: [0.14, 0.10] },
            Water { top: (0.470, 0.440, 0.380), bottom: (0.290, 0.265, 0.230), light: (1.00, 0.97, 0.88), layers: [0.24, 0.16] },
        ],
        style: 3.0,
        mode: 2.0,
        tint_opacity: 0.50,
        blur_radius: 26.0,
        accent_tint: 0.0,
        corner_radius: 30.0,
    },
    // A pond of lotus: the pink of the flower on the dark green of the water.
    Scheme {
        accent: Some((0.860, 0.300, 0.520)),
        glass: Some(((0.040, 0.095, 0.100), (0.990, 0.958, 0.968))),
        rim: (1.00, 0.86, 0.92),
        water: [
            Water { top: (0.030, 0.155, 0.158), bottom: (0.010, 0.060, 0.070), light: (1.00, 0.70, 0.82), layers: [0.19, 0.13] },
            Water { top: (0.070, 0.330, 0.330), bottom: (0.025, 0.160, 0.180), light: (1.00, 0.82, 0.90), layers: [0.28, 0.18] },
        ],
        style: 0.0,
        mode: 1.0,
        tint_opacity: 0.48,
        blur_radius: 28.0,
        accent_tint: 0.10,
        corner_radius: 44.0,
    },
];

impl Scheme {
    /// The set of colours in use.
    pub fn of(cfg: &Config) -> &'static Scheme {
        &SCHEMES[(cfg.scheme.round().max(0.0) as usize).min(SCHEMES.len() - 1)]
    }
}

/// Makes one of the sets of colours the one in use, with the glass it was chosen for.
/// Everything it sets can still be changed afterwards, one setting at a time.
pub fn choose(index: usize, cfg: &mut Config) {
    let Some(scheme) = SCHEMES.get(index) else { return };
    cfg.scheme = index as f32;
    cfg.style = scheme.style;
    cfg.theme = scheme.mode;
    cfg.tint_opacity = scheme.tint_opacity;
    cfg.blur_radius = scheme.blur_radius;
    cfg.accent_tint = scheme.accent_tint;
    cfg.corner_radius = scheme.corner_radius;
}

/// The accent colour in use: the theme's own, or `windows`, the one Windows has.
pub fn accent(cfg: &Config, windows: (f32, f32, f32)) -> (f32, f32, f32) {
    Scheme::of(cfg).accent.unwrap_or(windows)
}

/// The water of the live wallpaper in the colours in use: the dark one, or the bright one.
pub fn water(cfg: &Config, bright: bool) -> &'static Water {
    &Scheme::of(cfg).water[bright as usize]
}

/// How wide and how tall the picture of a set of colours is (see `swatch`), in points.
pub const SWATCH: (f32, f32) = (134.0, 84.0);

/// A small picture of one of the sets of colours, for the settings window to show: the
/// island above and the dock with three apps below, on the set's water, as on the logo.
/// `unit`: how many pixels a point is. `system_dark` and `windows`: what Windows is set to,
/// for the set that goes by it.
pub fn swatch(index: usize, unit: f32, system_dark: bool, windows: (f32, f32, f32)) -> Canvas {
    let mut cfg = Config::default();
    choose(index, &mut cfg);
    let theme = build(&cfg, system_dark, windows, 1.0);
    let scheme = Scheme::of(&cfg);
    let (w, h) = ((SWATCH.0 * unit).round(), (SWATCH.1 * unit).round());
    let mut canvas = Canvas::new(w as u32, h as u32);
    let solid = |c: (f32, f32, f32), a: f32| -> Rgba { [c.0, c.1, c.2, a] };
    let plain = |c: Color| -> Rgba { [c.R as f32 / 255.0, c.G as f32 / 255.0, c.B as f32 / 255.0, c.A as f32 / 255.0] };

    // The water: the bright one under light glass, the dark one under dark glass. The
    // light on it stays well inside the picture's round corners.
    let water = &scheme.water[!theme.dark as usize];
    canvas.sheet(0.0, 0.0, w, h, 10.0 * unit, solid(water.top, 1.0), solid(water.bottom, 1.0));
    for (x, y, reach) in [(0.27, 0.42, 0.25), (0.71, 0.56, 0.29), (0.50, 0.30, 0.19)] {
        canvas.glow(w * x, h * y, w * reach, solid(water.light, water.layers[0] * 1.5));
    }

    // A piece of glass: its colour, the light from above on it, and its bright edge all the
    // way round, brightest along the top.
    let mut glass = |material: &Material, x: f32, y: f32, wide: f32, tall: f32, round: f32| {
        canvas.sheet(x, y, wide, tall, round, plain(material.tint), plain(material.tint));
        canvas.sheet(x, y, wide, tall, round, plain(material.sheen_top), plain(material.sheen_bottom));
        let (edge, faint) = (fade(plain(material.rim), 0.85), fade(plain(material.rim), 0.30));
        let (left, right, top, bottom) = (x + 0.5 * unit, x + wide - 0.5 * unit, y + 0.5 * unit, y + tall - 0.5 * unit);
        let turn = (round - 0.5 * unit).max(0.0);
        canvas.line(left + turn, top, right - turn, top, unit, edge);
        canvas.line(left + turn, bottom, right - turn, bottom, unit, faint);
        canvas.line(left, top + turn, left, bottom - turn, unit, faint);
        canvas.line(right, top + turn, right, bottom - turn, unit, faint);
        // The four corners, clockwise from the top left one.
        canvas.arc(left + turn, top + turn, turn, unit, 0.75, 0.25, faint, edge);
        canvas.arc(right - turn, top + turn, turn, unit, 0.0, 0.25, edge, faint);
        canvas.arc(right - turn, bottom - turn, turn, unit, 0.25, 0.25, faint, faint);
        canvas.arc(left + turn, bottom - turn, turn, unit, 0.5, 0.25, faint, faint);
    };
    let (island_w, island_h) = (46.0 * unit, 12.0 * unit);
    glass(&theme.island, (w - island_w) * 0.5, 9.0 * unit, island_w, island_h, island_h * 0.5);
    let (dock_w, dock_h) = (82.0 * unit, 24.0 * unit);
    let dock_top = h - dock_h - 9.0 * unit;
    glass(&theme.dock, (w - dock_w) * 0.5, dock_top, dock_w, dock_h, dock_h * cfg.corner_radius.clamp(0.0, 50.0) / 100.0);
    // The apps: the first in the accent colour, the others in what is written on the glass.
    let ink: Rgba = if theme.dark { [1.0, 1.0, 1.0, 1.0] } else { [0.0, 0.0, 0.0, 1.0] };
    for (place, colour) in [(-1.0, solid(theme.accent, 1.0)), (0.0, fade(ink, 0.80)), (1.0, fade(ink, 0.42))] {
        canvas.disc(w * 0.5 + place * 24.0 * unit, dock_top + dock_h * 0.5, 6.5 * unit, colour);
    }
    canvas
}

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    /// The surfaces are dark, so what is written on them is light.
    pub dark: bool,
    pub accent: (f32, f32, f32),
    pub dock: Material,
    pub island: Material,
    /// Menus, name tags, window previews: small things that must read clearly over anything.
    pub bubble: Material,
    /// The app tray.
    pub sheet: Material,
    pub text: D2D1_COLOR_F,
    pub text_dim: D2D1_COLOR_F,
    /// The drop of light under the cursor: top of its fill, bottom of its fill, its edge.
    pub bead: (Color, Color, Color),
    /// A control's resting fill (a slider's track, a button's back).
    pub track: Color,
    pub divider: Color,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn white(a: f32) -> Color {
    rgba(1.0, 1.0, 1.0, a)
}

fn black(a: f32) -> Color {
    rgba(0.0, 0.0, 0.0, a)
}

/// Builds the theme. `system_dark`: Windows is in dark mode. `scale`: the display scale
/// (blur and shadow are in physical pixels).
pub fn build(cfg: &Config, system_dark: bool, accent: (f32, f32, f32), scale: f32) -> Theme {
    let style = Style::of(cfg);
    let scheme = Scheme::of(cfg);
    let accent = scheme.accent.unwrap_or(accent);
    let dark = style == Style::Ink || cfg.is_dark_forced().unwrap_or(system_dark);
    let t = cfg.tint_opacity.clamp(0.0, 1.0);
    let blurring = style != Style::Solid && cfg.blur_radius >= 1.0;

    // The glass's own colour, pulled towards the accent colour.
    let base = match (style, dark) {
        (Style::Ink, _) => (0.024, 0.024, 0.031),
        (Style::Solid, true) => (0.133, 0.137, 0.157),
        (_, true) => (0.094, 0.102, 0.125),
        (Style::Solid, false) => (0.957, 0.961, 0.973),
        (_, false) => (0.973, 0.976, 0.988),
    };
    // (One of the app's own sets of colours has glass of its own colour. Ink stays close
    // to black: half of its own, half of the set's.)
    let base = match scheme.glass {
        Some((night, day)) => {
            let own = if dark { night } else { day };
            let share = if style == Style::Ink { 0.5 } else { 1.0 };
            (lerp(base.0, own.0, share), lerp(base.1, own.1, share), lerp(base.2, own.2, share))
        }
        None => base,
    };
    let pull = cfg.accent_tint.clamp(0.0, 1.0) * if dark { 0.55 } else { 0.38 };
    let glass = (lerp(base.0, accent.0, pull), lerp(base.1, accent.1, pull), lerp(base.2, accent.2, pull));

    // How much of it covers what is behind, at the "opacity" setting's two ends.
    // Light glass has dark writing on it, so over a dark picture it has to cover more than
    // dark glass does to stay readable.
    let (clear, dense, blur_gain, saturation) = match (style, dark) {
        (Style::Liquid, true) => (0.10, 0.62, 1.0, 1.75),
        (Style::Liquid, false) => (0.32, 0.80, 1.0, 1.75),
        (Style::Frosted, true) => (0.40, 0.88, 1.5, 1.3),
        (Style::Frosted, false) => (0.56, 0.92, 1.5, 1.3),
        (Style::Ink, _) => (0.60, 0.95, 1.0, 1.4),
        (Style::Solid, _) => (0.97, 0.97, 0.0, 1.0),
    };
    let mut cover = lerp(clear, dense, t);
    if !blurring {
        // Nothing softens what shows through: make the glass more solid so text stays readable.
        cover = cover.max(0.82);
    }
    let blur = blurring.then_some(cfg.blur_radius * blur_gain * scale);

    let (sheen_top, sheen_bottom, rim, outline, shadow) = match (style, dark) {
        (Style::Liquid, true) => (white(0.15), black(0.12), white(0.62), black(0.44), 0.52),
        (Style::Liquid, false) => (white(0.55), white(0.12), white(0.96), black(0.15), 0.26),
        (Style::Frosted, true) => (white(0.07), black(0.06), white(0.28), black(0.42), 0.46),
        (Style::Frosted, false) => (white(0.34), white(0.04), white(0.80), black(0.14), 0.22),
        (Style::Ink, _) => (white(0.10), black(0.10), white(0.32), black(0.60), 0.58),
        (Style::Solid, true) => (white(0.05), black(0.04), white(0.18), black(0.50), 0.44),
        (Style::Solid, false) => (white(0.40), white(0.0), white(0.85), black(0.17), 0.22),
    };
    let shadow = if cfg.shadow { shadow } else { 0.0 };
    // (The bright edge in the colour the set gives it, as bright as the material has it.)
    let rim = rgba(scheme.rim.0, scheme.rim.1, scheme.rim.2, rim.A as f32 / 255.0);
    let piece = |extra: f32, blur_scale: f32, reach: f32| Material {
        tint: rgba(glass.0, glass.1, glass.2, (cover + extra).clamp(0.0, 0.985)),
        blur: blur.map(|b| b * blur_scale),
        saturation,
        sheen_top,
        sheen_bottom,
        rim,
        outline,
        shadow,
        shadow_reach: (reach * scale).round(),
    };

    Theme {
        dark,
        accent,
        dock: piece(0.0, 1.0, 22.0),
        island: piece(0.20, 1.0, 20.0),
        bubble: piece(0.36, 1.1, 18.0),
        // The app tray is made of the same glass as the dock (a little denser, for the
        // writing on it), so that it reads as part of it.
        sheet: piece(0.10, 1.0, 22.0),
        text: if dark { color(1.0, 1.0, 1.0, 0.95) } else { color(0.0, 0.0, 0.0, 0.88) },
        text_dim: if dark { color(1.0, 1.0, 1.0, 0.56) } else { color(0.0, 0.0, 0.0, 0.52) },
        bead: if dark { (white(0.27), white(0.09), white(0.36)) } else { (black(0.045), black(0.11), white(0.78)) },
        track: if dark { white(0.16) } else { black(0.12) },
        divider: if dark { white(0.15) } else { black(0.12) },
    }
}

impl Theme {
    pub fn accent_color(&self, alpha: f32) -> Color {
        rgba(self.accent.0, self.accent.1, self.accent.2, alpha)
    }

    /// Text in the accent colour, light enough to read on dark glass and deep enough on light.
    pub fn accent_text(&self) -> D2D1_COLOR_F {
        let (r, g, b) = self.accent;
        if self.dark {
            color(lerp(r, 1.0, 0.45), lerp(g, 1.0, 0.45), lerp(b, 1.0, 0.45), 1.0)
        } else {
            color(r * 0.75, g * 0.75, b * 0.75, 1.0)
        }
    }

    /// The neutral tone on this glass (white on dark, black on light) at `alpha`.
    pub fn tone(&self, alpha: f32) -> Color {
        if self.dark {
            white(alpha)
        } else {
            black(alpha)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCENT: (f32, f32, f32) = (0.0, 0.47, 0.84);

    fn with(style: f32, f: impl FnOnce(&mut Config)) -> Theme {
        let mut c = Config::default();
        c.style = style;
        f(&mut c);
        build(&c, true, ACCENT, 1.0)
    }

    #[test]
    fn ink_is_dark_whatever_the_mode() {
        let t = with(2.0, |c| c.theme = 2.0);
        assert!(t.dark);
        assert!(t.dock.tint.R < 40);
        let light = with(0.0, |c| c.theme = 2.0);
        assert!(!light.dark && light.dock.tint.R > 200);
    }

    #[test]
    fn solid_never_blurs_and_no_blur_means_solid_enough_to_read() {
        assert!(with(3.0, |_| {}).dock.blur.is_none());
        let plain = with(0.0, |c| c.blur_radius = 0.0);
        assert!(plain.dock.blur.is_none());
        assert!(plain.dock.tint.A >= 205);
        assert!(with(0.0, |_| {}).dock.blur.is_some());
    }

    #[test]
    fn menus_are_denser_than_the_dock_and_opacity_moves_the_tint() {
        let t = with(0.0, |_| {});
        assert!(t.bubble.tint.A > t.dock.tint.A);
        assert!(t.island.tint.A > t.dock.tint.A);
        let clear = with(0.0, |c| c.tint_opacity = 0.0);
        let dense = with(0.0, |c| c.tint_opacity = 1.0);
        assert!(clear.dock.tint.A < t.dock.tint.A && t.dock.tint.A < dense.dock.tint.A);
    }

    #[test]
    fn shadow_can_be_switched_off_and_scales_with_the_display() {
        assert_eq!(with(0.0, |c| c.shadow = false).dock.shadow, 0.0);
        let mut c = Config::default();
        c.style = 0.0;
        let big = build(&c, true, ACCENT, 2.0);
        assert_eq!(big.dock.shadow_reach, 44.0);
        assert_eq!(big.dock.blur, Some(52.0));
    }

    #[test]
    fn accent_tint_pulls_the_glass_towards_the_accent() {
        let none = with(0.0, |c| c.accent_tint = 0.0);
        let full = with(0.0, |c| c.accent_tint = 1.0);
        // The accent is blue: more blue, no more red.
        assert!(full.dock.tint.B > none.dock.tint.B);
        assert!(full.dock.tint.R <= none.dock.tint.R);
    }

    /// How far apart two colours are in brightness, as the eye has it: from 1 (the same) to
    /// 21 (black and white).
    fn contrast(a: (f32, f32, f32), b: (f32, f32, f32)) -> f32 {
        let part = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        let bright = |c: (f32, f32, f32)| 0.2126 * part(c.0) + 0.7152 * part(c.1) + 0.0722 * part(c.2);
        let (a, b) = (bright(a), bright(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn every_set_of_colours_can_be_read() {
        let (white, black) = ((1.0, 1.0, 1.0), (0.0, 0.0, 0.0));
        assert_eq!(contrast(white, black).round(), 21.0);
        for (index, scheme) in SCHEMES.iter().enumerate() {
            // White is written on the accent colour: today's date, the choice that is taken.
            if let Some(accent) = scheme.accent {
                assert!(contrast(accent, white) >= 3.0, "set {index}: white on its accent colour");
            }
            // White on its dark glass, black on its light glass.
            if let Some((night, day)) = scheme.glass {
                assert!(contrast(night, white) >= 12.0, "set {index}: its dark glass");
                assert!(contrast(day, black) >= 14.0, "set {index}: its light glass");
            }
            // The large clock is white and lies bare on the water.
            for water in &scheme.water {
                assert!(contrast(water.top, white) >= 3.0, "set {index}: its water");
                assert!(contrast(water.bottom, white) >= contrast(water.top, white), "set {index}: its water is deeper at the bottom");
            }
        }
    }

    #[test]
    fn the_settings_name_every_set_of_colours() {
        let p = crate::config::PARAMS.iter().find(|p| p.key == "scheme").expect("the setting");
        let crate::config::Kind::Choice(names) = p.kind else { panic!("the sets are a choice") };
        assert_eq!(names.len(), SCHEMES.len());
        // No two are the same set under two names.
        for (index, a) in SCHEMES.iter().enumerate() {
            assert!(SCHEMES[..index].iter().all(|b| b.accent != a.accent && b.water != a.water), "set {index}");
        }
        // Past the last one there is none: the last is taken.
        let mut c = Config::default();
        c.scheme = 99.0;
        assert_eq!(Scheme::of(&c), &SCHEMES[SCHEMES.len() - 1]);
    }

    #[test]
    fn choosing_a_set_sets_the_glass_it_was_made_for_and_nothing_else() {
        let mut c = Config::default();
        (c.icon_size, c.shadow, c.wallpaper) = (31.0, false, 3.0);
        choose(4, &mut c);
        assert_eq!((c.scheme, c.style, c.theme, c.corner_radius), (4.0, 1.0, 1.0, 38.0));
        assert_eq!((c.icon_size, c.shadow, c.wallpaper), (31.0, false, 3.0));
        // There is no such set: nothing happens.
        let before = c.clone();
        choose(SCHEMES.len(), &mut c);
        assert_eq!(c, before);
        // The first is how the app looks when it has never been set up.
        choose(0, &mut c);
        (c.icon_size, c.shadow, c.wallpaper) = (Config::default().icon_size, true, 0.0);
        assert_eq!(c, Config::default());
    }

    #[test]
    fn a_set_gives_its_colours_to_the_accent_the_glass_and_its_edge() {
        // Without one: Windows' accent colour, and a white edge.
        let plain = with(0.0, |_| {});
        assert_eq!(plain.accent, ACCENT);
        assert_eq!((plain.dock.rim.R, plain.dock.rim.G, plain.dock.rim.B), (255, 255, 255));
        assert_eq!(accent(&Config::default(), ACCENT), ACCENT);
        // Lanterns: red, on glass that is warm and nearly black, with a gold edge.
        let lanterns = with(0.0, |c| choose(6, c));
        assert_eq!(lanterns.accent, SCHEMES[6].accent.unwrap());
        assert!(lanterns.dark && lanterns.dock.tint.R > lanterns.dock.tint.B && lanterns.dock.tint.R < 60);
        assert!(lanterns.dock.rim.R > lanterns.dock.rim.B);
        assert_eq!(lanterns.dock.rim.A, with(2.0, |_| {}).dock.rim.A);
        // Light or dark stays the user's to say: the same set, with its other glass.
        let by_day = with(0.0, |c| {
            choose(1, c);
            c.theme = 2.0;
        });
        assert!(!by_day.dark && by_day.dock.tint.B > 230);
        // The water goes with the set.
        let mut c = Config::default();
        assert_eq!(water(&c, false), &SCHEMES[0].water[0]);
        choose(8, &mut c);
        assert_eq!(water(&c, true), &SCHEMES[8].water[1]);
    }

    #[test]
    fn a_picture_of_every_set_can_be_painted() {
        let pictures: Vec<Canvas> = (0..SCHEMES.len()).map(|index| swatch(index, 2.0, true, ACCENT)).collect();
        for (index, picture) in pictures.iter().enumerate() {
            assert_eq!((picture.w, picture.h), (268, 168));
            let alpha = |x: u32, y: u32| picture.bgra[((y * picture.w + x) * 4 + 3) as usize];
            // Solid in the middle, clear at the very corner (the picture is rounded).
            assert_eq!(alpha(134, 84), 255, "set {index}");
            assert_eq!(alpha(0, 0), 0, "set {index}");
            assert!(pictures[..index].iter().all(|other| other.bgra != picture.bgra), "set {index} looks like another");
        }
    }

    /// Not a check of anything: saves the picture of every set of colours, large, so that
    /// they can be looked at.
    /// Run by hand: BEN_OUT=<folder> cargo test themes_for_looking_at -- --ignored
    /// (BEN_LIGHT=1: as when Windows is set to light.)
    #[test]
    #[ignore]
    fn themes_for_looking_at() {
        let out = std::path::PathBuf::from(std::env::var("BEN_OUT").expect("BEN_OUT names a folder"));
        for index in 0..SCHEMES.len() {
            let picture = swatch(index, 3.0, std::env::var("BEN_LIGHT").is_err(), ACCENT);
            crate::camera::save_picture(&out.join(format!("theme{index}.bmp")), picture.w as i32, picture.h as i32, &picture.bgra);
        }
    }
}
