//! Text for the top bar, the launcher and the previews: a line of text is
//! painted once into a surface and shown by a sprite; it is painted again
//! only when the words, the colour or the font change.

use crate::gfx::Gfx;
use crate::glass::{Glass, Sprite};
use crate::util::wide;
use windows::core::{Result, PCWSTR};
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::DirectWrite::*;
use windows::UI::Composition::CompositionDrawingSurface;

pub const UI_FONT: &str = "Segoe UI Variable Text";
/// Windows 11's symbol font (battery, volume, Wi-Fi, search...).
pub const ICON_FONT: &str = "Segoe Fluent Icons";

pub fn format(gfx: &Gfx, family: &str, size: f32, weight: DWRITE_FONT_WEIGHT) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = gfx.dwrite.CreateTextFormat(PCWSTR(wide(family).as_ptr()), None, weight, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, PCWSTR(wide("").as_ptr()))?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(format)
    }
}

/// Lays out one line. If `max_width` is given and the text is wider, it is shortened and
/// ends in an ellipsis. Returns the layout and its size.
pub fn fit(gfx: &Gfx, format: &IDWriteTextFormat, text: &str, max_width: Option<f32>) -> Result<(IDWriteTextLayout, f32, f32)> {
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    loop {
        let shown: String = if keep < chars.len() { chars[..keep].iter().collect::<String>().trim_end().to_string() + "…" } else { text.to_string() };
        let utf16: Vec<u16> = shown.encode_utf16().collect();
        unsafe {
            let layout = gfx.dwrite.CreateTextLayout(&utf16, format, 8000.0, 400.0)?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            let fits = max_width.map(|w| m.width <= w).unwrap_or(true);
            if fits || keep <= 1 {
                return Ok((layout, m.width.ceil().max(1.0), m.height.ceil().max(1.0)));
            }
            // Cut in proportion to how much too wide it is, and at least one character.
            let target = (keep as f32 * max_width.unwrap_or(m.width) / m.width) as usize;
            keep = target.min(keep - 1).max(1);
        }
    }
}

/// A sprite showing one line of text.
pub struct Line {
    sprite: Sprite,
    _surface: Option<CompositionDrawingSurface>,
    shown: Option<(String, [u32; 4], u32)>,
    pub w: f32,
    pub h: f32,
}

impl Line {
    pub fn new(sprite: Sprite) -> Self {
        Line { sprite, _surface: None, shown: None, w: 0.0, h: 0.0 }
    }

    /// Shows `text`. Repaints only if the text, colour or available width changed.
    pub fn set(&mut self, glass: &Glass, gfx: &Gfx, format: &IDWriteTextFormat, text: &str, color: D2D1_COLOR_F, max_width: Option<f32>) -> Result<()> {
        let key = (text.to_string(), [color.r.to_bits(), color.g.to_bits(), color.b.to_bits(), color.a.to_bits()], max_width.unwrap_or(0.0).to_bits());
        if self.shown.as_ref() == Some(&key) {
            return Ok(());
        }
        if text.is_empty() {
            self.w = 0.0;
            self.sprite.set_opacity(0.0)?;
            self.shown = Some(key);
            return Ok(());
        }
        let (layout, w, h) = fit(gfx, format, text, max_width)?;
        let surface = glass.surface_from_text(&layout, w, h, color)?;
        self.sprite.set_surface(&surface)?;
        self.sprite.set_opacity(1.0)?;
        self._surface = Some(surface);
        self.shown = Some(key);
        (self.w, self.h) = (w, h);
        Ok(())
    }

    /// Forgets what is painted, so the next `set` paints again (after a font or device change).
    pub fn forget(&mut self) {
        self.shown = None;
    }

    pub fn place(&mut self, x: f32, y: f32) -> Result<()> {
        self.sprite.place(x.round(), y.round(), self.w, self.h)
    }

    pub fn set_opacity(&mut self, opacity: f32) -> Result<()> {
        self.sprite.set_opacity(opacity)
    }
}
