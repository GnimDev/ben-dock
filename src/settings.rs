//! The settings window: one row per entry of `config::PARAMS`, with a slider,
//! a switch or a set of choices. Every change is applied to the dock while
//! the control is being moved and written to ben.ini when it is released.
//!
//! The rows are sorted into tabs, listed down the left side (see `TABS`): a
//! tab shows the groups of settings that belong to one part of the app.

use crate::config::{self, Config, Kind, PARAMS};
use crate::app::{self, Event};
use crate::gfx::{color, rect};
use crate::system;
use crate::util::wide;
use std::cell::{Cell, RefCell};
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Vector2;

const CLASS_NAME: &str = "BenSettingsWindow";
const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

// Layout, in logical pixels.
const COLUMN_W: f32 = 470.0;
const MARGIN: f32 = 22.0;
const ROW_H: f32 = 34.0;
const HEADER_H: f32 = 38.0;
const CONTROL_W: f32 = 150.0;
const VALUE_W: f32 = 46.0;
const BUTTON_H: f32 = 30.0;
/// The list of tabs down the left side: its width, the height of one tab, and the room
/// between its edge and what is in it.
const SIDE_W: f32 = 196.0;
const TAB_H: f32 = 38.0;
const SIDE_PAD: f32 = 10.0;

/// One tab: its name, its symbol in Windows' icon font, and the groups of settings
/// (`Param::section`) it shows, in this order.
struct Tab {
    name: &'static str,
    glyph: u32,
    sections: &'static [&'static str],
}

const TABS: [Tab; 6] = [
    Tab { name: "Giao diện", glyph: 0xE771, sections: &["Giao diện", "Chuyển động"] },
    Tab { name: "Dock", glyph: 0xE90E, sections: &["Dock"] },
    Tab { name: "Đảo và bốn góc", glyph: 0xECA5, sections: &["Đảo động", "Bốn góc"] },
    Tab { name: "Màn hình nền", glyph: 0xE91B, sections: &["Widget", "Hình nền"] },
    Tab { name: "Cửa sổ", glyph: 0xE7C4, sections: &["Cửa sổ"] },
    Tab { name: "Hành vi", glyph: 0xE713, sections: &["Hành vi"] },
];

/// The tab a group of settings is shown on. A group no tab names goes on the last one, so
/// that a new setting can never be left out of the window.
fn tab_of(section: &str) -> usize {
    TABS.iter().position(|tab| tab.sections.contains(&section)).unwrap_or(TABS.len() - 1)
}

#[derive(Clone, Copy, PartialEq)]
enum Control {
    /// Index into `PARAMS`.
    Param(usize),
    Header(&'static str),
    Autostart,
    Reset,
    OpenFile,
}

#[derive(Clone, Copy)]
struct Row {
    control: Control,
    /// Left edge of the row's column and top of the row, in logical pixels.
    x: f32,
    y: f32,
}

struct State {
    hwnd: HWND,
    target: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
    text: IDWriteTextFormat,
    text_right: IDWriteTextFormat,
    text_center: IDWriteTextFormat,
    header: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    cfg: Config,
    /// The tab on show, and its rows.
    tab: usize,
    rows: Vec<Row>,
    scale: f32,
    dark: bool,
    accent: (f32, f32, f32),
    /// Slider being dragged.
    drag: Option<usize>,
    autostart: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// The tab that was on show when the window was last closed: it opens there again.
    static LAST_TAB: Cell<usize> = const { Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// For looking at the window in a test: it opens behind every other window and does
    /// not take the keyboard.
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// The rows of one tab, in one column to the right of the list of tabs, and how tall
/// they are together.
fn tab_rows(tab: usize) -> (Vec<Row>, f32) {
    let mut rows = Vec::new();
    let (x, mut y) = (SIDE_W, MARGIN - 8.0);
    // The tab's groups in the tab's own order, each under its heading.
    let mut sections: Vec<&str> = TABS[tab].sections.to_vec();
    for p in PARAMS.iter().filter(|p| p.in_window && tab_of(p.section) == tab) {
        if !sections.contains(&p.section) {
            sections.push(p.section);
        }
    }
    for section in sections {
        let mut headed = false;
        for (index, p) in PARAMS.iter().enumerate().filter(|(_, p)| p.in_window && p.section == section) {
            if !std::mem::replace(&mut headed, true) {
                rows.push(Row { control: Control::Header(p.section), x, y });
                y += HEADER_H;
            }
            rows.push(Row { control: Control::Param(index), x, y });
            y += ROW_H;
        }
    }
    // Starting with Windows is not a line of ben.ini, but it is how the app behaves.
    if tab == TABS.len() - 1 {
        rows.push(Row { control: Control::Autostart, x, y });
        y += ROW_H;
    }
    (rows, y + MARGIN)
}

/// The size of the window's content: as tall as the tallest tab.
fn content_size() -> (f32, f32) {
    let tallest = (0..TABS.len()).map(|tab| tab_rows(tab).1).fold(0.0, f32::max);
    // The list of tabs needs its own height too: the tabs, and the two buttons under them.
    let side = SIDE_PAD + TABS.len() as f32 * TAB_H + 2.0 * (BUTTON_H + 8.0) + MARGIN + SIDE_PAD;
    (SIDE_W + COLUMN_W, tallest.max(side))
}

/// The rows on show with tab `tab`: its own, and the two buttons at the foot of the list of
/// tabs, which are there on every tab.
fn build_rows(tab: usize, height: f32) -> Vec<Row> {
    let (mut rows, _) = tab_rows(tab);
    let foot = height - MARGIN + 4.0;
    rows.push(Row { control: Control::Reset, x: 0.0, y: foot - 2.0 * BUTTON_H - 8.0 });
    rows.push(Row { control: Control::OpenFile, x: 0.0, y: foot - BUTTON_H });
    rows
}

/// Where a tab is in the list down the left side (logical pixels).
fn tab_rect(tab: usize) -> (f32, f32, f32, f32) {
    let top = SIDE_PAD + tab as f32 * TAB_H;
    (SIDE_PAD, top, SIDE_W - SIDE_PAD, top + TAB_H - 4.0)
}

/// Opens the settings window, or brings it forward if it is already open.
pub fn open(cfg: &Config) {
    let existing = STATE.with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().map(|s| s.hwnd)));
    if let Some(hwnd) = existing {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    if let Err(e) = create(cfg) {
        crate::log!("could not open the settings window: {e}");
    }
}

/// The settings as currently shown in the window, if it is open.
pub fn current() -> Option<Config> {
    STATE.with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().map(|s| s.cfg.clone())))
}

/// Shows new values (after ben.ini was edited by hand, for example).
pub fn sync(cfg: &Config) {
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            if let Some(state) = s.as_mut() {
                state.cfg = cfg.clone();
                unsafe {
                    let _ = InvalidateRect(Some(state.hwnd), None, false);
                }
            }
        }
    });
}

fn create(cfg: &Config) -> Result<()> {
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
        // Fails harmlessly if the class is already registered from an earlier open.
        RegisterClassExW(&class);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("Cài đặt Bến").as_ptr()),
            STYLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            400,
            300,
            None,
            None,
            Some(instance),
            None,
        )?;

        let dark = system::system_is_dark();
        let on: i32 = dark as i32;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &on as *const i32 as *const _, 4);

        // Size the window so the content fits, then centre it on the screen.
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let (content_w, content_h) = content_size();
        let tab = LAST_TAB.get().min(TABS.len() - 1);
        let rows = build_rows(tab, content_h);
        let mut frame = RECT { left: 0, top: 0, right: (content_w * scale).round() as i32, bottom: (content_h * scale).round() as i32 };
        let _ = AdjustWindowRectEx(&mut frame, STYLE, false, WINDOW_EX_STYLE(0));
        let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let x = work.left + ((work.right - work.left) - w).max(0) / 2;
        let y = work.top + ((work.bottom - work.top) - h).max(0) / 2;
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);

        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let target = factory.CreateHwndRenderTarget(
            &D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            },
            &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U { width: client.right.max(1) as u32, height: client.bottom.max(1) as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            },
        )?;
        let brush = target.CreateSolidColorBrush(&color(1.0, 1.0, 1.0, 1.0), None)?;

        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let make = |size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT| -> Result<IDWriteTextFormat> {
            let format = dwrite.CreateTextFormat(
                PCWSTR(wide("Segoe UI Variable Text").as_ptr()),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size * scale,
                PCWSTR(wide("").as_ptr()),
            )?;
            format.SetTextAlignment(align)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(format)
        };

        let state = State {
            hwnd,
            target,
            brush,
            text: make(13.5, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            text_right: make(13.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            text_center: make(13.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER)?,
            header: make(12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            icons: {
                let format = dwrite.CreateTextFormat(PCWSTR(wide("Segoe Fluent Icons").as_ptr()), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 15.0 * scale, PCWSTR(wide("").as_ptr()))?;
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                format
            },
            cfg: cfg.clone(),
            tab,
            rows,
            scale,
            dark,
            accent: system::accent_color(),
            drag: None,
            autostart: system::autostart_enabled(),
        };
        STATE.with(|s| *s.borrow_mut() = Some(state));
        #[cfg(test)]
        if QUIET.get() {
            let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            return Ok(());
        }
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
        Ok(())
    }
}

// --------------------------------------------------------------- geometry

/// Where a row's control sits (logical pixels): the slider track, the switch, or the choice strip.
fn control_rect(row: &Row, control: Control) -> (f32, f32, f32, f32) {
    let right = row.x + COLUMN_W - MARGIN;
    let cy = row.y + ROW_H * 0.5;
    match control {
        Control::Param(index) => match PARAMS[index].kind {
            Kind::Number { .. } => (right - VALUE_W - CONTROL_W, cy - 10.0, right - VALUE_W, cy + 10.0),
            Kind::Toggle => (right - 40.0, cy - 11.0, right, cy + 11.0),
            Kind::Choice(names) => {
                // Narrower cells when there are many choices, so the label still fits beside them.
                let w = if names.len() > 3 { 70.0 } else { 92.0 } * names.len() as f32;
                (right - w, cy - 13.0, right, cy + 13.0)
            }
        },
        Control::Autostart => (right - 40.0, cy - 11.0, right, cy + 11.0),
        // The two buttons are as wide as the list of tabs they stand under.
        Control::Reset | Control::OpenFile => (row.x + SIDE_PAD, row.y, row.x + SIDE_W - SIDE_PAD, row.y + BUTTON_H),
        Control::Header(_) => (0.0, 0.0, 0.0, 0.0),
    }
}

fn inside(r: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= r.0 && x < r.2 && y >= r.1 && y < r.3
}

/// Slider value for a horizontal position.
fn slider_value(index: usize, track: (f32, f32, f32, f32), x: f32) -> f32 {
    let p = &PARAMS[index];
    let Kind::Number { min, max, .. } = p.kind else { return 0.0 };
    // The thumb travels between the two ends of the track, inset by its radius.
    let (a, b) = (track.0 + 8.0, track.2 - 8.0);
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    p.clean(min + (max - min) * t)
}

// ------------------------------------------------------------------ input

impl State {
    fn changed(&mut self, done: bool) {
        if done {
            self.cfg.save();
        }
        app::post(Event::Config(done));
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Shows another tab.
    fn show_tab(&mut self, tab: usize) {
        if tab == self.tab || tab >= TABS.len() {
            return;
        }
        self.tab = tab;
        LAST_TAB.set(tab);
        self.rows = build_rows(tab, content_size().1);
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32) {
        if let Some(tab) = (0..TABS.len()).find(|&tab| inside(tab_rect(tab), x, y)) {
            self.show_tab(tab);
            return;
        }
        for row in self.rows.clone() {
            let area = control_rect(&row, row.control);
            match row.control {
                Control::Param(index) => {
                    let p = &PARAMS[index];
                    match p.kind {
                        Kind::Number { .. } => {
                            // A little slack above and below makes the thin track easy to hit.
                            if inside((area.0 - 8.0, row.y, area.2 + 8.0, row.y + ROW_H), x, y) {
                                self.drag = Some(index);
                                unsafe {
                                    SetCapture(self.hwnd);
                                }
                                (p.set)(&mut self.cfg, slider_value(index, area, x));
                                self.changed(false);
                                return;
                            }
                        }
                        Kind::Toggle => {
                            if inside((area.0 - 6.0, row.y, area.2 + 6.0, row.y + ROW_H), x, y) {
                                let on = (p.get)(&self.cfg) >= 0.5;
                                (p.set)(&mut self.cfg, if on { 0.0 } else { 1.0 });
                                self.changed(true);
                                return;
                            }
                        }
                        Kind::Choice(names) => {
                            if inside(area, x, y) {
                                let pick = ((x - area.0) / ((area.2 - area.0) / names.len() as f32)).floor();
                                (p.set)(&mut self.cfg, p.clean(pick));
                                self.changed(true);
                                return;
                            }
                        }
                    }
                }
                Control::Autostart => {
                    if inside((area.0 - 6.0, row.y, area.2 + 6.0, row.y + ROW_H), x, y) {
                        system::set_autostart(!self.autostart);
                        self.autostart = system::autostart_enabled();
                        unsafe {
                            let _ = InvalidateRect(Some(self.hwnd), None, false);
                        }
                        return;
                    }
                }
                Control::Reset => {
                    if inside(area, x, y) {
                        self.cfg = Config::default();
                        self.changed(true);
                        return;
                    }
                }
                Control::OpenFile => {
                    if inside(area, x, y) {
                        crate::shell::launch_soon(&config::config_file().to_string_lossy());
                        return;
                    }
                }
                Control::Header(_) => {}
            }
        }
    }

    fn mouse_move(&mut self, x: f32) {
        let Some(index) = self.drag else { return };
        let Some(row) = self.rows.iter().find(|r| r.control == Control::Param(index)).copied() else { return };
        let value = slider_value(index, control_rect(&row, row.control), x);
        let p = &PARAMS[index];
        if (p.get)(&self.cfg) != value {
            (p.set)(&mut self.cfg, value);
            self.changed(false);
        }
    }

    fn mouse_up(&mut self) {
        if self.drag.take().is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
            self.changed(true);
        }
    }

    // --------------------------------------------------------------- paint

    fn paint(&mut self) {
        let s = self.scale;
        let dark = self.dark;
        let (ar, ag, ab) = self.accent;
        let bg = if dark { color(0.125, 0.125, 0.135, 1.0) } else { color(0.955, 0.955, 0.965, 1.0) };
        let fg = if dark { color(1.0, 1.0, 1.0, 0.92) } else { color(0.0, 0.0, 0.0, 0.88) };
        let dim = if dark { color(1.0, 1.0, 1.0, 0.56) } else { color(0.0, 0.0, 0.0, 0.56) };
        let track = if dark { color(1.0, 1.0, 1.0, 0.16) } else { color(0.0, 0.0, 0.0, 0.14) };
        let line = if dark { color(1.0, 1.0, 1.0, 0.08) } else { color(0.0, 0.0, 0.0, 0.08) };
        let side = if dark { color(0.0, 0.0, 0.0, 0.16) } else { color(0.0, 0.0, 0.0, 0.035) };
        let accent = color(ar, ag, ab, 1.0);
        let knob = color(1.0, 1.0, 1.0, 1.0);

        let t = &self.target;
        let brush = &self.brush;
        let px = |r: (f32, f32, f32, f32)| rect(r.0 * s, r.1 * s, r.2 * s, r.3 * s);
        let round = |r: (f32, f32, f32, f32), radius: f32| D2D1_ROUNDED_RECT { rect: px(r), radiusX: radius * s, radiusY: radius * s };
        let circle = |x: f32, y: f32, r: f32| D2D1_ELLIPSE { point: Vector2 { X: x * s, Y: y * s }, radiusX: r * s, radiusY: r * s };
        let write = |text: &str, format: &IDWriteTextFormat, r: (f32, f32, f32, f32), c: &D2D1_COLOR_F| unsafe {
            let utf16: Vec<u16> = text.encode_utf16().collect();
            brush.SetColor(c);
            t.DrawText(&utf16, format, &px(r), brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        };
        let switch = |area: (f32, f32, f32, f32), on: bool| unsafe {
            brush.SetColor(if on { &accent } else { &track });
            t.FillRoundedRectangle(&round(area, 11.0), brush);
            brush.SetColor(&knob);
            let cx = if on { area.2 - 11.0 } else { area.0 + 11.0 };
            t.FillEllipse(&circle(cx, (area.1 + area.3) * 0.5, 8.0), brush);
        };

        unsafe {
            t.BeginDraw();
            t.Clear(Some(&bg));

            // The list of tabs down the left side, a shade apart from the rest, and a line
            // between the two.
            let mut client = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut client);
            brush.SetColor(&side);
            t.FillRectangle(&rect(0.0, 0.0, SIDE_W * s, client.bottom as f32), brush);
            brush.SetColor(&line);
            t.FillRectangle(&rect((SIDE_W * s).round(), 0.0, (SIDE_W * s).round() + 1.0, client.bottom as f32), brush);
            for (index, tab) in TABS.iter().enumerate() {
                let area = tab_rect(index);
                let picked = index == self.tab;
                if picked {
                    // The tab on show: a plate under it and a short bar of the accent colour.
                    brush.SetColor(&track);
                    t.FillRoundedRectangle(&round(area, 7.0), brush);
                    brush.SetColor(&accent);
                    let middle = (area.1 + area.3) * 0.5;
                    t.FillRoundedRectangle(&round((area.0, middle - 8.0, area.0 + 3.0, middle + 8.0), 1.5), brush);
                }
                let symbol = char::from_u32(tab.glyph).map(String::from).unwrap_or_default();
                write(&symbol, &self.icons, (area.0 + 8.0, area.1, area.0 + 40.0, area.3), if picked { &fg } else { &dim });
                write(tab.name, if picked { &self.header } else { &self.text }, (area.0 + 44.0, area.1, area.2 - 4.0, area.3), &fg);
            }

            for row in &self.rows {
                let label_area = (row.x + MARGIN, row.y, row.x + COLUMN_W - MARGIN, row.y + ROW_H);
                let area = control_rect(row, row.control);
                match row.control {
                    Control::Header(name) => {
                        write(&name.to_uppercase(), &self.header, (row.x + MARGIN, row.y + 8.0, row.x + COLUMN_W - MARGIN, row.y + HEADER_H), &dim);
                    }
                    Control::Param(index) => {
                        let p = &PARAMS[index];
                        let value = (p.get)(&self.cfg);
                        write(p.label, &self.text, label_area, &fg);
                        match p.kind {
                            Kind::Number { min, max, .. } => {
                                let cy = (area.1 + area.3) * 0.5;
                                let f = if max > min { ((value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
                                let thumb_x = area.0 + 8.0 + (area.2 - area.0 - 16.0) * f;
                                brush.SetColor(&track);
                                t.FillRoundedRectangle(&round((area.0, cy - 2.0, area.2, cy + 2.0), 2.0), brush);
                                brush.SetColor(&accent);
                                t.FillRoundedRectangle(&round((area.0, cy - 2.0, thumb_x, cy + 2.0), 2.0), brush);
                                let grabbed = self.drag == Some(index);
                                brush.SetColor(&knob);
                                t.FillEllipse(&circle(thumb_x, cy, if grabbed { 9.0 } else { 8.0 }), brush);
                                brush.SetColor(&accent);
                                t.FillEllipse(&circle(thumb_x, cy, if grabbed { 4.0 } else { 5.0 }), brush);
                                write(&p.format(value), &self.text_right, (area.2, row.y, area.2 + VALUE_W, row.y + ROW_H), &dim);
                            }
                            Kind::Toggle => switch(area, value >= 0.5),
                            Kind::Choice(names) => {
                                brush.SetColor(&track);
                                t.FillRoundedRectangle(&round(area, 7.0), brush);
                                let w = (area.2 - area.0) / names.len() as f32;
                                for (i, name) in names.iter().enumerate() {
                                    let cell = (area.0 + w * i as f32, area.1, area.0 + w * (i + 1) as f32, area.3);
                                    let picked = value.round() as usize == i;
                                    if picked {
                                        brush.SetColor(&accent);
                                        t.FillRoundedRectangle(&round((cell.0 + 2.0, cell.1 + 2.0, cell.2 - 2.0, cell.3 - 2.0), 5.0), brush);
                                    }
                                    write(name, &self.text_center, cell, if picked { &knob } else { &fg });
                                }
                            }
                        }
                    }
                    Control::Autostart => {
                        write("Khởi động cùng Windows", &self.text, label_area, &fg);
                        switch(area, self.autostart);
                    }
                    Control::Reset | Control::OpenFile => {
                        brush.SetColor(&track);
                        t.FillRoundedRectangle(&round(area, 6.0), brush);
                        let text = if row.control == Control::Reset { "Về mặc định" } else { "Mở file ben.ini" };
                        write(text, &self.text_center, area, &fg);
                    }
                }
            }

            if let Err(e) = t.EndDraw(None, None) {
                crate::log!("settings window draw failed: {e}");
            }
        }
    }
}

fn point(lparam: LPARAM) -> (f32, f32) {
    (((lparam.0 & 0xffff) as i16) as f32, (((lparam.0 >> 16) & 0xffff) as i16) as f32)
}

/// Runs `f` on the window state unless it is already in use further up the call stack.
fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            if let Some(state) = s.as_mut() {
                f(state);
            }
        }
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            with_state(|s| s.paint());
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_LBUTTONDOWN => {
            let (x, y) = point(lparam);
            with_state(|s| s.mouse_down(x / s.scale, y / s.scale));
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let (x, _) = point(lparam);
            with_state(|s| s.mouse_move(x / s.scale));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            with_state(|s| s.mouse_up());
            return LRESULT(0);
        }
        WM_DESTROY => {
            STATE.with(|s| {
                if let Ok(mut s) = s.try_borrow_mut() {
                    *s = None;
                }
            });
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a check of anything: opens the window behind every other one and goes through
    /// its tabs, a second and a half each, so that they can be looked at.
    /// Run by hand: cargo test --release settings_tab_by_tab -- --ignored
    #[test]
    #[ignore]
    fn settings_tab_by_tab() {
        QUIET.set(true);
        open(&Config::default());
        let started = std::time::Instant::now();
        let mut msg = MSG::default();
        loop {
            let tab = (started.elapsed().as_secs_f32() / 1.5) as usize;
            if tab >= TABS.len() {
                break;
            }
            with_state(|state| state.show_tab(tab));
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn every_setting_of_the_window_is_on_exactly_one_tab() {
        let mut shown = vec![0; PARAMS.len()];
        let mut autostart = 0;
        for tab in 0..TABS.len() {
            let (rows, height) = tab_rows(tab);
            assert!(height > 0.0 && height <= content_size().1);
            for row in rows {
                match row.control {
                    Control::Param(index) => shown[index] += 1,
                    Control::Autostart => autostart += 1,
                    _ => {}
                }
                assert_eq!(row.x, SIDE_W);
            }
        }
        for (index, p) in PARAMS.iter().enumerate() {
            assert_eq!(shown[index], p.in_window as usize, "{}", p.key);
        }
        assert_eq!(autostart, 1);
    }

    #[test]
    fn every_group_has_a_tab_and_every_tab_has_rows() {
        for p in PARAMS.iter().filter(|p| p.in_window) {
            assert!(TABS.iter().any(|tab| tab.sections.contains(&p.section)), "no tab names the group '{}'", p.section);
        }
        for tab in 0..TABS.len() {
            assert!(tab_rows(tab).0.iter().any(|row| matches!(row.control, Control::Param(_))), "tab '{}' is empty", TABS[tab].name);
            // A tab starts with the heading of its first group.
            assert!(matches!(tab_rows(tab).0[0].control, Control::Header(_)));
        }
        assert_eq!(tab_of("a group that is new"), TABS.len() - 1);
    }

    #[test]
    fn the_tabs_and_the_buttons_do_not_overlap() {
        let (_, height) = content_size();
        let rows = build_rows(0, height);
        let reset = rows.iter().find(|row| row.control == Control::Reset).unwrap();
        let open = rows.iter().find(|row| row.control == Control::OpenFile).unwrap();
        let last_tab = tab_rect(TABS.len() - 1);
        assert!(control_rect(reset, reset.control).1 >= last_tab.3, "the buttons start under the last tab");
        assert!(control_rect(open, open.control).1 >= control_rect(reset, reset.control).3);
        assert!(control_rect(open, open.control).3 <= height);
        assert!(control_rect(open, open.control).2 <= SIDE_W);
    }
}
