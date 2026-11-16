//! Bubbles: the menus. A piece of glass that swells out of the thing it
//! belongs to (a dock icon, the island's power button), one row per command,
//! and a drop of light that glides to the row under the cursor.
//!
//! The bubble takes the keyboard while it is open (arrows, Enter, Esc) and
//! closes as soon as another window is activated.

use crate::app::{self, Event};
use crate::gfx::Gfx;
use crate::motion::{Glide, Motion};
use crate::scene::{Bead, Glass, Pill, Scene, Stage};
use crate::spring::Spring;
use crate::text::{self, glyph, Line, ICON_FONT, UI_FONT};
use crate::theme::Theme;
use crate::util::wide;
use std::cell::Cell;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "BenMenuWindow";
const WM_MOUSELEAVE: u32 = 0x02A3;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_UP: u16 = 0x26;
const VK_DOWN: u16 = 0x28;

/// What the bubble's window reports to the app.
#[derive(Clone, Copy, Debug)]
pub enum Input {
    Move(i32, i32),
    Leave,
    /// A button released.
    Click(i32, i32),
    Key(u16),
    /// Another window was activated. Carries the number of the bubble that was showing when
    /// it happened (see `SHOWING`), so a report about an earlier bubble can be told apart.
    Lost(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Item,
    Separator,
    /// A dimmed line that cannot be chosen.
    Header,
}

/// One row of a menu.
#[derive(Clone, Debug)]
pub struct Entry {
    pub id: usize,
    pub text: String,
    /// Shown dimmed on the right (a keyboard shortcut).
    pub hint: String,
    pub enabled: bool,
    pub checked: bool,
    pub kind: Kind,
}

impl Entry {
    pub fn item(id: usize, text: &str) -> Self {
        Entry { id, text: text.to_string(), hint: String::new(), enabled: true, checked: false, kind: Kind::Item }
    }

    pub fn separator() -> Self {
        Entry { id: 0, text: String::new(), hint: String::new(), enabled: false, checked: false, kind: Kind::Separator }
    }

    pub fn header(text: &str) -> Self {
        Entry { id: 0, text: text.to_string(), hint: String::new(), enabled: false, checked: false, kind: Kind::Header }
    }

    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }
}

/// Where a bubble opens, in screen pixels. It grows out of that point.
#[derive(Clone, Copy, Debug)]
pub enum Anchor {
    /// Hanging from this point, centred on it (under the island).
    Below { center: i32, top: i32 },
    /// Standing on this point, centred on it (above a dock icon).
    Above { center: i32, bottom: i32 },
}

/// What a click or a key press in the bubble led to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    Nothing,
    Closed,
    Chosen(usize),
}

struct Row {
    entry: Entry,
    label: Line,
    hint: Line,
    check: Line,
    /// The thin line of a separator.
    rule: Option<Pill>,
    y: f32,
    h: f32,
}

struct Fonts {
    regular: IDWriteTextFormat,
    header: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    scale: f32,
}

pub struct Bubble {
    hwnd: HWND,
    _stage: Stage,
    glass: Glass,
    highlight: Bead,
    highlight_at: Glide,
    rows: Vec<Row>,
    fonts: Option<Fonts>,
    open: bool,
    /// The window is on screen (open, or fading away).
    visible: bool,
    hot: Option<usize>,
    scale: f32,
    size: (f32, f32),
    /// Room around the glass inside the window, for its shadow.
    margin: f32,
    /// The point the bubble grows out of, in the glass's own coordinates.
    root: (f32, f32),
    /// 0 = away, 1 = fully out.
    show: Spring,
    /// This bubble's number (see `SHOWING`).
    number: u32,
}

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
    /// Counts bubbles as they are shown; the window procedure stamps its reports with it.
    static SHOWING: Cell<u32> = const { Cell::new(0) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let send = |input: Input| app::post(Event::Bubble(input));
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
        WM_LBUTTONUP | WM_RBUTTONUP => {
            send(Input::Click(x, y));
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            send(Input::Key(wparam.0 as u16));
            return LRESULT(0);
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xffff) == 0 {
                send(Input::Lost(SHOWING.get()));
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            // Alt+F4 on a menu closes the menu, not the app.
            send(Input::Lost(SHOWING.get()));
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Bubble {
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
            let stage = scene.stage(hwnd)?;
            let glass = scene.glass_in(&stage.content()?, &theme.bubble)?;
            let highlight = scene.bead_in(&glass.inner()?)?;
            Ok(Bubble {
                hwnd,
                _stage: stage,
                glass,
                highlight,
                highlight_at: Glide::new(motion),
                rows: Vec::new(),
                fonts: None,
                open: false,
                visible: false,
                hot: None,
                scale: 1.0,
                size: (1.0, 1.0),
                margin: 0.0,
                root: (0.0, 0.0),
                show: Spring::with(0.0, motion.small()),
                number: 0,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_theme(&mut self, theme: &Theme) {
        self.glass.set_material(&theme.bubble);
        // The rows are painted in the old colours.
        self.close_now();
    }

    /// Forgets fonts (after a GPU change). An open bubble is closed.
    pub fn forget(&mut self) {
        self.close_now();
        self.fonts = None;
    }

    /// Shows the bubble with these rows. `bounds` is the monitor: the bubble is kept inside it.
    pub fn open(&mut self, scene: &Scene, gfx: &Gfx, theme: &Theme, entries: Vec<Entry>, anchor: Anchor, bounds: RECT, scale: f32) -> Result<()> {
        let s = scale.max(0.5);
        let px = |v: f32| (v * s).round();
        self.scale = s;
        // Blur and shadow are sized for the screen it opens on.
        self.glass.set_material(&theme.bubble);
        if self.fonts.as_ref().map(|f| f.scale != s).unwrap_or(true) {
            self.fonts = Some(Fonts {
                regular: text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                header: text::format(gfx, UI_FONT, px(11.5), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                icons: text::format(gfx, ICON_FONT, px(11.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                scale: s,
            });
        }
        let (fg, dim) = (theme.text, theme.text_dim);

        self.rows.clear();
        self.hot = None;
        let content = self.glass.outer()?;
        let any_tick = entries.iter().any(|e| e.checked);
        let (pad, inset) = (px(6.0), px(if any_tick { 28.0 } else { 14.0 }));
        let (mut y, mut widest_label, mut widest_hint) = (pad, 0.0f32, 0.0f32);
        {
            let fonts = self.fonts.as_ref().unwrap();
            for entry in entries {
                let (h, rule) = match entry.kind {
                    Kind::Separator => (px(11.0), Some(scene.pill_in(&content)?)),
                    Kind::Header => (px(22.0), None),
                    Kind::Item => (px(27.0), None),
                };
                let (mut label, mut hint, mut check) = (Line::new_in(scene, &content)?, Line::new_in(scene, &content)?, Line::new_in(scene, &content)?);
                match entry.kind {
                    Kind::Separator => {}
                    Kind::Header => label.set(scene, gfx, &fonts.header, &entry.text, dim, Some(px(340.0)))?,
                    Kind::Item => {
                        label.set(scene, gfx, &fonts.regular, &entry.text, if entry.enabled { fg } else { dim }, Some(px(340.0)))?;
                        hint.set(scene, gfx, &fonts.regular, &entry.hint, dim, None)?;
                        if entry.checked {
                            check.set(scene, gfx, &fonts.icons, &glyph(0xE73E), fg, None)?;
                        }
                    }
                }
                widest_label = widest_label.max(label.w);
                widest_hint = widest_hint.max(hint.w);
                self.rows.push(Row { entry, label, hint, check, rule, y, h });
                y += h;
            }
        }
        let gap = if widest_hint > 0.0 { px(30.0) + widest_hint } else { 0.0 };
        let w = (pad + inset + widest_label + gap + px(18.0) + pad).max(px(184.0));
        let h = y + pad;
        self.size = (w, h);

        // Everything has its place for good: the rows do not move while the bubble is open.
        for row in &mut self.rows {
            let mid = |line: &Line| row.y + ((row.h - line.h) * 0.5).round();
            if let Some(rule) = row.rule.as_mut() {
                rule.place(pad + px(9.0), (row.y + row.h * 0.5).floor(), w - 2.0 * (pad + px(9.0)), s.round().max(1.0), 0.0)?;
                rule.set_color(theme.divider)?;
                continue;
            }
            let y = mid(&row.label);
            row.label.place(pad + inset, y)?;
            if row.hint.w > 0.0 {
                let y = mid(&row.hint);
                row.hint.place(w - pad - px(12.0) - row.hint.w, y)?;
            }
            if row.check.w > 0.0 {
                let y = mid(&row.check);
                row.check.place(pad + px(9.0), y)?;
            }
        }

        let (wi, hi) = (w.ceil() as i32, h.ceil() as i32);
        let edge = px(6.0) as i32;
        let (centre, mut top, standing) = match anchor {
            Anchor::Below { center, top } => (center, top, false),
            Anchor::Above { center, bottom } => (center, bottom - hi, true),
        };
        let left = (centre - wi / 2).min(bounds.right - wi - edge).max(bounds.left + edge);
        top = top.min(bounds.bottom - hi - edge).max(bounds.top + edge);
        // It grows out of the point it is anchored to, wherever the screen's edge pushed it.
        self.root = ((centre - left) as f32, if standing { h } else { 0.0 });
        let margin = theme.bubble.shadow_reach + px(8.0);
        self.margin = margin;
        self.glass.set_rect(scene, margin, margin, w, h, px(13.0))?;
        self.open = true;
        if !self.visible {
            self.show.snap(0.0);
        }
        self.show.set(1.0);
        // From here on, a "lost focus" report stamped with an older number is about a bubble
        // that was hidden just before this one was shown, and is ignored.
        SHOWING.set(SHOWING.get().wrapping_add(1));
        self.number = SHOWING.get();
        self.apply()?;
        let m = margin as i32;
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), left - m, top - m, wi + 2 * m, hi + 2 * m, SWP_SHOWWINDOW);
        }
        self.visible = true;
        crate::shell::activate(self.hwnd);
        if unsafe { GetForegroundWindow() } != self.hwnd {
            crate::log!("menu: Windows did not give the menu the keyboard; it will close when another window comes to the front");
        }
        Ok(())
    }

    /// Sends the bubble away; it shrinks back over the next frames.
    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.hot = None;
            self.show.set(0.0);
        }
    }

    /// Takes the bubble off the screen at once.
    pub fn close_now(&mut self) {
        self.open = false;
        self.hot = None;
        self.show.snap(0.0);
        if self.visible {
            self.visible = false;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
        self.rows.clear();
    }

    /// The row at a point in window pixels, if it can be chosen.
    fn row_at(&self, x: f32, y: f32) -> Option<usize> {
        let (x, y) = (x - self.margin, y - self.margin);
        if x < 0.0 || x >= self.size.0 {
            return None;
        }
        self.rows.iter().position(|r| y >= r.y && y < r.y + r.h).filter(|&i| self.rows[i].entry.kind == Kind::Item && self.rows[i].entry.enabled)
    }

    /// The next row that can be chosen, going up (-1) or down (+1) from the marked one.
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
            Input::Move(x, y) => self.hot = self.row_at(x as f32, y as f32),
            Input::Leave => self.hot = None,
            Input::Click(x, y) => {
                if let Some(i) = self.row_at(x as f32, y as f32) {
                    return Outcome::Chosen(self.rows[i].entry.id);
                }
            }
            Input::Key(VK_ESCAPE) => return Outcome::Closed,
            Input::Key(VK_DOWN) => self.hot = self.step(1),
            Input::Key(VK_UP) => self.hot = self.step(-1),
            Input::Key(VK_RETURN) => {
                if let Some(i) = self.hot {
                    return Outcome::Chosen(self.rows[i].entry.id);
                }
            }
            Input::Key(_) => {}
            Input::Lost(number) => {
                // A report left over from an earlier bubble (hidden, then another opened at
                // once) must not close this one.
                if number == self.number {
                    return Outcome::Closed;
                }
            }
        }
        Outcome::Nothing
    }

    fn apply(&mut self) -> Result<()> {
        let shown = self.show.pos;
        let grow = 0.84 + 0.16 * shown.clamp(0.0, 1.15);
        self.glass.set_scale(grow, grow, self.root.0, self.root.1)?;
        self.glass.set_opacity(shown.clamp(0.0, 1.0))
    }

    /// Advances the opening / closing and the mark under the cursor. Returns true while
    /// something is still moving.
    pub fn update(&mut self, theme: &Theme, motion: &Motion, dt: f32) -> Result<bool> {
        if !self.visible {
            return Ok(false);
        }
        // Out with a little spring, away without one.
        self.show.tune(if self.open { motion.small() } else { motion.fade() });
        self.show.step(dt);
        if !self.open && self.show.pos < 0.02 {
            self.close_now();
            return Ok(false);
        }
        self.apply()?;

        let s = self.scale;
        let pad = (6.0 * s).round();
        let target = self.hot.and_then(|i| self.rows.get(i)).map(|row| (pad, row.y, self.size.0 - 2.0 * pad, row.h));
        self.highlight_at.tune(motion);
        self.highlight_at.aim(target);
        self.highlight_at.step(dt);
        let shape = self.highlight_at.shape(motion.squash());
        self.highlight.place(shape.x, shape.y, shape.w, shape.h, (8.0 * s).round())?;
        let (top, bottom, edge) = theme.bead;
        self.highlight.set_colors(top, bottom, edge)?;
        self.highlight.set_opacity(shape.alpha)?;
        Ok(!self.show.settled(1e-3) || !self.highlight_at.settled())
    }
}

impl Drop for Bubble {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
