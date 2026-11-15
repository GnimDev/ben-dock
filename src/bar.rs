//! The top bar: a glass strip along the top of the screen. On the left the
//! dock's mark, the name of the app in front in bold and a row of menus; on
//! the right sound, battery, network, search and the date and time.
//!
//! Everything but the search symbol opens one of the dock's own drop-downs
//! (see `menu`, `menus` and `calendar`).

use crate::dock::{self, Event};
use crate::gfx::{color, Gfx};
use crate::glass::{rgba, Glass, Palette, Pill, Popup};
use crate::spring::Spring;
use crate::system::{AppBar, Status};
use crate::menus::TITLES;
use crate::text::{self, Line, ICON_FONT, UI_FONT};
use crate::util::wide;
use std::cell::Cell;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "BenBarWindow";
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_BAR_APPBAR: u32 = WM_APP + 2;

/// The clickable parts of the bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Zone {
    /// The dock's mark: the dock's and the computer's menu.
    Logo,
    /// The name of the app in front: that app's menu.
    App,
    /// One of the menu titles (index into `menus::TITLES`).
    Title(usize),
    Search,
    /// The speaker: volume and mute.
    Volume,
    Battery,
    Network,
    /// The date and time: opens the calendar.
    Clock,
}

impl Zone {
    /// Whether clicking this part opens one of the dock's own menus.
    pub fn is_menu(self) -> bool {
        matches!(self, Zone::Logo | Zone::App | Zone::Title(_) | Zone::Volume | Zone::Battery | Zone::Network)
    }

    /// The menu to the left (-1) or right (+1) of this one, going round at the ends.
    pub fn neighbour(self, dir: i32) -> Zone {
        let count = TITLES.len() as i32 + 2;
        let at = match self {
            Zone::Logo => 0,
            Zone::App => 1,
            Zone::Title(i) => i as i32 + 2,
            other => return other,
        };
        match (at + dir).rem_euclid(count) {
            0 => Zone::Logo,
            1 => Zone::App,
            n => Zone::Title(n as usize - 2),
        }
    }
}

/// The dock's own mark at the left end of the bar: an anchor, for a place where boats tie up.
const MARK: &str = "\u{2693}\u{FE0E}";
const MARK_FONT: &str = "Segoe UI Symbol";

struct Fonts {
    regular: IDWriteTextFormat,
    bold: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    mark: IDWriteTextFormat,
}

pub struct Bar {
    hwnd: HWND,
    popup: Popup,
    appbar: AppBar,
    logo: Line,
    app: Line,
    titles: Vec<Line>,
    search: Line,
    network: Line,
    volume: Line,
    battery: Line,
    battery_text: Line,
    clock: Line,
    highlight: Pill,
    fonts: Option<Fonts>,
    zones: Vec<(Zone, f32, f32)>,
    hover: Option<f32>,
    /// The part whose menu is open.
    open: Option<Zone>,
    /// Top-left corner of the bar on screen.
    origin: (i32, i32),
    /// 0 = shown, 1 = slid up out of view.
    slide: Spring,
    width: f32,
    height: f32,
    scale: f32,
    /// What `place` was last called with, so repeating it does nothing.
    placed: Option<(RECT, i32, bool)>,
}

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let x = (lparam.0 & 0xffff) as i16 as i32;
    match msg {
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            if !TRACKING.get() {
                let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                TRACKING.set(unsafe { TrackMouseEvent(&mut track) }.is_ok());
            }
            dock::post(Event::BarMove(x));
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            TRACKING.set(false);
            dock::post(Event::BarMove(-1));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            dock::post(Event::BarClick(x));
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Bar {
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
            let line = || -> Result<Line> { Ok(Line::new(glass.sprite_in(popup.content()?)?)) };
            Ok(Bar {
                hwnd,
                logo: line()?,
                app: line()?,
                titles: (0..TITLES.len()).map(|_| line()).collect::<Result<Vec<_>>>()?,
                search: line()?,
                network: line()?,
                volume: line()?,
                battery: line()?,
                battery_text: line()?,
                clock: line()?,
                highlight,
                popup,
                appbar: AppBar::new(hwnd),
                fonts: None,
                zones: Vec::new(),
                hover: None,
                open: None,
                origin: (0, 0),
                slide: Spring::new(0.0, 16.0, 1.0),
                width: 1.0,
                height: 1.0,
                scale: 1.0,
                placed: None,
            })
        }
    }

    /// Puts the bar along the top of `monitor`, `height` logical pixels high, and (with
    /// `reserve`) keeps maximised windows from sliding under it.
    pub fn place(&mut self, monitor: RECT, scale: f32, height: f32, reserve: bool) {
        let h = (height * scale).round().max(16.0);
        let w = (monitor.right - monitor.left) as f32;
        // Asking the shell for the same strip again would only make it notify every other bar.
        if self.placed == Some((monitor, h as i32, reserve)) {
            return;
        }
        self.placed = Some((monitor, h as i32, reserve));
        let mut top = monitor.top;
        if reserve {
            self.appbar.register(WM_BAR_APPBAR);
            if let Some(strip) = self.appbar.reserve_top(monitor, h as i32) {
                top = strip.top;
            }
        } else {
            self.appbar.remove();
        }
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), monitor.left, top, w as i32, h as i32, SWP_NOACTIVATE);
            self.origin = (monitor.left, top);
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        if (self.height, self.scale) != (h, scale) {
            // Text is painted for one size.
            self.fonts = None;
            self.forget();
        }
        (self.width, self.height, self.scale) = (w, h, scale);
    }

    /// Forgets painted text, so it is painted again (after a theme, size or GPU change).
    pub fn forget(&mut self) {
        for line in [&mut self.logo, &mut self.app, &mut self.search, &mut self.network, &mut self.volume, &mut self.battery, &mut self.battery_text, &mut self.clock] {
            line.forget();
        }
        for line in &mut self.titles {
            line.forget();
        }
    }

    pub fn set_palette(&self, palette: &Palette) {
        let _ = self.popup.set_palette(palette);
    }

    pub fn set_hover(&mut self, x: Option<f32>) {
        self.hover = x;
    }

    pub fn zone_at(&self, x: f32) -> Option<Zone> {
        self.zones.iter().find(|(_, a, b)| x >= *a && x < *b).map(|(z, _, _)| *z)
    }

    /// Marks the part whose menu is open (it stays highlighted).
    pub fn set_open(&mut self, zone: Option<Zone>) {
        self.open = zone;
    }

    /// Where a menu opened from `zone` hangs: the screen position of the zone's lower-left corner.
    pub fn drop_point(&self, zone: Zone) -> Option<(i32, i32)> {
        let (left, _, bottom) = self.span(zone)?;
        Some((left, bottom))
    }

    /// A part's left and right edge and the bar's bottom edge, on screen.
    pub fn span(&self, zone: Zone) -> Option<(i32, i32, i32)> {
        let (_, left, right) = self.zones.iter().find(|(z, _, _)| *z == zone)?;
        Some((self.origin.0 + left.round() as i32, self.origin.0 + right.round() as i32, self.origin.1 + self.height.round() as i32))
    }

    /// Brings the bar's contents up to date and lays them out. `hidden` slides it out of view
    /// (for full-screen apps). Returns true while it is still sliding.
    pub fn update(&mut self, glass: &Glass, gfx: &Gfx, app_name: &str, status: &Status, clock: &str, dark: bool, hidden: bool, dt: f32) -> Result<bool> {
        let (s, w, h) = (self.scale, self.width, self.height);
        self.slide.set(if hidden { 1.0 } else { 0.0 });
        self.slide.step(dt);
        // One pixel wider than the window on the left, top and right, so only the bottom
        // edge of the panel's hairline border is on screen.
        self.popup.set_frame(-1.0, -1.0 - (self.slide.pos * (h + 2.0)).round(), w + 2.0, h + 1.0, 0.0)?;

        if self.fonts.is_none() {
            let size = (h * 0.44).round();
            self.fonts = Some(Fonts {
                regular: text::format(gfx, UI_FONT, size, DWRITE_FONT_WEIGHT_NORMAL)?,
                bold: text::format(gfx, UI_FONT, size, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                icons: text::format(gfx, ICON_FONT, (h * 0.5).round(), DWRITE_FONT_WEIGHT_NORMAL)?,
                mark: text::format(gfx, MARK_FONT, (h * 0.56).round(), DWRITE_FONT_WEIGHT_NORMAL)?,
            });
        }
        let fonts = self.fonts.as_ref().unwrap();
        let fg = if dark { color(1.0, 1.0, 1.0, 0.94) } else { color(0.0, 0.0, 0.0, 0.86) };
        let dim = if dark { color(1.0, 1.0, 1.0, 0.36) } else { color(0.0, 0.0, 0.0, 0.32) };
        let glyph = |code: u32| char::from_u32(code).map(String::from).unwrap_or_default();

        // What each part says.
        self.logo.set(glass, gfx, &fonts.mark, MARK, fg, None)?;
        self.app.set(glass, gfx, &fonts.bold, app_name, fg, Some(w * 0.22))?;
        for (line, title) in self.titles.iter_mut().zip(TITLES) {
            line.set(glass, gfx, &fonts.regular, title, fg, None)?;
        }
        self.search.set(glass, gfx, &fonts.icons, &glyph(0xE721), fg, None)?;
        // Wi-Fi symbol: full strength when online, faint when not.
        self.network.set(glass, gfx, &fonts.icons, &glyph(0xE701), if status.online { fg } else { dim }, None)?;
        let volume = match status.volume {
            Some((_, true)) | None => 0xE74F,
            Some((level, false)) => 0xE992 + ((level * 3.0).ceil() as u32).min(3),
        };
        self.volume.set(glass, gfx, &fonts.icons, &glyph(volume), fg, None)?;
        match status.battery {
            Some((percent, charging)) => {
                // Eleven battery symbols, empty to full, in two sets (on battery / charging).
                let step = (percent as u32 + 5) / 10;
                let code = match (charging, step) {
                    (false, 10) => 0xE83F,
                    (false, n) => 0xE850 + n,
                    (true, 10) => 0xEA93,
                    (true, n) => 0xE85A + n,
                };
                self.battery.set(glass, gfx, &fonts.icons, &glyph(code), fg, None)?;
                self.battery_text.set(glass, gfx, &fonts.regular, &format!("{percent}%"), fg, None)?;
            }
            None => {
                self.battery.set(glass, gfx, &fonts.icons, "", fg, None)?;
                self.battery_text.set(glass, gfx, &fonts.regular, "", fg, None)?;
            }
        }
        self.clock.set(glass, gfx, &fonts.regular, clock, fg, None)?;

        // Layout. Left side: the mark, the app in front, then the menus. Each takes its
        // text plus the same space on both sides, and the parts touch.
        let px = |v: f32| (v * s).round();
        let mid = |line: &Line| (h - line.h) * 0.5;
        let side = px(10.0);
        self.zones.clear();
        let mut x = px(8.0);
        let (y, lw) = (mid(&self.logo), self.logo.w);
        self.logo.place(x + side, y)?;
        self.zones.push((Zone::Logo, x, x + lw + 2.0 * side));
        x += lw + 2.0 * side;
        let (y, aw) = (mid(&self.app), self.app.w);
        self.app.place(x + side, y)?;
        self.zones.push((Zone::App, x, x + aw + 2.0 * side));
        x += aw + 2.0 * side;
        for (index, line) in self.titles.iter_mut().enumerate() {
            let y = (h - line.h) * 0.5;
            line.place(x + side, y)?;
            self.zones.push((Zone::Title(index), x, x + line.w + 2.0 * side));
            x += line.w + 2.0 * side;
        }

        // Right side, from the edge inwards: date and time, search, network, battery, sound.
        let mut right = w - px(14.0);
        let (y, cw) = (mid(&self.clock), self.clock.w);
        self.clock.place(right - cw, y)?;
        self.zones.push((Zone::Clock, right - cw - px(9.0), w));
        right -= cw + px(20.0);

        let (y, sw) = (mid(&self.search), self.search.w);
        self.search.place(right - sw, y)?;
        self.zones.push((Zone::Search, right - sw - px(9.0), right + px(9.0)));
        right -= sw + px(20.0);

        // Each status symbol is a button of its own; the battery's includes its percentage.
        let half = px(6.0);
        let (y, nw) = (mid(&self.network), self.network.w);
        self.network.place(right - nw, y)?;
        self.zones.push((Zone::Network, right - nw - half, right + half));
        right -= nw + 2.0 * half;
        if self.battery.w > 0.0 {
            let end = right;
            let (y, tw) = (mid(&self.battery_text), self.battery_text.w);
            self.battery_text.place(right - tw, y)?;
            right -= tw + px(5.0);
            let (y, bw) = (mid(&self.battery), self.battery.w);
            self.battery.place(right - bw, y)?;
            self.zones.push((Zone::Battery, right - bw - half, end + half));
            right -= bw + 2.0 * half;
        }
        let (y, vw) = (mid(&self.volume), self.volume.w);
        self.volume.place(right - vw, y)?;
        self.zones.push((Zone::Volume, right - vw - half, right + half));

        // The part whose menu is open stays lit; otherwise the part under the cursor, fainter.
        let find = |zone: Zone| self.zones.iter().find(|(z, _, _)| *z == zone).copied();
        let lit = match self.open.and_then(find) {
            Some(zone) => Some((zone, true)),
            None => self.hover.and_then(|x| self.zones.iter().find(|(_, a, b)| x >= *a && x < *b).copied()).map(|zone| (zone, false)),
        };
        match lit {
            Some(((_, a, b), open)) => {
                self.highlight.place(a.max(px(4.0)), px(3.0), b.min(w - px(4.0)) - a.max(px(4.0)), h - px(6.0), px(5.0))?;
                let c = if dark { 1.0 } else { 0.0 };
                self.highlight.set_color(rgba(c, c, c, if open { 0.20 } else { 0.11 }))?;
                self.highlight.set_opacity(1.0)?;
            }
            None => self.highlight.set_opacity(0.0)?,
        }
        Ok(!self.slide.settled(1e-3))
    }
}

impl Drop for Bar {
    fn drop(&mut self) {
        self.appbar.remove();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
