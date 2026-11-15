//! Drop-down and context menus drawn by the dock itself: a rounded glass
//! panel, one row per command, the row under the cursor filled with the
//! accent colour, a shortcut hint on the right and a tick on the left.
//!
//! The menu window takes the keyboard while it is open (arrows, Enter, Esc)
//! and closes as soon as another window is activated.

use crate::dock::{self, Event};
use crate::gfx::{color, Gfx};
use crate::glass::{rgba, Glass, Palette, Pill, Popup};
use crate::text::{self, Line, ICON_FONT, UI_FONT};
use crate::util::wide;
use std::cell::Cell;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "BenMenuWindow";
const WM_MOUSELEAVE: u32 = 0x02A3;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;

/// What the menu window reports to the dock.
#[derive(Clone, Copy, Debug)]
pub enum Input {
    Move(i32, i32),
    Leave,
    /// Left button pressed.
    Down(i32, i32),
    /// A button released.
    Click(i32, i32),
    /// A typed character (UTF-16 unit).
    Char(u16),
    Key(u16),
    /// Another window was activated. Carries the number of the menu that was showing when it
    /// happened (see `SHOWING`), so a report about an earlier menu can be told apart.
    Lost(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Item,
    Separator,
    /// A dimmed line that cannot be chosen.
    Header,
    /// The menu's name, in bold. Cannot be chosen.
    Title,
    /// A slider from 0 to 1 (the entry's `value`).
    Slider,
    /// A field to type a password into; the entry's text is shown while it is empty.
    Input,
}

/// One row of a menu.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: usize,
    pub text: String,
    /// Shown on the right (a keyboard shortcut).
    pub hint: String,
    pub enabled: bool,
    pub checked: bool,
    pub kind: Kind,
    /// Position of a slider, 0 to 1.
    pub value: f32,
    /// The hint is made of symbols (signal strength, a lock), not words.
    pub symbols: bool,
}

impl Entry {
    pub fn item(id: usize, text: &str) -> Self {
        Entry { id, text: text.to_string(), hint: String::new(), enabled: true, checked: false, kind: Kind::Item, value: 0.0, symbols: false }
    }

    pub fn separator() -> Self {
        Entry { id: 0, text: String::new(), hint: String::new(), enabled: false, checked: false, kind: Kind::Separator, value: 0.0, symbols: false }
    }

    pub fn header(text: &str) -> Self {
        Entry { id: 0, text: text.to_string(), hint: String::new(), enabled: false, checked: false, kind: Kind::Header, value: 0.0, symbols: false }
    }

    pub fn title(text: &str) -> Self {
        Entry { id: 0, text: text.to_string(), hint: String::new(), enabled: false, checked: false, kind: Kind::Title, value: 0.0, symbols: false }
    }

    pub fn slider(id: usize, value: f32) -> Self {
        Entry { id, text: String::new(), hint: String::new(), enabled: true, checked: false, kind: Kind::Slider, value: value.clamp(0.0, 1.0), symbols: false }
    }

    pub fn input(id: usize, placeholder: &str) -> Self {
        Entry { id, text: placeholder.to_string(), hint: String::new(), enabled: true, checked: false, kind: Kind::Input, value: 0.0, symbols: false }
    }

    /// A hint made of symbols from Windows' symbol font.
    pub fn symbols(mut self, symbols: &str) -> Self {
        self.hint = symbols.to_string();
        self.symbols = true;
        self
    }

    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = hint.to_string();
        self
    }

    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    pub fn enabled(mut self, on: bool) -> Self {
        self.enabled = on;
        self
    }
}

/// Where a menu opens, in screen pixels.
#[derive(Clone, Copy, Debug)]
pub enum Anchor {
    /// Hanging down from this top-left corner (from the top bar).
    Below { left: i32, top: i32 },
    /// Standing on this point, centred on it (above a dock icon).
    Above { center: i32, bottom: i32 },
    /// Beside another menu: to its right (or its left if there is no room), with the top at
    /// `top`. `left` and `right` are the other menu's edges.
    Beside { left: i32, right: i32, top: i32 },
}

/// What a click or a key press in the menu led to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Nothing,
    Closed,
    Chosen(usize),
    /// A slider was moved: its id and new position (0 to 1). The menu stays open.
    Value(usize, f32),
    /// Enter was pressed in a text field (its id). The text is in `Menu::typed`.
    Entered(usize),
    /// Left / right arrow: go to the neighbouring menu of the top bar.
    Step(i32),
}

struct Row {
    entry: Entry,
    label: Line,
    hint: Line,
    check: Line,
    /// The thin line of a separator.
    rule: Option<Pill>,
    /// A slider's track, its filled part and its knob; or a text field's background.
    slider: Vec<Pill>,
    y: f32,
    h: f32,
}

struct Fonts {
    regular: IDWriteTextFormat,
    header: IDWriteTextFormat,
    title: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    scale: f32,
}

pub struct Menu {
    hwnd: HWND,
    popup: Popup,
    highlight: Pill,
    rows: Vec<Row>,
    fonts: Option<Fonts>,
    open: bool,
    hot: Option<usize>,
    scale: f32,
    dark: bool,
    accent: (f32, f32, f32),
    width: f32,
    /// Left edge of the labels.
    text_x: f32,
    /// The slider being dragged.
    dragging: Option<usize>,
    /// What has been typed into the text field, and whether it is shown as typed or as dots.
    typed: String,
    reveal: bool,
    /// This menu's number (see `SHOWING`).
    number: u32,
    /// Where the window is on the screen.
    origin: (i32, i32),
    /// The row (by id) that stays marked while a panel beside this menu belongs to it.
    pinned: Option<usize>,
}

thread_local! {
    /// The window whose mouse-leave is being tracked (0: none).
    static TRACKING: Cell<isize> = const { Cell::new(0) };
    /// The window of the panel that opens beside a menu (0: none); its reports are sent as
    /// `Event::Side`.
    static SIDE: Cell<isize> = const { Cell::new(0) };
    /// Counts menus as they are shown; the window procedure stamps its reports with it.
    static SHOWING: Cell<u32> = const { Cell::new(0) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let me = hwnd.0 as isize;
    let send = |input: Input| dock::post(if me == SIDE.get() { Event::Side(input) } else { Event::Menu(input) });
    let (x, y) = ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_MOUSEMOVE => {
            if TRACKING.get() != me {
                let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                if unsafe { TrackMouseEvent(&mut track) }.is_ok() {
                    TRACKING.set(me);
                }
            }
            send(Input::Move(x, y));
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            if TRACKING.get() == me {
                TRACKING.set(0);
            }
            send(Input::Leave);
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            // Keep receiving the mouse while a slider is dragged past the menu's edge.
            unsafe {
                SetCapture(hwnd);
            }
            send(Input::Down(x, y));
            return LRESULT(0);
        }
        WM_LBUTTONUP | WM_RBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            send(Input::Click(x, y));
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            send(Input::Key(wparam.0 as u16));
            return LRESULT(0);
        }
        WM_CHAR => {
            send(Input::Char(wparam.0 as u16));
            return LRESULT(0);
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xffff) == 0 {
                send(Input::Lost(SHOWING.get()));
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            // Alt+F4 on a menu closes the menu, not the dock.
            send(Input::Lost(SHOWING.get()));
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Menu {
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
            // This window takes focus while it is open: that is how it gets the keyboard and
            // how it learns that the user clicked somewhere else.
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(wide("Bến - Menu").as_ptr()),
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
            // Made first, so it lies under the text of the rows.
            let highlight = glass.pill_in(popup.content()?)?;
            Ok(Menu { hwnd, popup, highlight, rows: Vec::new(), fonts: None, open: false, hot: None, scale: 1.0, dark: true, accent: (0.2, 0.5, 1.0), width: 1.0, text_x: 0.0, dragging: None, typed: String::new(), reveal: false, number: 0, origin: (0, 0), pinned: None })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Makes this the panel that opens beside the main menu: its reports come as `Event::Side`.
    pub fn make_side(&self) {
        SIDE.set(self.hwnd.0 as isize);
    }

    /// Takes the "another window was activated" reports that are still on their way as being
    /// about this menu again (a panel beside it was closed and this one has the focus back).
    pub fn rearm(&mut self) {
        self.number = SHOWING.get();
    }

    /// Keeps the row with this id marked while a panel beside the menu belongs to it.
    pub fn pin(&mut self, id: Option<usize>) {
        self.pinned = id;
    }

    /// Where the row with this id is on the screen: left, top, right, bottom.
    pub fn row_rect(&self, id: usize) -> Option<(i32, i32, i32, i32)> {
        let row = self.rows.iter().find(|r| r.entry.id == id)?;
        Some((self.origin.0, self.origin.1 + row.y.round() as i32, self.origin.0 + self.width.round() as i32, self.origin.1 + (row.y + row.h).round() as i32))
    }

    /// Puts text into the field (a panel opened again after a note, keeping what was typed).
    pub fn set_typed(&mut self, text: &str) {
        if self.field().is_some() {
            self.typed = text.to_string();
        }
    }

    pub fn set_palette(&self, palette: &Palette) {
        let _ = self.popup.set_palette(palette);
    }

    /// Forgets painted text and fonts (after a GPU change). An open menu is closed.
    pub fn forget(&mut self) {
        self.close();
        self.fonts = None;
    }

    fn colors(&self) -> (windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F, windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F) {
        if self.dark {
            (color(1.0, 1.0, 1.0, 0.94), color(1.0, 1.0, 1.0, 0.40))
        } else {
            (color(0.0, 0.0, 0.0, 0.88), color(0.0, 0.0, 0.0, 0.36))
        }
    }

    /// Shows the menu with these rows. `bounds` is the monitor: the menu is kept inside it.
    pub fn open(&mut self, glass: &Glass, gfx: &Gfx, entries: Vec<Entry>, anchor: Anchor, bounds: RECT, scale: f32, dark: bool, accent: (f32, f32, f32)) -> Result<()> {
        let s = scale.max(0.5);
        let px = |v: f32| (v * s).round();
        (self.scale, self.dark, self.accent) = (s, dark, accent);
        if self.fonts.as_ref().map(|f| f.scale != s).unwrap_or(true) {
            self.fonts = Some(Fonts {
                regular: text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                header: text::format(gfx, UI_FONT, px(11.5), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                title: text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                icons: text::format(gfx, ICON_FONT, px(11.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                scale: s,
            });
        }
        let (fg, dim) = self.colors();

        // The rows, painted once in their resting colours to learn how wide they are.
        self.rows.clear();
        self.hot = None;
        self.dragging = None;
        self.typed.clear();
        self.reveal = false;
        let any_tick = entries.iter().any(|e| e.checked);
        let any_slider = entries.iter().any(|e| matches!(e.kind, Kind::Slider | Kind::Input));
        let (pad, inset) = (px(5.0), px(if any_tick { 28.0 } else { 14.0 }));
        let (mut y, mut widest_label, mut widest_hint) = (pad, 0.0f32, 0.0f32);
        {
            let fonts = self.fonts.as_ref().unwrap();
            for entry in entries {
                let line = || -> Result<Line> { Ok(Line::new(glass.sprite_in(self.popup.content()?)?)) };
                let (h, rule) = match entry.kind {
                    Kind::Separator => (px(11.0), Some(glass.pill_in(self.popup.content()?)?)),
                    Kind::Header => (px(20.0), None),
                    Kind::Title => (px(26.0), None),
                    Kind::Slider => (px(30.0), None),
                    Kind::Input => (px(34.0), None),
                    Kind::Item => (px(24.0), None),
                };
                // Fills are made before the row's text, so they lie under it.
                let mut slider = Vec::new();
                let fills = match entry.kind {
                    Kind::Slider => 3,
                    Kind::Input => 1,
                    _ => 0,
                };
                for _ in 0..fills {
                    slider.push(glass.pill_in(self.popup.content()?)?);
                }
                let (mut label, mut hint, check) = (line()?, line()?, line()?);
                match entry.kind {
                    Kind::Separator | Kind::Slider => {}
                    Kind::Input => label.set(glass, gfx, &fonts.regular, &entry.text, dim, Some(px(300.0)))?,
                    Kind::Header => label.set(glass, gfx, &fonts.header, &entry.text, dim, Some(px(360.0)))?,
                    Kind::Title => label.set(glass, gfx, &fonts.title, &entry.text, fg, Some(px(360.0)))?,
                    Kind::Item => {
                        label.set(glass, gfx, &fonts.regular, &entry.text, if entry.enabled { fg } else { dim }, Some(px(360.0)))?;
                        hint.set(glass, gfx, if entry.symbols { &fonts.icons } else { &fonts.regular }, &entry.hint, dim, None)?;
                    }
                }
                widest_label = widest_label.max(label.w);
                widest_hint = widest_hint.max(hint.w);
                self.rows.push(Row { entry, label, hint, check, rule, slider, y, h });
                y += h;
            }
        }
        let gap = if widest_hint > 0.0 { px(34.0) + widest_hint } else { 0.0 };
        let w = (pad + inset + widest_label + gap + px(16.0) + pad).max(px(if any_slider { 250.0 } else { 190.0 }));
        let h = y + pad;
        (self.width, self.text_x) = (w, pad + inset);

        // Separators: a hairline across the menu.
        for row in &mut self.rows {
            if let Some(rule) = row.rule.as_mut() {
                let c = if dark { 1.0 } else { 0.0 };
                rule.place(pad + px(9.0), (row.y + row.h * 0.5).floor(), w - 2.0 * (pad + px(9.0)), s.round().max(1.0), 0.0)?;
                rule.set_color(rgba(c, c, c, if dark { 0.16 } else { 0.13 }))?;
                rule.set_opacity(1.0)?;
            }
        }

        let (wi, hi) = (w.ceil() as i32, h.ceil() as i32);
        let (mut left, mut top) = match anchor {
            Anchor::Below { left, top } => (left, top),
            Anchor::Above { center, bottom } => (center - wi / 2, bottom - hi),
            Anchor::Beside { left: l, right: r, top } => {
                let gap = px(3.0) as i32;
                let x = if r + gap + wi + px(6.0) as i32 <= bounds.right { r + gap } else { l - gap - wi };
                (x, top - pad as i32)
            }
        };
        let margin = px(6.0) as i32;
        left = left.min(bounds.right - wi - margin).max(bounds.left + margin);
        top = top.min(bounds.bottom - hi - margin).max(bounds.top + margin);
        self.origin = (left, top);
        self.pinned = None;
        self.popup.set_frame(0.0, 0.0, w, h, px(9.0))?;
        self.open = true;
        // From here on, a "lost focus" report stamped with an older number is about a menu
        // that was hidden just before this one was shown, and is ignored.
        SHOWING.set(SHOWING.get().wrapping_add(1));
        self.number = SHOWING.get();
        self.update(glass, gfx)?;
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), left, top, wi, hi, SWP_SHOWWINDOW);
        }
        crate::shell::activate(self.hwnd);
        if unsafe { GetForegroundWindow() } != self.hwnd {
            crate::log!("menu: Windows did not give the menu the keyboard; it will close when another window comes to the front");
        }
        Ok(())
    }

    /// Hides the menu.
    pub fn close(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        self.hot = None;
        self.dragging = None;
        self.typed.clear();
        unsafe {
            let _ = ReleaseCapture();
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
        self.rows.clear();
        let _ = self.highlight.set_opacity(0.0);
    }

    fn row_at(&self, y: f32) -> Option<usize> {
        self.rows.iter().position(|r| y >= r.y && y < r.y + r.h).filter(|&i| self.rows[i].entry.kind == Kind::Item && self.rows[i].entry.enabled)
    }

    /// The id of the menu's text field, if it has one.
    fn field(&self) -> Option<usize> {
        self.rows.iter().find(|r| r.entry.kind == Kind::Input).map(|r| r.entry.id)
    }

    /// What has been typed into the text field.
    pub fn typed(&self) -> String {
        self.typed.clone()
    }

    /// Left and right end of a slider's track.
    fn track(&self) -> (f32, f32) {
        let inset = ((5.0 + 16.0) * self.scale).round();
        (inset, self.width - inset)
    }

    /// Moves slider `index` to where the cursor is. Returns what to report.
    fn slide(&mut self, index: usize, x: f32) -> Outcome {
        let (a, b) = self.track();
        let value = ((x - a) / (b - a).max(1.0)).clamp(0.0, 1.0);
        match self.rows.get_mut(index) {
            Some(row) => {
                row.entry.value = value;
                Outcome::Value(row.entry.id, value)
            }
            None => Outcome::Nothing,
        }
    }

    /// The next row that can be chosen, going up (-1) or down (+1) from the highlighted one.
    fn step(&self, dir: i32) -> Option<usize> {
        let n = self.rows.len() as i32;
        if n == 0 {
            return None;
        }
        let mut i = match (self.hot, dir > 0) {
            (Some(i), _) => i as i32,
            (None, true) => -1,
            (None, false) => n,
        };
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            let e = &self.rows[i as usize].entry;
            if e.kind == Kind::Item && e.enabled {
                return Some(i as usize);
            }
        }
        None
    }

    pub fn handle(&mut self, input: Input) -> Outcome {
        if !self.open {
            return Outcome::Nothing;
        }
        match input {
            Input::Down(x, y) => {
                let on = self.rows.iter().position(|r| (y as f32) >= r.y && (y as f32) < r.y + r.h);
                if let Some(index) = on.filter(|&i| self.rows[i].entry.kind == Kind::Slider) {
                    self.dragging = Some(index);
                    return self.slide(index, x as f32);
                }
            }
            Input::Move(x, y) => {
                if let Some(index) = self.dragging {
                    return self.slide(index, x as f32);
                }
                self.hot = if x >= 0 && (x as f32) < self.width { self.row_at(y as f32) } else { None };
            }
            Input::Leave => self.hot = None,
            Input::Click(x, y) => {
                if let Some(index) = self.dragging.take() {
                    // Letting go of a slider sets it one last time and leaves the menu open.
                    return self.slide(index, x as f32);
                }
                let field = self.rows.iter().find(|r| r.entry.kind == Kind::Input && (y as f32) >= r.y && (y as f32) < r.y + r.h);
                if field.is_some() {
                    // The right end of the field is the eye.
                    if (x as f32) > self.width - (56.0 * self.scale).round() {
                        self.reveal = !self.reveal;
                    }
                    return Outcome::Nothing;
                }
                if x >= 0 && (x as f32) < self.width {
                    if let Some(i) = self.row_at(y as f32) {
                        return Outcome::Chosen(self.rows[i].entry.id);
                    }
                }
            }
            Input::Char(unit) => {
                if self.field().is_some() {
                    match unit {
                        0x08 => {
                            self.typed.pop();
                        }
                        // A Wi-Fi password is at most 63 characters.
                        0x20..=0xD7FF if unit != 0x7F && self.typed.chars().count() < 63 => {
                            if let Some(c) = char::from_u32(unit as u32) {
                                self.typed.push(c);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Input::Key(VK_ESCAPE) => return Outcome::Closed,
            // While typing, Enter sends the text and the arrows left and right do nothing.
            Input::Key(VK_RETURN) if self.field().is_some() && !self.typed.is_empty() => {
                if let Some(id) = self.field() {
                    return Outcome::Entered(id);
                }
            }
            Input::Key(VK_LEFT) | Input::Key(VK_RIGHT) if self.field().is_some() => {}
            Input::Key(VK_DOWN) => self.hot = self.step(1),
            Input::Key(VK_UP) => self.hot = self.step(-1),
            Input::Key(VK_LEFT) => return Outcome::Step(-1),
            Input::Key(VK_RIGHT) => return Outcome::Step(1),
            Input::Key(VK_RETURN) => {
                if let Some(i) = self.hot {
                    return Outcome::Chosen(self.rows[i].entry.id);
                }
            }
            Input::Key(_) => {}
            Input::Lost(number) => {
                // A report left over from an earlier menu (hidden, then another opened at once)
                // must not close this one.
                if number == self.number {
                    return Outcome::Closed;
                }
            }
        }
        Outcome::Nothing
    }

    /// Paints and places the rows: the highlighted one is filled with the accent colour and
    /// its text turns white.
    pub fn update(&mut self, glass: &Glass, gfx: &Gfx) -> Result<()> {
        if !self.open {
            return Ok(());
        }
        let Some(fonts) = self.fonts.as_ref() else { return Ok(()) };
        let s = self.scale;
        let px = |v: f32| (v * s).round();
        let (fg, dim) = self.colors();
        let white = color(1.0, 1.0, 1.0, 1.0);
        let (pad, w, text_x) = (px(5.0), self.width, self.text_x);
        let tick = char::from_u32(0xE73E).map(String::from).unwrap_or_default();
        for (index, row) in self.rows.iter_mut().enumerate() {
            let hot = self.hot == Some(index);
            match row.entry.kind {
                Kind::Separator => continue,
                Kind::Slider => {
                    let inset = px(5.0 + 16.0);
                    let (a, b) = (inset, w - inset);
                    let (cy, thick, knob) = (row.y + row.h * 0.5, px(6.0), px(16.0));
                    let at = a + (b - a) * row.entry.value;
                    let c = if self.dark { 1.0 } else { 0.0 };
                    let (r, g, bl) = self.accent;
                    if let [track, fill, thumb] = row.slider.as_mut_slice() {
                        track.place(a, (cy - thick * 0.5).round(), b - a, thick, thick * 0.5)?;
                        track.set_color(rgba(c, c, c, 0.20))?;
                        track.set_opacity(1.0)?;
                        fill.place(a, (cy - thick * 0.5).round(), (at - a).max(thick), thick, thick * 0.5)?;
                        fill.set_color(rgba(r, g, bl, 1.0))?;
                        fill.set_opacity(1.0)?;
                        thumb.place((at - knob * 0.5).round(), (cy - knob * 0.5).round(), knob, knob, knob * 0.5)?;
                        thumb.set_color(if self.dark { rgba(1.0, 1.0, 1.0, 1.0) } else { rgba(r, g, bl, 1.0) })?;
                        thumb.set_opacity(1.0)?;
                    }
                    continue;
                }
                Kind::Input => {
                    // The field, then dots (or the text itself), or the placeholder while empty.
                    let (fx, fw) = (pad + px(9.0), w - 2.0 * (pad + px(9.0)));
                    let c = if self.dark { 1.0 } else { 0.0 };
                    if let [field] = row.slider.as_mut_slice() {
                        field.place(fx, row.y + px(4.0), fw, row.h - px(8.0), px(6.0))?;
                        field.set_color(rgba(c, c, c, if self.dark { 0.12 } else { 0.07 }))?;
                        field.set_opacity(1.0)?;
                    }
                    let shown = if self.typed.is_empty() {
                        row.entry.text.clone()
                    } else if self.reveal {
                        self.typed.clone()
                    } else {
                        "\u{2022}".repeat(self.typed.chars().count())
                    };
                    row.label.set(glass, gfx, &fonts.regular, &shown, if self.typed.is_empty() { dim } else { fg }, Some(fw - px(44.0)))?;
                    let y = row.y + (row.h - row.label.h) * 0.5;
                    row.label.place(fx + px(9.0), y)?;
                    // An eye at the right end: click it to see what was typed.
                    let eye = char::from_u32(0xE7B3).map(String::from).unwrap_or_default();
                    row.hint.set(glass, gfx, &fonts.icons, &eye, if self.reveal { fg } else { dim }, None)?;
                    let y = row.y + (row.h - row.hint.h) * 0.5;
                    row.hint.place(fx + fw - px(11.0) - row.hint.w, y)?;
                    continue;
                }
                Kind::Header => {
                    row.label.set(glass, gfx, &fonts.header, &row.entry.text, dim, Some(px(360.0)))?;
                }
                Kind::Title => {
                    row.label.set(glass, gfx, &fonts.title, &row.entry.text, fg, Some(px(360.0)))?;
                }
                Kind::Item => {
                    let main = if hot { white } else if row.entry.enabled { fg } else { dim };
                    row.label.set(glass, gfx, &fonts.regular, &row.entry.text, main, Some(px(360.0)))?;
                    row.hint.set(glass, gfx, if row.entry.symbols { &fonts.icons } else { &fonts.regular }, &row.entry.hint, if hot { white } else { dim }, None)?;
                    row.check.set(glass, gfx, &fonts.icons, if row.entry.checked { &tick } else { "" }, main, None)?;
                }
            }
            let mid = |line: &Line| row.y + (row.h - line.h) * 0.5;
            let y = mid(&row.label);
            row.label.place(text_x, y)?;
            if row.hint.w > 0.0 {
                let y = mid(&row.hint);
                row.hint.place(w - pad - px(11.0) - row.hint.w, y)?;
            }
            if row.check.w > 0.0 {
                let y = mid(&row.check);
                row.check.place(pad + px(9.0), y)?;
            }
        }
        let pinned = self.pinned.and_then(|id| self.rows.iter().position(|r| r.entry.id == id));
        match self.hot.or(pinned).and_then(|i| self.rows.get(i)) {
            Some(row) => {
                let (r, g, b) = self.accent;
                self.highlight.place(pad, row.y, w - 2.0 * pad, row.h, px(5.0))?;
                if self.hot.is_some() {
                    self.highlight.set_color(rgba(r, g, b, 1.0))?;
                } else {
                    let c = if self.dark { 1.0 } else { 0.0 };
                    self.highlight.set_color(rgba(c, c, c, 0.16))?;
                }
                self.highlight.set_opacity(1.0)?;
            }
            None => self.highlight.set_opacity(0.0)?,
        }
        Ok(())
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
