//! The window preview: rest the cursor on a running app's icon and a piece of
//! glass swells up above it with a live picture and the title of each of that
//! app's windows. Click one to switch to it, middle-click to close it.

use crate::app::{self, Event};
use crate::gfx::Gfx;
use crate::motion::{Glide, Motion};
use crate::scene::{Bead, Glass, Scene, Sprite, Stage};
use crate::spring::Spring;
use crate::text;
use crate::theme::Theme;
use crate::util::{from_wide, wide};
use windows::core::{Result, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::CompositionDrawingSurface;

const CLASS_NAME: &str = "BenPreviewWindow";
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Everything the preview needs to know to lay itself out.
pub struct Layout {
    /// Screen x the panel is centred on (the icon's centre).
    pub anchor_x: i32,
    /// Screen y of the panel's bottom edge.
    pub bottom_y: i32,
    pub monitor: RECT,
    pub scale: f32,
    /// Width of one window picture, in logical pixels.
    pub size: f32,
}

struct Cell {
    window: HWND,
    thumb: isize,
    /// The cell (title + picture) and the picture alone, in panel pixels.
    area: (f32, f32, f32, f32),
    picture: (f32, f32, f32, f32),
    _title: Option<Sprite>,
    _title_surface: Option<CompositionDrawingSurface>,
}

pub struct Preview {
    hwnd: HWND,
    _stage: Stage,
    glass: Glass,
    highlight: Bead,
    highlight_at: Glide,
    cells: Vec<Cell>,
    /// Which dock item the preview is showing. `None` once it is told to go, even while it
    /// is still fading away.
    key: Option<String>,
    /// The window is on screen (open, or fading away).
    visible: bool,
    /// Panel rectangle on screen.
    rect: RECT,
    /// Space around the panel inside the window, for its shadow.
    margin: f32,
    size: (f32, f32),
    scale: f32,
    /// 0 = away, 1 = fully there.
    show: Spring,
    shown: Option<(u8, i32)>,
}

thread_local! {
    static TRACKING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        // The cursor moving over the panel, or leaving it, is what moves the mark and what
        // lets the panel close: tell the app, which sleeps while nothing happens.
        WM_MOUSEMOVE => {
            if !TRACKING.get() {
                let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                TRACKING.set(unsafe { TrackMouseEvent(&mut track) }.is_ok());
            }
            app::post(Event::PreviewHover);
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            TRACKING.set(false);
            app::post(Event::PreviewHover);
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            app::post(Event::PreviewClick(false));
            return LRESULT(0);
        }
        WM_MBUTTONUP => {
            app::post(Event::PreviewClick(true));
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Preview {
    pub fn new(scene: &Scene, theme: &Theme, motion: &Motion) -> Result<Self> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class_name = wide(CLASS_NAME);
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                hCursor: LoadCursorW(None, IDC_ARROW)?,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )?;
            let stage = scene.stage(hwnd)?;
            let glass = scene.glass_in(&stage.content()?, &theme.bubble)?;
            let highlight = scene.bead_in(&glass.inner()?)?;
            Ok(Preview {
                hwnd,
                _stage: stage,
                glass,
                highlight,
                highlight_at: Glide::new(motion),
                cells: Vec::new(),
                key: None,
                visible: false,
                rect: RECT::default(),
                margin: 0.0,
                size: (1.0, 1.0),
                scale: 1.0,
                show: Spring::with(0.0, motion.small()),
                shown: None,
            })
        }
    }

    pub fn is_open(&self) -> bool {
        self.key.is_some()
    }

    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    pub fn windows(&self) -> Vec<HWND> {
        self.cells.iter().map(|c| c.window).collect()
    }

    pub fn set_theme(&mut self, theme: &Theme) {
        self.glass.set_material(&theme.bubble);
        // Titles are painted in the old colour.
        self.hide_now();
    }

    /// True when a screen point is on the panel (with a little slack around it).
    pub fn contains(&self, pt: POINT, slack: i32) -> bool {
        self.is_open() && pt.x >= self.rect.left - slack && pt.x < self.rect.right + slack && pt.y >= self.rect.top - slack && pt.y < self.rect.bottom + slack
    }

    /// The window whose cell is under a screen point.
    pub fn window_at(&self, pt: POINT) -> Option<HWND> {
        let (x, y) = ((pt.x - self.rect.left) as f32, (pt.y - self.rect.top) as f32);
        self.cells.iter().find(|c| x >= c.area.0 && x < c.area.2 && y >= c.area.1 && y < c.area.3).map(|c| c.window)
    }

    /// Opens the preview for one dock item, or rebuilds it if it is already open.
    pub fn show(&mut self, scene: &Scene, gfx: &Gfx, theme: &Theme, key: &str, windows: &[HWND], layout: &Layout) -> Result<()> {
        let was_open = self.is_open();
        self.clear();
        let s = layout.scale;
        let px = |v: f32| (v * s).round();
        let (pad, gap, title_h) = (px(10.0), px(8.0), px(24.0));
        let (box_w, box_h) = (px(layout.size), px(layout.size * 0.6));
        let monitor_w = (layout.monitor.right - layout.monitor.left) as f32;
        let fit = (((monitor_w - px(16.0) - pad * 2.0 + gap) / (box_w + gap)).floor() as usize).max(1);
        let windows = &windows[..windows.len().min(fit)];
        if windows.is_empty() {
            self.hide_now();
            return Ok(());
        }
        let n = windows.len() as f32;
        let (w, h) = (pad * 2.0 + n * box_w + (n - 1.0) * gap, pad * 2.0 + title_h + box_h);
        let margin = theme.bubble.shadow_reach + px(6.0);
        (self.margin, self.size, self.scale) = (margin, (w, h), s);
        // Blur and shadow are sized for the screen it opens on.
        self.glass.set_material(&theme.bubble);

        unsafe {
            let format = text::format(gfx, text::UI_FONT, 12.0 * s, DWRITE_FONT_WEIGHT_NORMAL)?;
            let content = self.glass.outer()?;
            for (i, &window) in windows.iter().enumerate() {
                let x = pad + i as f32 * (box_w + gap);
                let area = (x, pad, x + box_w, pad + title_h + box_h);
                let Ok(thumb) = DwmRegisterThumbnail(self.hwnd, window) else { continue };

                // Fit the window's picture into the box, keeping its proportions.
                let source = DwmQueryThumbnailSourceSize(thumb).unwrap_or(SIZE { cx: 16, cy: 10 });
                let (sw, sh) = (source.cx.max(1) as f32, source.cy.max(1) as f32);
                let k = (box_w / sw).min(box_h / sh).min(1.0);
                let (pw, ph) = ((sw * k).round(), (sh * k).round());
                let left = (x + (box_w - pw) * 0.5).round();
                let top = (pad + title_h + (box_h - ph) * 0.5).round();
                let picture = (left, top, left + pw, top + ph);

                // Title, shortened with an ellipsis until it fits above the picture.
                let mut buf = [0u16; 256];
                let len = GetWindowTextW(window, &mut buf).max(0) as usize;
                let (text_layout, tw, th) = text::fit(gfx, &format, &from_wide(&buf[..len]), Some(box_w - px(4.0)))?;
                let painted = scene.surface_from_text(&text_layout, tw, th, theme.text)?;
                let mut sprite = scene.sprite_in(&content)?;
                sprite.set_surface(&painted)?;
                sprite.place((x + px(2.0)).round(), (pad + (title_h - th) * 0.5 - px(1.0)).round(), tw, th)?;
                sprite.set_opacity(0.92)?;
                self.cells.push(Cell { window, thumb, area, picture, _title: Some(sprite), _title_surface: Some(painted) });
            }

            // Centre above the icon, kept on screen.
            let x = (layout.anchor_x - (w * 0.5) as i32).clamp(layout.monitor.left + px(8.0) as i32, (layout.monitor.right - w as i32 - px(8.0) as i32).max(layout.monitor.left));
            let y = layout.bottom_y - h as i32;
            self.rect = RECT { left: x, top: y, right: x + w as i32, bottom: y + h as i32 };
            self.glass.set_rect(scene, margin, margin, w, h, px(14.0))?;
            let m = margin as i32;
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x - m, y - m, w as i32 + 2 * m, h as i32 + 2 * m, SWP_NOACTIVATE);

            self.key = Some(key.to_string());
            if !was_open && !self.visible {
                // Swell in. When switching between apps the panel just changes in place.
                self.show.snap(0.0);
            }
            self.show.set(1.0);
            self.shown = None;
            self.apply()?;
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            self.visible = true;
        }
        Ok(())
    }

    /// Fades the panel in or out and marks the cell under the cursor. Call once per frame.
    /// Returns true while something is still moving.
    pub fn update(&mut self, theme: &Theme, motion: &Motion, cursor: POINT, dt: f32) -> Result<bool> {
        if !self.visible {
            return Ok(false);
        }
        self.show.tune(if self.key.is_some() { motion.small() } else { motion.fade() });
        self.show.step(dt);
        if self.key.is_none() && self.show.pos < 0.02 {
            self.hide_now();
            return Ok(false);
        }
        self.apply()?;

        let (x, y) = ((cursor.x - self.rect.left) as f32, (cursor.y - self.rect.top) as f32);
        let hot = self.cells.iter().find(|c| self.key.is_some() && x >= c.area.0 && x < c.area.2 && y >= c.area.1 && y < c.area.3).map(|c| c.area);
        let grow = (5.0 * self.scale).round();
        self.highlight_at.tune(motion);
        self.highlight_at.aim(hot.map(|a| (a.0 - grow, a.1 - grow, a.2 - a.0 + 2.0 * grow, a.3 - a.1 + 2.0 * grow)));
        self.highlight_at.step(dt);
        let s = self.highlight_at.shape(motion.squash());
        self.highlight.place(s.x, s.y, s.w, s.h, 9.0 * self.scale)?;
        let (top, bottom, edge) = theme.bead;
        self.highlight.set_colors(top, bottom, edge)?;
        self.highlight.set_opacity(s.alpha)?;
        Ok(!self.show.settled(1e-3) || !self.highlight_at.settled())
    }

    /// Applies the current size and fade to the panel and to the window pictures (those are
    /// drawn by Windows, not by the scene, so they are moved by hand to match).
    fn apply(&mut self) -> Result<()> {
        let alpha = self.show.pos.clamp(0.0, 1.0);
        let grow = 0.9 + 0.1 * self.show.pos.clamp(0.0, 1.1);
        let (w, h) = self.size;
        // It grows out of the icon below it.
        self.glass.set_scale(grow, grow, w * 0.5, h)?;
        self.glass.set_opacity(alpha)?;
        let byte = (alpha * 255.0).round() as u8;
        let step = (grow * 2000.0) as i32;
        if self.shown == Some((byte, step)) {
            return Ok(());
        }
        self.shown = Some((byte, step));
        let (cx, cy) = (self.margin + w * 0.5, self.margin + h);
        let place = |v: f32, centre: f32| (centre + (v - centre) * grow).round() as i32;
        for cell in &self.cells {
            let (l, t, r, b) = cell.picture;
            let props = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY,
                rcDestination: RECT { left: place(l + self.margin, cx), top: place(t + self.margin, cy), right: place(r + self.margin, cx), bottom: place(b + self.margin, cy) },
                rcSource: RECT::default(),
                opacity: byte,
                fVisible: BOOL(1),
                fSourceClientAreaOnly: BOOL(0),
            };
            unsafe {
                let _ = DwmUpdateThumbnailProperties(cell.thumb, &props);
            }
        }
        Ok(())
    }

    fn clear(&mut self) {
        for cell in self.cells.drain(..) {
            unsafe {
                let _ = DwmUnregisterThumbnail(cell.thumb);
            }
        }
    }

    /// Sends the preview away; it fades out over the next frames.
    pub fn hide(&mut self) {
        if self.key.take().is_some() {
            self.show.set(0.0);
        }
    }

    /// Takes the preview off the screen at once.
    pub fn hide_now(&mut self) {
        self.key = None;
        if self.visible {
            self.visible = false;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
        self.show.snap(0.0);
        self.clear();
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.hide_now();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
