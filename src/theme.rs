//! Themes: what the glass is made of and what is written on it.
//!
//! One function turns the settings into every colour the interface uses, so
//! nothing else in the app holds a colour of its own. A theme is a material
//! (liquid, frosted, ink, solid), a mode (dark or light), and the Windows
//! accent colour.
//!
//!   liquid   clear glass: little tint, strong colours behind, a bright rim
//!   frosted  more blur and more tint: calmer, the easiest to read
//!   ink      smoked black glass whatever the mode, like a phone's island
//!   solid    no blur at all: the lightest on the machine

use crate::config::Config;
use crate::gfx::color;
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
        sheet: piece(0.30, 1.4, 28.0),
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
}
