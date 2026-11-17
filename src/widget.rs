//! Widgets: things to look at that lie on the desktop. They are not all of one
//! make. Two lie bare on the wallpaper, with no glass under them:
//!
//!   big        the time in very large, thin digits, the date under it, and a
//!              bead of light that crosses a hairline with the seconds
//!   sound      a row of thin bars that rise with the sound the computer plays
//!              and travel outwards from the middle, over their own reflection
//!
//! The others are of the same glass as the island, each with a picture of its
//! own (see `art`):
//!
//!   clock      the time and the date beside a clock's face
//!   calendar   this month, today lit
//!   monitor    the processor (with its last minute as a graph), the memory,
//!              the disk and the battery as round gauges, and the network
//!   to-do      the tasks in todo.txt; a click marks one done
//!   focus      a round timer for stretches of focus and the rests between them
//!   note       what is written in note.txt
//!   progress   how much of the day, the week, the month and the year has gone
//!              by, as four rings one inside the other
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
use crate::paint::{fade, mix, on_circle, turn, Canvas, Rgba};
use crate::scene::{rgba, Bead, Glass, Layer, Scene, Sprite, Stage};
use crate::shade::Shade;
use crate::spring::Spring;
use crate::tasks::{self, Task};
use crate::text::{self, glyph, Line, DISPLAY_FONT, ICON_FONT, UI_FONT};
use crate::theme::Theme;
use crate::util::{data_dir, wide};
use crate::vitals::{self, Reading};
use std::cell::Cell;
use std::collections::HashMap;
use windows::core::{Interface, Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::D2D1_COLOR_F;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::{CompositionDrawingSurface, VisualCollection};

const CLASS_NAME: &str = "BenWidgetWindow";
const WM_MOUSELEAVE: u32 = 0x02A3;

// A card, in logical pixels: its width, the tallest it gets, its corners, and the room
// between its edge and what is on it.
const CARD_W: f32 = 248.0;
const TALLEST: f32 = 330.0;
const RADIUS: f32 = 22.0;
const PAD: f32 = 16.0;
/// The large clock, which lies bare on the desktop: how wide and how tall it is.
const BIG: (f32, f32) = (520.0, 224.0);
/// The sound's bars, bare too: how wide and how tall they are with their reflection, and
/// how many bars stand on each side of the middle.
const WAVE: (f32, f32) = (372.0, 108.0);
const BARS: usize = 30;
/// The shadow under what lies bare: how soft it is, how far under, and how dark.
const SHADOW: (f32, f32, f32) = (5.0, 1.0, 0.7);
/// The focus timer is round: how far across it is.
const PUCK: f32 = 176.0;
/// Most tasks the to-do card lists.
const MOST_TASKS: usize = 8;
/// Bars in the row that shows the processor's last minute.
pub const HISTORY: usize = 32;

// Symbols of Windows' icon font.
const PENCIL: u32 = 0xE70F;
const CHECK: u32 = 0xE73E;
const PLAY: u32 = 0xE768;
const PAUSE: u32 = 0xE769;
const AGAIN: u32 = 0xE72C;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Big,
    Sound,
    Clock,
    Calendar,
    Monitor,
    Todo,
    Focus,
    Note,
    Progress,
}

impl Kind {
    pub const ALL: [Kind; 9] = [Kind::Big, Kind::Sound, Kind::Clock, Kind::Calendar, Kind::Monitor, Kind::Todo, Kind::Focus, Kind::Note, Kind::Progress];

    /// Its name in widgets.txt, and at the end of the setting that switches it on
    /// ("widget_clock").
    pub fn name(self) -> &'static str {
        match self {
            Kind::Big => "big",
            Kind::Sound => "sound",
            Kind::Clock => "clock",
            Kind::Calendar => "calendar",
            Kind::Monitor => "monitor",
            Kind::Todo => "todo",
            Kind::Focus => "focus",
            Kind::Note => "note",
            Kind::Progress => "progress",
        }
    }

    /// How wide it is and the tallest it gets, in logical pixels.
    fn size(self) -> (f32, f32) {
        match self {
            Kind::Big => BIG,
            Kind::Sound => WAVE,
            Kind::Focus => (PUCK, PUCK),
            _ => (CARD_W, TALLEST),
        }
    }

    /// True for those that are written and drawn straight on the desktop, with no glass
    /// under them.
    fn bare(self) -> bool {
        matches!(self, Kind::Big | Kind::Sound)
    }

    /// Where it lies until it is dragged somewhere. The two that lie bare are in the middle
    /// of the screen, the clock in its upper part and the sound under the clock. The cards
    /// are in two columns down the right side (the icons are usually on the left), starting
    /// under the line the island and the top corners are on. `screen`: the whole screen.
    /// Returns its top-left corner on screen.
    pub fn home(self, screen: RECT, scale: f32) -> (i32, i32) {
        let px = |v: f32| (v * scale).round() as i32;
        let (wide, _) = self.size();
        let middle = ((screen.left + screen.right - px(wide)) / 2, screen.top + (screen.bottom - screen.top) / 6);
        // The column (0 = the one at the edge), and how far down it, in logical pixels.
        let (column, down) = match self {
            Kind::Big => return middle,
            Kind::Sound => return (middle.0, middle.1 + px(BIG.1 + 26.0)),
            Kind::Clock => (0, 0.0),
            Kind::Calendar => (0, 146.0),
            Kind::Monitor => (0, 394.0),
            Kind::Todo => (1, 0.0),
            Kind::Focus => (1, 322.0),
            Kind::Progress => (1, 514.0),
            Kind::Note => (1, 680.0),
        };
        // (One that is narrower than its column stands in the middle of it.)
        (screen.right - px(24.0) - px(CARD_W) - column * (px(CARD_W) + px(16.0)) + px((CARD_W - wide) * 0.5), screen.top + px(60.0) + px(down))
    }
}

/// What the widgets show; read by the app, handed over every pass.
pub struct Info<'a> {
    /// "13:49"
    pub time: &'a str,
    /// "Chủ nhật, 15 tháng 11"
    pub date: &'a str,
    pub today: (i32, u32, u32),
    /// The hour, the minute and the second it is, for the hands of the clock's face.
    pub now: (u32, u32, u32),
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
    /// How loud what the computer plays is right now, to the left and to the right, each 0
    /// to 1. `None` while that cannot be told or need not be (the widget that shows it is
    /// off, or under a maximised window).
    pub sound: Option<(f32, f32)>,
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
    tiny: IDWriteTextFormat,
    title: IDWriteTextFormat,
    /// The name of the month.
    head: IDWriteTextFormat,
    /// The clock's digits; how busy the processor is, and the time left on the timer.
    big: IDWriteTextFormat,
    mid: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    /// The tick in the disc of a task that is done.
    check: IDWriteTextFormat,
    /// Like `text`, for words that run on over several lines.
    wrap: IDWriteTextFormat,
    /// The large clock's digits, very thin, and the date under them.
    huge: IDWriteTextFormat,
    date: IDWriteTextFormat,
}

/// The display font at a weight given as a number, for strokes thinner than any of the
/// font's named weights: Segoe UI Variable can be drawn at any weight from 100 up, but
/// only when the weight is asked for along the font's own axis. Where that cannot be done
/// (an older Windows), the lightest named weight is used.
fn thin(gfx: &Gfx, size: f32, weight: f32) -> Result<IDWriteTextFormat> {
    let made = || -> Result<IDWriteTextFormat> {
        unsafe {
            let factory: IDWriteFactory6 = gfx.dwrite.cast()?;
            // (A font that varies is one family here, not one for each size it is cut for.)
            let fonts = factory.GetSystemFontCollection(false, DWRITE_FONT_FAMILY_MODEL_TYPOGRAPHIC)?;
            let axes = [DWRITE_FONT_AXIS_VALUE { axisTag: DWRITE_FONT_AXIS_TAG_WEIGHT, value: weight }, DWRITE_FONT_AXIS_VALUE { axisTag: DWRITE_FONT_AXIS_TAG_OPTICAL_SIZE, value: 36.0 }];
            let format = factory.CreateTextFormat(PCWSTR(wide("Segoe UI Variable").as_ptr()), &fonts, &axes, size.max(1.0), PCWSTR(wide("").as_ptr()))?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            format.cast()
        }
    };
    made().or_else(|_| text::format(gfx, DISPLAY_FONT, size, DWRITE_FONT_WEIGHT_LIGHT))
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

/// What a widget's words are written with: lines of text are taken from a heap as they are
/// asked for, and what is left unused at the end is put out of sight.
struct Pen<'a> {
    scene: &'a Scene,
    gfx: &'a Gfx,
    words: &'a VisualCollection,
    lines: &'a mut Vec<Line>,
    line: usize,
    /// The words lie on the wallpaper itself: each line throws a shadow.
    bare: bool,
    zones: Zones,
    /// Where the words of the tasks that are done stand: left, middle, width (they are
    /// struck through, see `art`).
    struck: Vec<(f32, f32, f32)>,
    s: f32,
}

impl Pen<'_> {
    fn px(&self, v: f32) -> f32 {
        (v * self.s).round()
    }

    /// Writes a line of text; `put` then says where. Returns which line, and its size.
    fn write(&mut self, font: &IDWriteTextFormat, text: &str, color: D2D1_COLOR_F, most: Option<f32>) -> Result<(usize, f32, f32)> {
        if self.line == self.lines.len() {
            let line = Line::new_in(self.scene, self.words)?;
            if self.bare {
                line.cast_shadow(self.scene, SHADOW.0, SHADOW.1, rgba(0.0, 0.0, 0.0, SHADOW.2))?;
            }
            self.lines.push(line);
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

    /// Puts out of sight what was not asked for this time.
    fn done(&mut self) -> Result<()> {
        for line in &mut self.lines[self.line..] {
            line.set_opacity(0.0)?;
        }
        Ok(())
    }
}

/// What the sound's bars remember: how loud the last second or so was to the left and to
/// the right, the newest first; where each bar stands now (it eases after that); how long
/// ago the newest was taken in; and the loudest it has been lately, which everything is
/// measured by, so that quiet music fills the bars as loud music does.
struct Wave {
    heard: [[f32; BARS]; 2],
    shown: [[f32; BARS]; 2],
    since: f32,
    top: f32,
}

impl Wave {
    /// Seconds between two bars: how fast what was heard travels outwards.
    const STEP: f32 = 0.042;
    /// Under this a bar is not there, and a sound is no sound.
    const FLOOR: f32 = 0.005;

    fn new() -> Self {
        Wave { heard: [[0.0; BARS]; 2], shown: [[0.0; BARS]; 2], since: 0.0, top: 0.0 }
    }

    /// Takes in how loud it is now and moves the bars on by `dt` seconds. Returns true
    /// while any of them still stands or is about to rise.
    fn step(&mut self, level: Option<(f32, f32)>, dt: f32) -> bool {
        let (left, right) = level.unwrap_or((0.0, 0.0));
        let loudest = left.max(right);
        // The loudest of late sinks slowly, and is never so low that a hiss fills the bars.
        self.top = (self.top - dt * 0.04).max(loudest).max(0.1);
        self.since += dt;
        if self.since >= Self::STEP {
            // (After a sleep one bar is taken in, not one for every step that was missed.)
            self.since = (self.since - Self::STEP).min(Self::STEP);
            for (heard, level) in self.heard.iter_mut().zip([left, right]) {
                heard.copy_within(0..BARS - 1, 1);
                // A beat stands out more than its loudness alone would make it.
                heard[0] = (level / self.top).clamp(0.0, 1.0).powf(1.7);
            }
        }
        let ease = 1.0 - (-dt / 0.06).exp();
        // (A sound that has only just begun is not in the bars yet, but soon will be.)
        let mut standing = loudest >= Self::FLOOR;
        for (shown, heard) in self.shown.iter_mut().zip(&self.heard) {
            for (index, bar) in shown.iter_mut().enumerate() {
                // What has travelled outwards has lost height on the way.
                let wanted = heard[index] * (1.0 - 0.72 * index as f32 / BARS as f32);
                *bar += (wanted - *bar) * ease;
                if wanted < Self::FLOOR && *bar < Self::FLOOR {
                    *bar = 0.0;
                }
                standing |= *bar > 0.0;
            }
        }
        standing
    }

    /// Where the bars stand: those to the left of the middle, then those to the right, each
    /// from the middle outwards.
    fn bars(&self) -> Vec<f32> {
        self.shown.concat()
    }
}

pub struct Widget {
    hwnd: HWND,
    kind: Kind,
    _stage: Stage,
    /// The window under this one that shows the glass's shadow.
    shade: Option<Shade>,
    /// The glass it is made of; none for one that lies bare on the desktop. That one's
    /// words and picture are in a group of their own instead, and each throws a shadow.
    glass: Option<Glass>,
    ground: Option<Layer>,
    /// The picture, and over it, the words.
    _shapes: Layer,
    words: Layer,
    /// The picture under the words (see `art`): the sprite that shows it, what it is painted
    /// on, and the number of the picture it shows now (see `art_key`).
    art: Sprite,
    art_surface: Option<CompositionDrawingSurface>,
    art_key: Option<u64>,
    lines: Vec<Line>,
    /// The words of the note.
    block: Block,
    /// What the sound's bars remember.
    wave: Wave,
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
            let (glass, ground, shade, inside) = if kind.bare() {
                // No glass: what is written and drawn lies on the wallpaper itself.
                let ground = scene.layer_in(&stage.content()?)?;
                let inside = ground.content()?;
                (None, Some(ground), None, inside)
            } else {
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
                (Some(glass), None, shade, inside)
            };
            let shapes = scene.layer_in(&inside)?;
            let art = scene.sprite_in(&shapes.content()?)?;
            if kind.bare() {
                // The wallpaper may be as light as what is drawn on it: a soft shadow under
                // the picture, and under every line of words (see `Pen::write`), keeps
                // them readable.
                scene.shadow_under(&art, SHADOW.0, SHADOW.1, rgba(0.0, 0.0, 0.0, SHADOW.2))?;
            }
            let hot = scene.bead_in(&shapes.content()?)?;
            let words = scene.layer_in(&inside)?;
            let block = Block { sprite: scene.sprite_in(&words.content()?)?, _surface: None, shown: None, h: 0.0 };
            Ok(Widget {
                hwnd,
                kind,
                _stage: stage,
                shade,
                glass,
                ground,
                _shapes: shapes,
                words,
                art,
                art_surface: None,
                art_key: None,
                lines: Vec::new(),
                block,
                wave: Wave::new(),
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
        let (wide, tallest) = self.kind.size();
        let win = ((px(wide) + 2.0 * margin) as i32, (px(tallest) + 2.0 * margin) as i32);
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
        if let Some(glass) = self.glass.as_mut() {
            glass.set_material(&theme.island);
        }
        self.forget();
    }

    /// Forgets painted text and fonts, so they are made again (after a theme, size or GPU change).
    pub fn forget(&mut self) {
        self.fonts = None;
        for line in &mut self.lines {
            line.forget();
        }
        self.block.shown = None;
        (self.art_surface, self.art_key) = (None, None);
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
        let card_w = (self.kind.size().0 * self.scale) as i32;
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
                tiny: text::format(gfx, UI_FONT, (10.5 * s).round(), DWRITE_FONT_WEIGHT_NORMAL)?,
                title: text::format(gfx, UI_FONT, px(11.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                head: text::format(gfx, UI_FONT, px(15.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                big: text::format(gfx, DISPLAY_FONT, px(44.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                mid: text::format(gfx, DISPLAY_FONT, px(27.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                icons: text::format(gfx, ICON_FONT, px(14.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                check: text::format(gfx, ICON_FONT, px(9.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                wrap,
                huge: thin(gfx, px(BIG_DIGITS), 150.0)?,
                date: thin(gfx, px(20.0), 320.0)?,
            });
        }
        let f = self.fonts.clone().unwrap();
        let (wide, tallest) = self.kind.size();
        let words = self.words.content()?;
        let mut pen = Pen { scene, gfx, words: &words, lines: &mut self.lines, line: 0, bare: self.kind.bare(), zones: Vec::new(), struck: Vec::new(), s };
        let mut note = (0.0, 0.0);
        let mut sounding = false;
        let tall = match self.kind {
            Kind::Big => big(&mut pen, &f, info)?,
            Kind::Sound => {
                // Nothing is written on it: its bars are all there is to it (see `art`).
                sounding = self.wave.step(info.sound, dt);
                pen.px(WAVE.1)
            }
            Kind::Clock => clock(&mut pen, &f, theme, info)?,
            Kind::Calendar => calendar(&mut pen, &f, theme, info)?,
            Kind::Monitor => monitor(&mut pen, &f, theme, info)?,
            Kind::Todo => todo(&mut pen, &f, theme, info)?,
            Kind::Focus => focus(&mut pen, &f, theme, info)?,
            Kind::Progress => progress(&mut pen, &f, theme, info)?,
            Kind::Note => {
                let top = heading(&mut pen, &f, theme, "GHI CHÚ", Some(Zone::EditNote))?;
                // (The words stand a little in from a stroke of the accent colour, see `art`.)
                let (inset, column) = (pen.px(12.0), pen.px(CARD_W - 2.0 * PAD - 12.0));
                let words = if info.note.trim().is_empty() { "(note.txt đang trống)" } else { info.note.trim_end() };
                self.block.set(scene, gfx, &f.wrap, words, theme.text, column, pen.px(TALLEST - 60.0))?;
                self.block.sprite.place(pen.px(PAD) + inset, top, column, self.block.h)?;
                note = (top, self.block.h);
                // Anywhere on the words opens the file too.
                let bottom = top + self.block.h + pen.px(14.0);
                pen.zones.push((Zone::EditNote, 0.0, top, pen.px(CARD_W), bottom));
                bottom
            }
        };
        pen.done()?;
        self.zones = std::mem::take(&mut pen.zones);
        let extra = Extra { struck: std::mem::take(&mut pen.struck), note, bars: if self.kind == Kind::Sound { self.wave.bars() } else { Vec::new() } };
        self.block.sprite.set_opacity(if self.kind == Kind::Note { 1.0 } else { 0.0 })?;

        // The picture under the words. It is painted again only when what it shows has
        // changed: most passes leave it as it is. (What lies bare on the wallpaper is white
        // whatever the theme: there is no glass of the theme's under it.)
        let palette = if self.kind.bare() { Palette::plain(theme.accent, [1.0, 1.0, 1.0, 1.0], [1.0, 1.0, 1.0, 0.26]) } else { Palette::of(theme) };
        let key = art_key(self.kind, info, &palette, s, &extra);
        if self.art_key != Some(key) {
            let (w, h) = (px(wide) as u32, px(tallest) as u32);
            let mut canvas = Canvas::new(w, h);
            art(self.kind, &mut canvas, &palette, info, s, &extra);
            match self.art_surface.as_ref() {
                Some(surface) => scene.repaint(surface, w, h, &canvas.bgra)?,
                None => {
                    let surface = scene.surface_from_bgra(w, h, &canvas.bgra)?;
                    self.art.set_surface(&surface)?;
                    self.art_surface = Some(surface);
                }
            }
            self.art.place(0.0, 0.0, w as f32, h as f32)?;
            self.art_key = Some(key);
        }

        // The glass: as tall as what is on it. Growing, it overshoots like a drop.
        let tall = tall.min(px(tallest));
        if tall != self.h.target {
            self.h.tune(if tall > self.h.target { motion.medium() } else { motion.calm() });
        }
        self.h.set(tall);
        self.h.step(dt);
        let (m, w, h) = (self.margin, px(wide), self.h.pos.max(8.0));
        if let Some(glass) = self.glass.as_mut() {
            // (The round one is round at any size.)
            glass.set_rect(scene, m, m, w, h, if self.kind == Kind::Focus { h * 0.5 } else { px(RADIUS).min(h * 0.5) })?;
        }
        if let Some(ground) = self.ground.as_mut() {
            // What lies bare has no glass to be inside of: it goes where the glass would be.
            ground.set_offset(m, m)?;
        }

        // The drop of light under the cursor, on what can be clicked.
        let hot = self.cursor.filter(|_| !self.dragged).and_then(|(cx, cy)| self.zones.iter().find(|(_, l, t, r, b)| cx >= *l && cx < *r && cy >= *t && cy < *b).copied());
        self.hot_at.tune(motion);
        // (The whole of a note can be clicked; it gets no light of its own.)
        self.hot_at.aim(hot.filter(|(_, l, _, r, _)| r - l < w).map(|(_, l, t, r, b)| (l, t, r - l, b - t)));
        self.hot_at.step(dt);
        let shape = self.hot_at.shape(motion.squash());
        let (bead_top, bead_bottom, bead_edge) = theme.bead;
        self.hot.place(shape.x, shape.y, shape.w, shape.h, if self.kind == Kind::Focus { px(BUTTONS.2) } else { px(9.0) })?;
        self.hot.set_colors(bead_top, bead_bottom, bead_edge)?;
        self.hot.set_opacity(shape.alpha)?;

        // Only the glass takes the mouse (where there is none: what would be glass).
        let h = if self.glass.is_some() { h } else { tall };
        let wanted = ((m - 2.0).floor() as i32, (m - 2.0).floor() as i32, (m + w + 2.0).ceil() as i32, (m + h + 2.0).ceil() as i32);
        if self.region != Some(wanted) {
            self.region = Some(wanted);
            unsafe {
                // The system takes ownership of the region handle.
                let rgn = CreateRectRgn(wanted.0, wanted.1, wanted.2, wanted.3);
                SetWindowRgn(self.hwnd, Some(rgn), false);
            }
        }
        Ok((self.glass.is_some() && !self.h.settled(0.05)) || !self.hot_at.settled() || self.dragged || sounding)
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
// Each is written and painted: `Pen` writes its words (and returns how tall it has to be,
// in pixels), `art` paints the picture under them. Both go by the same marks, which are in
// logical pixels from its top-left corner.

/// The large clock: the size of its digits, and how far down the middle of the digits, the
/// date, and the hairline the seconds cross are; and half the length of that hairline.
const BIG_DIGITS: f32 = 136.0;
const BIG_MARKS: (f32, f32, f32, f32) = (80.0, 174.0, 206.0, 96.0);
/// The sound's bars: how far down the line they stand on is, how tall the tallest is, how
/// wide one is, and how far it is from one to the next.
const BAR: (f32, f32, f32, f32) = (74.0, 62.0, 3.0, 6.0);
/// The clock's face: where its middle is, and how large it is.
const DIAL: (f32, f32, f32) = (CARD_W - PAD - 42.0, 64.0, 42.0);
/// The graph of the processor's last minute: left, top, width, height.
const GRAPH: (f32, f32, f32, f32) = (96.0, 42.0, CARD_W - PAD - 96.0, 50.0);
/// The monitor's round gauges: how far down their middles are, and how large they are.
const GAUGE: (f32, f32) = (132.0, 22.0);
/// The focus timer's ring (its middle, its size), and the two buttons under the time left:
/// how far across their middles are, how far down, and how large they are.
const TIMER: (f32, f32, f32) = (PUCK * 0.5, PUCK * 0.5, 76.0);
const BUTTONS: ([f32; 2], f32, f32) = ([PUCK * 0.5 - 21.0, PUCK * 0.5 + 21.0], 123.0, 15.0);
/// The four rings of the progress card (their middle, the outermost one's size), and the
/// list beside them: its left edge, its first line, and the step from line to line.
const ROUNDS: (f32, f32, f32) = (PAD + 44.0, 94.0, 44.0);
const LEGEND: (f32, f32, f32) = (118.0, 58.0, 24.0);
/// The to-do list: where its first row begins, and how tall a row is.
const ROWS: (f32, f32) = (50.0, 28.0);

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

/// The large clock. No colon: the hour stands out, and the minutes step back beside it.
fn big(pen: &mut Pen, f: &Fonts, info: &Info) -> Result<f32> {
    let white = |solid: f32| D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: solid };
    let (hours, minutes) = info.time.split_once(':').unwrap_or((info.time, ""));
    let (middle, gap) = (pen.px(BIG.0 * 0.5), pen.px(13.0));
    pen.right(&f.huge, hours, white(1.0), None, middle - gap, pen.px(BIG_MARKS.0))?;
    pen.left(&f.huge, minutes, white(0.6), None, middle + gap, pen.px(BIG_MARKS.0))?;
    pen.middle(&f.date, info.date, white(0.94), middle, pen.px(BIG_MARKS.1))?;
    Ok(pen.px(BIG.1))
}

fn clock(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    pen.left(&f.big, info.time, theme.text, None, pen.px(PAD - 1.0), pen.px(48.0))?;
    // The day of the week stands out; the date follows it, quieter.
    let (day, date) = info.date.split_once(", ").unwrap_or((info.date, ""));
    let room = pen.px(DIAL.0 - DIAL.2 - PAD - 6.0);
    pen.left(&f.bold, day, theme.accent_text(), Some(room), pen.px(PAD), pen.px(87.0))?;
    pen.left(&f.small, date, theme.text_dim, Some(room), pen.px(PAD), pen.px(106.0))?;
    Ok(pen.px(128.0))
}

/// Where a day of the month stands on the calendar card: the middle of its cell.
fn day_spot(px: impl Fn(f32) -> f32, year: i32, month: u32, day: u32) -> (f32, f32) {
    let (side, cell_w, row_h, top) = (px(12.0), px((CARD_W - 24.0) / 7.0), px(27.0), px(66.0));
    let (column, row) = dates::cell(year, month, day);
    (side + cell_w * (column as f32 + 0.5), top + row_h * (row as f32 + 0.5))
}

fn calendar(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    let (year, month, today) = info.today;
    let s = pen.s;
    let px = |v: f32| (v * s).round();
    pen.left(&f.head, &format!("Tháng {month}"), theme.text, None, px(PAD), px(23.0))?;
    pen.right(&f.small, &year.to_string(), theme.text_dim, None, px(CARD_W - PAD), px(24.0))?;
    let (side, cell_w) = (px(12.0), px((CARD_W - 24.0) / 7.0));
    for (column, name) in dates::WEEKDAYS_SHORT.iter().enumerate() {
        pen.middle(&f.small, name, theme.text_dim, side + cell_w * (column as f32 + 0.5), px(52.0))?;
    }
    for day in 1..=dates::days_in_month(year, month) {
        let (column, _) = dates::cell(year, month, day);
        let (cx, cy) = day_spot(px, year, month, day);
        if day == today {
            // Today stands on a lit disc of the accent colour (painted, see `art`).
            pen.middle(&f.bold, &day.to_string(), D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, cx, cy)?;
        } else {
            // Saturday and Sunday are quieter.
            pen.middle(&f.text, &day.to_string(), if column >= 5 { theme.text_dim } else { theme.text }, cx, cy)?;
        }
    }
    Ok(px(66.0) + px(27.0) * dates::week_rows(year, month) as f32 + px(10.0))
}

/// The monitor's round gauges: a name, a few words, and how full it is (0 to 1).
fn gauges(info: &Info) -> Vec<(&'static str, String, f32)> {
    let vital = info.vital;
    let mut list = vec![("RAM", format!("{:.1}/{:.0} GB", vital.used, vital.total), vital.memory as f32 / 100.0)];
    if let Some((used, total)) = info.disk {
        list.push(("Ổ đĩa", format!("{used:.0}/{total:.0} GB"), used / total.max(1.0)));
    }
    if let Some((percent, plugged)) = info.battery {
        list.push(("Pin", if plugged { "đang sạc" } else { "đang dùng" }.to_string(), percent as f32 / 100.0));
    }
    list
}

/// How far across the card the middle of a gauge is, of `count` side by side.
fn gauge_x(index: usize, count: usize) -> f32 {
    CARD_W * (index as f32 + 0.5) / count.max(1) as f32
}

fn monitor(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    heading(pen, f, theme, "MÁY TÍNH", None)?;
    let pad = pen.px(PAD);
    // The processor: how busy it is now, large, beside the graph of its last minute.
    pen.left(&f.small, "CPU", theme.text_dim, None, pad, pen.px(51.0))?;
    pen.left(&f.mid, &format!("{}%", info.vital.processor), theme.text, None, pad - pen.px(1.0), pen.px(75.0))?;
    // Memory, disk and battery: a round gauge each, with what it stands at inside it.
    let list = gauges(info);
    let below = GAUGE.0 + GAUGE.1;
    for (index, (name, words, share)) in list.iter().enumerate() {
        let cx = pen.px(gauge_x(index, list.len()));
        pen.middle(&f.bold, &format!("{}%", (share * 100.0).round() as u32), theme.text, cx, pen.px(GAUGE.0))?;
        pen.middle(&f.small, name, theme.text, cx, pen.px(below + 15.0))?;
        pen.middle(&f.tiny, words, theme.text_dim, cx, pen.px(below + 31.0))?;
    }
    // The network, under a hairline.
    let (down, up) = info.traffic;
    let y = pen.px(below + 60.0);
    pen.left(&f.small, &format!("↓ {}", vitals::speed_text(down)), theme.text, None, pad, y)?;
    pen.right(&f.small, &format!("↑ {}", vitals::speed_text(up)), theme.text_dim, None, pen.px(CARD_W - PAD), y)?;
    Ok(y + pen.px(20.0))
}

fn todo(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    let left = info.tasks.iter().filter(|task| !task.done).count();
    let words = if info.tasks.is_empty() { "VIỆC CẦN LÀM".to_string() } else { format!("VIỆC CẦN LÀM · CÒN {left}") };
    let top = heading(pen, f, theme, &words, Some(Zone::EditTasks))?;
    let (pad, row_h, card_w) = (pen.px(PAD), pen.px(ROWS.1), pen.px(CARD_W));
    if info.tasks.is_empty() {
        pen.left(&f.small, "Chưa có việc nào. Bấm cây bút để thêm.", theme.text_dim, Some(card_w - 2.0 * pad), pad, top + row_h * 0.5)?;
        return Ok(top + row_h + pen.px(10.0));
    }
    // (Under the heading a thin bar says how much is done; it and the rings to tick are
    // painted, see `art`.)
    let mut y = pen.px(ROWS.0);
    let white = D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 };
    for (index, task) in info.tasks.iter().enumerate().take(MOST_TASKS) {
        let middle = y + row_h * 0.5;
        if task.done {
            pen.middle(&f.check, &glyph(CHECK), white, pad + pen.px(8.0), middle)?;
        }
        // Done, the words step back and are struck through.
        let x = pad + pen.px(26.0);
        let w = pen.left(&f.text, &task.text, if task.done { theme.text_dim } else { theme.text }, Some(card_w - pad - x), x, middle)?;
        if task.done {
            pen.struck.push((x, middle, w));
        }
        pen.zones.push((Zone::Task(index), pen.px(8.0), y, card_w - pen.px(8.0), y + row_h));
        y += row_h;
    }
    if info.tasks.len() > MOST_TASKS {
        pen.left(&f.small, &format!("và {} việc nữa", info.tasks.len() - MOST_TASKS), theme.text_dim, None, pad + pen.px(26.0), y + pen.px(11.0))?;
        y += pen.px(22.0);
    }
    Ok(y + pen.px(10.0))
}

fn focus(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    let (stage, seconds, _, running) = info.focus;
    // It is round, and everything on it stands in the middle of its ring: what it counts,
    // small; the time left, large; and two round buttons: start or stop, and back to the
    // start.
    let across = pen.px(TIMER.0);
    let words = match (stage, running) {
        (tasks::Stage::Focus, true) => "TẬP TRUNG",
        (tasks::Stage::Rest, true) => "NGHỈ",
        (_, false) => "ĐANG DỪNG",
    };
    pen.middle(&f.title, words, theme.text_dim, across, pen.px(52.0))?;
    pen.middle(&f.mid, &format!("{:02}:{:02}", seconds / 60, seconds % 60), theme.text, across, pen.px(83.0))?;
    let (middle, reach) = (pen.px(BUTTONS.1), pen.px(BUTTONS.2));
    for (at, (symbol, zone)) in BUTTONS.0.into_iter().zip([(if running { PAUSE } else { PLAY }, Zone::FocusToggle), (AGAIN, Zone::FocusReset)]) {
        let x = pen.px(at);
        pen.middle(&f.icons, &glyph(symbol), theme.text, x, middle)?;
        pen.zones.push((zone, x - reach, middle - reach, x + reach, middle + reach));
    }
    Ok(pen.px(PUCK))
}

fn progress(pen: &mut Pen, f: &Fonts, theme: &Theme, info: &Info) -> Result<f32> {
    heading(pen, f, theme, "TIẾN ĐỘ", None)?;
    for (index, (name, share)) in ["Hôm nay", "Tuần này", "Tháng này", "Năm nay"].into_iter().zip(info.gone).enumerate() {
        let y = pen.px(LEGEND.1) + pen.px(LEGEND.2) * index as f32;
        pen.left(&f.small, name, theme.text, None, pen.px(LEGEND.0 + 14.0), y)?;
        pen.right(&f.bold, &format!("{}%", (share * 100.0).floor() as u32), theme.text, None, pen.px(CARD_W - PAD), y)?;
    }
    Ok(pen.px(150.0))
}

// ------------------------------------------------------------- their pictures

/// The colours the pictures are painted in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The accent colour, and the lighter one beside it on the colour wheel that it runs into.
    pub accent: Rgba,
    pub bright: Rgba,
    /// What is written on the glass, and the groove a gauge runs in.
    pub ink: Rgba,
    pub track: Rgba,
}

impl Palette {
    pub fn of(theme: &Theme) -> Self {
        let track = theme.track;
        Palette::plain(theme.accent, [theme.text.r, theme.text.g, theme.text.b, theme.text.a], [track.R as f32 / 255.0, track.G as f32 / 255.0, track.B as f32 / 255.0, track.A as f32 / 255.0])
    }

    /// From the colours themselves (the settings window has no theme of the app's).
    pub fn plain(accent: (f32, f32, f32), ink: Rgba, track: Rgba) -> Self {
        let accent = [accent.0, accent.1, accent.2, 1.0];
        Palette { accent, bright: lighter(accent), ink, track }
    }

    /// The two colours of one of several things shown side by side: the first has the accent
    /// colour, each next one a colour further round the wheel.
    fn series(&self, index: usize) -> (Rgba, Rgba) {
        let colour = turn(self.accent, 38.0 * index as f32);
        (colour, lighter(colour))
    }
}

/// The lighter colour a stroke runs into at its head.
fn lighter(colour: Rgba) -> Rgba {
    mix(turn(colour, -16.0), [1.0, 1.0, 1.0, 1.0], 0.34)
}

/// What a picture needs that only writing the card's words tells: where the words of the
/// tasks that are done stand (left, middle, width), and where the note's words begin and
/// how tall they are.
#[derive(Default)]
struct Extra {
    struck: Vec<(f32, f32, f32)>,
    note: (f32, f32),
    /// Where the sound's bars stand (see `Wave::bars`).
    bars: Vec<f32>,
}

/// The sound's bars: those to the left of `middle` and those to the right, each from the
/// middle outwards, standing on the line `floor` down, the tallest `tall` high; and under
/// the line, fainter and shorter, their reflection, as of things that stand at the water's
/// edge. A bar with nothing to show is a dot on the line. `unit`: pixels to one of `BAR`'s.
fn bars(canvas: &mut Canvas, p: &Palette, middle: f32, floor: f32, tall: f32, unit: f32, levels: &[f32]) {
    let (wide, pitch) = ((BAR.2 * unit).round().max(1.0), (BAR.3 * unit).round().max(2.0));
    let each = (levels.len() / 2).max(1);
    for (index, level) in levels.iter().enumerate() {
        let (side, out) = (index / each, (index % each) as f32);
        let x = (if side == 0 { middle - pitch * (out + 0.5) } else { middle + pitch * (out + 0.5) } - wide * 0.5).round();
        // The newest, in the middle, has some of the accent colour; further out they pale.
        let colour = fade(mix(p.bright, p.ink, (out / 5.0).min(1.0)), 1.0 - 0.6 * out / each as f32);
        let high = wide + level.clamp(0.0, 1.0) * (tall - wide);
        canvas.bar(x, floor - high, wide, high, wide * 0.5, colour, colour);
        if high > wide * 2.0 {
            let mirrored = fade(colour, 0.2);
            canvas.bar(x, floor + wide, wide, high * 0.36, wide * 0.5, mirrored, mirrored);
        }
    }
}

/// A clock's face: a rim, a mark for every hour, the two hands, and a bead of light that
/// goes round the rim with the seconds, drawing a short tail.
fn dial(canvas: &mut Canvas, p: &Palette, cx: f32, cy: f32, radius: f32, now: (u32, u32, u32)) {
    let unit = radius / 42.0;
    let rim = radius - 3.0 * unit;
    canvas.arc(cx, cy, rim, 2.4 * unit, 0.0, 1.0, p.track, p.track);
    for hour in 0..12 {
        let turns = hour as f32 / 12.0;
        if hour % 3 == 0 {
            let (from, to) = (on_circle(cx, cy, rim - 6.5 * unit, turns), on_circle(cx, cy, rim - 11.5 * unit, turns));
            canvas.line(from.0, from.1, to.0, to.1, 2.0 * unit, fade(p.ink, 0.7));
        } else {
            let at = on_circle(cx, cy, rim - 8.5 * unit, turns);
            canvas.disc(at.0, at.1, 1.15 * unit, fade(p.ink, 0.32));
        }
    }
    let (hour, minute, second) = (now.0 % 12, now.1 % 60, now.2 % 60);
    let seconds = second as f32 / 60.0;
    canvas.arc(cx, cy, rim, 2.4 * unit, seconds - 0.16, 0.16, fade(p.accent, 0.0), p.bright);
    let bead = on_circle(cx, cy, rim, seconds);
    canvas.glow(bead.0, bead.1, 10.0 * unit, fade(p.bright, 0.5));
    canvas.disc(bead.0, bead.1, 3.3 * unit, p.bright);
    let hand = on_circle(cx, cy, 17.0 * unit, (hour as f32 + minute as f32 / 60.0) / 12.0);
    canvas.line(cx, cy, hand.0, hand.1, 4.2 * unit, p.ink);
    let hand = on_circle(cx, cy, 27.5 * unit, (minute as f32 + seconds) / 60.0);
    canvas.line(cx, cy, hand.0, hand.1, 2.8 * unit, p.ink);
    canvas.disc(cx, cy, 3.8 * unit, p.accent);
}

/// A round gauge: a groove, and in it an arc as long as `share` of the way round, which
/// can be lit at its head.
fn round_gauge(canvas: &mut Canvas, p: &Palette, cx: f32, cy: f32, radius: f32, thick: f32, share: f32, colours: (Rgba, Rgba), lit: bool) {
    canvas.arc(cx, cy, radius, thick, 0.0, 1.0, p.track, p.track);
    let share = share.clamp(0.0, 1.0);
    if share < 0.004 {
        return;
    }
    canvas.arc(cx, cy, radius, thick, 0.0, share, colours.0, colours.1);
    if lit && share < 0.995 {
        let head = on_circle(cx, cy, radius, share);
        canvas.glow(head.0, head.1, thick * 1.7, fade(colours.1, 0.55));
    }
}

/// Paints what a card shows besides its words. The picture is the card: its top-left
/// corner is the card's. `s`: pixels to a logical pixel.
fn art(kind: Kind, canvas: &mut Canvas, p: &Palette, info: &Info, s: f32, extra: &Extra) {
    let px = |v: f32| (v * s).round();
    if !kind.bare() {
        // A faint light of the accent colour in the far corner of every card.
        canvas.glow(px(kind.size().0) - px(10.0), -px(20.0), px(150.0), fade(p.accent, 0.13));
    }
    match kind {
        Kind::Big => {
            // A hairline under the date. A bead of light crosses it once a minute, with the
            // seconds, and draws its tail behind it.
            let (middle, y, half) = (px(BIG.0 * 0.5), px(BIG_MARKS.2), px(BIG_MARKS.3));
            let thin = s.round().max(1.0);
            canvas.bar(middle - half, y - thin * 0.5, 2.0 * half, thin, thin * 0.5, p.track, p.track);
            let head = middle - half + 2.0 * half * (info.now.2 % 60) as f32 / 59.0;
            canvas.bar(middle - half, y - thin, head - (middle - half), 2.0 * thin, thin, fade(p.bright, 0.0), p.bright);
            canvas.glow(head, y, 10.0 * s, fade(p.bright, 0.6));
            canvas.disc(head, y, 3.2 * s, p.bright);
        }
        Kind::Sound => bars(canvas, p, px(WAVE.0 * 0.5), px(BAR.0), px(BAR.1), s, &extra.bars),
        Kind::Clock => dial(canvas, p, px(DIAL.0), px(DIAL.1), px(DIAL.2), info.now),
        Kind::Calendar => {
            let (year, month, today) = info.today;
            let (cx, cy) = day_spot(px, year, month, today);
            canvas.glow(cx, cy, px(24.0), fade(p.accent, 0.5));
            canvas.disc(cx, cy, px(12.5), p.accent);
        }
        Kind::Monitor => {
            // The processor's last minute, the oldest at the left; what is not yet known is nought.
            let mut values = vec![0.0; HISTORY.saturating_sub(info.history.len())];
            values.extend(info.history.iter().map(|&percent| percent as f32 / 100.0));
            let (x, y, w, h) = (px(GRAPH.0), px(GRAPH.1), px(GRAPH.2), px(GRAPH.3));
            let thick = 2.0 * s;
            canvas.graph(x, y, w, h, &values, thick, p.bright, fade(p.accent, 0.5));
            // The newest end of the line is lit.
            let newest = y + h - thick * 0.5 - values.last().copied().unwrap_or(0.0).clamp(0.0, 1.0) * (h - thick);
            canvas.glow(x + w - 2.0 * s, newest, 8.0 * s, fade(p.bright, 0.55));
            canvas.disc(x + w - 2.0 * s, newest, 2.6 * s, p.bright);
            let list = gauges(info);
            for (index, (_, _, share)) in list.iter().enumerate() {
                round_gauge(canvas, p, px(gauge_x(index, list.len())), px(GAUGE.0), px(GAUGE.1), px(5.0), *share, (p.accent, p.bright), false);
            }
            canvas.bar(px(PAD), px(GAUGE.0 + GAUGE.1 + 45.0), px(CARD_W - 2.0 * PAD), s.round().max(1.0), 0.0, p.track, p.track);
        }
        Kind::Todo => {
            if info.tasks.is_empty() {
                return;
            }
            // How much is done, as a thin bar under the heading.
            let done = info.tasks.iter().filter(|task| task.done).count() as f32 / info.tasks.len() as f32;
            let (x, y, w, h) = (px(PAD), px(39.0), px(CARD_W - 2.0 * PAD), px(3.0));
            canvas.bar(x, y, w, h, h * 0.5, p.track, p.track);
            if done > 0.0 {
                canvas.bar(x, y, (w * done).max(h), h, h * 0.5, p.accent, p.bright);
            }
            // A ring to tick in front of each task; ticked, it is a disc of the accent colour.
            for (index, task) in info.tasks.iter().enumerate().take(MOST_TASKS) {
                let (cx, cy) = (px(PAD) + px(8.0), px(ROWS.0) + px(ROWS.1) * (index as f32 + 0.5));
                if task.done {
                    canvas.disc(cx, cy, px(8.0), p.accent);
                } else {
                    let ring = fade(p.ink, 0.42);
                    canvas.arc(cx, cy, px(7.0), 1.5 * s, 0.0, 1.0, ring, ring);
                }
            }
            for &(x, y, w) in &extra.struck {
                canvas.line(x, y + s, x + w, y + s, s.max(1.0), fade(p.ink, 0.42));
            }
        }
        Kind::Focus => {
            let (stage, _, share, running) = info.focus;
            // A rest is drawn quieter than a stretch of focus, and a timer that is stopped
            // quieter than one that runs.
            let colours = if stage == tasks::Stage::Focus { (p.accent, p.bright) } else { (fade(p.ink, 0.4), fade(p.ink, 0.7)) };
            let colours = if running { colours } else { (fade(colours.0, 0.5), fade(colours.1, 0.5)) };
            // The ring runs just inside the round card's rim.
            round_gauge(canvas, p, px(TIMER.0), px(TIMER.1), px(TIMER.2), px(6.0), share, colours, running);
            for at in BUTTONS.0 {
                canvas.disc(px(at), px(BUTTONS.1), px(BUTTONS.2), fade(p.ink, 0.09));
            }
        }
        Kind::Note => {
            // A stroke of the accent colour down the side of the words.
            let (top, tall) = extra.note;
            canvas.bar(px(PAD), top + px(2.0), px(3.0), (tall - px(4.0)).max(px(3.0)), px(1.5), p.accent, p.accent);
        }
        Kind::Progress => {
            let (thick, step) = (px(6.0), px(9.0));
            for (index, share) in info.gone.into_iter().enumerate() {
                let colours = p.series(index);
                round_gauge(canvas, p, px(ROUNDS.0), px(ROUNDS.1), px(ROUNDS.2) - thick * 0.5 - step * index as f32, thick, share, colours, true);
                canvas.disc(px(LEGEND.0) + px(4.0), px(LEGEND.1) + px(LEGEND.2) * index as f32, px(4.0), colours.1);
            }
        }
    }
}

/// Tells one picture from another without painting it: what `art` would show, boiled down
/// to a number. The same number, the same picture.
fn art_key(kind: Kind, info: &Info, p: &Palette, s: f32, extra: &Extra) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let bits = |v: f32| v.to_bits();
    (kind as u8, bits(s)).hash(&mut hasher);
    for colour in [p.accent, p.bright, p.ink, p.track] {
        colour.map(bits).hash(&mut hasher);
    }
    match kind {
        Kind::Big => info.now.2.hash(&mut hasher),
        // (A bar is painted again when it is a pixel taller or shorter, not before.)
        Kind::Sound => extra.bars.iter().map(|level| (level * BAR.1 * s) as u32).collect::<Vec<_>>().hash(&mut hasher),
        Kind::Clock => info.now.hash(&mut hasher),
        Kind::Calendar => info.today.hash(&mut hasher),
        Kind::Monitor => {
            info.history.hash(&mut hasher);
            for (_, _, share) in gauges(info) {
                ((share * 400.0) as u32).hash(&mut hasher);
            }
        }
        Kind::Todo => {
            for task in info.tasks.iter().take(MOST_TASKS) {
                task.done.hash(&mut hasher);
            }
            info.tasks.len().hash(&mut hasher);
            for &(x, y, w) in &extra.struck {
                (bits(x), bits(y), bits(w)).hash(&mut hasher);
            }
        }
        Kind::Focus => {
            let (stage, _, share, running) = info.focus;
            (stage == tasks::Stage::Focus, (share * 720.0) as u32, running).hash(&mut hasher);
        }
        Kind::Note => (bits(extra.note.0), bits(extra.note.1)).hash(&mut hasher),
        Kind::Progress => info.gone.map(|share| (share * 720.0) as u32).hash(&mut hasher),
    }
    hasher.finish()
}

/// A small square picture of a widget, for the page of the settings that lists them: what
/// makes the card its own, drawn with the same strokes.
pub fn emblem(kind: Kind, size: u32, p: &Palette) -> Canvas {
    let mut canvas = Canvas::new(size, size);
    let n = size as f32;
    let (middle, unit) = (n * 0.5, n / 64.0);
    match kind {
        Kind::Big => {
            // Its digits are written by whoever shows the picture (they are text); here is
            // what is under them: the hairline and its bead.
            canvas.bar(8.0 * unit, 51.0 * unit, 48.0 * unit, unit.max(1.0), unit * 0.5, p.track, p.track);
            canvas.bar(8.0 * unit, 50.5 * unit, 30.0 * unit, 2.0 * unit, unit, fade(p.bright, 0.0), p.bright);
            canvas.glow(38.0 * unit, 51.5 * unit, 8.0 * unit, fade(p.bright, 0.6));
            canvas.disc(38.0 * unit, 51.5 * unit, 2.6 * unit, p.bright);
        }
        Kind::Sound => {
            // A beat in the middle, and the ones before it on their way out.
            let half = [0.95, 0.5, 0.74, 0.34, 0.5, 0.2, 0.26];
            bars(&mut canvas, p, middle, 44.0 * unit, 34.0 * unit, unit * 0.8, &[half, half].concat());
        }
        Kind::Clock => dial(&mut canvas, p, middle, middle, n * 0.47, (10, 9, 37)),
        Kind::Calendar => {
            // A month as rows of dots under a heading, one of the days lit.
            canvas.bar(middle - 25.0 * unit, 8.0 * unit, 22.0 * unit, 4.5 * unit, 2.2 * unit, fade(p.ink, 0.8), fade(p.ink, 0.8));
            for row in 0..4 {
                for column in 0..5 {
                    let (x, y) = (middle + (column as f32 - 2.0) * 11.5 * unit, 24.0 * unit + row as f32 * 10.5 * unit);
                    if (row, column) == (2, 3) {
                        canvas.glow(x, y, 11.0 * unit, fade(p.accent, 0.55));
                        canvas.disc(x, y, 5.0 * unit, p.accent);
                    } else {
                        canvas.disc(x, y, 2.3 * unit, fade(p.ink, if column == 4 { 0.25 } else { 0.5 }));
                    }
                }
            }
        }
        Kind::Monitor => {
            let values = [0.28, 0.42, 0.34, 0.55, 0.4, 0.72, 0.5, 0.64, 0.86];
            canvas.graph(4.0 * unit, 10.0 * unit, n - 8.0 * unit, n - 22.0 * unit, &values, 2.4 * unit, p.bright, fade(p.accent, 0.55));
            canvas.glow(n - 6.0 * unit, 17.5 * unit, 9.0 * unit, fade(p.bright, 0.55));
            canvas.disc(n - 6.0 * unit, 17.5 * unit, 3.0 * unit, p.bright);
        }
        Kind::Todo => {
            // Three tasks, the first two done.
            for (row, done) in [true, true, false].into_iter().enumerate() {
                let y = 15.0 * unit + row as f32 * 17.0 * unit;
                if done {
                    canvas.disc(12.0 * unit, y, 6.0 * unit, p.accent);
                } else {
                    let ring = fade(p.ink, 0.5);
                    canvas.arc(12.0 * unit, y, 5.2 * unit, 1.6 * unit, 0.0, 1.0, ring, ring);
                }
                let words = fade(p.ink, if done { 0.35 } else { 0.8 });
                canvas.bar(24.0 * unit, y - 2.2 * unit, [30.0, 22.0, 34.0][row] * unit, 4.4 * unit, 2.2 * unit, words, words);
            }
        }
        Kind::Focus => {
            round_gauge(&mut canvas, p, middle, middle, n * 0.38, 7.0 * unit, 0.7, (p.accent, p.bright), true);
            // (Running: the two bars that stop it.)
            for x in [-4.5, 4.5] {
                canvas.bar(middle + (x - 1.8) * unit, middle - 7.0 * unit, 3.6 * unit, 14.0 * unit, 1.8 * unit, p.ink, p.ink);
            }
        }
        Kind::Note => {
            canvas.bar(9.0 * unit, 12.0 * unit, 3.4 * unit, 40.0 * unit, 1.7 * unit, p.accent, p.accent);
            for (row, width) in [36.0, 30.0, 38.0, 20.0].into_iter().enumerate() {
                let words = fade(p.ink, 0.7);
                canvas.bar(18.0 * unit, 14.0 * unit + row as f32 * 11.0 * unit, width * unit, 4.4 * unit, 2.2 * unit, words, words);
            }
        }
        Kind::Progress => {
            for (index, share) in [0.62, 0.3, 0.78, 0.9].into_iter().enumerate() {
                round_gauge(&mut canvas, p, middle, middle, n * 0.45 - 6.6 * unit * index as f32, 3.8 * unit, share, p.series(index), index == 0);
            }
        }
    }
    canvas
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
    /// (BEN_LIGHT=1 for the light look, BEN_SECONDS for how long, BEN_SCALE for a screen
    /// that is scaled. With BEN_OUT naming a folder, the widgets are put on a screen that is
    /// not there instead, and a picture of each is saved in that folder: nothing shows on
    /// the real screens. BEN_BACK=r,g,b, each 0 to 255, puts a colour behind each, as a
    /// wallpaper of that colour would be; without it they are on black. BEN_SOUND=1 has the
    /// sound's bars follow what the computer is playing, in place of made-up music.)
    #[test]
    #[ignore]
    fn widgets_on_the_desktop() {
        let gfx = Gfx::new(false).expect("a graphics device");
        let scene = Scene::new(&gfx).expect("a compositor");
        let mut cfg = Config::default();
        cfg.theme = if std::env::var("BEN_LIGHT").is_ok() { 2.0 } else { 1.0 };
        let theme = theme::build(&cfg, true, (0.0, 0.47, 0.84), 1.0);
        let motion = Motion::new(1.0, 0.5, false);
        let out = std::env::var("BEN_OUT").ok().map(std::path::PathBuf::from);
        let work = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        let mut widgets: Vec<Widget> = Kind::ALL.into_iter().map(|kind| Widget::new(&scene, &theme, &motion, kind).expect("a widget")).collect();
        let scale: f32 = std::env::var("BEN_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
        for (index, widget) in widgets.iter_mut().enumerate() {
            let at = if out.is_some() { (-20000 + index as i32 * 1400, 100) } else { widget.kind().home(work, scale) };
            widget.place(at, scale, &theme);
        }
        let back: Vec<f32> = std::env::var("BEN_BACK").unwrap_or_default().split(',').filter_map(|part| part.trim().parse().ok()).collect();
        let _backs: Vec<_> = widgets.iter().filter(|_| back.len() == 3).map(|widget| scene.backdrop_in(&widget._stage.content().expect("a stage"), rgba(back[0] / 255.0, back[1] / 255.0, back[2] / 255.0, 1.0)).expect("a backdrop")).collect();
        let history: Vec<u8> = (0..HISTORY).map(|i| (30.0 + 25.0 * (i as f32 * 0.5).sin() + i as f32) as u8).collect();
        let tasks = vec![
            Task { text: "Viết báo cáo tuần".into(), done: false },
            Task { text: "Gọi cho Lan về buổi họp thứ sáu, nhớ hỏi cả lịch tháng sau".into(), done: false },
            Task { text: "Mua cà phê".into(), done: true },
        ];
        let meter = std::env::var("BEN_SOUND").ok().and_then(|_| {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
            }
            crate::audio::Meter::new()
        });
        let seconds: f32 = std::env::var("BEN_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(if out.is_some() { 1.6 } else { 4.0 });
        let started = std::time::Instant::now();
        let mut msg = MSG::default();
        while started.elapsed().as_secs_f32() < seconds {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            // Music, made up: a beat a little over twice a second, louder to the left now
            // and to the right then.
            let time = started.elapsed().as_secs_f32();
            let loud = 0.16 + 0.7 * (1.0 - (time * 2.2).fract()).powi(4) + 0.07 * (time * 13.0).sin();
            let sound = ((loud * (0.9 + 0.1 * (time * 3.0).sin())).clamp(0.0, 1.0), (loud * (0.9 - 0.1 * (time * 3.0).sin())).clamp(0.0, 1.0));
            // (Or what the computer is playing in earnest, read as the app reads it.)
            let sound = meter.as_ref().and_then(|meter| meter.peaks()).unwrap_or(sound);
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
                now: (14, 5, 38),
                note: "Họp nhóm lúc 15:00 ở phòng 3.
Nhớ mang theo bản in và hỏi về hạn nộp của đợt tới.",
                focus: (tasks::Stage::Focus, 1124, 0.25, true),
                gone: [0.59, 0.08, 0.52, 0.88],
                sound: Some(sound),
            };
            for widget in &mut widgets {
                widget.update(&scene, &gfx, &theme, &motion, &info, 1.0 / 60.0).expect("an update");
            }
            scene.commit();
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        if let Some(out) = out {
            std::thread::sleep(std::time::Duration::from_millis(150));
            for widget in &widgets {
                let pixels = unsafe { crate::camera::print(widget.hwnd, widget.win.0, widget.win.1) }.expect("a picture of the window");
                crate::camera::save_picture(&out.join(format!("{}.bmp", widget.kind.name())), widget.win.0, widget.win.1, &pixels);
                println!("{}: {}x{}", widget.kind.name(), widget.win.0, widget.win.1);
            }
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
            let (wide, _) = kind.size();
            if kind.bare() {
                // In the middle of the screen.
                assert_eq!(x + wide as i32 / 2, 960, "{kind:?}");
            } else {
                assert!(x >= 1300 && x + wide as i32 <= 1920 - 20, "{kind:?} at {x}");
            }
            // Under the island's line, and its top well clear of the dock.
            assert!(y >= 56 && y < 760, "{kind:?} at {y}");
            assert!(!Kind::ALL[..index].iter().any(|other| other.home(work, 1.0) == (x, y)));
        }
        // At twice the scale everything is twice as far from the corner.
        let (x, _) = Kind::Clock.home(work, 2.0);
        assert_eq!(1920 - x, 2 * (1920 - Kind::Clock.home(work, 1.0).0));
    }

    #[test]
    fn no_two_cards_lie_on_each_other_where_they_start() {
        let work = RECT { left: 0, top: 0, right: 1920, bottom: 1080 };
        // How tall each is at its tallest, as its words are written (see the cards).
        let tall = |kind: Kind| match kind {
            Kind::Clock => 128.0,
            Kind::Calendar => 238.0,
            Kind::Monitor => 234.0,
            Kind::Todo => 306.0,
            Kind::Progress => 150.0,
            Kind::Note => 120.0,
            other => other.size().1,
        };
        let rect = |kind: Kind| {
            let (x, y) = kind.home(work, 1.0);
            (x, y, x + kind.size().0 as i32, y + tall(kind) as i32)
        };
        for (index, kind) in Kind::ALL.into_iter().enumerate() {
            let a = rect(kind);
            for other in &Kind::ALL[..index] {
                let b = rect(*other);
                assert!(a.2 <= b.0 || b.2 <= a.0 || a.3 <= b.1 || b.3 <= a.1, "{kind:?} lies on {other:?}");
            }
        }
    }

    #[test]
    fn the_sound_s_bars_rise_travel_outwards_and_come_to_rest() {
        let mut wave = Wave::new();
        // Silence: nothing stands, and nothing has to be drawn again.
        assert!(!wave.step(None, 0.016));
        assert!(!wave.step(Some((0.0, 0.0)), 0.016));
        assert!(wave.bars().iter().all(|bar| *bar == 0.0));
        assert_eq!(wave.bars().len(), 2 * BARS);
        // A beat to the left only: after a moment the bars next to the middle on that side
        // stand, and those on the other side do not.
        for _ in 0..6 {
            assert!(wave.step(Some((0.8, 0.0)), 0.016));
        }
        let bars = wave.bars();
        assert!(bars[0] > 0.2, "{}", bars[0]);
        assert_eq!(bars[BARS], 0.0);
        assert_eq!(bars[BARS - 1], 0.0, "it has not travelled to the far end yet");
        // Then silence: what was heard travels on outwards, losing height, and in the end
        // everything is at rest again.
        let mut furthest = 0;
        let mut frames = 0;
        while wave.step(Some((0.0, 0.0)), 0.016) {
            let bars = wave.bars();
            furthest = furthest.max((0..BARS).rev().find(|index| bars[*index] > 0.0).unwrap_or(0));
            frames += 1;
            assert!(frames < 600, "it never came to rest");
        }
        assert!(furthest > BARS / 2, "it got as far as bar {furthest}");
        assert!(wave.bars().iter().all(|bar| *bar == 0.0));
        // Quiet music fills the bars as loud music does: they are measured by the loudest
        // of late, which sinks when it gets quieter.
        let mut quiet = Wave::new();
        for _ in 0..400 {
            quiet.step(Some((0.12, 0.12)), 0.016);
        }
        assert!(quiet.bars()[0] > 0.6, "{}", quiet.bars()[0]);
    }
}
