//! The window preview: rest the cursor on a running app's icon and a small
//! glass panel appears above it with a live picture and the title of each of
//! that app's windows. Click one to switch to it, middle-click to close it.

use crate::dock::{self, Event};
use crate::gfx::{color, Gfx};
use crate::glass::{rgba, Glass, Palette, Pill, Popup, Sprite};
use crate::spring::Spring;
use crate::util::{from_wide, wide};
use windows::core::{Result, BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::CompositionDrawingSurface;

const CLASS_NAME: &str = "BenPreviewWindow";

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
    pub dark: bool,
}

struct Cell {
    window: HWND,
    thumb: isize,
    /// The cell (title + picture) and the picture alone, in panel pixels.
    area: (f32, f32, f32, f32),
    picture: RECT,
    _title: Option<Sprite>,
    _title_surface: Option<CompositionDrawingSurface>,
}

pub struct Preview {
    hwnd: HWND,
    popup: Popup,
    highlight: Pill,
    cells: Vec<Cell>,
    /// Which dock item the preview is showing.
    key: Option<String>,
    /// Panel rectangle on screen.
    rect: RECT,
    opacity: Spring,
    shown_opacity: u8,
    dark: bool,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_LBUTTONUP => {
            dock::post(Event::PreviewClick(false));
            return LRESULT(0);
        }
        WM_MBUTTONUP => {
            dock::post(Event::PreviewClick(true));
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Preview {
    pub fn new(glass: &Glass, blur_sigma: Option<f32>, palette: &Palette) -> Result<Self> {
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
            let popup = glass.popup(hwnd, blur_sigma, palette)?;
            let highlight = glass.pill_in(popup.content()?)?;
            Ok(Preview { hwnd, popup, highlight, cells: Vec::new(), key: None, rect: RECT::default(), opacity: Spring::new(0.0, 30.0, 1.0), shown_opacity: 0, dark: true })
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

    pub fn set_palette(&self, palette: &Palette) {
        let _ = self.popup.set_palette(palette);
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
    pub fn show(&mut self, glass: &Glass, gfx: &Gfx, key: &str, windows: &[HWND], layout: &Layout) -> Result<()> {
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
            self.hide();
            return Ok(());
        }
        let n = windows.len() as f32;
        let (w, h) = (pad * 2.0 + n * box_w + (n - 1.0) * gap, pad * 2.0 + title_h + box_h);
        self.dark = layout.dark;

        unsafe {
            let format = gfx.dwrite.CreateTextFormat(
                PCWSTR(wide("Segoe UI Variable Text").as_ptr()),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                12.0 * s,
                PCWSTR(wide("").as_ptr()),
            )?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            let fg = if layout.dark { 1.0 } else { 0.08 };

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
                let picture = RECT { left: left as i32, top: top as i32, right: (left + pw) as i32, bottom: (top + ph) as i32 };

                // Title, shortened with an ellipsis until it fits above the picture.
                let mut buf = [0u16; 256];
                let len = GetWindowTextW(window, &mut buf).max(0) as usize;
                let full: Vec<char> = from_wide(&buf[..len]).chars().collect();
                let mut title = None;
                let mut surface = None;
                let mut keep = full.len();
                for _ in 0..12 {
                    let text: String = if keep < full.len() { full[..keep].iter().collect::<String>().trim_end().to_string() + "…" } else { full.iter().collect() };
                    let utf16: Vec<u16> = text.encode_utf16().collect();
                    let text_layout = gfx.dwrite.CreateTextLayout(&utf16, &format, 4000.0, 100.0)?;
                    let mut tm = DWRITE_TEXT_METRICS::default();
                    text_layout.GetMetrics(&mut tm)?;
                    if tm.width <= box_w - px(4.0) || keep <= 1 {
                        let (tw, th) = (tm.width.ceil().max(1.0), tm.height.ceil().max(1.0));
                        let painted = glass.surface_from_text(&text_layout, tw, th, color(fg, fg, fg, 1.0))?;
                        let mut sprite = glass.sprite_in(self.popup.content()?)?;
                        sprite.set_surface(&painted)?;
                        sprite.place((x + px(2.0)).round(), (pad + (title_h - th) * 0.5 - px(1.0)).round(), tw, th)?;
                        sprite.set_opacity(0.9)?;
                        title = Some(sprite);
                        surface = Some(painted);
                        break;
                    }
                    // Cut in proportion to how much too wide it is, then try again.
                    keep = ((keep as f32 * (box_w - px(4.0)) / tm.width) as usize).min(keep - 1).max(1);
                }
                self.cells.push(Cell { window, thumb, area, picture, _title: title, _title_surface: surface });
            }

            // Centre above the icon, kept on screen.
            let x = (layout.anchor_x - (w * 0.5) as i32).clamp(layout.monitor.left + px(8.0) as i32, (layout.monitor.right - w as i32 - px(8.0) as i32).max(layout.monitor.left));
            let y = layout.bottom_y - h as i32;
            self.rect = RECT { left: x, top: y, right: x + w as i32, bottom: y + h as i32 };
            self.popup.set_size(w, h, px(12.0))?;
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, w as i32, h as i32, SWP_NOACTIVATE);

            self.key = Some(key.to_string());
            if !was_open {
                // Fade in. When switching between apps the panel just changes in place.
                self.opacity = Spring::new(0.0, 30.0, 1.0);
            }
            self.opacity.set(1.0);
            self.shown_opacity = 0;
            self.apply_opacity(true);
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        Ok(())
    }

    /// Fades the panel in and highlights the cell under the cursor. Call once per frame while open.
    pub fn update(&mut self, cursor: POINT, dt: f32) {
        if !self.is_open() {
            return;
        }
        self.opacity.step(dt);
        self.apply_opacity(false);

        let (x, y) = ((cursor.x - self.rect.left) as f32, (cursor.y - self.rect.top) as f32);
        let hot = self.cells.iter().find(|c| x >= c.area.0 && x < c.area.2 && y >= c.area.1 && y < c.area.3).map(|c| c.area);
        let c = if self.dark { 1.0 } else { 0.0 };
        let _ = self.highlight.set_color(rgba(c, c, c, if self.dark { 0.12 } else { 0.08 }));
        match hot {
            Some(a) => {
                let _ = self.highlight.place(a.0 - 5.0, a.1 - 5.0, a.2 - a.0 + 10.0, a.3 - a.1 + 10.0, 8.0);
                let _ = self.highlight.set_opacity(1.0);
            }
            None => {
                let _ = self.highlight.set_opacity(0.0);
            }
        }
    }

    /// Applies the current fade to the panel and to the window pictures.
    fn apply_opacity(&mut self, force: bool) {
        let alpha = self.opacity.pos.clamp(0.0, 1.0);
        let _ = self.popup.set_opacity(alpha);
        let byte = (alpha * 255.0).round() as u8;
        if byte == self.shown_opacity && !force {
            return;
        }
        self.shown_opacity = byte;
        for cell in &self.cells {
            let props = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY,
                rcDestination: cell.picture,
                rcSource: RECT::default(),
                opacity: byte,
                fVisible: BOOL(1),
                fSourceClientAreaOnly: BOOL(0),
            };
            unsafe {
                let _ = DwmUpdateThumbnailProperties(cell.thumb, &props);
            }
        }
    }

    fn clear(&mut self) {
        for cell in self.cells.drain(..) {
            unsafe {
                let _ = DwmUnregisterThumbnail(cell.thumb);
            }
        }
    }

    pub fn hide(&mut self) {
        if self.key.take().is_some() {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
        self.clear();
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.hide();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
