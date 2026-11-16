//! Widgets: small cards of the same glass as the island, lying on the desktop.
//!
//!   clock      the time, large, and the date
//!   calendar   this month, today marked
//!   monitor    the processor (with its last minute as a row of bars), the memory,
//!              the disk, the network and the battery
//!   to-do      the tasks in todo.txt; a click marks one done
//!   focus      a timer for stretches of focus and the rests between them
//!   note       what is written in note.txt
//!   progress   how much of the day, the week, the month and the year has gone by
//!
//! Each is a window of its own, under every app's windows and over the
//! desktop's icons, and can be dragged anywhere; where it was put is kept in
//! widgets.txt. As with the corners, the window never changes size: only the
//! glass inside it does, and only the glass takes the mouse.
//!
//! "Show desktop" lifts the desktop over every ordinary window, these
//! included. The app notices and lifts them along (see `desktop::Mark`).

use crate::app::{self, Event};
use crate::dates;
use crate::gfx::Gfx;
use crate::island::Input;
use crate::motion::{Glide, Motion};
use crate::scene::{Bead, Glass, Layer, Pill, Scene, Sprite, Stage};
use crate::shade::Shade;
use crate::spring::Spring;
use crate::tasks::{self, Task};
use crate::text::{self, glyph, Line, DISPLAY_FONT, ICON_FONT, UI_FONT};
use crate::theme::Theme;
use crate::util::{data_dir, wide};
use crate::vitals::{self, Reading};
use std::cell::Cell;
use std::collections::HashMap;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Color;
use windows::UI::Composition::{CompositionDrawingSurface, VisualCollection};

const CLASS_NAME: &str = "BenWidgetWindow";
const WM_MOUSELEAVE: u32 = 0x02A3;

// A card, in logical pixels: its width, the tallest it gets, its corners, and the room
// between its edge and what is on it.
const CARD_W: f32 = 248.0;
const TALLEST: f32 = 330.0;
const RADIUS: f32 = 22.0;
const PAD: f32 = 16.0;
/// Most tasks the to-do card lists.
const MOST_TASKS: usize = 8;
/// Bars in the row that shows the processor's last minute.
pub const HISTORY: usize = 32;

// Symbols of Windows' icon font.
const PENCIL: u32 = 0xE70F;
const RING: u32 = 0xEA3A;
const TICKED: u32 = 0xE930;
const PLAY: u32 = 0xE768;
const PAUSE: u32 = 0xE769;
const AGAIN: u32 = 0xE72C;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Clock,
    Calendar,
    Monitor,
    Todo,
    Focus,
    Note,
    Progress,
}

impl Kind {
    pub const ALL: [Kind; 7] = [Kind::Clock, Kind::Calendar, Kind::Monitor, Kind::Todo, Kind::Focus, Kind::Note, Kind::Progress];

    /// Its name in widgets.txt.
    fn name(self) -> &'static str {
        match self {
            Kind::Clock => "clock",
            Kind::Calendar => "calendar",
            Kind::Monitor => "monitor",
            Kind::Todo => "todo",
            Kind::Focus => "focus",
            Kind::Note => "note",
            Kind::Progress => "progress",
        }
    }

    /// Where it lies until it is dragged somewhere: in one of two columns down the right
    /// side of the screen (the icons are usually on the left), starting under the line the
    /// island and the top corners are on. `screen`: the whole screen. Returns the card's
    /// top-left corner on screen.
    pub fn home(self, screen: RECT, scale: f32) -> (i32, i32) {
        let px = |v: f32| (v * scale).round() as i32;
        // The column (0 = the one at the edge), and how far down it, in logical pixels.
        let (column, down) = match self {
            Kind::Clock => (0, 0.0),
            Kind::Calendar => (0, 124.0),
            Kind::Monitor => (0, 372.0),
            Kind::Todo => (1, 0.0),
            Kind::Focus => (1, 290.0),
            Kind::Note => (1, 418.0),
            Kind::Progress => (1, 590.0),
        };
        (screen.right - px(24.0) - px(CARD_W) - column * (px(CARD_W) + px(16.0)), screen.top + px(60.0) + px(down))
    }
}

/// What the widgets show; read by the app, handed over every pass.
pub struct Info<'a> {
    /// "13:49"
    pub time: &'a str,
    /// "Chủ nhật, 15 tháng 11"
    pub date: &'a str,
    pub today: (i32, u32, u32),
    pub vital: Reading,
    /// The processor's use over the last minute, in percent, oldest first.
    pub history: &'a [u8],
    /// Gigabytes taken on the disk Windows is on, and gigabytes in all.
    pub disk: Option<(f32, f32)>,
    /// Bytes a second coming in and going out.
    pub traffic: (f32, f32),
    pub battery: Option<(u8, bool)>,
    pub tasks: &'a [Task],
    pub note: &'a str,
    /// The focus timer: what it counts, the seconds left, how much has gone by (0 to 1),
    /// and whether it is running.
    pub focus: (tasks::Stage, u32, f32, bool),
    /// How much of the day, the week, the month and the year has gone by, each 0 to 1.
    pub gone: [f32; 4],
}

/// What a click on a widget asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// It was dragged to a new place.
    Moved,
    /// This task was clicked: mark it done, or not done.
    Task(usize),
    /// Open todo.txt, or note.txt, to be written in.
    EditTasks,
    EditNote,
    /// Start the focus timer or stop it; put it back to the start.
    FocusToggle,
    FocusReset,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    Task(usize),
    EditTasks,
    EditNote,
    FocusToggle,
    FocusReset,
}

type Zones = Vec<(Zone, f32, f32, f32, f32)>;

#[derive(Clone)]
struct Fonts {
    text: IDWriteTextFormat,
    bold: IDWriteTextFormat,
    small: IDWriteTextFormat,
    title: IDWriteTextFormat,
    /// The clock's digits, and the timer's.
    big: IDWriteTextFormat,
    mid: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    icons_big: IDWriteTextFormat,
    /// Like `text`, for words that run on over several lines.
    wrap: IDWriteTextFormat,
}

/// A few lines of text, broken where the room ends, painted once and shown by a sprite
/// (`text::Line` is the same for one line).
struct Block {
    sprite: Sprite,
    _surface: Option<CompositionDrawingSurface>,
    shown: Option<(String, [u32; 4], u32)>,
    h: f32,
}

impl Block {
    /// Shows `text` in a column `width` wide and at most `most` high. Repaints only if the
    /// text, the colour or the width changed.
    fn set(&mut self, scene: &Scene, gfx: &Gfx, format: &IDWriteTextFormat, text: &str, color: D2D1_COLOR_F, width: f32, most: f32) -> Result<()> {
        let key = (text.to_string(), [color.r.to_bits(), color.g.to_bits(), color.b.to_bits(), color.a.to_bits()], width.to_bits());
        if self.shown.as_ref() == Some(&key) {
            return Ok(());
        }
        let utf16: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = gfx.dwrite.CreateTextLayout(&utf16, format, width.max(1.0), most.max(1.0))?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m)?;
            let h = m.height.ceil().clamp(1.0, most.max(1.0));
            let surface = scene.surface_from_text(&layout, width.ceil(), h, color)?;
            self.sprite.set_surface(&surface)?;
            self._surface = Some(surface);
            self.h = h;
            self.sprite.place(0.0, 0.0, width.ceil(), h)?;
        }
        self.shown = Some(key);
        Ok(())
    }
}

/// What a card is drawn with: lines of text and bars are taken from two heaps as they are
/// asked for, and what is left unused at the end is put out of sight.
struct Pen<'a> {
    scene: &'a Scene,
    gfx: &'a Gfx,
    /// Where bars go, and over them, text.
    shapes: &'a VisualCollection,
    words: &'a VisualCollection,
    lines: &'a mut Vec<Line>,
    pills: &'a mut Vec<Pill>,
    line: usize,
    pill: usize,
    zones: Zones,
    s: f32,
}

impl Pen<'_> {
    fn px(&self, v: f32) -> f32 {
        (v * self.s).round()
    }

    /// Writes a line of text; `put` then says where. Returns which line, and its size.
    fn write(&mut self, font: &IDWriteTextFormat, text: &str, color: D2D1_COLOR_F, most: Option<f32>) -> Result<(usize, f32, f32)> {
        if self.line == self.lines.len() {
            self.lines.push(Line::new_in(self.scene, self.words)?);
        }
        let index = self.line;
        self.line += 1;
        let line = &mut self.lines[index];
        line.set(self.scene, self.gfx, font, text, color, most)?;
        line.set_opacity(1.0)?;
        Ok((index, line.w, line.h))
    }

    /// A line whose left end is at `x` and whose middle is `y` down. Returns its width.
    fn left(&mut self, font: &IDWriteTextFormat, text: &str, color: D2D1_COLOR_F, most: Option<f32>, x: f32, y: f32) -> Result<f32> {
        let (index, w, h) = self.write(font, text, color, most)?;
        self.lines[index].place(x, y - h * 0.5)?;
        Ok(w)
    }

    /// The same, ending at `x`.
    fn right(&mut self, font: &IDWriteTextFormat, text: &str, color: D2D1_COLOR_F, most: Option<f32>, x: f32, y: f32) -> Result<f32> {
        let (index, w, h) = self.write(font, text, color, most)?;
        self.lines[index].place(x - w, y - h * 0.5)?;
        Ok(w)
    }

    /// The same, with its middle at `x`.
    fn middle(&mut self, font: &IDWriteTextFormat, text: &str, color: D2D1_COLOR_F, x: f32, y: f32) -> Result<f32> {
        let (index, w, h) = self.write(font, text, color, None)?;
        self.lines[index].place(x - w * 0.5, y - h * 0.5)?;
        Ok(w)
    }

    /// A rounded bar.
    fn bar(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color, opacity: f32) -> Result<()> {
        if self.pill == self.pills.len() {
            self.pills.push(self.scene.pill_in(self.shapes)?);
        }
        let pill = &mut self.pills[self.pill];
        self.pill += 1;
        pill.place(x.round(), y.round(), w.max(0.0), h, (h.min(w.max(0.0)) * 0.5).max(0.0))?;
        pill.set_color(color)?;
        pill.set_opacity(opacity)
    }

    /// A gauge: a track from `x` to `x + w`, filled up to `share` of it.
    fn gauge(&mut self, x: f32, y: f32, w: f32, share: f32, track: Color, fill: Color) -> Result<()> {
        let h = self.px(4.0);
        self.bar(x, y, w, h, track, 1.0)?;
        let filled = (w * share.clamp(0.0, 1.0)).round();
        self.bar(x, y, filled.max(h), h, fill, if share > 0.004 { 1.0 } else { 0.0 })
    }

    /// Puts out of sight what was not asked for this time.
    fn done(&mut self) -> Result<()> {
        for line in &mut self.lines[self.line..] {
            line.set_opacity(0.0)?;
        }
        for pill in &mut self.pills[self.pill..] {
            pill.set_opacity(0.0)?;
        }
        Ok(())
    }
}

pub struct Widget {
    hwnd: HWND,
    kind: Kind,
    _stage: Stage,
    /// The window under this one that shows the glass's shadow.
    shade: Option<Shade>,
    glass: Glass,
    /// Bars, and over them, text.
    shapes: Layer,
    words: Layer,
    lines: Vec<Line>,
    pills: Vec<Pill>,
    /// The words of the note.
    block: Block,
    /// The drop of light under the cursor.
    hot: Bead,
    hot_at: Glide,
    fonts: Option<Fonts>,
    /// How tall the glass is.
    h: Spring,
    /// The card's top-left corner on screen.
    at: (i32, i32),
    scale: f32,
    /// Room around the glass inside the window, for its shadow.
    margin: f32,
    win: (i32, i32),
    shown: bool,
    /// What can be clicked, in the card's own pixels.
    zones: Zones,
    pressed: Option<Zone>,
    cursor: Option<(f32, f32)>,
    /// A press that may turn into a drag: where the cursor was on screen, and the card.
    grab: Option<((i32, i32), (i32, i32))>,
    dragged: bool,
    region: Option<(i32, i32, i32, i32)>,
}

thread_local! {
    /// The widget window whose mouse-leave is being tracked (0: none).
    static TRACKING: Cell<isize> = const { Cell::new(0) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let me = hwnd.0 as isize;
    let send = |input: Input| app::post(Event::Widget(me, input));
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
            // A card can be dragged off to anywhere: keep hearing from the mouse out there.
            unsafe {
                SetCapture(hwnd);
            }
            send(Input::Down(x, y));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            send(Input::Up(x, y));
            return LRESULT(0);
        }
        // A click on a card must not take the keyboard from the app in use, nor bring the
        // card in front of it.
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Widget {
    pub fn new(scene: &Scene, theme: &Theme, motion: &Motion, kind: Kind) -> Result<Self> {
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
            // Not topmost, unlike the dock and the island: it belongs to the desktop.
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW, PCWSTR(class_name.as_ptr()), PCWSTR(wide("Bến - Widget").as_ptr()), WS_POPUP, 0, 0, 1, 1, None, None, Some(instance), None)?;
            let stage = scene.stage(hwnd)?;
            let mut glass = scene.glass_in(&stage.content()?, &theme.island)?;
            // The window is cut down to the glass, so its shadow needs a window of its own.
            let shade = match Shade::new(scene).and_then(|shade| glass.cast_shadow_on(&shade.content()?).map(|_| shade)) {
                Ok(shade) => Some(shade),
                Err(e) => {
                    crate::log!("no window for a widget's shadow ({e}); it goes without one");
                    glass.without_shadow();
                    None
                }
            };
            // Everything lives inside the glass's shape, so the shape cuts it as it shrinks.
            let inside = glass.inner()?;
            let shapes = scene.layer_in(&inside)?;
            let hot = scene.bead_in(&shapes.content()?)?;
            let words = scene.layer_in(&inside)?;
            let block = Block { sprite: scene.sprite_in(&words.content()?)?, _surface: None, shown: None, h: 0.0 };
            Ok(Widget {
                hwnd,
                kind,
                _stage: stage,
                shade,
                glass,
                shapes,
                words,
                lines: Vec::new(),
                pills: Vec::new(),
                block,
                hot,
                hot_at: Glide::new(motion),
                fonts: None,
                h: Spring::with(40.0, motion.medium()),
                at: (0, 0),
                scale: 1.0,
                margin: 0.0,
                win: (1, 1),
                shown: false,
                zones: Vec::new(),
                pressed: None,
                cursor: None,
                grab: None,
                dragged: false,
                region: None,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The card's top-left corner on screen.
    pub fn at(&self) -> (i32, i32) {
        self.at
    }

    /// Puts the card's top-left corner at a point of the screen, sized for `scale`.
    pub fn place(&mut self, at: (i32, i32), scale: f32, theme: &Theme) {
        let px = |v: f32| (v * scale).round();
        let margin = theme.island.shadow_reach + px(10.0);
        let win = ((px(CARD_W) + 2.0 * margin) as i32, (px(TALLEST) + 2.0 * margin) as i32);
        if self.scale != scale {
            // Text is painted for one size.
            self.forget();
        }
        (self.at, self.scale, self.margin, self.win) = (at, scale, margin, win);
        self.region = None;
        let (x, y) = (at.0 - margin as i32, at.1 - margin as i32);
        unsafe {
            if self.shown {
                let _ = SetWindowPos(self.hwnd, None, x, y, win.0, win.1, SWP_NOACTIVATE | SWP_NOZORDER);
            } else {
                // Under every other window: on the desktop.
                let _ = SetWindowPos(self.hwnd, Some(HWND_BOTTOM), x, y, win.0, win.1, SWP_NOACTIVATE | SWP_SHOWWINDOW);
                self.shown = true;
            }
        }
        if let Some(shade) = self.shade.as_mut() {
            shade.place(self.hwnd, x, y, win.0, win.1);
        }
    }

    /// Lifts the card over the lifted desktop (`under`: the window it then goes right
    /// under), or lets it down onto the desktop again (`None`).
    pub fn lift(&mut self, under: Option<HWND>) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(under.unwrap_or(HWND_BOTTOM)), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        if let Some(shade) = self.shade.as_ref() {
            shade.tuck_under(self.hwnd);
        }
    }

    pub fn set_theme(&mut self, theme: &Theme) {
        self.glass.set_material(&theme.island);
        self.forget();
    }

    /// Forgets painted text and fonts, so they are made again (after a theme, size or GPU change).
    pub fn forget(&mut self) {
        self.fonts = None;
        for line in &mut self.lines {
            line.forget();
        }
        self.block.shown = None;
    }

    fn zone_at(&self, x: f32, y: f32) -> Option<Zone> {
        self.zones.iter().find(|(_, l, t, r, b)| x >= *l && x < *r && y >= *t && y < *b).map(|(zone, ..)| *zone)
    }

    /// Reacts to the mouse: a press that moves is a drag, one that does not is a click.
    pub fn handle(&mut self, input: Input) -> Action {
        let on_card = |x: i32, y: i32, margin: f32| (x as f32 - margin, y as f32 - margin);
        let cursor_on_screen = || {
            let mut pt = POINT::default();
            unsafe { GetCursorPos(&mut pt) }.ok().map(|_| (pt.x, pt.y))
        };
        match input {
            Input::Move(x, y) => {
                self.cursor = Some(on_card(x, y, self.margin));
                if let (Some((from, card)), Some(now)) = (self.grab, cursor_on_screen()) {
                    let (dx, dy) = (now.0 - from.0, now.1 - from.1);
                    if !self.dragged && dx.abs().max(dy.abs()) as f32 >= 4.0 * self.scale {
                        self.dragged = true;
                    }
                    if self.dragged {
                        self.move_to((card.0 + dx, card.1 + dy));
                    }
                }
            }
            Input::Leave => self.cursor = None,
            Input::Down(x, y) => {
                let (cx, cy) = on_card(x, y, self.margin);
                self.pressed = self.zone_at(cx, cy);
                self.grab = cursor_on_screen().map(|now| (now, self.at));
                self.dragged = false;
            }
            Input::Up(x, y) => {
                let pressed = self.pressed.take();
                self.grab = None;
                if std::mem::take(&mut self.dragged) {
                    return Action::Moved;
                }
                let (cx, cy) = on_card(x, y, self.margin);
                return match self.zone_at(cx, cy).filter(|zone| Some(*zone) == pressed) {
                    Some(Zone::Task(index)) => Action::Task(index),
                    Some(Zone::EditTasks) => Action::EditTasks,
                    Some(Zone::EditNote) => Action::EditNote,
                    Some(Zone::FocusToggle) => Action::FocusToggle,
                    Some(Zone::FocusReset) => Action::FocusReset,
                    None => Action::None,
                };
            }
            _ => {}
        }
        Action::None
    }

    /// Moves the card while it is dragged. It stays where it can be reached: at least a
    /// corner of it on some screen.
    fn move_to(&mut self, at: (i32, i32)) {
        let (left, top, w, h) = unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN), GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN)) };
        let keep = (60.0 * self.scale) as i32;
        let card_w = (CARD_W * self.scale) as i32;
        let at = (at.0.clamp(left + keep - card_w, left + w - keep), at.1.clamp(top, top + h - keep));
        if at == self.at {
            return;
        }
        self.at = at;
        let (x, y) = (at.0 - self.margin as i32, at.1 - self.margin as i32);
        unsafe {
            let _ = SetWindowPos(self.hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        if let Some(shade) = self.shade.as_mut() {
            shade.place(self.hwnd, x, y, self.win.0, self.win.1);
        }
    }

    /// Brings the card up to date with what it shows and lays it out.
    /// Returns true while something is still moving.
    pub fn update(&mut self, scene: &Scene, gfx: &Gfx, theme: &Theme, motion: &Motion, info: &Info, dt: f32) -> Result<bool> {
        let s = self.scale;
        let px = |v: f32| (v * s).round();
        if self.fonts.is_none() {
            let wrap = text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_NORMAL)?;
            unsafe { wrap.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)? };
            self.fonts = Some(Fonts {
                text: text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                bold: text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                small: text::format(gfx, UI_FONT, px(12.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                title: text::format(gfx, UI_FONT, px(11.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                big: text::format(gfx, DISPLAY_FONT, px(50.0), DWRITE_FONT_WEIGHT_SEMI_LIGHT)?,
                mid: text::format(gfx, DISPLAY_FONT, px(36.0), DWRITE_FONT_WEIGHT_SEMI_LIGHT)?,
                icons: text::format(gfx, ICON_FONT, px(14.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                icons_big: text::format(gfx, ICON_FONT, px(16.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                wrap,
            });
        }
        let f = self.fonts.clone().unwrap();
        let (shapes, words) = (self.shapes.content()?, self.words.content()?);
        let mut pen = Pen { scene, gfx, shapes: &shapes, words: &words, lines: &mut self.lines, pills: &mut self.pills, line: 0, pill: 0, zones: Vec::new(), s };
        let tall = match self.kind {
            Kind::Clock => clock(&mut pen, &f, theme, info)?,
            Kind::Calendar => calendar(&mut pen, &f, theme, info)?,
            Kind::Monitor => monitor(&mut pen, &f, theme, info)?,
            Kind::Todo => todo(&mut pen, &f, theme, info)?,
            Kind::Focus => focus(&mut pen, &f, theme, info)?,
            Kind::Progress => progress(&mut pen, &f, theme, info)?,
            Kind::Note => {
                let top = heading(&mut pen, &f, theme, "GHI CHÚ", Some(Zone::EditNote))?;
                let column = pen.px(CARD_W - 2.0 * PAD);
                let words = if info.note.trim().is_empty() { "(note.txt đang trống)" } else { info.note.trim_end() };
                self.block.set(scene, gfx, &f.wrap, words, theme.text, column, pen.px(TALLEST - 60.0))?;
                self.block.sprite.place(pen.px(PAD), top, column, self.block.h)?;
                // Anywhere on the words opens the file too.
                let bottom = top + self.block.h + pen.px(14.0);
                pen.zones.push((Zone::EditNote, 0.0, top, pen.px(CARD_W), bottom));
                bottom
            }
        };
        pen.done()?;
        self.zones = std::mem::take(&mut pen.zones);
        self.block.sprite.set_opacity(if self.kind == Kind::Note { 1.0 } else { 0.0 })?;

        // The glass: as tall as what is on it. Growing, it overshoots like a drop.
        let tall = tall.min(px(TALLEST));
        if tall != self.h.target {
            self.h.tune(if tall > self.h.target { motion.medium() } else { motion.calm() });
        }
        self.h.set(tall);
        self.h.step(dt);
        let (m, w, h) = (self.margin, px(CARD_W), self.h.pos.max(8.0));
        self.glass.set_rect(scene, m, m, w, h, px(RADIUS).min(h * 0.5))?;

        // The drop of light under the cursor, on what can be clicked.
        let hot = self.cursor.filter(|_| !self.dragged).and_then(|(cx, cy)| self.zones.iter().find(|(_, l, t, r, b)| cx >= *l && cx < *r && cy >= *t && cy < *b).copied());
        self.hot_at.tune(motion);
        // (The whole of a note can be clicked; it gets no light of its own.)
        self.hot_at.aim(hot.filter(|(_, l, _, r, _)| r - l < w).map(|(_, l, t, r, b)| (l, t, r - l, b - t)));
        self.hot_at.step(dt);
        let shape = self.hot_at.shape(motion.squash());
        let (bead_top, bead_bottom, bead_edge) = theme.bead;
        self.hot.place(shape.x, shape.y, shape.w, shape.h, px(9.0))?;
        self.hot.set_colors(bead_top, bead_bottom, bead_edge)?;
        self.hot.set_opacity(shape.alpha)?;

        // Only the glass takes the mouse.
        let wanted = ((m - 2.0).floor() as i32, (m - 2.0).floor() as i32, (m + w + 2.0).ceil() as i32, (m + h + 2.0).ceil() as i32);
        if self.region != Some(wanted) {
            self.region = Some(wanted);
            unsafe {
                // The system takes ownership of the region handle.
                let rgn = CreateRectRgn(wanted.0, wanted.1, wanted.2, wanted.3);
                SetWindowRgn(self.hwnd, Some(rgn), false);
            }
        }
        Ok(!self.h.settled(0.05) || !self.hot_at.settled() || self.dragged)
    }
}

impl Drop for Widget {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

// --------------------------------------------------------------- the cards
//
// Each draws one card and returns how tall it has to be, in pixels.

/// A card's heading: a few small capitals, and at the far end a pencil to click if the card
/// has a file behind it. Returns where what follows begins.
fn heading(pen: &mut Pen, f: &Fonts, theme: &Theme, words: &str, pencil: Option<Zone>) -> Result<f32> {
    let (pad, middle) = (pen.px(PAD), pen.px(20.0));
    pen.left(&f.title, words, theme.text_dim, None, pad, middle)?;
    if let Some(zone) = pencil {
        let end = pen.px(CARD_W - PAD);
        let w = pen.right(&f.icons, &glyph(PENCIL), theme.text_dim, None, end, middle)?;
        let slack = pen.px(7.0);
        pen.zones.push((zone, end - w - slack, middle - w * 0.5 - slack, end + slack, middle + w * 0.5 + slack));
    }
    Ok(pen.px(38.0))
}

fn clock(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    pen.left(&f.big, info.time, theme.text, None, pen.px(PAD - 2.0), pen.px(44.0))?;
    pen.left(&f.text, info.date, theme.text_dim, Some(pen.px(CARD_W - 2.0 * PAD)), pen.px(PAD), pen.px(84.0))?;
    Ok(pen.px(106.0))
}

fn calendar(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    let (year, month, today) = info.today;
    let (side, cell_w, row_h) = (pen.px(12.0), pen.px((CARD_W - 24.0) / 7.0), pen.px(27.0));
    pen.left(&f.bold, &format!("Tháng {month}, {year}"), theme.text, None, pen.px(PAD), pen.px(22.0))?;
    for (column, name) in dates::WEEKDAYS_SHORT.iter().enumerate() {
        pen.middle(&f.small, name, theme.text_dim, side + cell_w * (column as f32 + 0.5), pen.px(52.0))?;
    }
    let top = pen.px(66.0);
    for day in 1..=dates::days_in_month(year, month) {
        let (column, row) = dates::cell(year, month, day);
        let (cx, cy) = (side + cell_w * (column as f32 + 0.5), top + row_h * (row as f32 + 0.5));
        if day == today {
            // Today stands on a disc of the accent colour.
            let disc = pen.px(25.0);
            pen.bar(cx - disc * 0.5, cy - disc * 0.5, disc, disc, theme.accent_color(1.0), 1.0)?;
            pen.middle(&f.bold, &day.to_string(), D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, cx, cy)?;
        } else {
            // Saturday and Sunday are quieter.
            pen.middle(&f.text, &day.to_string(), if column >= 5 { theme.text_dim } else { theme.text }, cx, cy)?;
        }
    }
    Ok(top + row_h * dates::week_rows(year, month) as f32 + pen.px(10.0))
}

/// One reading of the monitor, or one stretch of time of the progress card: a name, a few
/// words at the far end, and under them a gauge. Returns where the next one begins.
fn reading(pen: &mut Pen, f: &Fonts, theme: &Theme, y: f32, name: &str, words: &str, share: Option<f32>) -> Result<f32> {
    let (pad, column) = (pen.px(PAD), pen.px(CARD_W - 2.0 * PAD));
    let middle = y + pen.px(9.0);
    let named = pen.left(&f.text, name, theme.text, None, pad, middle)?;
    pen.right(&f.small, words, theme.text_dim, Some(column - named - pen.px(10.0)), pad + column, middle)?;
    match share {
        Some(share) => {
            pen.gauge(pad, y + pen.px(22.0), column, share, theme.track, theme.accent_color(1.0))?;
            Ok(y + pen.px(34.0))
        }
        None => Ok(y + pen.px(24.0)),
    }
}

fn monitor(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    let top = heading(pen, f, theme, "MÁY TÍNH", None)?;
    // The processor's last minute: a bar for every look at it, the newest at the right end
    // and the clearest.
    let (pad, column, tall) = (pen.px(PAD), pen.px(CARD_W - 2.0 * PAD), pen.px(34.0));
    let step = column / HISTORY as f32;
    let empty = HISTORY.saturating_sub(info.history.len());
    for slot in 0..HISTORY {
        let percent = if slot >= empty { info.history[slot - empty] } else { 0 };
        let h = (tall * percent as f32 / 100.0).max(pen.px(2.0));
        let fresh = 0.3 + 0.7 * (slot as f32 / (HISTORY - 1) as f32);
        pen.bar(pad + step * slot as f32, top + tall - h, (step * 0.62).max(1.0), h, theme.accent_color(1.0), fresh)?;
    }
    let mut y = top + tall + pen.px(12.0);
    let vital = info.vital;
    y = reading(pen, f, theme, y, "CPU", &format!("{}%", vital.processor), Some(vital.processor as f32 / 100.0))?;
    y = reading(pen, f, theme, y, "RAM", &format!("{:.1} / {:.0} GB", vital.used, vital.total), Some(vital.memory as f32 / 100.0))?;
    if let Some((used, total)) = info.disk {
        y = reading(pen, f, theme, y, "Ổ đĩa", &format!("{used:.0} / {total:.0} GB"), Some(used / total.max(1.0)))?;
    }
    if let Some((percent, plugged)) = info.battery {
        y = reading(pen, f, theme, y, "Pin", &format!("{percent}%{}", if plugged { " · đang sạc" } else { "" }), Some(percent as f32 / 100.0))?;
    }
    let (down, up) = info.traffic;
    y = reading(pen, f, theme, y, "Mạng", &format!("↓ {}   ↑ {}", vitals::speed_text(down), vitals::speed_text(up)), None)?;
    Ok(y + pen.px(8.0))
}

fn todo(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    let left = info.tasks.iter().filter(|task| !task.done).count();
    let words = if info.tasks.is_empty() { "VIỆC CẦN LÀM".to_string() } else { format!("VIỆC CẦN LÀM · CÒN {left}") };
    let mut y = heading(pen, f, theme, &words, Some(Zone::EditTasks))?;
    let (pad, row_h, card_w) = (pen.px(PAD), pen.px(28.0), pen.px(CARD_W));
    if info.tasks.is_empty() {
        pen.left(&f.small, "Chưa có việc nào. Bấm cây bút để thêm.", theme.text_dim, Some(card_w - 2.0 * pad), pad, y + row_h * 0.5)?;
        return Ok(y + row_h + pen.px(10.0));
    }
    for (index, task) in info.tasks.iter().enumerate().take(MOST_TASKS) {
        let middle = y + row_h * 0.5;
        // A ring to tick; ticked, it is filled in the accent colour and the words step back.
        let mark = if task.done { (TICKED, theme.accent_text()) } else { (RING, theme.text_dim) };
        pen.left(&f.icons, &glyph(mark.0), mark.1, None, pad, middle)?;
        pen.left(&f.text, &task.text, if task.done { theme.text_dim } else { theme.text }, Some(card_w - 2.0 * pad - pen.px(24.0)), pad + pen.px(24.0), middle)?;
        pen.zones.push((Zone::Task(index), pen.px(8.0), y, card_w - pen.px(8.0), y + row_h));
        y += row_h;
    }
    if info.tasks.len() > MOST_TASKS {
        pen.left(&f.small, &format!("và {} việc nữa", info.tasks.len() - MOST_TASKS), theme.text_dim, None, pad + pen.px(24.0), y + pen.px(11.0))?;
        y += pen.px(22.0);
    }
    Ok(y + pen.px(10.0))
}

fn focus(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    let (stage, seconds, share, running) = info.focus;
    let words = match (stage, running) {
        (tasks::Stage::Focus, true) => "TẬP TRUNG",
        (tasks::Stage::Focus, false) => "TẬP TRUNG · ĐANG DỪNG",
        (tasks::Stage::Rest, true) => "NGHỈ",
        (tasks::Stage::Rest, false) => "NGHỈ · ĐANG DỪNG",
    };
    heading(pen, f, theme, words, None)?;
    let (pad, column, middle) = (pen.px(PAD), pen.px(CARD_W - 2.0 * PAD), pen.px(60.0));
    pen.left(&f.mid, &format!("{:02}:{:02}", seconds / 60, seconds % 60), theme.text, None, pad - pen.px(1.0), middle)?;
    // Two things to click at the far end: start or stop, and back to the start.
    let (slack, gap) = (pen.px(8.0), pen.px(22.0));
    let mut end = pad + column;
    for (symbol, zone) in [(AGAIN, Zone::FocusReset), (if running { PAUSE } else { PLAY }, Zone::FocusToggle)] {
        let w = pen.right(&f.icons_big, &glyph(symbol), theme.text, None, end, middle)?;
        pen.zones.push((zone, end - w - slack, middle - w * 0.5 - slack, end + slack, middle + w * 0.5 + slack));
        end -= w + gap;
    }
    // A rest is drawn quieter than a stretch of focus.
    let fill = if stage == tasks::Stage::Focus { theme.accent_color(1.0) } else { theme.tone(0.55) };
    pen.gauge(pad, pen.px(92.0), column, share, theme.track, fill)?;
    Ok(pen.px(112.0))
}

fn progress(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    let mut y = heading(pen, f, theme, "TIẾN ĐỘ", None)?;
    for (name, share) in ["Hôm nay", "Tuần này", "Tháng này", "Năm nay"].into_iter().zip(info.gone) {
        y = reading(pen, f, theme, y, name, &format!("{}%", (share * 100.0).floor() as u32), Some(share))?;
    }
    Ok(y + pen.px(6.0))
}

// ------------------------------------------------------------- their places

fn places_file() -> std::path::PathBuf {
    data_dir().join("widgets.txt")
}

/// Where each widget was last put (the card's top-left corner on screen), by its name.
pub fn load_places() -> HashMap<Kind, (i32, i32)> {
    let text = std::fs::read_to_string(places_file()).unwrap_or_default();
    parse_places(&text)
}

fn parse_places(text: &str) -> HashMap<Kind, (i32, i32)> {
    let mut places = HashMap::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let Some((name, at)) = line.split_once('=') else { continue };
        let Some(kind) = Kind::ALL.into_iter().find(|kind| kind.name() == name.trim()) else { continue };
        let Some((x, y)) = at.split_once(',') else { continue };
        if let (Ok(x), Ok(y)) = (x.trim().parse::<i32>(), y.trim().parse::<i32>()) {
            places.insert(kind, (x, y));
        }
    }
    places
}

fn places_text(places: &HashMap<Kind, (i32, i32)>) -> String {
    let mut text = String::from("# Chỗ của các widget trên màn hình nền (góc trên trái, tính bằng điểm ảnh). Kéo widget là file này được ghi lại.\n");
    for kind in Kind::ALL {
        if let Some((x, y)) = places.get(&kind) {
            text.push_str(&format!("{} = {x}, {y}\n", kind.name()));
        }
    }
    text
}

/// Keeps where the widgets are. Those that are switched off keep the place they had.
pub fn save_places(widgets: &[Widget]) {
    let mut places = load_places();
    for widget in widgets {
        places.insert(widget.kind(), widget.at());
    }
    if let Err(e) = std::fs::write(places_file(), places_text(&places)) {
        crate::log!("could not save widgets.txt: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::theme;

    /// Not a check of anything: puts every widget on the real desktop for a few seconds,
    /// with made-up things to show, so that they can be looked at.
    /// Run by hand: cargo test --release widgets_on_the_desktop -- --ignored
    /// (BEN_LIGHT=1 for the light look, BEN_SECONDS for how long.)
    #[test]
    #[ignore]
    fn widgets_on_the_desktop() {
        let gfx = Gfx::new(false).expect("a graphics device");
        let scene = Scene::new(&gfx).expect("a compositor");
        let mut cfg = Config::default();
        cfg.theme = if std::env::var("BEN_LIGHT").is_ok() { 2.0 } else { 1.0 };
        let theme = theme::build(&cfg, true, (0.0, 0.47, 0.84), 1.0);
        let motion = Motion::new(1.0, 0.5, false);
        let work = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        let mut widgets: Vec<Widget> = Kind::ALL.into_iter().map(|kind| Widget::new(&scene, &theme, &motion, kind).expect("a widget")).collect();
        for widget in &mut widgets {
            widget.place(widget.kind().home(work, 1.0), 1.0, &theme);
        }
        let history: Vec<u8> = (0..HISTORY).map(|i| (30.0 + 25.0 * (i as f32 * 0.5).sin() + i as f32) as u8).collect();
        let tasks = vec![
            Task { text: "Viết báo cáo tuần".into(), done: false },
            Task { text: "Gọi cho Lan về buổi họp thứ sáu, nhớ hỏi cả lịch tháng sau".into(), done: false },
            Task { text: "Mua cà phê".into(), done: true },
        ];
        let seconds: f32 = std::env::var("BEN_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(4.0);
        let started = std::time::Instant::now();
        let mut msg = MSG::default();
        while started.elapsed().as_secs_f32() < seconds {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            let info = Info {
                time: "14:05",
                date: "Thứ hai, 16 tháng 11",
                today: (2026, 11, 16),
                vital: Reading { processor: 37, memory: 61, used: 9.8, total: 16.0 },
                history: &history,
                disk: Some((310.0, 476.0)),
                traffic: (1_300_000.0, 82_000.0),
                battery: Some((85, true)),
                tasks: &tasks,
                note: "Họp nhóm lúc 15:00 ở phòng 3.
Nhớ mang theo bản in và hỏi về hạn nộp của đợt tới.",
                focus: (tasks::Stage::Focus, 1124, 0.25, true),
                gone: [0.59, 0.08, 0.52, 0.88],
            };
            for widget in &mut widgets {
                widget.update(&scene, &gfx, &theme, &motion, &info, 1.0 / 60.0).expect("an update");
            }
            scene.commit();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }

    #[test]
    fn places_are_written_and_read_back() {
        let mut places = HashMap::new();
        places.insert(Kind::Clock, (1640, 64));
        places.insert(Kind::Note, (-300, 12));
        let text = places_text(&places);
        assert_eq!(parse_places(&text), places);
        // What cannot be read is left out, not guessed.
        let odd = parse_places("clock = 10\nsundial = 1, 2\nnote = a, b\n# remark\ntodo=5,6\n");
        assert_eq!(odd.len(), 1);
        assert_eq!(odd[&Kind::Todo], (5, 6));
    }

    #[test]
    fn every_widget_has_a_name_and_a_home_of_its_own() {
        let work = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        for (index, kind) in Kind::ALL.into_iter().enumerate() {
            assert!(!Kind::ALL[..index].iter().any(|other| other.name() == kind.name()));
            let (x, y) = kind.home(work, 1.0);
            assert!(x >= 1300 && x + CARD_W as i32 <= 1920 - 20, "{kind:?} at {x}");
            // Under the island's line, and well clear of the dock.
            assert!(y >= 56 && y < 700, "{kind:?} at {y}");
            assert!(!Kind::ALL[..index].iter().any(|other| other.home(work, 1.0) == (x, y)));
        }
        // At twice the scale everything is twice as far from the corner.
        let (x, _) = Kind::Clock.home(work, 2.0);
        assert_eq!(1920 - x, 2 * (1920 - Kind::Clock.home(work, 1.0).0));
    }
}
