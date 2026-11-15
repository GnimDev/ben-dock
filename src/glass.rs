//! Everything the dock shows is a tree of Windows.UI.Composition visuals:
//!
//! ```text
//! root
//! ├── panel   (rounded clip)
//! │   ├── backdrop  live blur of whatever is behind the window
//! │   └── tint      translucent colour on top of the blur
//! ├── border  hairline outline of the panel
//! ├── layer   one sprite per icon, running indicators, the separator
//! └── overlay the name label
//! ```
//!
//! The dock changes positions, sizes and opacities of these visuals once per
//! display refresh. All changes made in one pass are handed to the desktop
//! compositor as a single batch and take effect in the same frame, so the
//! panel and the icons can never be a frame apart, whatever the settings.
//! The compositor also does all the per-frame GPU work; the dock itself only
//! paints an icon or a label once, into a surface, when it first appears.

use crate::gfx::{color, rect, Gfx};
use crate::shell::IconPixels;
use windows::core::{implement, Interface, Result, GUID, HSTRING, PCWSTR};
use windows::Foundation::{IPropertyValue, PropertyValue, Size};
use windows::Graphics::DirectX::{DirectXAlphaMode, DirectXPixelFormat};
use windows::Graphics::Effects::{IGraphicsEffect, IGraphicsEffectSource, IGraphicsEffectSource_Impl, IGraphicsEffect_Impl};
use windows::System::DispatcherQueueController;
use windows::Win32::Foundation::{E_BOUNDS, E_INVALIDARG, HWND, POINT};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::System::WinRT::Composition::{ICompositionDrawingSurfaceInterop, ICompositionGraphicsDeviceInterop, ICompositorDesktopInterop, ICompositorInterop};
use windows::Win32::System::WinRT::Graphics::Direct2D::{
    IGraphicsEffectD2D1Interop, IGraphicsEffectD2D1Interop_Impl, GRAPHICS_EFFECT_PROPERTY_MAPPING, GRAPHICS_EFFECT_PROPERTY_MAPPING_DIRECT,
};
use windows::Win32::System::WinRT::{CreateDispatcherQueueController, DispatcherQueueOptions, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT};
use windows::UI::Color;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::UI::Composition::*;
use windows_numerics::{Matrix3x2, Vector2, Vector3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub radius: f32,
}

pub struct Palette {
    pub tint: Color,
    pub border_inner: Color,
    pub border_outer: Color,
}

/// How many copies of a window picture are kept: full size and five halvings (down to 1/32).
const MAX_LEVELS: usize = 6;

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

/// A colour from 0..1 components (not premultiplied).
pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> Color {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    Color { A: byte(a), R: byte(r), G: byte(g), B: byte(b) }
}

pub struct Glass {
    _controller: DispatcherQueueController,
    compositor: Compositor,
    _target: DesktopWindowTarget,
    graphics: CompositionGraphicsDevice,
    root: ContainerVisual,
    panel: ContainerVisual,
    backdrop: Option<SpriteVisual>,
    clip_geo: CompositionRoundedRectangleGeometry,
    tint_brush: CompositionColorBrush,
    border: ShapeVisual,
    outer_geo: CompositionRoundedRectangleGeometry,
    inner_geo: CompositionRoundedRectangleGeometry,
    outer_brush: CompositionColorBrush,
    inner_brush: CompositionColorBrush,
    layer: ContainerVisual,
    overlay: ContainerVisual,
    last_panel: Option<PanelRect>,
    last_slide: f32,
    pub blur_active: bool,
}

impl Glass {
    pub fn new(hwnd: HWND, gfx: &Gfx, width: f32, height: f32, blur_sigma: Option<f32>, palette: &Palette) -> Result<Self> {
        unsafe {
            // Windows.UI.Composition needs a dispatcher queue on the thread that owns the compositor.
            let controller = CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            })?;
            let compositor = Compositor::new()?;
            let desktop: ICompositorDesktopInterop = compositor.cast()?;
            let target = desktop.CreateDesktopWindowTarget(hwnd, false)?;
            let interop: ICompositorInterop = compositor.cast()?;
            let graphics = interop.CreateGraphicsDevice(&gfx.d2d)?;

            let root = compositor.CreateContainerVisual()?;
            root.SetSize(Vector2 { X: width, Y: height })?;
            target.SetRoot(&root)?;

            // Panel: clipped to a rounded rectangle.
            let panel = compositor.CreateContainerVisual()?;
            let clip_geo = compositor.CreateRoundedRectangleGeometry()?;
            let clip = compositor.CreateGeometricClipWithGeometry(&clip_geo)?;
            panel.SetClip(&clip)?;
            root.Children()?.InsertAtTop(&panel)?;

            // Lets this window sample what is behind it (needed for the blur).
            let on: i32 = 1;
            if let Err(e) = DwmSetWindowAttribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, &on as *const i32 as *const _, 4) {
                crate::log!("DWMWA_USE_HOSTBACKDROPBRUSH failed: {e}");
            }

            let tint_brush = compositor.CreateColorBrushWithColor(palette.tint)?;
            let tint = compositor.CreateSpriteVisual()?;
            tint.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
            tint.SetBrush(&tint_brush)?;
            panel.Children()?.InsertAtTop(&tint)?;

            // Border: two hairlines, dark outside and light inside, so the edge reads on any wallpaper.
            let border = compositor.CreateShapeVisual()?;
            let outer_geo = compositor.CreateRoundedRectangleGeometry()?;
            let inner_geo = compositor.CreateRoundedRectangleGeometry()?;
            let outer_brush = compositor.CreateColorBrushWithColor(palette.border_outer)?;
            let inner_brush = compositor.CreateColorBrushWithColor(palette.border_inner)?;
            for (geo, brush) in [(&outer_geo, &outer_brush), (&inner_geo, &inner_brush)] {
                let shape = compositor.CreateSpriteShapeWithGeometry(geo)?;
                shape.SetStrokeBrush(brush)?;
                shape.SetStrokeThickness(1.0)?;
                border.Shapes()?.Append(&shape)?;
            }
            root.Children()?.InsertAtTop(&border)?;

            // Icons and indicators, then the label above everything.
            let layer = compositor.CreateContainerVisual()?;
            layer.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
            root.Children()?.InsertAtTop(&layer)?;
            let overlay = compositor.CreateContainerVisual()?;
            overlay.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
            root.Children()?.InsertAtTop(&overlay)?;

            let mut glass = Glass {
                _controller: controller,
                compositor,
                _target: target,
                graphics,
                root,
                panel,
                backdrop: None,
                clip_geo,
                tint_brush,
                border,
                outer_geo,
                inner_geo,
                outer_brush,
                inner_brush,
                layer,
                overlay,
                last_panel: None,
                last_slide: f32::NAN,
                blur_active: false,
            };
            glass.set_blur(blur_sigma);
            Ok(glass)
        }
    }

    /// Switches to a new GPU device after the old one was lost. Surfaces painted before this
    /// are empty afterwards and must be painted again.
    pub fn set_rendering_device(&self, gfx: &Gfx) -> Result<()> {
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

    /// Turns the blur behind the panel on (with the given strength) or off.
    pub fn set_blur(&mut self, sigma: Option<f32>) {
        if let Some(old) = self.backdrop.take() {
            if let Ok(children) = self.panel.Children() {
                let _ = children.Remove(&old);
            }
        }
        self.blur_active = false;
        let Some(sigma) = sigma else { return };
        let made = make_backdrop_brush(&self.compositor, sigma).and_then(|brush| {
            let backdrop = self.compositor.CreateSpriteVisual()?;
            backdrop.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
            backdrop.SetBrush(&brush)?;
            // Underneath the tint.
            self.panel.Children()?.InsertAtBottom(&backdrop)?;
            Ok(backdrop)
        });
        match made {
            Ok(backdrop) => {
                self.backdrop = Some(backdrop);
                self.blur_active = true;
            }
            Err(e) => crate::log!("blur unavailable, using a plain translucent panel: {e}"),
        }
    }

    pub fn resize(&mut self, width: f32, height: f32) -> Result<()> {
        self.root.SetSize(Vector2 { X: width, Y: height })
    }

    pub fn set_palette(&self, palette: &Palette) -> Result<()> {
        self.tint_brush.SetColor(palette.tint)?;
        self.outer_brush.SetColor(palette.border_outer)?;
        self.inner_brush.SetColor(palette.border_inner)
    }

    /// Moves and resizes the glass panel. Cheap to call every frame: nothing is sent when the rectangle is unchanged.
    pub fn set_panel(&mut self, p: PanelRect) -> Result<()> {
        if self.last_panel == Some(p) {
            return Ok(());
        }
        self.last_panel = Some(p);
        self.panel.SetOffset(Vector3 { X: p.x, Y: p.y, Z: 0.0 })?;
        self.panel.SetSize(Vector2 { X: p.w, Y: p.h })?;
        self.clip_geo.SetSize(Vector2 { X: p.w, Y: p.h })?;
        self.clip_geo.SetCornerRadius(Vector2 { X: p.radius, Y: p.radius })?;

        // The border visual is one pixel larger on every side so the outer hairline is not clipped.
        self.border.SetOffset(Vector3 { X: p.x - 1.0, Y: p.y - 1.0, Z: 0.0 })?;
        self.border.SetSize(Vector2 { X: p.w + 2.0, Y: p.h + 2.0 })?;
        self.outer_geo.SetOffset(Vector2 { X: 0.5, Y: 0.5 })?;
        self.outer_geo.SetSize(Vector2 { X: p.w + 1.0, Y: p.h + 1.0 })?;
        self.outer_geo.SetCornerRadius(Vector2 { X: p.radius + 0.5, Y: p.radius + 0.5 })?;
        self.inner_geo.SetOffset(Vector2 { X: 1.5, Y: 1.5 })?;
        self.inner_geo.SetSize(Vector2 { X: (p.w - 1.0).max(0.0), Y: (p.h - 1.0).max(0.0) })?;
        self.inner_geo.SetCornerRadius(Vector2 { X: (p.radius - 0.5).max(0.0), Y: (p.radius - 0.5).max(0.0) })
    }

    /// Slides the whole dock down by `dy` pixels (used to tuck it away under full-screen apps).
    pub fn set_slide(&mut self, dy: f32) -> Result<()> {
        if self.last_slide == dy {
            return Ok(());
        }
        self.last_slide = dy;
        self.root.SetOffset(Vector3 { X: 0.0, Y: dy, Z: 0.0 })
    }

    // ------------------------------------------------------------- surfaces

    fn paint(&self, width: u32, height: u32, draw: impl FnOnce(&ID2D1DeviceContext) -> Result<()>) -> Result<CompositionDrawingSurface> {
        let size = Size { Width: width.max(1) as f32, Height: height.max(1) as f32 };
        let surface = self.graphics.CreateDrawingSurface(size, DirectXPixelFormat::B8G8R8A8UIntNormalized, DirectXAlphaMode::Premultiplied)?;
        Self::paint_on(&surface, draw)?;
        Ok(surface)
    }

    fn paint_on(surface: &CompositionDrawingSurface, draw: impl FnOnce(&ID2D1DeviceContext) -> Result<()>) -> Result<()> {
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
        Ok(())
    }

    /// A surface holding an icon's pixels.
    pub fn surface_from_pixels(&self, px: &IconPixels) -> Result<CompositionDrawingSurface> {
        self.surface_from_bgra(px.size, px.size, &px.bgra)
    }

    /// A picture that will be shown much smaller than it is (a window flying into its icon).
    /// It is stored together with pre-shrunk copies, each half the size of the one before, and
    /// every slice reads from the copy closest to the size it is shown at. Without this a
    /// picture squeezed to a fraction of its width sparkles, because most of its pixels are
    /// simply skipped. (The compositor's own mip-mapped surface cannot be used for this: a
    /// surface brush refuses it.) The first entry is the picture at full size.
    pub fn picture_levels(&self, width: u32, height: u32, bgra: &[u8]) -> Result<Vec<Level>> {
        if bgra.len() < (width as usize) * (height as usize) * 4 || width == 0 || height == 0 {
            return Err(E_INVALIDARG.into());
        }
        let mut levels = vec![Level { surface: self.surface_from_bgra(width, height, bgra)?.cast()?, width: width as f32, height: height as f32 }];
        let (mut w, mut h, mut pixels) = (width, height, bgra.to_vec());
        while levels.len() < MAX_LEVELS && w.min(h) >= 32 {
            (w, h, pixels) = halve(w, h, &pixels);
            match self.surface_from_bgra(w, h, &pixels).and_then(|surface| surface.cast()) {
                Ok(surface) => levels.push(Level { surface, width: w as f32, height: h as f32 }),
                Err(e) => {
                    crate::log!("pre-shrunk picture copy {w}x{h} unavailable ({e}); stopping at {} copies", levels.len());
                    break;
                }
            }
        }
        Ok(levels)
    }

    /// A surface holding a picture given as premultiplied BGRA pixels, top row first.
    pub fn surface_from_bgra(&self, width: u32, height: u32, bgra: &[u8]) -> Result<CompositionDrawingSurface> {
        if bgra.len() < (width as usize) * (height as usize) * 4 {
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

    // -------------------------------------------------------------- visuals

    fn parent(&self, on_overlay: bool) -> Result<VisualCollection> {
        if on_overlay {
            self.overlay.Children()
        } else {
            self.layer.Children()
        }
    }

    /// A rectangle that shows a surface (an icon, a label's text).
    pub fn sprite(&self, on_overlay: bool) -> Result<Sprite> {
        self.sprite_in(self.parent(on_overlay)?)
    }

    /// A rounded rectangle filled with one colour (indicators, the separator, the label's background).
    pub fn pill(&self, on_overlay: bool) -> Result<Pill> {
        self.pill_in(self.parent(on_overlay)?)
    }

    pub fn sprite_in(&self, parent: VisualCollection) -> Result<Sprite> {
        let visual = self.compositor.CreateSpriteVisual()?;
        let brush = self.compositor.CreateSurfaceBrush()?;
        brush.SetStretch(CompositionStretch::Fill)?;
        visual.SetBrush(&brush)?;
        parent.InsertAtTop(&visual)?;
        Ok(Sprite { visual, brush, parent, last: None, last_opacity: f32::NAN })
    }

    pub fn pill_in(&self, parent: VisualCollection) -> Result<Pill> {
        let visual = self.compositor.CreateShapeVisual()?;
        let geo = self.compositor.CreateRoundedRectangleGeometry()?;
        let brush = self.compositor.CreateColorBrush()?;
        let shape = self.compositor.CreateSpriteShapeWithGeometry(&geo)?;
        shape.SetFillBrush(&brush)?;
        visual.Shapes()?.Append(&shape)?;
        parent.InsertAtTop(&visual)?;
        Ok(Pill { visual, geo, brush, parent, last: None, last_color: None, last_opacity: f32::NAN })
    }

    /// A bare visual tree for another window of this app (the effect window).
    pub fn stage(&self, hwnd: HWND) -> Result<Stage> {
        unsafe {
            let desktop: ICompositorDesktopInterop = self.compositor.cast()?;
            let target = desktop.CreateDesktopWindowTarget(hwnd, false)?;
            let root = self.compositor.CreateContainerVisual()?;
            root.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
            // Smooth (anti-aliased) edges for everything on the stage.
            root.SetBorderMode(CompositionBorderMode::Soft)?;
            target.SetRoot(&root)?;
            Ok(Stage { _target: target, root, last_opacity: f32::NAN })
        }
    }

    /// One horizontal slice of a picture, for the stage.
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

    /// A glass panel filling another window of this app (the window-preview popup).
    pub fn popup(&self, hwnd: HWND, blur_sigma: Option<f32>, palette: &Palette) -> Result<Popup> {
        unsafe {
            let desktop: ICompositorDesktopInterop = self.compositor.cast()?;
            let target = desktop.CreateDesktopWindowTarget(hwnd, false)?;
            let root = self.compositor.CreateContainerVisual()?;
            target.SetRoot(&root)?;

            let panel = self.compositor.CreateContainerVisual()?;
            panel.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
            let clip_geo = self.compositor.CreateRoundedRectangleGeometry()?;
            panel.SetClip(&self.compositor.CreateGeometricClipWithGeometry(&clip_geo)?)?;
            root.Children()?.InsertAtTop(&panel)?;

            let on: i32 = 1;
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, &on as *const i32 as *const _, 4);
            if let Some(sigma) = blur_sigma {
                match make_backdrop_brush(&self.compositor, sigma) {
                    Ok(brush) => {
                        let backdrop = self.compositor.CreateSpriteVisual()?;
                        backdrop.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
                        backdrop.SetBrush(&brush)?;
                        panel.Children()?.InsertAtBottom(&backdrop)?;
                    }
                    Err(e) => crate::log!("preview blur unavailable: {e}"),
                }
            }
            let tint_brush = self.compositor.CreateColorBrushWithColor(palette.tint)?;
            let tint = self.compositor.CreateSpriteVisual()?;
            tint.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
            tint.SetBrush(&tint_brush)?;
            panel.Children()?.InsertAtTop(&tint)?;

            // One light hairline just inside the edge.
            let border = self.compositor.CreateShapeVisual()?;
            border.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
            let border_geo = self.compositor.CreateRoundedRectangleGeometry()?;
            let border_brush = self.compositor.CreateColorBrushWithColor(palette.border_inner)?;
            let shape = self.compositor.CreateSpriteShapeWithGeometry(&border_geo)?;
            shape.SetStrokeBrush(&border_brush)?;
            shape.SetStrokeThickness(1.0)?;
            border.Shapes()?.Append(&shape)?;
            root.Children()?.InsertAtTop(&border)?;

            let layer = self.compositor.CreateContainerVisual()?;
            layer.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?;
            root.Children()?.InsertAtTop(&layer)?;

            Ok(Popup { _target: target, root, clip_geo, tint_brush, border_geo, border_brush, layer, last_opacity: f32::NAN })
        }
    }
}

/// The visuals of a popup window: a rounded glass panel and a layer for its content.
pub struct Popup {
    _target: DesktopWindowTarget,
    root: ContainerVisual,
    clip_geo: CompositionRoundedRectangleGeometry,
    tint_brush: CompositionColorBrush,
    border_geo: CompositionRoundedRectangleGeometry,
    border_brush: CompositionColorBrush,
    layer: ContainerVisual,
    last_opacity: f32,
}

impl Popup {
    pub fn set_size(&self, w: f32, h: f32, radius: f32) -> Result<()> {
        self.root.SetSize(Vector2 { X: w, Y: h })?;
        self.clip_geo.SetSize(Vector2 { X: w, Y: h })?;
        self.clip_geo.SetCornerRadius(Vector2 { X: radius, Y: radius })?;
        self.border_geo.SetOffset(Vector2 { X: 0.5, Y: 0.5 })?;
        self.border_geo.SetSize(Vector2 { X: w - 1.0, Y: h - 1.0 })?;
        self.border_geo.SetCornerRadius(Vector2 { X: radius - 0.5, Y: radius - 0.5 })
    }

    pub fn set_palette(&self, palette: &Palette) -> Result<()> {
        self.tint_brush.SetColor(palette.tint)?;
        self.border_brush.SetColor(palette.border_inner)
    }

    /// Like `set_size`, but the panel may start outside the window: a panel one pixel larger
    /// than its window on some sides keeps the border line off those sides.
    pub fn set_frame(&self, x: f32, y: f32, w: f32, h: f32, radius: f32) -> Result<()> {
        self.root.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })?;
        self.set_size(w, h, radius.max(0.5))
    }

    /// Scales the popup's content about a point (the launcher's zoom when it opens).
    pub fn set_content_scale(&self, scale: f32, cx: f32, cy: f32) -> Result<()> {
        self.layer.SetCenterPoint(Vector3 { X: cx, Y: cy, Z: 0.0 })?;
        self.layer.SetScale(Vector3 { X: scale, Y: scale, Z: 1.0 })
    }

    pub fn set_opacity(&mut self, opacity: f32) -> Result<()> {
        if self.last_opacity == opacity {
            return Ok(());
        }
        self.last_opacity = opacity;
        self.root.SetOpacity(opacity)
    }

    /// Where the popup's content (sprites, pills) goes.
    pub fn content(&self) -> Result<VisualCollection> {
        self.layer.Children()
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
}

impl Drop for Pill {
    fn drop(&mut self) {
        let _ = self.parent.Remove(&self.visual);
    }
}

/// Blur of the desktop behind the window, with colours pushed slightly so the glass does not look washed out.
fn make_backdrop_brush(compositor: &Compositor, sigma: f32) -> Result<CompositionEffectBrush> {
    let backdrop = compositor.CreateHostBackdropBrush()?;
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
        props: vec![Prop::F32(1.25)],
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

/// The visual tree of the effect window.
pub struct Stage {
    _target: DesktopWindowTarget,
    root: ContainerVisual,
    last_opacity: f32,
}

impl Stage {
    pub fn set_opacity(&mut self, opacity: f32) -> Result<()> {
        if self.last_opacity == opacity {
            return Ok(());
        }
        self.last_opacity = opacity;
        self.root.SetOpacity(opacity)
    }
}

/// A picture at one size (see `Glass::picture_levels`).
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

/// A band of rows from a picture, shown stretched into a rectangle. Removes itself when dropped.
pub struct Slice {
    visual: SpriteVisual,
    brush: CompositionSurfaceBrush,
    parent: VisualCollection,
    last: Option<[f32; 6]>,
    hidden: bool,
    level: usize,
}

impl Slice {
    /// Shows the band of the picture from `src_top` to `src_bottom` (fractions of its height,
    /// 0 = top row) stretched into the rectangle `(x, y, w, h)`.
    pub fn show(&mut self, levels: &[Level], x: f32, y: f32, w: f32, h: f32, src_top: f32, src_bottom: f32) -> Result<()> {
        let now = [x, y, w, h, src_top, src_bottom];
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
        self.visual.SetOffset(Vector3 { X: x, Y: y, Z: 0.0 })?;
        self.visual.SetSize(Vector2 { X: w, Y: h })?;
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
