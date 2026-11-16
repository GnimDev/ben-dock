//! The island: one capsule of glass at the top centre of a screen that is the
//! clock, the status lights and the control panel, and that changes shape to
//! tell what is going on.
//!
//! It moves between its shapes like a drop of liquid:
//!
//!   compact   the time, the date, network / sound / battery, and three
//!             dancing bars while music plays
//!   alert     swells for a couple of seconds when something happens: the
//!             volume changed, the charger went in, the network dropped, a
//!             new track started
//!   open      under the cursor it opens into a panel. Its first page has what
//!             is playing, the volume, and a row of cards; a card turns the
//!             panel into that card's own page (Wi-Fi, sound, Bluetooth,
//!             battery, the month) without leaving the island
//!
//! What a page *says* is decided by the app (`app.rs`), which hands the island
//! a list of rows; the island only lays them out and reports what was clicked.
//! The window is as large as the largest page and never moves; only the glass
//! inside it changes size, and only the glass takes the mouse.

use crate::app::{self, Event};
use crate::config::Config;
use crate::dates;
use crate::gfx::Gfx;
use crate::media::NowPlaying;
use crate::motion::{Glide, Motion};
use crate::scene::{rgba, Bead, Glass, Layer, Pill, Scene, Stage};
use crate::shade::Shade;
use crate::spring::Spring;
use crate::system::{AppBar, Monitor, Status};
use crate::text::{self, glyph, Line, DISPLAY_FONT, ICON_FONT, UI_FONT};
use crate::theme::Theme;
use crate::util::wide;
use std::cell::Cell;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::VisualCollection;

const CLASS_NAME: &str = "BenIslandWindow";
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_ISLAND_APPBAR: u32 = WM_APP + 2;

const TIMER_ALERT: usize = 1;
const TIMER_DWELL: usize = 2;
const TIMER_LEAVE: usize = 3;
/// How long an alert stays, how long the cursor must rest before the island opens, and how
/// long after it left before the island closes. Milliseconds.
const ALERT_MS: u32 = 2400;
const DWELL_MS: u32 = 110;
const LEAVE_MS: u32 = 320;

// The open panel, in logical pixels.
const OPEN_W: f32 = 372.0;
/// The tallest a page gets (the window is made this tall).
const OPEN_H_MOST: f32 = 430.0;
const OPEN_RADIUS: f32 = 24.0;
/// Most rows a list page shows.
pub const MOST_ROWS: usize = 11;

/// What the island's window reports to the app.
#[derive(Clone, Copy, Debug)]
pub enum Input {
    Move(i32, i32),
    Leave,
    Down(i32, i32),
    Up(i32, i32),
    /// Mouse wheel; positive = away from the user.
    Wheel(i32),
    /// One of the island's own timers ran out.
    Timer(usize),
    /// A typed character (UTF-16 unit), while a page with a text field has the keyboard.
    Char(u16),
    /// Another window took the keyboard away.
    Lost,
}

/// Something worth a moment's notice.
#[derive(Clone, Debug, PartialEq)]
pub enum Alert {
    Volume { level: f32, muted: bool },
    Power { percent: u8, plugged: bool },
    LowBattery(u8),
    Network(bool),
    Track { title: String, artist: String },
}

/// The pages of the open panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Home,
    Wifi,
    Sound,
    Bluetooth,
    Battery,
    Calendar,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowKind {
    /// Can be clicked.
    Item,
    /// A dimmed line that only says something.
    Note,
    /// A field to type into (a password); the row's text is shown while it is empty.
    Field,
    /// A switch, on or off. Can be clicked.
    Switch(bool),
}

/// One row of a card's page.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: usize,
    pub kind: RowKind,
    /// A symbol at the left (0 = none).
    pub glyph: u32,
    pub text: String,
    /// Dimmed text at the right.
    pub hint: String,
    /// A symbol at the far right (0 = none): a tick, a lock.
    pub mark: u32,
}

impl Row {
    pub fn item(id: usize, glyph: u32, text: &str) -> Self {
        Row { id, kind: RowKind::Item, glyph, text: text.to_string(), hint: String::new(), mark: 0 }
    }

    pub fn note(text: &str) -> Self {
        Row { id: 0, kind: RowKind::Note, glyph: 0, text: text.to_string(), hint: String::new(), mark: 0 }
    }

    pub fn field(id: usize, placeholder: &str) -> Self {
        Row { id, kind: RowKind::Field, glyph: 0, text: placeholder.to_string(), hint: String::new(), mark: 0 }
    }

    pub fn switch(id: usize, glyph: u32, text: &str, on: bool) -> Self {
        Row { id, kind: RowKind::Switch(on), glyph, text: text.to_string(), hint: String::new(), mark: 0 }
    }

    pub fn hint(mut self, hint: &str) -> Self {
        self.hint = hint.to_string();
        self
    }

    pub fn mark(mut self, symbol: u32) -> Self {
        self.mark = symbol;
        self
    }
}

/// What a click on the island asks the app to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    None,
    Previous,
    PlayPause,
    Next,
    Volume(f32),
    Mute,
    Settings,
    Lock,
    Sleep,
    /// The power button: the app shows the menu (restart, shut down...).
    Power,
    /// A card was clicked: the app fills that page and shows it.
    Open(Page),
    /// A row of a page was clicked.
    Row(Page, usize),
    /// Enter was pressed in a page's text field (what was typed is in `Island::typed`).
    Enter(Page),
}

/// What the island shows; read by the app, handed over every frame.
pub struct Info<'a> {
    pub time: &'a str,
    pub day: &'a str,
    pub date: &'a str,
    /// Year, month (1..=12), day.
    pub today: (i32, u32, u32),
    pub status: Status,
    pub media: Option<&'a NowPlaying>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Compact,
    Alert,
    Open,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    Previous,
    PlayPause,
    Next,
    Mute,
    Slider,
    Settings,
    Lock,
    Sleep,
    Power,
    Date,
    Status,
    Card(Page),
    /// Back to the first page.
    Back,
    /// A row of a list page (its index).
    Row(usize),
    /// The eye in a text field: show what was typed.
    Eye,
    MonthBefore,
    MonthAfter,
}

type Zones = Vec<(Zone, f32, f32, f32, f32)>;

#[derive(Clone)]
struct Fonts {
    text: IDWriteTextFormat,
    bold: IDWriteTextFormat,
    small: IDWriteTextFormat,
    big: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    icons_big: IDWriteTextFormat,
}

/// A button of the first page: a rounded back, a symbol and a word.
struct Button {
    zone: Zone,
    code: u32,
    text: &'static str,
    back: Pill,
    icon: Line,
    label: Line,
}

/// The visuals of one row of a list page.
struct RowLook {
    back: Pill,
    knob: Pill,
    icon: Line,
    text: Line,
    hint: Line,
    mark: Line,
}

pub struct Island {
    hwnd: HWND,
    stage: Stage,
    /// The window under this one that shows the glass's shadow.
    shade: Option<Shade>,
    glass: Glass,
    appbar: AppBar,

    compact: Layer,
    bars: Vec<Pill>,
    c_time: Line,
    c_day: Line,
    c_net: Line,
    c_mute: Line,
    c_batt: Line,
    c_batt_text: Line,

    alert: Layer,
    a_track: Pill,
    a_fill: Pill,
    a_icon: Line,
    a_text: Line,
    a_value: Line,

    // The first page.
    home: Layer,
    p_tile: Pill,
    p_track: Pill,
    p_fill: Pill,
    cards: Vec<Button>,
    buttons: Vec<Button>,
    /// The drop of light under the cursor, one for each kind of page.
    hot: [Bead; 3],
    hot_at: Glide,
    p_knob: Pill,
    p_time: Line,
    p_date: Line,
    p_net: Line,
    p_batt: Line,
    p_batt_text: Line,
    p_note: Line,
    p_title: Line,
    p_artist: Line,
    p_prev: Line,
    p_play: Line,
    p_next: Line,
    p_vol: Line,
    p_vol_text: Line,

    // A card's page: a title and a list of rows.
    list: Layer,
    list_content: VisualCollection,
    l_back: Line,
    l_title: Line,
    looks: Vec<RowLook>,

    // The month.
    month: Layer,
    m_back: Line,
    m_title: Line,
    m_before: Line,
    m_after: Line,
    m_today: Pill,
    m_heads: Vec<Line>,
    m_days: Vec<Line>,
    /// The month on show (year 0: not chosen yet, start at this month).
    shown_month: (i32, u32),

    fonts: Option<Fonts>,
    mode: Mode,
    page: Page,
    title: String,
    rows: Vec<Row>,
    /// What has been typed into the page's text field, and whether it is shown as typed.
    typed: String,
    reveal: bool,
    /// The island has taken the keyboard (a page with a text field is open).
    focused: bool,
    w: Spring,
    h: Spring,
    r: Spring,
    /// How much of each content is showing: compact, alert, first page, list page, month.
    show: [Spring; 5],
    current: Option<Alert>,
    alerting: bool,
    /// The alert's bar (the volume), 0 to 1.
    level: Spring,
    hovering: bool,
    dragging: bool,
    pressed: Option<Zone>,
    cursor: Option<(f32, f32)>,
    /// Clickable parts of the page on show, in window pixels.
    zones: Zones,
    /// Ends of the volume slider's track, in window pixels.
    slider: (f32, f32),
    volume: (f32, bool),
    hover_opens: bool,

    scale: f32,
    /// Gap between the screen's top edge and the glass.
    top: f32,
    win: (f32, f32),
    /// Size of the alert as last laid out (it keeps its shape while it fades away).
    alert_size: (f32, f32),
    region: Option<(i32, i32, i32, i32)>,
    /// What `place` was last called with, so repeating it does nothing.
    placed: Option<(RECT, i32, bool, i32)>,
}

thread_local! {
    /// The island window whose mouse-leave is being tracked (0: none).
    static TRACKING: Cell<isize> = const { Cell::new(0) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let me = hwnd.0 as isize;
    let send = |input: Input| app::post(Event::Island(me, input));
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
            send(Input::Down(x, y));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            send(Input::Up(x, y));
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            send(Input::Wheel(((wparam.0 >> 16) & 0xffff) as i16 as i32));
            return LRESULT(0);
        }
        WM_CHAR => {
            send(Input::Char(wparam.0 as u16));
            return LRESULT(0);
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xffff) == 0 {
                send(Input::Lost);
            }
            return LRESULT(0);
        }
        WM_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), wparam.0);
            }
            send(Input::Timer(wparam.0));
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The battery symbol for a charge level: eleven of them, empty to full, in two sets.
pub fn battery_glyph(percent: u8, charging: bool) -> u32 {
    let step = (percent as u32 + 5) / 10;
    match (charging, step) {
        (false, 10) => 0xE83F,
        (false, n) => 0xE850 + n,
        (true, 10) => 0xEA93,
        (true, n) => 0xE85A + n,
    }
}

fn volume_glyph(level: f32, muted: bool) -> u32 {
    if muted {
        0xE74F
    } else {
        0xE992 + ((level * 3.0).ceil() as u32).min(3)
    }
}

impl Island {
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
                PCWSTR(wide("Bến - Đảo").as_ptr()),
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
            let mut glass = scene.glass_in(&stage.content()?, &theme.island)?;
            // The island's window is cut down to the glass, so its shadow needs a window of its own.
            let shade = match Shade::new(scene).and_then(|shade| glass.cast_shadow_on(&shade.content()?).map(|_| shade)) {
                Ok(shade) => Some(shade),
                Err(e) => {
                    crate::log!("no window for the island's shadow ({e}); the island goes without one");
                    glass.without_shadow();
                    None
                }
            };
            // Everything lives inside the glass's shape, so the shape cuts it as it shrinks.
            let inside = glass.inner()?;
            let line = |parent: &VisualCollection| Line::new_in(scene, parent);

            let compact = scene.layer_in(&inside)?;
            let c = compact.content()?;
            let bars = (0..3).map(|_| scene.pill_in(&c)).collect::<Result<Vec<_>>>()?;
            let (c_time, c_day, c_net, c_mute, c_batt, c_batt_text) = (line(&c)?, line(&c)?, line(&c)?, line(&c)?, line(&c)?, line(&c)?);

            let alert = scene.layer_in(&inside)?;
            let a = alert.content()?;
            let (a_track, a_fill) = (scene.pill_in(&a)?, scene.pill_in(&a)?);
            let (a_icon, a_text, a_value) = (line(&a)?, line(&a)?, line(&a)?);

            let home = scene.layer_in(&inside)?;
            let p = home.content()?;
            // Fills first, so they lie under the drop of light and the text.
            let (p_tile, p_track, p_fill) = (scene.pill_in(&p)?, scene.pill_in(&p)?, scene.pill_in(&p)?);
            let backs = (0..8).map(|_| scene.pill_in(&p)).collect::<Result<Vec<_>>>()?;
            let hot_home = scene.bead_in(&p)?;
            let p_knob = scene.pill_in(&p)?;
            let specs = [
                (Zone::Card(Page::Wifi), 0xE701, "Wi-Fi"),
                (Zone::Card(Page::Bluetooth), 0xE702, "Bluetooth"),
                (Zone::Card(Page::Sound), 0xE767, "Âm thanh"),
                (Zone::Card(Page::Battery), 0xE83F, "Pin"),
                (Zone::Settings, 0xE713, "Cài đặt"),
                (Zone::Lock, 0xE72E, "Khoá"),
                (Zone::Sleep, 0xE708, "Ngủ"),
                (Zone::Power, 0xE7E8, "Nguồn"),
            ];
            let mut made = Vec::new();
            for (back, (zone, code, text)) in backs.into_iter().zip(specs) {
                made.push(Button { zone, code, text, back, icon: line(&p)?, label: line(&p)? });
            }
            let buttons = made.split_off(4);
            let cards = made;

            let list = scene.layer_in(&inside)?;
            let l = list.content()?;
            let hot_list = scene.bead_in(&l)?;
            let (l_back, l_title) = (line(&l)?, line(&l)?);

            let month = scene.layer_in(&inside)?;
            let m = month.content()?;
            let m_today = scene.pill_in(&m)?;
            let hot_month = scene.bead_in(&m)?;

            Ok(Island {
                hwnd,
                appbar: AppBar::new(hwnd),
                bars,
                c_time,
                c_day,
                c_net,
                c_mute,
                c_batt,
                c_batt_text,
                compact,
                a_track,
                a_fill,
                a_icon,
                a_text,
                a_value,
                alert,
                p_tile,
                p_track,
                p_fill,
                cards,
                buttons,
                hot: [hot_home, hot_list, hot_month],
                hot_at: Glide::new(motion),
                p_knob,
                p_time: line(&p)?,
                p_date: line(&p)?,
                p_net: line(&p)?,
                p_batt: line(&p)?,
                p_batt_text: line(&p)?,
                p_note: line(&p)?,
                p_title: line(&p)?,
                p_artist: line(&p)?,
                p_prev: line(&p)?,
                p_play: line(&p)?,
                p_next: line(&p)?,
                p_vol: line(&p)?,
                p_vol_text: line(&p)?,
                home,
                l_back,
                l_title,
                looks: Vec::new(),
                list_content: l,
                list,
                m_back: line(&m)?,
                m_title: line(&m)?,
                m_before: line(&m)?,
                m_after: line(&m)?,
                m_today,
                m_heads: (0..7).map(|_| line(&m)).collect::<Result<Vec<_>>>()?,
                m_days: (0..31).map(|_| line(&m)).collect::<Result<Vec<_>>>()?,
                shown_month: (0, 0),
                month,
                glass,
                stage,
                shade,
                fonts: None,
                mode: Mode::Compact,
                page: Page::Home,
                title: String::new(),
                rows: Vec::new(),
                typed: String::new(),
                reveal: false,
                focused: false,
                w: Spring::with(40.0, motion.medium()),
                h: Spring::with(8.0, motion.medium()),
                r: Spring::with(4.0, motion.medium()),
                show: [Spring::with(0.0, motion.fade()); 5],
                current: None,
                alerting: false,
                level: Spring::with(0.0, motion.small()),
                hovering: false,
                dragging: false,
                pressed: None,
                cursor: None,
                zones: Vec::new(),
                slider: (0.0, 1.0),
                volume: (0.0, false),
                hover_opens: true,
                scale: 1.0,
                top: 6.0,
                win: (1.0, 1.0),
                alert_size: (120.0, 36.0),
                region: None,
                placed: None,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Puts the island at the top centre of `monitor` and, with `reserve_space`, keeps
    /// maximised windows from sliding under it.
    pub fn place(&mut self, monitor: &Monitor, cfg: &Config, theme: &Theme) {
        let s = monitor.scale();
        let px = |v: f32| (v * s).round();
        let top = px(6.0);
        let margin = theme.island.shadow_reach + px(10.0);
        let (win_w, win_h) = (px(OPEN_W) + 2.0 * margin, top + px(OPEN_H_MOST) + margin);
        let strip = (top + px(cfg.island_height) + px(4.0)) as i32;
        // Asking the shell for the same strip again would only make it notify every other bar.
        let key = (monitor.rect, strip, cfg.reserve_space, win_w as i32);
        if self.placed == Some(key) {
            return;
        }
        self.placed = Some(key);
        let mut y = monitor.rect.top;
        if cfg.reserve_space {
            self.appbar.register(WM_ISLAND_APPBAR);
            if let Some(granted) = self.appbar.reserve_top(monitor.rect, strip) {
                y = granted.top;
            }
        } else {
            self.appbar.remove();
        }
        let x = monitor.rect.left + ((monitor.width() as f32 - win_w) * 0.5).round() as i32;
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, win_w as i32, win_h as i32, SWP_NOACTIVATE);
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        if let Some(shade) = self.shade.as_mut() {
            shade.place(self.hwnd, x, y, win_w as i32, win_h as i32);
        }
        if self.scale != s {
            // Text is painted for one size.
            self.forget();
        }
        (self.scale, self.top, self.win) = (s, top, (win_w, win_h));
        self.region = None;
    }

    pub fn set_theme(&mut self, theme: &Theme) {
        self.glass.set_material(&theme.island);
        self.forget();
    }

    /// Forgets painted text and fonts, so they are made again (after a theme, size or GPU change).
    pub fn forget(&mut self) {
        self.fonts = None;
        let lines = [
            &mut self.c_time,
            &mut self.c_day,
            &mut self.c_net,
            &mut self.c_mute,
            &mut self.c_batt,
            &mut self.c_batt_text,
            &mut self.a_icon,
            &mut self.a_text,
            &mut self.a_value,
            &mut self.p_time,
            &mut self.p_date,
            &mut self.p_net,
            &mut self.p_batt,
            &mut self.p_batt_text,
            &mut self.p_note,
            &mut self.p_title,
            &mut self.p_artist,
            &mut self.p_prev,
            &mut self.p_play,
            &mut self.p_next,
            &mut self.p_vol,
            &mut self.p_vol_text,
            &mut self.l_back,
            &mut self.l_title,
            &mut self.m_back,
            &mut self.m_title,
            &mut self.m_before,
            &mut self.m_after,
        ];
        for line in lines.into_iter().chain(self.m_heads.iter_mut()).chain(self.m_days.iter_mut()) {
            line.forget();
        }
        for button in self.buttons.iter_mut().chain(self.cards.iter_mut()) {
            button.icon.forget();
            button.label.forget();
        }
        // Rows are made again from scratch.
        self.looks.clear();
    }

    fn timer(&self, id: usize, ms: u32) {
        unsafe {
            SetTimer(Some(self.hwnd), id, ms, None);
        }
    }

    fn cancel(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id);
        }
    }

    /// Takes the keyboard (for a page with a text field) or gives it up again. The island
    /// normally never takes focus, so that using it does not disturb the app in front.
    fn focus(&mut self, on: bool) {
        if self.focused == on {
            return;
        }
        self.focused = on;
        unsafe {
            let style = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let quiet = WS_EX_NOACTIVATE.0 as isize;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, if on { style & !quiet } else { style | quiet });
        }
        if on {
            crate::shell::activate(self.hwnd);
        }
    }

    /// Makes the island swell to tell about something for a moment.
    pub fn alert(&mut self, alert: Alert) {
        if let Alert::Volume { level, muted } = &alert {
            self.level.set(if *muted { 0.0 } else { *level });
            // The first time the bar appears it starts at its value instead of filling up from empty.
            if !matches!(self.current, Some(Alert::Volume { .. })) || !self.alerting {
                self.level.snap(self.level.target);
            }
        }
        self.current = Some(alert);
        self.alerting = true;
        if self.mode != Mode::Open {
            self.mode = Mode::Alert;
        }
        self.timer(TIMER_ALERT, ALERT_MS);
    }

    /// Closes the open panel (something else took the stage).
    pub fn collapse(&mut self) {
        if self.mode == Mode::Open {
            self.mode = if self.alerting { Mode::Alert } else { Mode::Compact };
        }
        self.page = Page::Home;
        self.pressed = None;
        self.typed.clear();
        self.focus(false);
    }

    pub fn is_open(&self) -> bool {
        self.mode == Mode::Open
    }

    /// The page on show, if the panel is open.
    pub fn page(&self) -> Option<Page> {
        (self.mode == Mode::Open).then_some(self.page)
    }

    /// Opens the panel on its first page (for the tour).
    pub fn open(&mut self) {
        self.mode = Mode::Open;
        self.page = Page::Home;
    }

    /// Shows a card's page with these rows (opening the panel if need be). Called again with
    /// new rows whenever what the page says has changed.
    pub fn show_page(&mut self, page: Page, title: &str, rows: Vec<Row>) {
        if self.page != page {
            self.typed.clear();
            self.reveal = false;
            self.pressed = None;
        }
        self.page = page;
        self.mode = Mode::Open;
        self.title = title.to_string();
        self.rows = rows;
        self.rows.truncate(MOST_ROWS);
        let field = self.rows.iter().any(|row| row.kind == RowKind::Field);
        if !field {
            self.typed.clear();
        }
        self.focus(field);
    }

    /// What has been typed into the page's text field.
    pub fn typed(&self) -> &str {
        &self.typed
    }

    fn zone_at(&self, x: f32, y: f32) -> Option<Zone> {
        self.zones.iter().find(|(_, l, t, r, b)| x >= *l && x < *r && y >= *t && y < *b).map(|(zone, ..)| *zone)
    }

    fn slider_value(&self, x: f32) -> f32 {
        let (a, b) = self.slider;
        ((x - a) / (b - a).max(1.0)).clamp(0.0, 1.0)
    }

    fn go_home(&mut self) {
        self.page = Page::Home;
        self.typed.clear();
        self.focus(false);
    }

    /// Reacts to the mouse, the keyboard and the island's timers.
    pub fn handle(&mut self, input: Input) -> Action {
        match input {
            Input::Move(x, y) => {
                self.cursor = Some((x as f32, y as f32));
                if !self.hovering {
                    self.hovering = true;
                    self.cancel(TIMER_LEAVE);
                    if self.mode != Mode::Open && self.hover_opens {
                        self.timer(TIMER_DWELL, DWELL_MS);
                    }
                }
                if self.dragging {
                    return Action::Volume(self.slider_value(x as f32));
                }
            }
            Input::Leave => {
                self.hovering = false;
                self.cursor = None;
                self.cancel(TIMER_DWELL);
                // While something is being typed the panel stays, wherever the cursor goes.
                if self.mode == Mode::Open && !self.dragging && !self.focused {
                    self.timer(TIMER_LEAVE, LEAVE_MS);
                }
            }
            Input::Down(x, y) => {
                if self.mode == Mode::Open {
                    self.pressed = self.zone_at(x as f32, y as f32);
                    if self.pressed == Some(Zone::Slider) {
                        // Keep receiving the mouse while the knob is dragged past the panel's edge.
                        self.dragging = true;
                        unsafe {
                            SetCapture(self.hwnd);
                        }
                        return Action::Volume(self.slider_value(x as f32));
                    }
                }
            }
            Input::Up(x, y) => {
                let pressed = self.pressed.take();
                if self.dragging {
                    self.dragging = false;
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    let (fx, fy) = (x as f32, y as f32);
                    if fx < 0.0 || fy < 0.0 || fx >= self.win.0 || fy >= self.win.1 {
                        // Let go outside the island: no "left" report will follow.
                        self.hovering = false;
                        self.cursor = None;
                        self.timer(TIMER_LEAVE, LEAVE_MS);
                    }
                    return Action::Volume(self.slider_value(fx));
                }
                if self.mode != Mode::Open {
                    self.cancel(TIMER_DWELL);
                    self.mode = Mode::Open;
                    self.page = Page::Home;
                    return Action::None;
                }
                let page = self.page;
                return match self.zone_at(x as f32, y as f32).filter(|zone| Some(*zone) == pressed) {
                    Some(Zone::Previous) => Action::Previous,
                    Some(Zone::PlayPause) => Action::PlayPause,
                    Some(Zone::Next) => Action::Next,
                    Some(Zone::Mute) => Action::Mute,
                    Some(Zone::Settings) => Action::Settings,
                    Some(Zone::Lock) => Action::Lock,
                    Some(Zone::Sleep) => Action::Sleep,
                    Some(Zone::Power) => Action::Power,
                    Some(Zone::Date) => {
                        // The month is the island's own: nothing to ask the app for.
                        self.page = Page::Calendar;
                        self.shown_month = (0, 0);
                        Action::None
                    }
                    Some(Zone::Status) => Action::Open(Page::Wifi),
                    Some(Zone::Card(card)) => Action::Open(card),
                    Some(Zone::Back) => {
                        self.go_home();
                        Action::None
                    }
                    Some(Zone::Row(index)) => match self.rows.get(index) {
                        Some(row) if matches!(row.kind, RowKind::Item | RowKind::Switch(_)) => Action::Row(page, row.id),
                        _ => Action::None,
                    },
                    Some(Zone::Eye) => {
                        self.reveal = !self.reveal;
                        Action::None
                    }
                    Some(Zone::MonthBefore) => {
                        self.shown_month = dates::shift_month(self.shown_month.0, self.shown_month.1, -1);
                        Action::None
                    }
                    Some(Zone::MonthAfter) => {
                        self.shown_month = dates::shift_month(self.shown_month.0, self.shown_month.1, 1);
                        Action::None
                    }
                    Some(Zone::Slider) | None => Action::None,
                };
            }
            Input::Wheel(delta) => {
                // Rolling the wheel over the island turns the volume.
                let step = 0.02 * (delta as f32 / 120.0);
                return Action::Volume((self.volume.0 + step).clamp(0.0, 1.0));
            }
            Input::Char(unit) => {
                if self.mode == Mode::Open && self.page != Page::Home && self.rows.iter().any(|row| row.kind == RowKind::Field) {
                    match unit {
                        8 => {
                            self.typed.pop();
                        }
                        13 => {
                            if !self.typed.is_empty() {
                                return Action::Enter(self.page);
                            }
                        }
                        27 => self.go_home(),
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
            Input::Lost => {
                // The keyboard went elsewhere (a click outside): the panel goes, like a menu.
                if self.focused {
                    self.collapse();
                }
            }
            Input::Timer(TIMER_ALERT) => {
                self.alerting = false;
                if self.mode == Mode::Alert {
                    self.mode = Mode::Compact;
                }
            }
            Input::Timer(TIMER_DWELL) => {
                if self.hovering && self.mode != Mode::Open {
                    self.mode = Mode::Open;
                    self.page = Page::Home;
                }
            }
            Input::Timer(TIMER_LEAVE) => {
                if !self.hovering && !self.dragging && !self.focused && self.mode == Mode::Open {
                    self.collapse();
                }
            }
            Input::Timer(_) => {}
        }
        Action::None
    }

    /// Where the power button is on the screen (its menu hangs from it): centre, bottom.
    pub fn power_anchor(&self) -> Option<(i32, i32)> {
        let mut rect = RECT::default();
        unsafe {
            let _ = GetWindowRect(self.hwnd, &mut rect);
        }
        self.zones.iter().find(|(zone, ..)| *zone == Zone::Power).map(|(_, l, _, r, b)| (rect.left + ((l + r) * 0.5) as i32, rect.top + *b as i32))
    }

    /// Brings the contents up to date, advances the shape and lays everything out.
    /// `hide`: 0 = shown, 1 = slid up out of view (for full-screen apps).
    /// Returns true while something is still moving.
    pub fn update(&mut self, scene: &Scene, gfx: &Gfx, cfg: &Config, theme: &Theme, motion: &Motion, info: &Info, hide: &Spring, dt: f32) -> Result<bool> {
        let s = self.scale;
        let px = |v: f32| (v * s).round();
        self.hover_opens = cfg.island_hover;
        self.volume = info.status.volume.unwrap_or((0.0, true));

        if self.fonts.is_none() {
            self.fonts = Some(Fonts {
                text: text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                bold: text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                small: text::format(gfx, UI_FONT, px(12.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                big: text::format(gfx, DISPLAY_FONT, px(25.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                icons: text::format(gfx, ICON_FONT, px(14.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                icons_big: text::format(gfx, ICON_FONT, px(17.0), DWRITE_FONT_WEIGHT_NORMAL)?,
            });
        }
        let f = self.fonts.clone().unwrap();
        let (fg, dim) = (theme.text, theme.text_dim);
        let media = info.media.filter(|_| cfg.island_media);
        let (level, muted) = self.volume;
        let battery = info.status.battery;
        let mid = |line: &Line, height: f32| ((height - line.h) * 0.5).round();

        // ------------------------------------------------------------ compact
        let h0 = px(cfg.island_height);
        self.c_time.set(scene, gfx, &f.bold, info.time, fg, None)?;
        self.c_day.set(scene, gfx, &f.text, info.day, dim, None)?;
        self.c_net.set(scene, gfx, &f.icons, &glyph(0xE701), if info.status.online { fg } else { dim }, None)?;
        self.c_mute.set(scene, gfx, &f.icons, &if muted && info.status.volume.is_some() { glyph(0xE74F) } else { String::new() }, dim, None)?;
        match battery {
            Some((percent, charging)) => {
                self.c_batt.set(scene, gfx, &f.icons, &glyph(battery_glyph(percent, charging)), fg, None)?;
                self.c_batt_text.set(scene, gfx, &f.small, &format!("{percent}%"), fg, None)?;
            }
            None => {
                self.c_batt.set(scene, gfx, &f.icons, "", fg, None)?;
                self.c_batt_text.set(scene, gfx, &f.small, "", fg, None)?;
            }
        }
        let mut x = px(14.0);
        // Three bars that dance while something plays. The compositor moves them; the app sleeps.
        let (bar_w, bar_h, bar_gap) = (px(3.0), px(13.0), px(2.0));
        let playing = media.map(|m| m.playing).unwrap_or(false);
        for (i, bar) in self.bars.iter_mut().enumerate() {
            bar.place(x + i as f32 * (bar_w + bar_gap), ((h0 - bar_h) * 0.5).round(), bar_w, bar_h, bar_w * 0.5)?;
            bar.set_color(theme.accent_color(1.0))?;
            bar.set_opacity(if media.is_some() { 1.0 } else { 0.0 })?;
            if playing && !motion.reduced() {
                bar.pulse(0.3, 0.78 + 0.17 * i as f32, i as f32 * 0.31)?;
            } else {
                bar.rest()?;
            }
        }
        if media.is_some() {
            x += 3.0 * bar_w + 2.0 * bar_gap + px(9.0);
        }
        let y = mid(&self.c_time, h0);
        self.c_time.place(x, y)?;
        x += self.c_time.w + px(8.0);
        let y = mid(&self.c_day, h0);
        self.c_day.place(x, y)?;
        x += self.c_day.w + px(13.0);
        let y = mid(&self.c_net, h0);
        self.c_net.place(x, y)?;
        x += self.c_net.w;
        if self.c_mute.w > 0.0 {
            x += px(8.0);
            let y = mid(&self.c_mute, h0);
            self.c_mute.place(x, y)?;
            x += self.c_mute.w;
        }
        if self.c_batt.w > 0.0 {
            x += px(9.0);
            let y = mid(&self.c_batt, h0);
            self.c_batt.place(x, y)?;
            x += self.c_batt.w + px(4.0);
            let y = mid(&self.c_batt_text, h0);
            self.c_batt_text.place(x, y)?;
            x += self.c_batt_text.w;
        }
        let compact_size = (x + px(14.0), h0);

        // -------------------------------------------------------------- alert
        let ah = h0 + px(6.0);
        if let Some(alert) = &self.current {
            let (code, words, value, bar) = match alert {
                Alert::Volume { level, muted } => (volume_glyph(*level, *muted), String::new(), if *muted { "Tắt tiếng".to_string() } else { format!("{}%", (level * 100.0).round() as i32) }, true),
                Alert::Power { percent, plugged } => (battery_glyph(*percent, *plugged), (if *plugged { "Đang sạc" } else { "Đang dùng pin" }).to_string(), format!("{percent}%"), false),
                Alert::LowBattery(percent) => (0xE850, "Pin yếu".to_string(), format!("{percent}%"), false),
                Alert::Network(online) => (0xE701, (if *online { "Đã có mạng" } else { "Mất kết nối mạng" }).to_string(), String::new(), false),
                Alert::Track { title, artist } => (0xEC4F, title.clone(), artist.clone(), false),
            };
            let warn = matches!(alert, Alert::LowBattery(_)) || matches!(alert, Alert::Network(false));
            self.a_icon.set(scene, gfx, &f.icons_big, &glyph(code), if warn { theme.text_dim } else { theme.accent_text() }, None)?;
            self.a_text.set(scene, gfx, &f.bold, &words, fg, Some(px(210.0)))?;
            self.a_value.set(scene, gfx, &f.text, &value, dim, Some(px(150.0)))?;
            let mut x = px(16.0);
            let y = mid(&self.a_icon, ah);
            self.a_icon.place(x, y)?;
            x += self.a_icon.w + px(10.0);
            if self.a_text.w > 0.0 {
                let y = mid(&self.a_text, ah);
                self.a_text.place(x, y)?;
                x += self.a_text.w + px(10.0);
            }
            if bar {
                let (bw, bh) = (px(116.0), px(5.0));
                let by = ((ah - bh) * 0.5).round();
                self.a_track.place(x, by, bw, bh, bh * 0.5)?;
                self.a_track.set_color(theme.track)?;
                self.a_track.set_opacity(1.0)?;
                self.level.tune(motion.small());
                self.level.step(dt);
                self.a_fill.place(x, by, (bw * self.level.pos.clamp(0.0, 1.0)).max(bh), bh, bh * 0.5)?;
                self.a_fill.set_color(theme.accent_color(1.0))?;
                self.a_fill.set_opacity(1.0)?;
                x += bw + px(10.0);
            } else {
                self.a_track.set_opacity(0.0)?;
                self.a_fill.set_opacity(0.0)?;
            }
            if self.a_value.w > 0.0 {
                let y = mid(&self.a_value, ah);
                self.a_value.place(x, y)?;
                x += self.a_value.w + px(2.0);
            } else {
                x -= px(10.0);
            }
            self.alert_size = (x + px(16.0), ah);
        }
        let alert_size = self.alert_size;

        // ------------------------------------------------------ the open panel
        let (ow, pad) = (px(OPEN_W), px(16.0));
        let x0 = ((self.win.0 - ow) * 0.5).round();
        let y0 = self.top;
        let open = self.mode == Mode::Open;
        let head = px(44.0);
        let chevron = glyph(0xE76B);

        // ---- first page. Row 1: the time and date on the left, the status lights on the right.
        let mut home_zones: Zones = Vec::new();
        let (row_y, row_h) = (px(12.0), px(40.0));
        self.p_time.set(scene, gfx, &f.big, info.time, fg, None)?;
        self.p_date.set(scene, gfx, &f.text, info.date, dim, Some(px(170.0)))?;
        let y = row_y + mid(&self.p_time, row_h);
        self.p_time.place(pad, y)?;
        let date_x = pad + self.p_time.w + px(10.0);
        let y = row_y + mid(&self.p_date, row_h) + px(2.0);
        self.p_date.place(date_x, y)?;
        home_zones.push((Zone::Date, pad - px(6.0), row_y + px(4.0), date_x + self.p_date.w + px(6.0), row_y + row_h - px(4.0)));
        let mut right = ow - pad;
        match battery {
            Some((percent, charging)) => {
                self.p_batt_text.set(scene, gfx, &f.text, &format!("{percent}%"), fg, None)?;
                self.p_batt.set(scene, gfx, &f.icons_big, &glyph(battery_glyph(percent, charging)), fg, None)?;
                right -= self.p_batt_text.w;
                let y = row_y + mid(&self.p_batt_text, row_h);
                self.p_batt_text.place(right, y)?;
                right -= self.p_batt.w + px(5.0);
                let y = row_y + mid(&self.p_batt, row_h);
                self.p_batt.place(right, y)?;
                right -= px(12.0);
            }
            None => {
                self.p_batt_text.set(scene, gfx, &f.text, "", fg, None)?;
                self.p_batt.set(scene, gfx, &f.icons_big, "", fg, None)?;
            }
        }
        self.p_net.set(scene, gfx, &f.icons_big, &glyph(0xE701), if info.status.online { fg } else { dim }, None)?;
        right -= self.p_net.w;
        let y = row_y + mid(&self.p_net, row_h);
        self.p_net.place(right, y)?;
        home_zones.push((Zone::Status, right - px(8.0), row_y + px(4.0), ow - pad + px(8.0), row_y + row_h - px(4.0)));

        // Row 2: what is playing, with its three buttons.
        let mut row_y = px(60.0);
        let media_alpha = if media.is_some() { 1.0 } else { 0.0 };
        let knob = px(34.0);
        if let Some(track) = media {
            let tile = px(40.0);
            let row_h = px(48.0);
            self.p_tile.place(pad, row_y + ((row_h - tile) * 0.5).round(), tile, tile, px(11.0))?;
            self.p_tile.set_color(theme.track)?;
            self.p_note.set(scene, gfx, &f.icons_big, &glyph(0xEC4F), theme.accent_text(), None)?;
            let (nw, nh) = (self.p_note.w, self.p_note.h);
            self.p_note.place(pad + (tile - nw) * 0.5, row_y + (row_h - nh) * 0.5)?;
            let controls = ow - pad - 3.0 * knob;
            let room = controls - (pad + tile + px(12.0)) - px(10.0);
            self.p_title.set(scene, gfx, &f.bold, if track.title.is_empty() { "Đang phát" } else { &track.title }, fg, Some(room))?;
            self.p_artist.set(scene, gfx, &f.small, &track.artist, dim, Some(room))?;
            let text_x = pad + tile + px(12.0);
            if self.p_artist.w > 0.0 {
                self.p_title.place(text_x, row_y + px(6.0))?;
                self.p_artist.place(text_x, row_y + px(25.0))?;
            } else {
                let y = row_y + mid(&self.p_title, row_h);
                self.p_title.place(text_x, y)?;
            }
            self.p_prev.set(scene, gfx, &f.icons, &glyph(0xE892), fg, None)?;
            self.p_play.set(scene, gfx, &f.icons_big, &glyph(if track.playing { 0xE769 } else { 0xE768 }), fg, None)?;
            self.p_next.set(scene, gfx, &f.icons, &glyph(0xE893), fg, None)?;
            let top = row_y + ((row_h - knob) * 0.5).round();
            for (i, (line, zone)) in [(&mut self.p_prev, Zone::Previous), (&mut self.p_play, Zone::PlayPause), (&mut self.p_next, Zone::Next)].into_iter().enumerate() {
                let left = controls + i as f32 * knob;
                line.place(left + (knob - line.w) * 0.5, top + (knob - line.h) * 0.5)?;
                home_zones.push((zone, left, top, left + knob, top + knob));
            }
            row_y += px(56.0);
        }
        self.p_tile.set_opacity(media_alpha)?;
        for line in [&mut self.p_note, &mut self.p_title, &mut self.p_artist, &mut self.p_prev, &mut self.p_play, &mut self.p_next] {
            line.set_opacity(media_alpha)?;
        }

        // Row 3: the volume.
        let row_h = px(34.0);
        self.p_vol.set(scene, gfx, &f.icons_big, &glyph(volume_glyph(level, muted)), if muted { dim } else { fg }, None)?;
        let (vw, vh) = (self.p_vol.w, self.p_vol.h);
        self.p_vol.place(pad + (px(26.0) - vw) * 0.5, row_y + (row_h - vh) * 0.5)?;
        home_zones.push((Zone::Mute, pad - px(4.0), row_y, pad + px(30.0), row_y + row_h));
        self.p_vol_text.set(scene, gfx, &f.text, &format!("{}%", (level * 100.0).round() as i32), dim, None)?;
        let tw = self.p_vol_text.w;
        let y = row_y + mid(&self.p_vol_text, row_h);
        self.p_vol_text.place(ow - pad - tw, y)?;
        let (track_a, track_b) = (pad + px(40.0), ow - pad - px(44.0));
        let (thick, dot) = (px(6.0), px(16.0));
        let cy = row_y + row_h * 0.5;
        let at = track_a + (track_b - track_a) * if muted { 0.0 } else { level.clamp(0.0, 1.0) };
        self.p_track.place(track_a, (cy - thick * 0.5).round(), track_b - track_a, thick, thick * 0.5)?;
        self.p_track.set_color(theme.track)?;
        self.p_fill.place(track_a, (cy - thick * 0.5).round(), (at - track_a).max(thick), thick, thick * 0.5)?;
        self.p_fill.set_color(theme.accent_color(1.0))?;
        let grab = if self.dragging { px(2.0) } else { 0.0 };
        self.p_knob.place((at - dot * 0.5 - grab).round(), (cy - dot * 0.5 - grab).round(), dot + 2.0 * grab, dot + 2.0 * grab, dot * 0.5 + grab)?;
        self.p_knob.set_color(rgba(1.0, 1.0, 1.0, 1.0))?;
        self.slider = (x0 + track_a, x0 + track_b);
        home_zones.push((Zone::Slider, track_a - px(8.0), row_y, track_b + px(8.0), row_y + row_h));
        row_y += px(42.0);

        // Below that: the cards, two across (each opens a page of its own, and its name needs
        // the room), then the buttons, four across.
        let gap = px(8.0);
        let pressed = self.pressed;
        let online = info.status.online;
        for (group, across, row_h) in [(&mut self.cards, 2usize, px(40.0)), (&mut self.buttons, 4usize, px(36.0))] {
            let each = ((ow - 2.0 * pad - (across as f32 - 1.0) * gap) / across as f32).floor();
            let rows = group.len().div_ceil(across);
            for (i, button) in group.iter_mut().enumerate() {
                let left = pad + (i % across) as f32 * (each + gap);
                let top = row_y + (i / across) as f32 * (row_h + gap);
                let sink = if pressed == Some(button.zone) { px(1.5) } else { 0.0 };
                button.back.place(left + sink, top + sink, each - 2.0 * sink, row_h - 2.0 * sink, px(11.0))?;
                button.back.set_color(theme.track)?;
                let code = match (button.zone, battery) {
                    (Zone::Card(Page::Battery), Some((percent, charging))) => battery_glyph(percent, charging),
                    _ => button.code,
                };
                let tone = if button.zone == Zone::Card(Page::Wifi) && !online { dim } else { fg };
                button.icon.set(scene, gfx, &f.icons, &glyph(code), tone, None)?;
                // A card is read from the left, like the rows of the page it opens; a button
                // carries its symbol and word in the middle.
                let (inset, between) = if across == 2 { (px(14.0), px(10.0)) } else { (px(0.0), px(6.0)) };
                button.label.set(scene, gfx, &f.small, button.text, fg, Some(each - button.icon.w - between - 2.0 * inset.max(px(6.0))))?;
                let together = button.icon.w + between + button.label.w;
                let start = if across == 2 { left + inset } else { left + ((each - together) * 0.5).round() };
                let y = top + ((row_h - button.icon.h) * 0.5).round();
                button.icon.place(start, y)?;
                let y = top + ((row_h - button.label.h) * 0.5).round();
                button.label.place(start + button.icon.w + between, y)?;
                home_zones.push((button.zone, left, top, left + each, top + row_h));
            }
            row_y += rows as f32 * (row_h + gap);
        }
        let home_size = (ow, row_y - gap + px(14.0));

        // ---- a card's page: a way back, a title, the rows.
        self.l_back.set(scene, gfx, &f.icons, &chevron, fg, None)?;
        let y = mid(&self.l_back, head) + px(2.0);
        self.l_back.place(pad, y)?;
        self.l_title.set(scene, gfx, &f.bold, &self.title, fg, Some(ow - 2.0 * pad - px(40.0)))?;
        let y = mid(&self.l_title, head) + px(2.0);
        self.l_title.place(pad + px(26.0), y)?;
        while self.looks.len() < self.rows.len() {
            let c = &self.list_content;
            self.looks.push(RowLook { back: scene.pill_in(c)?, knob: scene.pill_in(c)?, icon: Line::new_in(scene, c)?, text: Line::new_in(scene, c)?, hint: Line::new_in(scene, c)?, mark: Line::new_in(scene, c)? });
        }
        let mut y = head + px(2.0);
        let mut list_zones: Zones = vec![(Zone::Back, pad - px(8.0), px(8.0), pad + px(24.0), head)];
        for (index, look) in self.looks.iter_mut().enumerate() {
            let Some(row) = self.rows.get(index) else {
                for line in [&mut look.icon, &mut look.text, &mut look.hint, &mut look.mark] {
                    line.set_opacity(0.0)?;
                }
                look.back.set_opacity(0.0)?;
                look.knob.set_opacity(0.0)?;
                continue;
            };
            let h = match row.kind {
                RowKind::Note => px(24.0),
                RowKind::Field => px(40.0),
                _ => px(34.0),
            };
            let text_x = if row.glyph != 0 { pad + px(28.0) } else { pad };
            look.back.set_opacity(0.0)?;
            look.knob.set_opacity(0.0)?;
            look.icon.set(scene, gfx, &f.icons, &if row.glyph != 0 { glyph(row.glyph) } else { String::new() }, fg, None)?;
            look.mark.set(scene, gfx, &f.icons, &if row.mark != 0 { glyph(row.mark) } else { String::new() }, theme.accent_text(), None)?;
            let mut edge = ow - pad;
            if look.mark.w > 0.0 {
                edge -= look.mark.w;
                let my = y + mid(&look.mark, h);
                look.mark.place(edge, my)?;
                edge -= px(8.0);
            }
            match row.kind {
                RowKind::Field => {
                    // A rounded field; dots (or the text itself) once something is typed.
                    look.back.place(pad, y + px(3.0), ow - 2.0 * pad, h - px(6.0), px(9.0))?;
                    look.back.set_color(theme.track)?;
                    look.back.set_opacity(1.0)?;
                    let shown = if self.typed.is_empty() {
                        row.text.clone()
                    } else if self.reveal {
                        self.typed.clone()
                    } else {
                        "\u{2022}".repeat(self.typed.chars().count())
                    };
                    look.text.set(scene, gfx, &f.text, &shown, if self.typed.is_empty() { dim } else { fg }, Some(ow - 2.0 * pad - px(52.0)))?;
                    let ty = y + mid(&look.text, h);
                    look.text.place(pad + px(12.0), ty)?;
                    // An eye at the right end: click it to see what was typed.
                    look.hint.set(scene, gfx, &f.icons, &glyph(0xE7B3), if self.reveal { fg } else { dim }, None)?;
                    let hy = y + mid(&look.hint, h);
                    look.hint.place(ow - pad - px(12.0) - look.hint.w, hy)?;
                    list_zones.push((Zone::Eye, ow - pad - px(40.0), y, ow - pad, y + h));
                }
                RowKind::Switch(on) => {
                    look.hint.set(scene, gfx, &f.small, "", dim, None)?;
                    let (sw, sh) = (px(34.0), px(18.0));
                    let sx = edge - sw;
                    let sy = y + ((h - sh) * 0.5).round();
                    look.back.place(sx, sy, sw, sh, sh * 0.5)?;
                    look.back.set_color(if on { theme.accent_color(1.0) } else { theme.track })?;
                    look.back.set_opacity(1.0)?;
                    let d = sh - px(6.0);
                    look.knob.place(if on { sx + sw - d - px(3.0) } else { sx + px(3.0) }, sy + px(3.0), d, d, d * 0.5)?;
                    look.knob.set_color(rgba(1.0, 1.0, 1.0, 1.0))?;
                    look.knob.set_opacity(1.0)?;
                    look.text.set(scene, gfx, &f.text, &row.text, fg, Some(sx - text_x - px(10.0)))?;
                    let ty = y + mid(&look.text, h);
                    look.text.place(text_x, ty)?;
                    list_zones.push((Zone::Row(index), pad - px(6.0), y, ow - pad + px(6.0), y + h));
                }
                RowKind::Item | RowKind::Note => {
                    look.hint.set(scene, gfx, &f.small, &row.hint, dim, Some(px(130.0)))?;
                    if look.hint.w > 0.0 {
                        edge -= look.hint.w;
                        let hy = y + mid(&look.hint, h);
                        look.hint.place(edge, hy)?;
                        edge -= px(10.0);
                    }
                    let note = row.kind == RowKind::Note;
                    look.text.set(scene, gfx, if note { &f.small } else { &f.text }, &row.text, if note { dim } else { fg }, Some((edge - text_x).max(px(40.0))))?;
                    let ty = y + mid(&look.text, h);
                    look.text.place(text_x, ty)?;
                    if !note {
                        list_zones.push((Zone::Row(index), pad - px(6.0), y, ow - pad + px(6.0), y + h));
                    }
                }
            }
            if look.icon.w > 0.0 {
                let iy = y + mid(&look.icon, h);
                look.icon.place(pad + (px(18.0) - look.icon.w) * 0.5, iy)?;
            }
            for line in [&mut look.icon, &mut look.text, &mut look.hint, &mut look.mark] {
                line.set_opacity(1.0)?;
            }
            y += h;
        }
        let list_size = (ow, (y + px(12.0)).min(px(OPEN_H_MOST)));

        // ---- the month: Monday first, today circled, arrows to leaf through.
        if self.shown_month.0 == 0 {
            self.shown_month = (info.today.0, info.today.1.clamp(1, 12));
        }
        let (year, month) = self.shown_month;
        self.m_back.set(scene, gfx, &f.icons, &chevron, fg, None)?;
        let y = mid(&self.m_back, head) + px(2.0);
        self.m_back.place(pad, y)?;
        self.m_title.set(scene, gfx, &f.bold, &format!("Tháng {month} năm {year}"), fg, None)?;
        let y = mid(&self.m_title, head) + px(2.0);
        self.m_title.place(pad + px(26.0), y)?;
        self.m_before.set(scene, gfx, &f.icons, &chevron, fg, None)?;
        self.m_after.set(scene, gfx, &f.icons, &glyph(0xE76C), fg, None)?;
        let arrow = px(30.0);
        let (after_x, before_x) = (ow - pad - arrow, ow - pad - 2.0 * arrow);
        let (bw, bh, aw, ah2) = (self.m_before.w, self.m_before.h, self.m_after.w, self.m_after.h);
        self.m_before.place(before_x + (arrow - bw) * 0.5, px(2.0) + (head - bh) * 0.5)?;
        self.m_after.place(after_x + (arrow - aw) * 0.5, px(2.0) + (head - ah2) * 0.5)?;
        let month_zones: Zones = vec![
            (Zone::Back, pad - px(8.0), px(8.0), pad + px(24.0), head),
            (Zone::MonthBefore, before_x, px(8.0), before_x + arrow, head),
            (Zone::MonthAfter, after_x, px(8.0), after_x + arrow, head),
        ];
        let (cell_w, cell_h) = (((ow - 2.0 * pad) / 7.0).floor(), px(30.0));
        let grid_x = ((ow - cell_w * 7.0) * 0.5).round();
        let mut y = head + px(4.0);
        for (index, line) in self.m_heads.iter_mut().enumerate() {
            line.set(scene, gfx, &f.small, dates::WEEKDAYS_SHORT[index], dim, None)?;
            line.place(grid_x + cell_w * index as f32 + (cell_w - line.w) * 0.5, y + (px(20.0) - line.h) * 0.5)?;
        }
        y += px(22.0);
        let count = dates::days_in_month(year, month);
        let this_month = (year, month) == (info.today.0, info.today.1);
        let mut today_cell = None;
        for (index, line) in self.m_days.iter_mut().enumerate() {
            let day = index as u32 + 1;
            if day > count {
                line.set(scene, gfx, &f.text, "", fg, None)?;
                continue;
            }
            let (col, row) = dates::cell(year, month, day);
            let is_today = this_month && day == info.today.2;
            line.set(scene, gfx, &f.text, &day.to_string(), if is_today { crate::gfx::color(1.0, 1.0, 1.0, 1.0) } else { fg }, None)?;
            let (cx, cy) = (grid_x + cell_w * col as f32, y + cell_h * row as f32);
            line.place(cx + (cell_w - line.w) * 0.5, cy + (cell_h - line.h) * 0.5)?;
            if is_today {
                today_cell = Some((cx, cy));
            }
        }
        match today_cell {
            Some((cx, cy)) => {
                let d = px(26.0);
                self.m_today.place((cx + (cell_w - d) * 0.5).round(), (cy + (cell_h - d) * 0.5).round(), d, d, d * 0.5)?;
                self.m_today.set_color(theme.accent_color(1.0))?;
                self.m_today.set_opacity(1.0)?;
            }
            None => self.m_today.set_opacity(0.0)?,
        }
        let month_size = (ow, y + cell_h * dates::week_rows(year, month) as f32 + px(12.0));

        // Only the page on show answers the mouse.
        let (which, open_size, page_zones) = match self.page {
            Page::Home => (0, home_size, home_zones),
            Page::Calendar => (2, month_size, month_zones),
            _ => (1, list_size, list_zones),
        };
        self.zones.clear();
        if open {
            self.zones.extend(page_zones.iter().map(|(zone, l, t, r, b)| (*zone, x0 + l, y0 + t, x0 + r, y0 + b)));
        }

        // The drop of light under the cursor, on the page that is showing.
        let hot = self.cursor.filter(|_| open && !self.dragging).and_then(|(cx, cy)| self.zones.iter().find(|(zone, l, t, r, b)| !matches!(zone, Zone::Slider | Zone::Eye) && cx >= *l && cx < *r && cy >= *t && cy < *b).copied());
        self.hot_at.tune(motion);
        self.hot_at.aim(hot.map(|(_, l, t, r, b)| (l - x0, t - y0, r - l, b - t)));
        self.hot_at.step(dt);
        let shape = self.hot_at.shape(motion.squash());
        let (bead_top, bead_bottom, bead_edge) = theme.bead;
        for (index, bead) in self.hot.iter_mut().enumerate() {
            if index == which {
                bead.place(shape.x, shape.y, shape.w, shape.h, px(11.0))?;
                bead.set_colors(bead_top, bead_bottom, bead_edge)?;
                bead.set_opacity(shape.alpha)?;
            } else {
                bead.set_opacity(0.0)?;
            }
        }

        // -------------------------------------------------------------- shape
        // Whole pixels, and centred on whole pixels, so the glass rests sharp.
        let win_w = self.win.0;
        let even = |v: f32| {
            let v = v.round();
            if ((win_w - v) as i32) % 2 != 0 {
                v + 1.0
            } else {
                v
            }
        };
        let (tw, th, tr) = match self.mode {
            Mode::Compact => (even(compact_size.0), compact_size.1, compact_size.1 * 0.5),
            Mode::Alert => (even(alert_size.0), alert_size.1, alert_size.1 * 0.5),
            Mode::Open => (even(open_size.0), open_size.1.round(), px(OPEN_RADIUS)),
        };
        // Growing, it overshoots like a drop; shrinking, it just settles.
        if tw != self.w.target || th != self.h.target {
            let growing = tw * th >= self.w.target * self.h.target;
            let tone = if growing { motion.medium() } else { motion.calm() };
            for spring in [&mut self.w, &mut self.h, &mut self.r] {
                spring.tune(tone);
            }
        }
        self.w.set(tw);
        self.h.set(th);
        self.r.set(tr);
        self.w.step(dt);
        self.h.step(dt);
        self.r.step(dt);
        let (w, h) = (self.w.pos.max(8.0), self.h.pos.max(6.0));
        let (gx, gy) = ((win_w - w) * 0.5, self.top);
        let slide = -(hide.pos.max(0.0) * (compact_size.1 + self.top + theme.island.shadow_reach + px(8.0))).round();
        self.stage.set_offset(0.0, slide)?;
        if let Some(shade) = self.shade.as_mut() {
            shade.set_offset(0.0, slide)?;
        }
        self.glass.set_rect(scene, gx, gy, w, h, self.r.pos.max(0.0))?;

        // Contents: each fades and swells in.
        let wanted = match (self.mode, which) {
            (Mode::Compact, _) => [1.0, 0.0, 0.0, 0.0, 0.0],
            (Mode::Alert, _) => [0.0, 1.0, 0.0, 0.0, 0.0],
            (Mode::Open, 0) => [0.0, 0.0, 1.0, 0.0, 0.0],
            (Mode::Open, 2) => [0.0, 0.0, 0.0, 0.0, 1.0],
            (Mode::Open, _) => [0.0, 0.0, 0.0, 1.0, 0.0],
        };
        for (spring, target) in self.show.iter_mut().zip(wanted) {
            spring.tune(motion.fade());
            spring.set(target);
            spring.step(dt);
        }
        let grow = |shown: f32| if motion.reduced() { 1.0 } else { 0.88 + 0.12 * shown };
        let shown: Vec<f32> = self.show.iter().map(|spring| spring.pos.clamp(0.0, 1.0)).collect();
        let layers: [(&mut Layer, (f32, f32), bool); 5] =
            [(&mut self.compact, compact_size, true), (&mut self.alert, alert_size, true), (&mut self.home, home_size, false), (&mut self.list, list_size, false), (&mut self.month, month_size, false)];
        for ((layer, size, centred), amount) in layers.into_iter().zip(shown) {
            // The small shapes sit in the middle of the glass; pages hang from its top.
            layer.set_offset(((w - size.0) * 0.5).round(), if centred { ((h - size.1) * 0.5).round() } else { 0.0 })?;
            layer.set_scale(grow(amount), grow(amount), size.0 * 0.5, if centred { size.1 * 0.5 } else { size.1 * 0.3 })?;
            layer.set_opacity(amount)?;
        }

        // Only the glass (and the strip between it and the top of the screen) takes the mouse.
        let hidden = hide.target == 1.0 && hide.pos > 0.97;
        let wanted = if hidden { (0, 0, 0, 0) } else { ((gx - 2.0).floor() as i32, 0, (gx + w + 2.0).ceil() as i32, (gy + h + 2.0).ceil() as i32) };
        if self.region != Some(wanted) {
            self.region = Some(wanted);
            unsafe {
                // The system takes ownership of the region handle.
                let rgn = CreateRectRgn(wanted.0, wanted.1, wanted.2, wanted.3);
                SetWindowRgn(self.hwnd, Some(rgn), false);
            }
        }

        let level_moving = matches!(self.current, Some(Alert::Volume { .. })) && self.alerting && !self.level.settled(1e-3);
        Ok(self.dragging
            || level_moving
            || !self.w.settled(0.05)
            || !self.h.settled(0.05)
            || !self.r.settled(0.05)
            || self.show.iter().any(|spring| !spring.settled(1e-3))
            || !self.hot_at.settled())
    }
}

impl Drop for Island {
    fn drop(&mut self) {
        self.appbar.remove();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
