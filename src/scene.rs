//! The scene: everything on screen is a tree of Windows.UI.Composition
//! visuals, and this file is the only place that talks to that API.
//!
//! The app never draws frames. It builds visuals once, then moves, resizes
//! and fades them; the desktop compositor does the per-frame GPU work, and
//! everything changed in one pass lands in the same frame. Direct2D is used
//! only to paint an icon or a line of text once, into a surface.
//!
//! The one material everything is made of is `Glass`, a piece of liquid
//! glass. From the back:
//!
//! ```text
//! glass
//! ├── shadow    soft, outside the shape only (it must not show through the glass)
//! ├── body      clipped to the rounded shape
//! │   ├── backdrop  live blur of whatever is behind the window, colours pushed
//! │   ├── tint      the glass's own colour
//! │   ├── sheen     light from above: bright at the top, fading out
//! │   └── inner     content that stays inside the shape (the drop of light)
//! ├── rim       the bright edge, strongest top-left, and a dark hairline around it
//! └── outer     content that may stick out (magnified icons)
//! ```

use crate::gfx::{color, rect, Gfx};
use crate::shell::IconPixels;
use std::cell::RefCell;
use std::collections::HashMap;
use windows::core::{implement, Interface, Ref, Result, GUID, HSTRING, PCWSTR};
use windows::Foundation::{IPropertyValue, PropertyValue, Size, TimeSpan};
use windows::Graphics::DirectX::{DirectXAlphaMode, DirectXPixelFormat};
use windows::Graphics::Effects::{IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectSource_Impl, IGraphicsEffect_Impl};
use windows::Graphics::{IGeometrySource2D, IGeometrySource2D_Impl};
use windows::System::DispatcherQueueController;
use windows::Win32::Foundation::{E_BOUNDS, E_INVALIDARG, E_NOTIMPL, HWND, POINT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::System::WinRT::Composition::{ICompositionDrawingSurfaceInterop, ICompositionGraphicsDeviceInterop, ICompositorDesktopInterop, ICompositorInterop};
use windows::Win32::System::WinRT::Graphics::Direct2D::{
    IGeometrySource2DInterop, IGeometrySource2DInterop_Impl, IGraphicsEffectD2D1Interop, IGraphicsEffectD2D1Interop_Impl, GRAPHICS_EFFECT_PROPERTY_MAPPING, GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT,
};
use windows::Win32::System::WinRT::{CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT};
use windows::UI::Color;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::*;
use windows_numerics::{Matrix3x2, Matrix4x4, Vector2, Vector3};

const FULL: Vector2 = Vector2 { X: 1.0, Y: 1.0 };

/// A colour from 0..1 components (not premultiplied).
pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color { A: byte(a), R: byte(r), G: byte(g), B: byte(b) }
}

/// The same colour, `factor` times as opaque.
pub fn faded(c: Color, factor: f32) -> Color {
    Color { A: (c.A as f32 * factor.clamp(0.0, 1.0)).round() as u8, ..c }
}

/// What a piece of glass looks like. Built by `theme`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// The glass's own colour and how much of it covers what is behind.
    pub tint: Color,
    /// Blur of what lies behind, as a standard deviation in pixels. `None`: no blur.
    pub blur: Option<f32>,
    /// How much the colours behind are pushed (1 = as they are).
    pub saturation: f32,
    /// Light from above: the colour at the top edge and at the bottom edge.
    pub sheen_top: Color,
    pub sheen_bottom: Color,
    /// The bright edge and the dark hairline around it.
    pub rim: Color,
    pub outline: Color,
    /// How dark the shadow is, 0 to 1, and how far it reaches in pixels.
    pub shadow: f32,
    pub shadow_reach: f32,
}

/// Halves a picture in both directions by averaging each 2x2 block (BGRA, top row first).
pub fn halve(width: u32, height: u32, bgra: &[u8]) -> (u32, u32, Vec<u8>) {
    let (w, h) = (width as usize, height as usize);
    let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; nw * nh * 4];
    for y in 0..nh {
        // At an odd edge (or a one-pixel-wide picture) the block is clamped to what exists.
        let (y0, y1) = ((y * 2).min(h - 1), (y * 2 + 1).min(h - 1));
        for x in 0..nw {
            let (x0, x1) = ((x * 2).min(w - 1), (x * 2 + 1).min(w - 1));
            let at = |px: usize, py: usize| (py * w + px) * 4;
            let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
            let o = (y * nw + x) * 4;
            for ch in 0..4 {
                out[o + ch] = ((bgra[a + ch] as u32 + bgra[b + ch] as u32 + bgra[c + ch] as u32 + bgra[d + ch] as u32 + 2) / 4) as u8;
            }
        }
    }
    (nw as u32, nh as u32, out)
}

/// The picture a soft shadow is stretched from: a rounded square's shadow, premultiplied
/// black. Returns (side, inset, pixels); the corners of the picture, `inset` pixels square,
/// are shown as they are and the strips between them are stretched.
///
/// The shadow is only painted outside the shape, so it never darkens the glass lying on it,
/// and it is stronger below than above, as if lit from the top.
pub fn shadow_tile(radius: u32, reach: u32) -> (u32, u32, Vec<u8>) {
    let (r, reach_f) = (radius as f32, reach.max(1) as f32);
    // Two stretchable pixels in the middle.
    let half = r + 1.0;
    let inset = radius + reach + 1;
    let side = inset * 2;
    let centre = side as f32 * 0.5;
    let mut out = vec![0u8; (side * side * 4) as usize];
    for y in 0..side {
        for x in 0..side {
            let (px, py) = (x as f32 + 0.5 - centre, y as f32 + 0.5 - centre);
            // Distance to the rounded square's edge: negative inside.
            let (dx, dy) = (px.abs() - (half - r), py.abs() - (half - r));
            let d = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - r;
            if d < -2.5 || d >= reach_f {
                continue;
            }
            // Full strength at the edge, tucked two pixels under the shape, falling off smoothly.
            let tuck = (d + 2.5).clamp(0.0, 1.0);
            let t = (d.max(0.0) / reach_f).clamp(0.0, 1.0);
            let fall = (1.0 - t).powi(3);
            // Lit from above: weaker over the shape, stronger under it.
            let below = (py / (half + reach_f)).clamp(-1.0, 1.0);
            let weight = 0.62 + 0.38 * below;
            let alpha = (tuck * fall * weight * 255.0).round().clamp(0.0, 255.0) as u8;
            out[((y * side + x) * 4 + 3) as usize] = alpha;
        }
    }
    (side, inset, out)
}

pub struct Scene {
    _controller: DispatcherQueueController,
    compositor: Compositor,
    graphics: CompositionGraphicsDevice,
    /// Shadow pictures, painted once per corner radius and reach.
    shadows: RefCell<HashMap<(u32, u32), (CompositionSurfaceBrush, f32)>>,
}

impl Scene {
    pub fn new(gfx: &Gfx) -> Result<Self> {
        unsafe {
            // Windows.UI.Composition needs a dispatcher queue on the thread that owns the compositor.
            let controller = CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            })?;
            let compositor = Compositor::new()?;
            let interop: ICompositorInterop = compositor.cast()?;
            let graphics = interop.CreateGraphicsDevice(&gfx.d2d)?;
            Ok(Scene { _controller: controller, compositor, graphics, shadows: RefCell::new(HashMap::new()) })
        }
    }

    /// Switches to a new GPU device after the old one was lost. Surfaces painted before this
    /// are empty afterwards and must be painted again.
    pub fn set_rendering_device(&self, gfx: &Gfx) -> Result<()> {
        self.shadows.borrow_mut().clear();
        unsafe {
            let interop: ICompositionGraphicsDeviceInterop = self.graphics.cast()?;
            interop.SetRenderingDevice(&gfx.d2d)
        }
    }

    /// Hands everything changed so far to the desktop compositor as one batch.
    pub fn commit(&self) {
        // Changes are committed when the thread next pumps messages; asking explicitly keeps
        // that from being put off.
        let _ = self.compositor.RequestCommitAsync();
    }

    /// Lets go of memory the compositor keeps for surfaces that are no longer shown.
    pub fn trim(&self) {
        let _ = self.graphics.Trim();
    }

    /// The visual tree of one window. Its windows have no surface of their own
    /// (WS_EX_NOREDIRECTIONBITMAP): what the stage holds is all there is to see.
    pub fn stage(&self, hwnd: HWND) -> Result<Stage> {
        unsafe {
            let desktop: ICompositorDesktopInterop = self.compositor.cast()?;
            let target = desktop.CreateDesktopWindowTarget(hwnd, false)?;
            let root = self.compositor.CreateContainerVisual()?;
            root.SetRelativeSizeAdjustment(FULL)?;
            target.SetRoot(&root)?;
            Ok(Stage { _target: target, root, last_opacity: f32::NAN, last_offset: (f32::NAN, f32::NAN) })
        }
    }

    // ------------------------------------------------------------- surfaces

    fn paint(&self, width: u32, height: u32, draw: impl FnOnce(&ID2D1DeviceContext) -> Result<()>) -> Result<CompositionDrawingSurface> {
        let size = Size { Width: width.max(1) as f32, Height: height.max(1) as f32 };
        let surface = self.graphics.CreateDrawingSurface(size, DirectXPixelFormat::B8G8R8A8UIntNormalized, DirectXAlphaMode::Premultiplied)?;
        unsafe {
            let interop: ICompositionDrawingSurfaceInterop = surface.cast()?;
            // The surface lives inside a larger shared texture; `offset` is where this one starts.
            let mut offset = POINT::default();
            let dc: ID2D1DeviceContext = interop.BeginDraw(None, &mut offset)?;
            dc.SetDpi(96.0, 96.0);
            dc.SetTransform(&Matrix3x2::translation(offset.x as f32, offset.y as f32));
            dc.Clear(Some(&color(0.0, 0.0, 0.0, 0.0)));
            let drawn = draw(&dc);
            interop.EndDraw()?;
            drawn?;
        }
        Ok(surface)
    }

    /// A surface holding an icon's pixels.
    pub fn surface_from_pixels(&self, px: &IconPixels) -> Result<CompositionDrawingSurface> {
        self.surface_from_bgra(px.size, px.size, &px.bgra)
    }

    /// A surface holding a picture given as premultiplied BGRA pixels, top row first.
    pub fn surface_from_bgra(&self, width: u32, height: u32, bgra: &[u8]) -> Result<CompositionDrawingSurface> {
        if width == 0 || height == 0 || bgra.len() < (width as usize) * (height as usize) * 4 {
            return Err(E_INVALIDARG.into());
        }
        self.paint(width, height, |dc| unsafe {
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            };
            let bitmap = dc.CreateBitmap(D2D_SIZE_U { width, height }, Some(bgra.as_ptr() as *const _), width * 4, &props)?;
            dc.DrawBitmap(&bitmap, Some(&rect(0.0, 0.0, width as f32, height as f32)), 1.0, D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR, None, None);
            Ok(())
        })
    }

    /// A surface holding one line of text in the given colour.
    pub fn surface_from_text(&self, layout: &IDWriteTextLayout, width: f32, height: f32, text_color: D2D1_COLOR_F) -> Result<CompositionDrawingSurface> {
        self.paint(width.ceil() as u32, height.ceil() as u32, |dc| unsafe {
            // ClearType needs an opaque background; on a transparent surface use greyscale smoothing.
            dc.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = dc.CreateSolidColorBrush(&text_color, None)?;
            dc.DrawTextLayout(Vector2 { X: 0.0, Y: 0.0 }, layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            Ok(())
        })
    }

    /// One size of a window's picture, as a surface (see `Level`). The effect keeps the
    /// picture at full size and at a few halvings of it, and every slice reads from the copy
    /// closest to the size it is shown at. Without this a picture squeezed to a fraction of
    /// its width sparkles, because most of its pixels are simply skipped. (The compositor's
    /// own mip-mapped surface cannot be used for this: a surface brush refuses it.)
    pub fn picture_level(&self, width: u32, height: u32, bgra: &[u8]) -> Result<Level> {
        Ok(Level { surface: self.surface_from_bgra(width, height, bgra)?.cast()?, width: width as f32, height: height as f32 })
    }

    /// The brush a shadow is drawn with, and the width of its fixed corners.
    fn shadow_brush(&self, radius: u32, reach: u32) -> Result<(CompositionSurfaceBrush, f32)> {
        if let Some(found) = self.shadows.borrow().get(&(radius, reach)) {
            return Ok(found.clone());
        }
        let (side, inset, pixels) = shadow_tile(radius, reach);
        let surface = self.surface_from_bgra(side, side, &pixels)?;
        let brush = self.compositor.CreateSurfaceBrushWithSurface(&surface)?;
        brush.SetStretch(CompositionStretch::Fill)?;
        let made = (brush, inset as f32);
        self.shadows.borrow_mut().insert((radius, reach), made.clone());
        Ok(made)
    }

    // -------------------------------------------------------------- visuals

    /// A group of visuals that moves, fades and scales as one.
    pub fn layer_in(&self, parent: &VisualCollection) -> Result<Layer> {
        let visual = self.compositor.CreateContainerVisual()?;
        parent.InsertAtTop(&visual)?;
        Ok(Layer { visual, parent: parent.clone(), last_offset: (f32::NAN, f32::NAN), last_opacity: f32::NAN, last_scale: [f32::NAN; 4] })
    }

    /// A rectangle that shows a surface (an icon, a line of text).
    pub fn sprite_in(&self, parent: &VisualCollection) -> Result<Sprite> {
        let visual = self.compositor.CreateSpriteVisual()?;
        let brush = self.compositor.CreateSurfaceBrush()?;
        brush.SetStretch(CompositionStretch::Fill)?;
        visual.SetBrush(&brush)?;
        parent.InsertAtTop(&visual)?;
        Ok(Sprite { visual, brush, parent: parent.clone(), last: None, last_opacity: f32::NAN })
    }

    /// A rounded rectangle filled with one colour (a running mark, a slider, a divider).
    pub fn pill_in(&self, parent: &VisualCollection) -> Result<Pill> {
        let visual = self.compositor.CreateShapeVisual()?;
        let geo = self.compositor.CreateRoundedRectangleGeometry()?;
        let brush = self.compositor.CreateColorBrush()?;
        let shape = self.compositor.CreateSpriteShapeWithGeometry(&geo)?;
        shape.SetFillBrush(&brush)?;
        visual.Shapes()?.Append(&shape)?;
        parent.InsertAtTop(&visual)?;
        Ok(Pill { visual, geo, brush, parent: parent.clone(), last: None, last_color: None, last_opacity: f32::NAN, pulsing: false })
    }

    /// A drop of light: a rounded shape lit from above with a bright edge. It marks what the
    /// cursor is on (see `motion::Glide`).
    pub fn bead_in(&self, parent: &VisualCollection) -> Result<Bead> {
        let c = &self.compositor;
        let visual = c.CreateShapeVisual()?;
        let geo = c.CreateRoundedRectangleGeometry()?;
        let fill = c.CreateLinearGradientBrush()?;
        fill.SetStartPoint(Vector2 { X: 0.0, Y: 0.0 })?;
        fill.SetEndPoint(Vector2 { X: 0.0, Y: 1.0 })?;
        let top = c.CreateColorGradientStopWithOffsetAndColor(0.0, Color::default())?;
        let bottom = c.CreateColorGradientStopWithOffsetAndColor(1.0, Color::default())?;
        fill.ColorStops()?.Append(&top)?;
        fill.ColorStops()?.Append(&bottom)?;
        let edge = c.CreateColorBrush()?;
        let shape = c.CreateSpriteShapeWithGeometry(&geo)?;
        shape.SetFillBrush(&fill)?;
        shape.SetStrokeBrush(&edge)?;
        shape.SetStrokeThickness(1.0)?;
        visual.Shapes()?.Append(&shape)?;
        parent.InsertAtTop(&visual)?;
        Ok(Bead { visual, geo, top, bottom, edge, parent: parent.clone(), last: None, last_colors: None, last_opacity: f32::NAN })
    }

    /// A piece of liquid glass.
    pub fn glass_in(&self, parent: &VisualCollection, material: &Material) -> Result<Glass> {
        let c = &self.compositor;
        let root = c.CreateContainerVisual()?;

        // Shadow: one small picture stretched to fit, corners kept as they are.
        let shadow = c.CreateSpriteVisual()?;
        let shadow_brush = c.CreateNineGridBrush()?;
        shadow_brush.SetIsCenterHollow(true)?;
        shadow.SetBrush(&shadow_brush)?;
        root.Children()?.InsertAtTop(&shadow)?;

        // Body: clipped to the rounded shape.
        let body = c.CreateContainerVisual()?;
        let clip_geo = c.CreateRoundedRectangleGeometry()?;
        body.SetClip(&c.CreateGeometricClipWithGeometry(&clip_geo)?)?;
        root.Children()?.InsertAtTop(&body)?;

        let tint_brush = c.CreateColorBrushWithColor(material.tint)?;
        let tint = c.CreateSpriteVisual()?;
        tint.SetRelativeSizeAdjustment(FULL)?;
        tint.SetBrush(&tint_brush)?;
        body.Children()?.InsertAtTop(&tint)?;

        let sheen_brush = c.CreateLinearGradientBrush()?;
        sheen_brush.SetStartPoint(Vector2 { X: 0.0, Y: 0.0 })?;
        sheen_brush.SetEndPoint(Vector2 { X: 0.0, Y: 1.0 })?;
        let sheen_stops = [
            c.CreateColorGradientStopWithOffsetAndColor(0.0, Color::default())?,
            c.CreateColorGradientStopWithOffsetAndColor(0.46, Color::default())?,
            c.CreateColorGradientStopWithOffsetAndColor(1.0, Color::default())?,
        ];
        for stop in &sheen_stops {
            sheen_brush.ColorStops()?.Append(stop)?;
        }
        let sheen = c.CreateSpriteVisual()?;
        sheen.SetRelativeSizeAdjustment(FULL)?;
        sheen.SetBrush(&sheen_brush)?;
        body.Children()?.InsertAtTop(&sheen)?;

        let inner = c.CreateContainerVisual()?;
        inner.SetRelativeSizeAdjustment(FULL)?;
        body.Children()?.InsertAtTop(&inner)?;

        // Rim: a dark hairline outside and a bright one inside it, brightest where the light
        // comes from (top-left) and where it leaves (bottom-right).
        let rim = c.CreateShapeVisual()?;
        let outline_geo = c.CreateRoundedRectangleGeometry()?;
        let rim_geo = c.CreateRoundedRectangleGeometry()?;
        let outline_brush = c.CreateColorBrushWithColor(material.outline)?;
        let rim_brush = c.CreateLinearGradientBrush()?;
        rim_brush.SetStartPoint(Vector2 { X: 0.0, Y: 0.0 })?;
        rim_brush.SetEndPoint(Vector2 { X: 1.0, Y: 1.0 })?;
        let rim_stops = [
            c.CreateColorGradientStopWithOffsetAndColor(0.0, Color::default())?,
            c.CreateColorGradientStopWithOffsetAndColor(0.38, Color::default())?,
            c.CreateColorGradientStopWithOffsetAndColor(0.62, Color::default())?,
            c.CreateColorGradientStopWithOffsetAndColor(1.0, Color::default())?,
        ];
        for stop in &rim_stops {
            rim_brush.ColorStops()?.Append(stop)?;
        }
        let outline = c.CreateSpriteShapeWithGeometry(&outline_geo)?;
        outline.SetStrokeBrush(&outline_brush)?;
        outline.SetStrokeThickness(1.0)?;
        rim.Shapes()?.Append(&outline)?;
        let bright = c.CreateSpriteShapeWithGeometry(&rim_geo)?;
        bright.SetStrokeBrush(&rim_brush)?;
        bright.SetStrokeThickness(1.0)?;
        rim.Shapes()?.Append(&bright)?;
        root.Children()?.InsertAtTop(&rim)?;

        let outer = c.CreateContainerVisual()?;
        root.Children()?.InsertAtTop(&outer)?;
        parent.InsertAtTop(&root)?;

        let mut glass = Glass {
            compositor: c.clone(),
            root,
            parent: parent.clone(),
            shadow,
            shadow_brush,
            shadow_key: None,
            shadow_inset: 0.0,
            shadow_home: None,
            shadowless: false,
            body,
            clip_geo,
            backdrop: None,
            tint_brush,
            sheen_stops,
            rim,
            outline_geo,
            rim_geo,
            outline_brush,
            rim_stops,
            inner,
            outer,
            material: *material,
            last: None,
            last_opacity: f32::NAN,
            last_scale: [f32::NAN; 4],
        };
        glass.apply(material, true);
        Ok(glass)
    }

    /// One horizontal slice of a picture, for the window effect.
    pub fn slice(&self, stage: &Stage, levels: &[Level]) -> Result<Slice> {
        let first = levels.first().ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?;
        let visual = self.compositor.CreateSpriteVisual()?;
        let brush = self.compositor.CreateSurfaceBrushWithSurface(&first.surface)?;
        // The brush draws the picture at its own size from the visual's top-left corner; scale
        // and offset then choose which rows land inside the visual.
        brush.SetStretch(CompositionStretch::None)?;
        brush.SetHorizontalAlignmentRatio(0.0)?;
        brush.SetVerticalAlignmentRatio(0.0)?;
        visual.SetBrush(&brush)?;
        let parent = stage.root.Children()?;
        parent.InsertAtTop(&visual)?;
        Ok(Slice { visual, brush, parent, last: None, hidden: false, level: 0 })
    }
}

/// The visual tree of one window.
pub struct Stage {
    _target: DesktopWindowTarget,
    root: ContainerVisual,
    last_opacity: f32,
    last_offset: (f32, f32),
}

impl Stage {
    /// Where this window's visuals go.
    pub fn content(&self) -> Result<VisualCollection> {
        self.root.Children()
    }

    pub fn set_opacity(&mut self, opacity: f32) -> Result<()> {
        if self.last_opacity == opacity {
            return Ok(());
        }
        self.last_opacity = opacity;
        self.root.SetOpacity(opacity)
    }

    /// Moves everything in the window (used to slide a whole surface out of view).
    pub fn set_offset(&mut self, x: f32, y: f32) -> Result<()> {
        if self.last_offset == (x, y) {
            return Ok(());
        }
        self.last_offset = (x, y);
        self.root.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })
    }

    /// Smooth (anti-aliased) edges for everything on the stage: for pictures that are
    /// stretched and squeezed, not for text.
    pub fn soften(&self) -> Result<()> {
        self.root.SetBorderMode(CompositionBorderMode::Soft)
    }
}

/// A group of visuals. Removes itself from the tree when dropped.
pub struct Layer {
    visual: ContainerVisual,
    parent: VisualCollection,
    last_offset: (f32, f32),
    last_opacity: f32,
    last_scale: [f32; 4],
}

impl Layer {
    pub fn content(&self) -> Result<VisualCollection> {
        self.visual.Children()
    }

    pub fn set_offset(&mut self, x: f32, y: f32) -> Result<()> {
        if self.last_offset == (x, y) {
            return Ok(());
        }
        self.last_offset = (x, y);
        self.visual.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })
    }

    pub fn set_opacity(&mut self, opacity: f32) -> Result<()> {
        if self.last_opacity == opacity {
            return Ok(());
        }
        self.last_opacity = opacity;
        self.visual.SetOpacity(opacity)
    }

    /// Scales the group about a point given in its own coordinates.
    pub fn set_scale(&mut self, sx: f32, sy: f32, cx: f32, cy: f32) -> Result<()> {
        if self.last_scale == [sx, sy, cx, cy] {
            return Ok(());
        }
        self.last_scale = [sx, sy, cx, cy];
        self.visual.SetCenterPoint(Vector3 { X: cx, Y: cy, Z: 0.0 })?;
        self.visual.SetScale(Vector3 { X: sx, Y: sy, Z: 1.0 })
    }
}

impl Drop for Layer {
    fn drop(&mut self) {
        let _ = self.parent.Remove(&self.visual);
    }
}

/// A piece of liquid glass. Removes itself from the tree when dropped.
pub struct Glass {
    compositor: Compositor,
    root: ContainerVisual,
    parent: VisualCollection,
    shadow: SpriteVisual,
    shadow_brush: CompositionNineGridBrush,
    /// Corner radius and reach of the shadow picture in use.
    shadow_key: Option<(u32, u32)>,
    shadow_inset: f32,
    /// Where the shadow lives when it is not inside the glass's own window (see `shade.rs`).
    shadow_home: Option<VisualCollection>,
    /// This piece goes without a shadow.
    shadowless: bool,
    body: ContainerVisual,
    clip_geo: CompositionRoundedRectangleGeometry,
    backdrop: Option<SpriteVisual>,
    tint_brush: CompositionColorBrush,
    sheen_stops: [CompositionColorGradientStop; 3],
    rim: ShapeVisual,
    outline_geo: CompositionRoundedRectangleGeometry,
    rim_geo: CompositionRoundedRectangleGeometry,
    outline_brush: CompositionColorBrush,
    rim_stops: [CompositionColorGradientStop; 4],
    inner: ContainerVisual,
    outer: ContainerVisual,
    material: Material,
    last: Option<[f32; 5]>,
    last_opacity: f32,
    last_scale: [f32; 4],
}

impl Glass {
    /// Content that stays inside the shape.
    pub fn inner(&self) -> Result<VisualCollection> {
        self.inner.Children()
    }

    /// Content laid over the glass that may reach past its edge.
    pub fn outer(&self) -> Result<VisualCollection> {
        self.outer.Children()
    }

    /// Changes what the glass is made of.
    pub fn set_material(&mut self, material: &Material) {
        if self.material != *material {
            self.apply(material, false);
        }
    }

    /// Moves the shadow out of the glass's own window into another one that covers the same
    /// part of the screen (see `shade.rs`), for glass whose own window is cut down to its
    /// shape. From then on the shadow follows `set_rect` only: it does not fade or scale
    /// with the glass.
    pub fn cast_shadow_on(&mut self, home: &VisualCollection) -> Result<()> {
        self.root.Children()?.Remove(&self.shadow)?;
        home.InsertAtBottom(&self.shadow)?;
        self.shadow_home = Some(home.clone());
        self.last = None;
        Ok(())
    }

    /// Goes without a shadow (when there is nowhere it could be shown whole).
    pub fn without_shadow(&mut self) {
        self.shadowless = true;
        let _ = self.shadow.SetOpacity(0.0);
    }

    fn apply(&mut self, m: &Material, first: bool) {
        let old = std::mem::replace(&mut self.material, *m);
        let _ = self.tint_brush.SetColor(m.tint);
        let _ = self.outline_brush.SetColor(m.outline);
        let clear = Color { A: 0, ..m.sheen_top };
        for (stop, c) in self.sheen_stops.iter().zip([m.sheen_top, clear, m.sheen_bottom]) {
            let _ = stop.SetColor(c);
        }
        for (stop, strength) in self.rim_stops.iter().zip([1.0, 0.30, 0.16, 0.62]) {
            let _ = stop.SetColor(faded(m.rim, strength));
        }
        let _ = self.shadow.SetOpacity(if self.shadowless { 0.0 } else { m.shadow.clamp(0.0, 1.0) });
        if first || old.blur != m.blur || old.saturation != m.saturation {
            if let Some(old) = self.backdrop.take() {
                if let Ok(children) = self.body.Children() {
                    let _ = children.Remove(&old);
                }
            }
            if let Some(sigma) = m.blur.filter(|s| *s > 0.2) {
                let made = make_backdrop_brush(&self.compositor, sigma, m.saturation).and_then(|brush| {
                    let backdrop = self.compositor.CreateSpriteVisual()?;
                    backdrop.SetRelativeSizeAdjustment(FULL)?;
                    backdrop.SetBrush(&brush)?;
                    // Underneath the tint.
                    self.body.Children()?.InsertAtBottom(&backdrop)?;
                    Ok(backdrop)
                });
                match made {
                    Ok(backdrop) => self.backdrop = Some(backdrop),
                    Err(e) => crate::log!("blur unavailable, the glass stays plain: {e}"),
                }
            }
        }
        if old.shadow_reach != m.shadow_reach {
            // Re-fitted on the next `set_rect`.
            self.shadow_key = None;
            self.last = None;
        }
    }

    /// Moves and reshapes the glass. Cheap to call every frame: nothing is sent when nothing changed.
    pub fn set_rect(&mut self, scene: &Scene, x: f32, y: f32, w: f32, h: f32, radius: f32) -> Result<()> {
        let (w, h) = (w.max(1.0), h.max(1.0));
        let radius = radius.clamp(0.0, w.min(h) * 0.5);
        if self.last == Some([x, y, w, h, radius]) {
            return Ok(());
        }
        self.last = Some([x, y, w, h, radius]);
        self.root.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })?;
        self.root.SetSize(Vector2 { X: w, Y: h })?;
        self.body.SetSize(Vector2 { X: w, Y: h })?;
        self.outer.SetSize(Vector2 { X: w, Y: h })?;
        self.clip_geo.SetSize(Vector2 { X: w, Y: h })?;
        self.clip_geo.SetCornerRadius(Vector2 { X: radius, Y: radius })?;

        // The rim's visual is one pixel larger on every side so the outer hairline is not clipped.
        self.rim.SetOffset(Vector3 { X: -1.0, Y: -1.0, Z: 0.0 })?;
        self.rim.SetSize(Vector2 { X: w + 2.0, Y: h + 2.0 })?;
        self.outline_geo.SetOffset(Vector2 { X: 0.5, Y: 0.5 })?;
        self.outline_geo.SetSize(Vector2 { X: w + 1.0, Y: h + 1.0 })?;
        self.outline_geo.SetCornerRadius(Vector2 { X: radius + 0.5, Y: radius + 0.5 })?;
        self.rim_geo.SetOffset(Vector2 { X: 1.5, Y: 1.5 })?;
        self.rim_geo.SetSize(Vector2 { X: (w - 1.0).max(0.0), Y: (h - 1.0).max(0.0) })?;
        self.rim_geo.SetCornerRadius(Vector2 { X: (radius - 0.5).max(0.0), Y: (radius - 0.5).max(0.0) })?;

        // Shadow: its corners are fixed pictures, made for exactly this corner radius. The
        // picture is hollow where the glass is, so it must sit squarely behind it: shifted,
        // or with a different radius, a dark line would show between the two.
        let reach = self.material.shadow_reach.round().max(0.0) as u32;
        if self.material.shadow > 0.0 && reach > 0 {
            let key = (radius.round() as u32, reach);
            if self.shadow_key != Some(key) {
                let (brush, inset) = scene.shadow_brush(key.0, key.1)?;
                self.shadow_brush.SetSource(&brush)?;
                self.shadow_brush.SetInsets(inset)?;
                self.shadow_key = Some(key);
                self.shadow_inset = inset;
            }
            let reach = reach as f32;
            // In another window it is placed in that window's coordinates, which are the same
            // as this one's; inside the glass, relative to the glass.
            let (ox, oy) = if self.shadow_home.is_some() { (x, y) } else { (0.0, 0.0) };
            self.shadow.SetOffset(Vector3 { X: ox - reach, Y: oy - reach, Z: 0.0 })?;
            self.shadow.SetSize(Vector2 { X: (w + 2.0 * reach).max(self.shadow_inset * 2.0), Y: (h + 2.0 * reach).max(self.shadow_inset * 2.0) })?;
        }
        Ok(())
    }

    pub fn set_opacity(&mut self, opacity: f32) -> Result<()> {
        if self.last_opacity == opacity {
            return Ok(());
        }
        self.last_opacity = opacity;
        self.root.SetOpacity(opacity)
    }

    /// Scales the whole piece about a point given in its own coordinates (0,0 = top-left).
    pub fn set_scale(&mut self, sx: f32, sy: f32, cx: f32, cy: f32) -> Result<()> {
        if self.last_scale == [sx, sy, cx, cy] {
            return Ok(());
        }
        self.last_scale = [sx, sy, cx, cy];
        self.root.SetCenterPoint(Vector3 { X: cx, Y: cy, Z: 0.0 })?;
        self.root.SetScale(Vector3 { X: sx, Y: sy, Z: 1.0 })
    }
}

impl Drop for Glass {
    fn drop(&mut self) {
        if let Some(home) = self.shadow_home.take() {
            let _ = home.Remove(&self.shadow);
        }
        let _ = self.parent.Remove(&self.root);
    }
}

/// A visual showing a painted surface. Removes itself from the tree when dropped.
pub struct Sprite {
    visual: SpriteVisual,
    brush: CompositionSurfaceBrush,
    parent: VisualCollection,
    last: Option<(f32, f32, f32, f32)>,
    last_opacity: f32,
}

impl Sprite {
    pub fn set_surface(&self, surface: &CompositionDrawingSurface) -> Result<()> {
        self.brush.SetSurface(surface)
    }

    pub fn place(&mut self, x: f32, y: f32, w: f32, h: f32) -> Result<()> {
        if self.last == Some((x, y, w, h)) {
            return Ok(());
        }
        self.last = Some((x, y, w, h));
        self.visual.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })?;
        self.visual.SetSize(Vector2 { X: w, Y: h })
    }

    pub fn set_opacity(&mut self, opacity: f32) -> Result<()> {
        if self.last_opacity == opacity {
            return Ok(());
        }
        self.last_opacity = opacity;
        self.visual.SetOpacity(opacity)
    }
}

impl Drop for Sprite {
    fn drop(&mut self) {
        let _ = self.parent.Remove(&self.visual);
    }
}

/// A solid rounded rectangle. Removes itself from the tree when dropped.
pub struct Pill {
    visual: ShapeVisual,
    geo: CompositionRoundedRectangleGeometry,
    brush: CompositionColorBrush,
    parent: VisualCollection,
    last: Option<(f32, f32, f32, f32, f32)>,
    last_color: Option<Color>,
    last_opacity: f32,
    pulsing: bool,
}

impl Pill {
    pub fn place(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) -> Result<()> {
        if self.last == Some((x, y, w, h, radius)) {
            return Ok(());
        }
        self.last = Some((x, y, w, h, radius));
        self.visual.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })?;
        self.visual.SetSize(Vector2 { X: w, Y: h })?;
        self.geo.SetSize(Vector2 { X: w, Y: h })?;
        self.geo.SetCornerRadius(Vector2 { X: radius, Y: radius })
    }

    pub fn set_color(&mut self, c: Color) -> Result<()> {
        if self.last_color == Some(c) {
            return Ok(());
        }
        self.last_color = Some(c);
        self.brush.SetColor(c)
    }

    pub fn set_opacity(&mut self, opacity: f32) -> Result<()> {
        if self.last_opacity == opacity {
            return Ok(());
        }
        self.last_opacity = opacity;
        self.visual.SetOpacity(opacity)
    }

    /// Makes the pill breathe up and down from its bottom edge, for ever, between `low` and 1
    /// of its height. The compositor runs this by itself: the app is not woken for it, so it
    /// costs nothing while everything else is at rest. `phase` (0..1) offsets it in time.
    pub fn pulse(&mut self, low: f32, seconds: f32, phase: f32) -> Result<()> {
        if self.pulsing {
            return Ok(());
        }
        let compositor = self.visual.Compositor()?;
        let animation = compositor.CreateScalarKeyFrameAnimation()?;
        let ease = compositor.CreateCubicBezierEasingFunction(Vector2 { X: 0.37, Y: 0.0 }, Vector2 { X: 0.63, Y: 1.0 })?;
        // Starting part-way through the cycle is what puts neighbouring pills out of step.
        let start = low + (1.0 - low) * (0.5 - 0.5 * (phase * std::f32::consts::TAU).cos());
        animation.InsertKeyFrame(0.0, start)?;
        animation.InsertKeyFrameWithEasingFunction(0.5 * (1.0 - phase.fract()).max(0.05), 1.0, &ease)?;
        animation.InsertKeyFrameWithEasingFunction((0.5 + 0.5 * (1.0 - phase.fract())).min(0.97), low, &ease)?;
        animation.InsertKeyFrameWithEasingFunction(1.0, start, &ease)?;
        animation.SetDuration(TimeSpan { Duration: (seconds.max(0.1) * 10_000_000.0) as i64 })?;
        animation.SetIterationBehavior(AnimationIterationBehavior::Forever)?;
        if let Some((_, _, w, h, _)) = self.last {
            self.visual.SetCenterPoint(Vector3 { X: w * 0.5, Y: h, Z: 0.0 })?;
        }
        self.visual.StartAnimation(&HSTRING::from("Scale.Y"), &animation)?;
        self.pulsing = true;
        Ok(())
    }

    pub fn rest(&mut self) -> Result<()> {
        if !self.pulsing {
            return Ok(());
        }
        self.pulsing = false;
        self.visual.StopAnimation(&HSTRING::from("Scale.Y"))?;
        self.visual.SetScale(Vector3 { X: 1.0, Y: 1.0, Z: 1.0 })
    }
}

impl Drop for Pill {
    fn drop(&mut self) {
        let _ = self.parent.Remove(&self.visual);
    }
}

/// A drop of light (see `Scene::bead_in`). Removes itself from the tree when dropped.
pub struct Bead {
    visual: ShapeVisual,
    geo: CompositionRoundedRectangleGeometry,
    top: CompositionColorGradientStop,
    bottom: CompositionColorGradientStop,
    edge: CompositionColorBrush,
    parent: VisualCollection,
    last: Option<(f32, f32, f32, f32, f32)>,
    last_colors: Option<(Color, Color, Color)>,
    last_opacity: f32,
}

impl Bead {
    pub fn place(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32) -> Result<()> {
        if self.last == Some((x, y, w, h, radius)) {
            return Ok(());
        }
        self.last = Some((x, y, w, h, radius));
        let radius = radius.min(w.min(h) * 0.5).max(0.0);
        self.visual.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })?;
        self.visual.SetSize(Vector2 { X: w, Y: h })?;
        // Half a pixel in, so the edge line is whole.
        self.geo.SetOffset(Vector2 { X: 0.5, Y: 0.5 })?;
        self.geo.SetSize(Vector2 { X: (w - 1.0).max(0.0), Y: (h - 1.0).max(0.0) })?;
        self.geo.SetCornerRadius(Vector2 { X: (radius - 0.5).max(0.0), Y: (radius - 0.5).max(0.0) })
    }

    /// The fill at its top and at its bottom, and the edge line.
    pub fn set_colors(&mut self, top: Color, bottom: Color, edge: Color) -> Result<()> {
        if self.last_colors == Some((top, bottom, edge)) {
            return Ok(());
        }
        self.last_colors = Some((top, bottom, edge));
        self.top.SetColor(top)?;
        self.bottom.SetColor(bottom)?;
        self.edge.SetColor(edge)
    }

    pub fn set_opacity(&mut self, opacity: f32) -> Result<()> {
        if self.last_opacity == opacity {
            return Ok(());
        }
        self.last_opacity = opacity;
        self.visual.SetOpacity(opacity)
    }
}

impl Drop for Bead {
    fn drop(&mut self) {
        let _ = self.parent.Remove(&self.visual);
    }
}

/// Blur of what is behind the glass, with its colours pushed so the glass looks wet rather
/// than washed out.
///
/// The picture comes from the compositor's *backdrop* brush: whatever is drawn behind the
/// visual. The window has no surface of its own, so that is the desktop and the windows
/// behind it. (There is also a *host* backdrop brush, meant for exactly this; on current
/// Windows 11 builds it gives solid black to a window like these, with or without the
/// switches that are supposed to enable it, which is why it is not used.)
fn make_backdrop_brush(compositor: &Compositor, sigma: f32, saturation: f32) -> Result<CompositionEffectBrush> {
    let backdrop = compositor.CreateBackdropBrush()?;
    let source: IGraphicsEffectSource = CompositionEffectSourceParameter::Create(&HSTRING::from("backdrop"))?.cast()?;

    let blur: IGraphicsEffect = D2DEffect {
        clsid: CLSID_D2D1GaussianBlur,
        name: "Blur",
        // StandardDeviation, Optimization (1 = balanced), BorderMode (1 = hard: do not fade the edges)
        props: vec![Prop::F32(sigma), Prop::U32(1), Prop::U32(1)],
        sources: vec![source.clone()],
    }
    .into();
    let saturate: IGraphicsEffect = D2DEffect {
        clsid: CLSID_D2D1Saturation,
        name: "Saturate",
        props: vec![Prop::F32(saturation.clamp(0.0, 2.0))],
        sources: vec![blur.cast()?],
    }
    .into();

    // Prefer blur + saturation; fall back to blur alone if this system rejects the chain.
    let factory = match compositor.CreateEffectFactory(&saturate) {
        Ok(f) => f,
        Err(e) => {
            crate::log!("blur+saturation effect rejected ({e}); trying blur only");
            compositor.CreateEffectFactory(&blur)?
        }
    };
    let brush = factory.CreateBrush()?;
    brush.SetSourceParameter(&HSTRING::from("backdrop"), &backdrop)?;
    Ok(brush)
}

#[derive(Clone, Copy)]
enum Prop {
    F32(f32),
    U32(u32),
}

/// Describes a built-in Direct2D effect to the compositor. This is the same contract Win2D's effect classes implement.
#[implement(IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectD2D1Interop)]
struct D2DEffect {
    clsid: GUID,
    name: &'static str,
    props: Vec<Prop>,
    sources: Vec<IGraphicsEffectSource>,
}

impl IGraphicsEffect_Impl for D2DEffect_Impl {
    fn Name(&self) -> Result<HSTRING> {
        Ok(HSTRING::from(self.name))
    }
    fn SetName(&self, _name: &HSTRING) -> Result<()> {
        Ok(())
    }
}

impl IGraphicsEffectSource_Impl for D2DEffect_Impl {}

impl IGraphicsEffectD2D1Interop_Impl for D2DEffect_Impl {
    fn GetEffectId(&self) -> Result<GUID> {
        Ok(self.clsid)
    }
    fn GetNamedPropertyMapping(&self, _name: &PCWSTR, index: *mut u32, mapping: *mut GRAPHICS_EFFECT_PROPERTY_MAPPING) -> Result<()> {
        // Nothing here is animated by name, but answer for property 0 so callers that probe do not fail.
        if index.is_null() || mapping.is_null() {
            return Err(E_INVALIDARG.into());
        }
        unsafe {
            *index = 0;
            *mapping = GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT;
        }
        Ok(())
    }
    fn GetPropertyCount(&self) -> Result<u32> {
        Ok(self.props.len() as u32)
    }
    fn GetProperty(&self, index: u32) -> Result<IPropertyValue> {
        let value = match self.props.get(index as usize) {
            Some(Prop::F32(v)) => PropertyValue::CreateSingle(*v)?,
            Some(Prop::U32(v)) => PropertyValue::CreateUInt32(*v)?,
            None => return Err(E_BOUNDS.into()),
        };
        value.cast()
    }
    fn GetSource(&self, index: u32) -> Result<IGraphicsEffectSource> {
        self.sources.get(index as usize).cloned().ok_or_else(|| E_BOUNDS.into())
    }
    fn GetSourceCount(&self) -> Result<u32> {
        Ok(self.sources.len() as u32)
    }
}

/// A picture at one size (see `Scene::picture_level`).
pub struct Level {
    pub surface: ICompositionSurface,
    pub width: f32,
    pub height: f32,
}

/// Which pre-shrunk copy to read when a picture is shown at `scale` times its size (the
/// smaller of the horizontal and vertical scale): the smallest copy that is still at least as
/// large as what is shown, so nothing is skipped and nothing is blown up.
pub fn level_for(scale: f32, count: usize) -> usize {
    if count == 0 || scale.is_nan() || scale <= 0.0 {
        return 0;
    }
    let steps = (1.0 / scale).log2().floor();
    if steps <= 0.0 {
        0
    } else {
        (steps as usize).min(count - 1)
    }
}

/// A band of rows from a picture, shown stretched into a shape with a level top and bottom
/// and leaning sides. Removes itself when dropped.
pub struct Slice {
    visual: SpriteVisual,
    brush: CompositionSurfaceBrush,
    parent: VisualCollection,
    last: Option<[f32; 8]>,
    hidden: bool,
    level: usize,
}

impl Slice {
    /// Shows the band of the picture from `src_top` to `src_bottom` (fractions of its height,
    /// 0 = top row) stretched into the `h` rows from `y` down: `w` wide from `x` at the top
    /// row, `foot_w` wide from `foot_x` at the bottom row, with straight sides between.
    pub fn show(&mut self, levels: &[Level], x: f32, y: f32, w: f32, h: f32, foot_x: f32, foot_w: f32, src_top: f32, src_bottom: f32) -> Result<()> {
        let now = [x, y, w, h, foot_x, foot_w, src_top, src_bottom];
        if self.last == Some(now) && !self.hidden {
            return Ok(());
        }
        let Some(full) = levels.first() else { return Ok(()) };
        self.last = Some(now);
        let band = (src_bottom - src_top).max(1e-6);
        // How much smaller than the full picture this band is being shown.
        let shown = (w / full.width.max(1.0)).min(h / (band * full.height).max(1e-3));
        let pick = level_for(shown, levels.len());
        let level = &levels[pick];
        if pick != self.level {
            self.brush.SetSurface(&level.surface)?;
            self.level = pick;
        }
        let (sx, sy) = (w / level.width.max(1.0), h / (band * level.height).max(1e-3));
        // The visual is the rectangle `w` by `h`; the shape it is shown as has sides that
        // lean, and no stretching or shearing turns a rectangle into that: it takes the
        // kind of transform a picture seen at an angle gets. A point (u, v) of the rectangle
        // goes to ((u + lean v) / (1 + k v), ratio v / (1 + k v)): the top row stays where it
        // is, and the bottom row (v = h) ends up `foot_w` wide, starting at `foot_x`.
        let ratio = w / foot_w.max(1e-3);
        let (lean, k) = ((foot_x - x) * ratio / h, (ratio - 1.0) / h);
        self.visual.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })?;
        // (Two screen rows more than its own, of the picture's next rows: the edges of a
        // leaning shape are smoothed, the level ones too, and where two slices meet what
        // lies behind would show through as a hairline. The slice below lies over these.)
        self.visual.SetSize(Vector2 { X: w, Y: h + 2.0 * ratio })?;
        self.visual.SetTransformMatrix(Matrix4x4 { M11: 1.0, M12: 0.0, M13: 0.0, M14: 0.0, M21: lean, M22: ratio, M23: 0.0, M24: k, M31: 0.0, M32: 0.0, M33: 1.0, M34: 0.0, M41: 0.0, M42: 0.0, M43: 0.0, M44: 1.0 })?;
        self.brush.SetScale(Vector2 { X: sx, Y: sy })?;
        self.brush.SetOffset(Vector2 { X: 0.0, Y: -src_top * level.height * sy })?;
        if self.hidden {
            self.hidden = false;
            self.visual.SetIsVisible(true)?;
        }
        Ok(())
    }

    pub fn hide(&mut self) -> Result<()> {
        if self.hidden {
            return Ok(());
        }
        self.hidden = true;
        self.visual.SetIsVisible(false)
    }
}

impl Drop for Slice {
    fn drop(&mut self) {
        let _ = self.parent.Remove(&self.visual);
    }
}

/// The outline of a piece of glass whose top edge is not straight: the dock's glass while
/// its surface moves like water (see `surface.rs`). A glass that wears one is cut to it and
/// its rim follows it; when it is taken off, the glass has its own rounded outline again.
pub struct Skin {
    factory: ID2D1Factory,
    /// What the glass is cut to, the dark hairline around that, and the bright line inside it.
    clip: CompositionPathGeometry,
    outline: CompositionPathGeometry,
    rim: CompositionPathGeometry,
    worn: bool,
}

impl Scene {
    /// An outline for a piece of glass to wear (see `Skin`).
    pub fn skin(&self, gfx: &Gfx) -> Result<Skin> {
        let c = &self.compositor;
        Ok(Skin { factory: unsafe { gfx.d2d.GetFactory()? }, clip: c.CreatePathGeometry()?, outline: c.CreatePathGeometry()?, rim: c.CreatePathGeometry()?, worn: false })
    }
}

impl Skin {
    /// Shapes `glass`, which has just been given its rectangle. `top`: its top edge from the
    /// left round corner to the right one, as points in the glass's own coordinates.
    /// `shoulder`: how far below the top of the rectangle the round corners begin, which is
    /// where the edge lies when it is straight (the room above is for waves).
    pub fn wear(&mut self, glass: &Glass, shoulder: f32, top: &[(f32, f32)]) -> Result<()> {
        let Some([_, _, w, h, radius]) = glass.last else { return Ok(()) };
        let radius = radius.min((h - shoulder) * 0.5).max(0.0);
        // The rim's visual starts one pixel above and to the left of the glass, and its two
        // lines lie half a pixel outside and half a pixel inside the shape (see `Glass::set_rect`).
        self.clip.SetPath(&self.path(w, h, radius, shoulder, top, 0.0, 0.0)?)?;
        self.outline.SetPath(&self.path(w, h, radius, shoulder, top, 0.5, 1.0)?)?;
        self.rim.SetPath(&self.path(w, h, radius, shoulder, top, -0.5, 1.0)?)?;
        if !self.worn {
            let (clip, outline, rim) = Self::parts(glass)?;
            clip.SetGeometry(&self.clip)?;
            outline.SetGeometry(&self.outline)?;
            rim.SetGeometry(&self.rim)?;
            self.worn = true;
        }
        Ok(())
    }

    /// Gives `glass` its own rounded outline back.
    pub fn take_off(&mut self, glass: &Glass) -> Result<()> {
        if !self.worn {
            return Ok(());
        }
        self.worn = false;
        let (clip, outline, rim) = Self::parts(glass)?;
        clip.SetGeometry(&glass.clip_geo)?;
        outline.SetGeometry(&glass.outline_geo)?;
        rim.SetGeometry(&glass.rim_geo)
    }

    /// What cuts a glass to its shape, and the two lines of its rim.
    fn parts(glass: &Glass) -> Result<(CompositionGeometricClip, CompositionSpriteShape, CompositionSpriteShape)> {
        let lines = glass.rim.Shapes()?;
        Ok((glass.body.Clip()?.cast()?, lines.GetAt(0)?.cast()?, lines.GetAt(1)?.cast()?))
    }

    /// The outline `grow` pixels outside the shape, in coordinates that start `shift` pixels
    /// above and to the left of the glass.
    fn path(&self, w: f32, h: f32, radius: f32, shoulder: f32, top: &[(f32, f32)], grow: f32, shift: f32) -> Result<CompositionPath> {
        let at = |x: f32, y: f32| Vector2 { X: x + shift, Y: y + shift };
        let r = (radius + grow).max(0.0);
        let (x0, y0, x1, y1) = (-grow, shoulder - grow, w + grow, h + grow);
        let arc = |x: f32, y: f32| D2D1_ARC_SEGMENT { point: at(x, y), size: D2D_SIZE_F { width: r, height: r }, rotationAngle: 0.0, sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE, arcSize: D2D1_ARC_SIZE_SMALL };
        unsafe {
            let geometry = self.factory.CreatePathGeometry()?;
            let sink = geometry.Open()?;
            // Clockwise from where the top edge leaves the left corner.
            sink.BeginFigure(at(x0 + r, y0), D2D1_FIGURE_BEGIN_FILLED);
            if top.len() > 2 {
                // Each point between the two ends bends the line, which passes halfway between
                // it and the next: a smooth curve through however many points there are.
                let bends: Vec<Vector2> = top[1..top.len() - 1].iter().map(|&(x, y)| at(x.clamp(x0 + r, x1 - r), y - grow)).collect();
                let mut curves: Vec<D2D1_QUADRATIC_BEZIER_SEGMENT> = bends.windows(2).map(|p| D2D1_QUADRATIC_BEZIER_SEGMENT { point1: p[0], point2: Vector2 { X: (p[0].X + p[1].X) * 0.5, Y: (p[0].Y + p[1].Y) * 0.5 } }).collect();
                curves.push(D2D1_QUADRATIC_BEZIER_SEGMENT { point1: bends[bends.len() - 1], point2: at(x1 - r, y0) });
                sink.AddQuadraticBeziers(&curves);
            } else {
                sink.AddLine(at(x1 - r, y0));
            }
            sink.AddArc(&arc(x1, y0 + r));
            sink.AddLine(at(x1, y1 - r));
            sink.AddArc(&arc(x1 - r, y1));
            sink.AddLine(at(x0 + r, y1));
            sink.AddArc(&arc(x0, y1 - r));
            sink.AddLine(at(x0, y0 + r));
            sink.AddArc(&arc(x0 + r, y0));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            sink.Close()?;
            let source: IGeometrySource2D = PathSource { geometry: geometry.cast()? }.into();
            CompositionPath::Create(&source)
        }
    }
}

/// Hands a Direct2D shape to the compositor. This is the same contract Win2D's geometry class implements.
#[implement(IGeometrySource2D, IGeometrySource2DInterop)]
struct PathSource {
    geometry: ID2D1Geometry,
}

impl IGeometrySource2D_Impl for PathSource_Impl {}

impl IGeometrySource2DInterop_Impl for PathSource_Impl {
    fn GetGeometry(&self) -> Result<ID2D1Geometry> {
        Ok(self.geometry.clone())
    }
    fn TryGetGeometryUsingFactory(&self, _factory: Ref<ID2D1Factory>) -> Result<ID2D1Geometry> {
        Err(E_NOTIMPL.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halving_averages_blocks() {
        // A 2x2 picture of four greys becomes their mean.
        let px = [10u8, 10, 10, 255, 20, 20, 20, 255, 30, 30, 30, 255, 40, 40, 40, 255];
        let (w, h, out) = halve(2, 2, &px);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, vec![25, 25, 25, 255]);
        // Odd sizes do not read past the edge.
        let (w, h, out) = halve(3, 1, &[9u8; 12]);
        assert_eq!((w, h, out.len()), (1, 1, 4));
    }

    #[test]
    fn levels_match_the_shown_size() {
        assert_eq!(level_for(1.0, 6), 0);
        assert_eq!(level_for(0.6, 6), 0);
        assert_eq!(level_for(0.5, 6), 1);
        assert_eq!(level_for(0.2, 6), 2);
        assert_eq!(level_for(0.001, 6), 5);
        assert_eq!(level_for(0.0, 6), 0);
        assert_eq!(level_for(0.3, 0), 0);
    }

    #[test]
    fn shadow_is_hollow_and_fades_out() {
        let (side, inset, px) = shadow_tile(12, 20);
        assert_eq!(side, inset * 2);
        let alpha = |x: u32, y: u32| px[((y * side + x) * 4 + 3) as usize];
        // Nothing under the middle of the shape, nothing at the very edge of the picture.
        assert_eq!(alpha(side / 2, side / 2), 0);
        assert_eq!(alpha(0, side / 2), 0);
        assert_eq!(alpha(side / 2, 0), 0);
        // Darkest just outside the shape, and darker below it than above it.
        let below = alpha(side / 2, inset + 13 + 1);
        let above = alpha(side / 2, inset - 13 - 2);
        assert!(below > 150, "below {below}");
        assert!(above > 20 && above < below, "above {above}, below {below}");
        // Colour channels stay black (premultiplied).
        assert!(px.chunks_exact(4).all(|p| p[0] == 0 && p[1] == 0 && p[2] == 0));
    }
}
