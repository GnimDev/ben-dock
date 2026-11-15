//! The date panel under the clock: today's date, a month laid out Monday
//! first with today circled, arrows to leaf through the months, and a row
//! that opens Windows' notifications.

use crate::dates;
use crate::dock::{self, Event};
use crate::gfx::{color, Gfx};
use crate::glass::{rgba, Glass, Palette, Pill, Popup};
use crate::menu::Input;
use crate::text::{self, Line, ICON_FONT, UI_FONT};
use crate::util::wide;
use std::cell::Cell;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "BenCalendarWindow";
const WM_MOUSELEAVE: u32 = 0x02A3;
const VK_ESCAPE: u16 = 0x1B;
const VK_LEFT: u16 = 0x25;
const VK_RIGHT: u16 = 0x27;

/// What a click or key press in the panel led to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Nothing,
    Closed,
    /// The "notifications" row was clicked.
    Notifications,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Part {
    Previous,
    Next,
    /// The month's name: back to this month.
    Month,
    Notifications,
}

struct Fonts {
    date: IDWriteTextFormat,
    month: IDWriteTextFormat,
    day: IDWriteTextFormat,
    small: IDWriteTextFormat,
    icons: IDWriteTextFormat,
}

pub struct Calendar {
    hwnd: HWND,
    popup: Popup,
    /// Under the cursor (arrows, month name, notifications row).
    hover_fill: Pill,
    /// The circle behind today's number.
    today_fill: Pill,
    rule: Pill,
    date: Line,
    month_name: Line,
    previous: Line,
    next: Line,
    heads: Vec<Line>,
    days: Vec<Line>,
    notifications: Line,
    fonts: Option<Fonts>,
    open: bool,
    today: (i32, u32, u32),
    /// The month on show.
    year: i32,
    month: u32,
    scale: f32,
    dark: bool,
    accent: (f32, f32, f32),
    /// Screen position of the panel's top-right corner, and the monitor.
    anchor: (i32, i32),
    bounds: RECT,
    cursor: Option<(f32, f32)>,
    /// Clickable parts: (part, left, top, right, bottom).
    parts: Vec<(Part, f32, f32, f32, f32)>,
    size: (f32, f32),
    /// Where the window was last put, so it is only moved when that changes.
    placed: Option<(i32, i32, i32, i32)>,
}

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let send = |input: Input| dock::post(Event::Calendar(input));
    let (x, y) = ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_MOUSEMOVE => {
            if !TRACKING.get() {
                let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                TRACKING.set(unsafe { TrackMouseEvent(&mut track) }.is_ok());
            }
            send(Input::Move(x, y));
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            TRACKING.set(false);
            send(Input::Leave);
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            send(Input::Click(x, y));
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            send(Input::Key(wparam.0 as u16));
            return LRESULT(0);
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xffff) == 0 {
                send(Input::Lost(0));
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            send(Input::Lost(0));
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Calendar {
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
            // Takes focus while open, like a menu: that is how it closes on an outside click.
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(wide("Bến - Lịch").as_ptr()),
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
            // Fills first, so they lie under the text.
            let hover_fill = glass.pill_in(popup.content()?)?;
            let today_fill = glass.pill_in(popup.content()?)?;
            let rule = glass.pill_in(popup.content()?)?;
            let line = || -> Result<Line> { Ok(Line::new(glass.sprite_in(popup.content()?)?)) };
            Ok(Calendar {
                hwnd,
                hover_fill,
                today_fill,
                rule,
                date: line()?,
                month_name: line()?,
                previous: line()?,
                next: line()?,
                heads: (0..7).map(|_| line()).collect::<Result<Vec<_>>>()?,
                days: (0..31).map(|_| line()).collect::<Result<Vec<_>>>()?,
                notifications: line()?,
                popup,
                fonts: None,
                open: false,
                today: (2000, 1, 1),
                year: 2000,
                month: 1,
                scale: 1.0,
                dark: true,
                accent: (0.2, 0.5, 1.0),
                anchor: (0, 0),
                bounds: RECT::default(),
                cursor: None,
                parts: Vec::new(),
                size: (1.0, 1.0),
                placed: None,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_palette(&self, palette: &Palette) {
        let _ = self.popup.set_palette(palette);
    }

    /// Forgets painted text and fonts (after a theme or GPU change). An open panel is closed.
    pub fn forget(&mut self) {
        self.close();
        self.fonts = None;
        let lines = [&mut self.date, &mut self.month_name, &mut self.previous, &mut self.next, &mut self.notifications];
        for line in lines.into_iter().chain(self.heads.iter_mut()).chain(self.days.iter_mut()) {
            line.forget();
        }
    }

    /// Shows the panel for `today`, its top-right corner at (`right`, `top`) on screen.
    pub fn open(&mut self, glass: &Glass, gfx: &Gfx, today: (i32, u32, u32), right: i32, top: i32, bounds: RECT, scale: f32, dark: bool, accent: (f32, f32, f32)) -> Result<()> {
        let s = scale.max(0.5);
        if self.scale != s || self.dark != dark {
            // Text is painted for one size and one theme.
            self.fonts = None;
        }
        (self.scale, self.dark, self.accent, self.today, self.anchor, self.bounds) = (s, dark, accent, today, (right, top), bounds);
        (self.year, self.month) = (today.0, today.1.clamp(1, 12));
        self.cursor = None;
        self.open = true;
        self.update(glass, gfx)?;
        crate::shell::activate(self.hwnd);
        Ok(())
    }

    pub fn close(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        self.placed = None;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    fn part_at(&self, x: f32, y: f32) -> Option<Part> {
        self.parts.iter().find(|(_, l, t, r, b)| x >= *l && x < *r && y >= *t && y < *b).map(|(p, ..)| *p)
    }

    pub fn handle(&mut self, input: Input) -> Outcome {
        if !self.open {
            return Outcome::Nothing;
        }
        match input {
            Input::Move(x, y) => self.cursor = Some((x as f32, y as f32)),
            Input::Leave => self.cursor = None,
            Input::Down(..) | Input::Char(_) => {}
            Input::Click(x, y) => match self.part_at(x as f32, y as f32) {
                Some(Part::Previous) => (self.year, self.month) = dates::shift_month(self.year, self.month, -1),
                Some(Part::Next) => (self.year, self.month) = dates::shift_month(self.year, self.month, 1),
                Some(Part::Month) => (self.year, self.month) = (self.today.0, self.today.1),
                Some(Part::Notifications) => return Outcome::Notifications,
                None => {}
            },
            Input::Key(VK_ESCAPE) => return Outcome::Closed,
            Input::Key(VK_LEFT) => (self.year, self.month) = dates::shift_month(self.year, self.month, -1),
            Input::Key(VK_RIGHT) => (self.year, self.month) = dates::shift_month(self.year, self.month, 1),
            Input::Key(_) => {}
            Input::Lost(_) => return Outcome::Closed,
        }
        Outcome::Nothing
    }

    /// Paints and places everything for the month on show, and sizes the window to fit it.
    pub fn update(&mut self, glass: &Glass, gfx: &Gfx) -> Result<()> {
        if !self.open {
            return Ok(());
        }
        let s = self.scale;
        let px = |v: f32| (v * s).round();
        if self.fonts.is_none() {
            self.fonts = Some(Fonts {
                date: text::format(gfx, UI_FONT, px(15.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                month: text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                day: text::format(gfx, UI_FONT, px(12.5), DWRITE_FONT_WEIGHT_NORMAL)?,
                small: text::format(gfx, UI_FONT, px(11.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                icons: text::format(gfx, ICON_FONT, px(11.0), DWRITE_FONT_WEIGHT_NORMAL)?,
            });
        }
        let fonts = self.fonts.as_ref().unwrap();
        let (fg, dim) = if self.dark { (color(1.0, 1.0, 1.0, 0.94), color(1.0, 1.0, 1.0, 0.42)) } else { (color(0.0, 0.0, 0.0, 0.88), color(0.0, 0.0, 0.0, 0.40)) };
        let white = color(1.0, 1.0, 1.0, 1.0);
        let glyph = |code: u32| char::from_u32(code).map(String::from).unwrap_or_default();

        let (year, month) = (self.year, self.month);
        let rows = dates::week_rows(year, month);
        let (pad, cell_w, cell_h) = (px(14.0), px(34.0), px(30.0));
        let w = pad * 2.0 + cell_w * 7.0;
        self.parts.clear();

        // Today, in words.
        let (ty, tm, td) = self.today;
        let today_text = format!("{}, {} tháng {}", dates::WEEKDAYS[dates::weekday(ty, tm, td) as usize], td, tm);
        self.date.set(glass, gfx, &fonts.date, &today_text, fg, Some(w - 2.0 * pad))?;
        let mut y = px(14.0);
        self.date.place(pad + px(4.0), y)?;
        y += self.date.h + px(12.0);

        // The month on show, with arrows to the months around it.
        self.month_name.set(glass, gfx, &fonts.month, &format!("Tháng {month} năm {year}"), fg, None)?;
        self.previous.set(glass, gfx, &fonts.icons, &glyph(0xE76B), fg, None)?;
        self.next.set(glass, gfx, &fonts.icons, &glyph(0xE76C), fg, None)?;
        let row_h = px(24.0);
        let mid = |line: &Line, top: f32, height: f32| top + (height - line.h) * 0.5;
        let my = mid(&self.month_name, y, row_h);
        self.month_name.place(pad + px(4.0), my)?;
        self.parts.push((Part::Month, pad, y, pad + px(8.0) + self.month_name.w, y + row_h));
        let button = px(26.0);
        let (next_x, prev_x) = (w - pad - button, w - pad - 2.0 * button);
        let (ny, nw) = (mid(&self.next, y, row_h), self.next.w);
        self.next.place(next_x + (button - nw) * 0.5, ny)?;
        let (py, pw) = (mid(&self.previous, y, row_h), self.previous.w);
        self.previous.place(prev_x + (button - pw) * 0.5, py)?;
        self.parts.push((Part::Previous, prev_x, y, prev_x + button, y + row_h));
        self.parts.push((Part::Next, next_x, y, next_x + button, y + row_h));
        y += row_h + px(6.0);

        // Monday to Sunday.
        for (index, head) in self.heads.iter_mut().enumerate() {
            head.set(glass, gfx, &fonts.small, dates::WEEKDAYS_SHORT[index], dim, None)?;
            let (hx, hy) = (pad + cell_w * index as f32 + (cell_w - head.w) * 0.5, y + (px(20.0) - head.h) * 0.5);
            head.place(hx, hy)?;
        }
        y += px(22.0);

        // The days. Only this month's are shown; the rest of the grid stays empty.
        let count = dates::days_in_month(year, month);
        let is_this_month = (year, month) == (ty, tm);
        let mut today_cell = None;
        for (index, line) in self.days.iter_mut().enumerate() {
            let day = index as u32 + 1;
            if day > count {
                line.set(glass, gfx, &fonts.day, "", fg, None)?;
                continue;
            }
            let (col, row) = dates::cell(year, month, day);
            let is_today = is_this_month && day == td;
            line.set(glass, gfx, &fonts.day, &day.to_string(), if is_today { white } else { fg }, None)?;
            let (cx, cy) = (pad + cell_w * col as f32, y + cell_h * row as f32);
            line.place(cx + (cell_w - line.w) * 0.5, cy + (cell_h - line.h) * 0.5)?;
            if is_today {
                today_cell = Some((cx, cy));
            }
        }
        match today_cell {
            Some((cx, cy)) => {
                let d = px(26.0);
                let (r, g, b) = self.accent;
                self.today_fill.place((cx + (cell_w - d) * 0.5).round(), (cy + (cell_h - d) * 0.5).round(), d, d, d * 0.5)?;
                self.today_fill.set_color(rgba(r, g, b, 1.0))?;
                self.today_fill.set_opacity(1.0)?;
            }
            None => self.today_fill.set_opacity(0.0)?,
        }
        y += cell_h * rows as f32 + px(6.0);

        // A hairline, then the way into Windows' notifications.
        let c = if self.dark { 1.0 } else { 0.0 };
        self.rule.place(pad, y, w - 2.0 * pad, s.round().max(1.0), 0.0)?;
        self.rule.set_color(rgba(c, c, c, if self.dark { 0.16 } else { 0.13 }))?;
        self.rule.set_opacity(1.0)?;
        y += px(6.0);
        let note_h = px(26.0);
        self.notifications.set(glass, gfx, &fonts.day, "Thông báo của Windows", fg, None)?;
        let ny = mid(&self.notifications, y, note_h);
        self.notifications.place(pad + px(4.0), ny)?;
        self.parts.push((Part::Notifications, px(5.0), y, w - px(5.0), y + note_h));
        y += note_h + px(6.0);
        let h = y;

        // What is under the cursor.
        let hot = self.cursor.and_then(|(x, y)| self.parts.iter().find(|(_, l, t, r, b)| x >= *l && x < *r && y >= *t && y < *b).copied());
        match hot {
            Some((_, l, t, r, b)) => {
                self.hover_fill.place(l, t, r - l, b - t, px(5.0))?;
                self.hover_fill.set_color(rgba(c, c, c, 0.12))?;
                self.hover_fill.set_opacity(1.0)?;
            }
            None => self.hover_fill.set_opacity(0.0)?,
        }

        // The window follows the month: a month of four weeks is shorter than one of six.
        if self.size != (w, h) {
            self.size = (w, h);
            self.popup.set_frame(0.0, 0.0, w, h, px(12.0))?;
        }
        let (wi, hi) = (w.ceil() as i32, h.ceil() as i32);
        let margin = px(6.0) as i32;
        let left = (self.anchor.0 - wi).min(self.bounds.right - wi - margin).max(self.bounds.left + margin);
        let top = self.anchor.1.min(self.bounds.bottom - hi - margin).max(self.bounds.top + margin);
        if self.placed != Some((left, top, wi, hi)) {
            self.placed = Some((left, top, wi, hi));
            unsafe {
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), left, top, wi, hi, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            }
        }
        Ok(())
    }
}

impl Drop for Calendar {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
