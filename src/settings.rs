//! The settings window: one row per entry of `config::PARAMS`, with a slider,
//! a switch or a set of choices. Every change is applied to the dock while
//! the control is being moved and written to ben.ini when it is released.

use crate::config::{self, Config, Kind, PARAMS};
use crate::app::{self, Event};
use crate::gfx::{color, rect};
use crate::system;
use crate::util::wide;
use std::cell::RefCell;
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
const COLUMN_W: f32 = 430.0;
const MARGIN: f32 = 22.0;
const ROW_H: f32 = 34.0;
const HEADER_H: f32 = 38.0;
const CONTROL_W: f32 = 150.0;
const VALUE_W: f32 = 46.0;
const BUTTON_H: f32 = 30.0;

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
    cfg: Config,
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
}

/// Rows in three columns, and the size of the content they need.
fn build_rows() -> (Vec<Row>, f32, f32) {
    // The groups that start a new column: how it looks and moves | the dock | how it behaves.
    const NEW_COLUMN: [&str; 2] = ["Dock", "Hành vi"];
    let mut rows = Vec::new();
    let mut section = "";
    let (mut x, mut y) = (0.0, MARGIN - 8.0);
    let mut tallest: f32 = 0.0;
    for (index, p) in PARAMS.iter().enumerate() {
        if !p.in_window {
            continue;
        }
        if p.section != section {
            section = p.section;
            if NEW_COLUMN.contains(&section) {
                tallest = tallest.max(y);
                x += COLUMN_W;
                y = MARGIN - 8.0;
            }
            rows.push(Row { control: Control::Header(section), x, y });
            y += HEADER_H;
        }
        rows.push(Row { control: Control::Param(index), x, y });
        y += ROW_H;
    }
    rows.push(Row { control: Control::Autostart, x, y });
    y += ROW_H + 14.0;
    rows.push(Row { control: Control::Reset, x, y });
    rows.push(Row { control: Control::OpenFile, x, y });
    y += BUTTON_H;
    tallest = tallest.max(y);
    (rows, x + COLUMN_W, tallest + MARGIN)
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
        let (rows, content_w, content_h) = build_rows();
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
            cfg: cfg.clone(),
            rows,
            scale,
            dark,
            accent: system::accent_color(),
            drag: None,
            autostart: system::autostart_enabled(),
        };
        STATE.with(|s| *s.borrow_mut() = Some(state));
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
                let w = if names.len() > 3 { 64.0 } else { 72.0 } * names.len() as f32;
                (right - w, cy - 13.0, right, cy + 13.0)
            }
        },
        Control::Autostart => (right - 40.0, cy - 11.0, right, cy + 11.0),
        Control::Reset => (row.x + MARGIN, row.y, row.x + MARGIN + 150.0, row.y + BUTTON_H),
        Control::OpenFile => (row.x + MARGIN + 162.0, row.y, row.x + MARGIN + 312.0, row.y + BUTTON_H),
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

    fn mouse_down(&mut self, x: f32, y: f32) {
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

            // Dividers between the columns.
            brush.SetColor(&line);
            let mut client = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut client);
            let columns = self.rows.iter().map(|r| (r.x / COLUMN_W).round() as usize).max().unwrap_or(0) + 1;
            for column in 1..columns {
                let x = (COLUMN_W * column as f32 * s).round();
                t.FillRectangle(&rect(x, MARGIN * s, x + 1.0, client.bottom as f32 - MARGIN * s), brush);
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
