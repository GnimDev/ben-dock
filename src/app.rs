//! The app: it owns every surface, hears everything that happens, and decides
//! what a click does. The surfaces themselves only know how to look and move.
//!
//! Every screen gets a dock and an island of its own (`Screen`). A screen's
//! dock shows the pinned apps and the apps that have a window on that screen.
//! The menus, the app tray, the window preview and the window effect exist
//! once and open on whichever screen asked for them.

use crate::audio;
use crate::bluetooth;
use crate::bubble::{self, Bubble, Entry};
use crate::config::{self, Config};
use crate::dock::{self, Dock, Released, LAUNCHER_KEY};
use crate::dropzone;
use crate::fx::{self, Fx};
use crate::gfx::Gfx;
use crate::island::{self, Action, Alert, Island, Page, Row};
use crate::launcher::{self, Launcher};
use crate::media::{self, Media, NowPlaying};
use crate::motion::Motion;
use crate::preview::{self, Preview};
use crate::scene::Scene;
use crate::settings;
use crate::shell::{self, AppScan, WindowTracker};
use crate::spring::Spring;
use crate::system::{self, AppBar, Clock, Monitor, Taskbar};
use crate::theme::{self, Theme};
use crate::util::wide;
use crate::wifi;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::time::{Instant, SystemTime};
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::ABN_POSCHANGED;
use windows::Win32::UI::WindowsAndMessaging::*;

/// The class of the docks' windows. The first screen's dock is the app's main window.
pub const CLASS_NAME: &str = "BenDockWindow";
const WM_APPBAR_CALLBACK: u32 = WM_APP + 1;
const WM_VOLUME_CHANGED: u32 = WM_APP + 3;
const WM_MEDIA_CHANGED: u32 = WM_APP + 4;
const WM_APPS_FOUND: u32 = WM_APP + 5;
const WM_PAGE_DATA: u32 = WM_APP + 6;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_DPICHANGED_MSG: u32 = 0x02E0;
const WM_DWMCOLORIZATIONCOLORCHANGED_MSG: u32 = 0x0320;
const TIMER_ID: usize = 1;
/// A one-shot timer that only wakes the main loop (see `App::wake_in`).
const TIMER_WAKE: usize = 2;
/// A one-shot timer that has the open windows read again (see `App::settle_newcomers`).
const TIMER_LOOK: usize = 3;
/// For how long after it opens a window is made to float if it turns out too large: some
/// apps show their window first and maximise it a moment later. Seconds.
const NEWCOMER_FOR: f32 = 1.5;
/// How long a new window must have kept its place and state before it is touched. Moving a
/// window while its app is in the middle of showing or maximising it leaves it anywhere.
const STEADY_FOR: f32 = 0.1;
/// How often a new window is looked at, in milliseconds.
const LOOK_EVERY: u32 = 120;
const HSHELL_FLASH: usize = 0x8006;
/// Windows asks where a window goes as it is minimised (or comes from as it is restored).
const HSHELL_GETMINRECT: usize = 5;
/// The least time between two readings of the open windows, in seconds.
const REFRESH_EVERY: f32 = 0.06;
/// How long after the last mouse movement or click the loop keeps turning at the display's
/// pace. Between movements it sleeps; this only keeps a fast mouse (which reports up to a
/// thousand positions a second) from turning the loop that often.
const PACE_AFTER_INPUT: f32 = 0.12;

/// How long the app must have been at rest before it gives memory back, and how long
/// the tray's icons are kept unused. Seconds.
const REST_AFTER: f32 = 20.0;
const KEEP_TRAY_ICONS: f32 = 90.0;

/// Menu commands.
mod cmd {
    pub const OPEN: usize = 1;
    pub const PIN: usize = 2;
    pub const CLOSE: usize = 3;
    pub const DIVIDER: usize = 4;
    pub const REMOVE_DIVIDER: usize = 5;
    pub const TASKBAR: usize = 10;
    pub const AUTOSTART: usize = 11;
    pub const SETTINGS: usize = 12;
    pub const QUIT: usize = 20;
    pub const WINDOWS_SETTINGS: usize = 30;
    pub const RESTART: usize = 33;
    pub const SHUT_DOWN: usize = 34;
    pub const SIGN_OUT: usize = 35;
}

/// Rows of the island's pages.
mod row {
    pub const SETTINGS: usize = 1;
    pub const MUTE: usize = 2;
    pub const SWITCH: usize = 3;
    pub const JOIN: usize = 4;
    pub const CANCEL: usize = 5;
    pub const REFRESH: usize = 6;
    pub const LOCATION: usize = 7;
    pub const PASSWORD: usize = 8;
    /// A row of a list: `LISTED + index into the list that was shown`.
    pub const LISTED: usize = 100;
}

// Symbols of Windows' icon font used in the pages.
const TICK: u32 = 0xE73E;
const LOCK: u32 = 0xE72E;
const GEAR: u32 = 0xE713;

// ------------------------------------------------------------------ events
//
// The window procedures only record what happened. The main loop handles the
// events afterwards, so no app code ever runs re-entrantly (for example
// while a message box is pumping messages).

#[derive(Clone, Copy, Debug)]
pub enum Event {
    /// Mouse over a dock (the window it happened in comes first).
    MouseMove(isize),
    MouseLeave(isize),
    LeftDown(isize, i32, i32),
    LeftUp(isize, i32, i32),
    MiddleUp(isize, i32, i32),
    RightUp(isize, i32, i32),
    /// Something a dock can hold is dragged over it from another program (see `dropzone.rs`),
    /// at this column of the screen.
    DragIn(isize, i32),
    /// It left the dock again.
    DragOut(isize),
    /// It was dropped on the dock.
    Dropped(isize),
    Tick,
    Shell(usize, isize),
    AppBar(usize),
    Relayout,
    Theme,
    TaskbarCreated,
    /// A window started to minimise (reported by Windows).
    Minimized(isize),
    /// A minimised window came back (reported by Windows).
    Restored(isize),
    /// The user began or finished moving or resizing a window.
    Moved(isize),
    /// Time to read the open windows again.
    WindowsChanged,
    /// Click in the window preview. `true` = middle button.
    PreviewClick(bool),
    /// The cursor moved over the window preview, or left it.
    PreviewHover,
    /// A timer the app set for itself ran out: time to look again.
    Wake,
    /// The volume or mute changed.
    Volume,
    /// The charger went in or out, or the battery level moved.
    Power,
    /// What is playing changed.
    Media,
    /// The background reader of the app list has something new.
    Apps,
    /// Something a page of the island was waiting for has been read.
    PageData,
    /// From an island (its window comes first).
    Island(isize, island::Input),
    Bubble(bubble::Input),
    Launcher(launcher::Input),
    /// The settings window changed something. `true` once the change is final (control released).
    Config(bool),
    Quit,
}

thread_local! {
    static EVENTS: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
    /// Where each open window's icon stands on its screen's dock (left, top, right, bottom in
    /// screen pixels). Windows asks every shell where a window goes as it is minimised, and
    /// plays its own animation towards the answer. The window procedure answers from this
    /// table; the app rewrites it whenever the docks change.
    static MIN_RECTS: RefCell<HashMap<isize, [i16; 4]>> = RefCell::new(HashMap::new());
}

/// What Windows hands over with that question: the window, and room for the answer as four
/// 16-bit numbers. (The documented layout has a full rectangle here; what actually arrives
/// through a registered shell window has always been these four short numbers.)
#[repr(C)]
struct MinRectQuery {
    window: isize,
    rect: [i16; 4],
}

/// Answers the question if the window has an icon on a dock. Explorer is asked first and has
/// filled in its taskbar button; the last answer is the one Windows uses.
unsafe fn answer_min_rect(lparam: LPARAM) -> bool {
    let query = lparam.0 as *mut MinRectQuery;
    if query.is_null() {
        return false;
    }
    let window = unsafe { (*query).window };
    let known = MIN_RECTS.with(|table| table.try_borrow().ok().and_then(|table| table.get(&window).copied()));
    match known {
        Some(rect) => {
            unsafe { (*query).rect = rect };
            true
        }
        None => false,
    }
}
static SHELL_HOOK_MSG: AtomicU32 = AtomicU32::new(0);
static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);
/// The app's main window: the one whose end is the end of the app.
static MAIN_WINDOW: AtomicIsize = AtomicIsize::new(0);

fn push(ev: Event) {
    EVENTS.with(|q| {
        let mut q = q.borrow_mut();
        if matches!(ev, Event::MouseMove(_)) && matches!(q.back(), Some(Event::MouseMove(_))) {
            return;
        }
        q.push_back(ev);
    });
}

/// Queues an event from one of the other windows.
pub fn post(ev: Event) {
    push(ev);
}

pub fn take_events() -> Vec<Event> {
    EVENTS.with(|q| q.borrow_mut().drain(..).collect())
}

/// Called by Windows (on this thread, while it pumps messages) when any window on the desktop
/// starts or stops being minimised, or finishes being moved.
unsafe extern "system" fn win_event(_hook: HWINEVENTHOOK, event: u32, hwnd: HWND, object: i32, _child: i32, _thread: u32, _time: u32) {
    // Object 0 is the window itself (not one of its parts).
    if object != 0 || hwnd.is_invalid() {
        return;
    }
    match event {
        EVENT_SYSTEM_MINIMIZESTART => push(Event::Minimized(hwnd.0 as isize)),
        EVENT_SYSTEM_MINIMIZEEND => push(Event::Restored(hwnd.0 as isize)),
        _ => push(Event::Moved(hwnd.0 as isize)),
    }
}

fn point(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32)
}

/// The window procedure of every dock's window.
pub unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let me = hwnd.0 as isize;
    let (x, y) = point(lparam);
    match msg {
        WM_MOUSEMOVE => push(Event::MouseMove(me)),
        WM_MOUSELEAVE => push(Event::MouseLeave(me)),
        WM_LBUTTONDOWN => push(Event::LeftDown(me, x, y)),
        WM_LBUTTONUP => push(Event::LeftUp(me, x, y)),
        WM_MBUTTONUP => push(Event::MiddleUp(me, x, y)),
        WM_RBUTTONUP => push(Event::RightUp(me, x, y)),
        WM_TIMER => {
            if wparam.0 == TIMER_WAKE {
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_WAKE);
                }
                push(Event::Wake);
            } else if wparam.0 == TIMER_LOOK {
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_LOOK);
                }
                push(Event::WindowsChanged);
            } else {
                push(Event::Tick);
            }
        }
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_DISPLAYCHANGE | WM_DPICHANGED_MSG => push(Event::Relayout),
        WM_SETTINGCHANGE => {
            push(Event::Relayout);
            push(Event::Theme);
        }
        WM_DWMCOLORIZATIONCOLORCHANGED_MSG => push(Event::Theme),
        WM_APPBAR_CALLBACK => push(Event::AppBar(wparam.0)),
        WM_VOLUME_CHANGED => {
            push(Event::Volume);
            return LRESULT(0);
        }
        WM_MEDIA_CHANGED => {
            push(Event::Media);
            return LRESULT(0);
        }
        WM_APPS_FOUND => {
            push(Event::Apps);
            return LRESULT(0);
        }
        WM_PAGE_DATA => {
            push(Event::PageData);
            return LRESULT(0);
        }
        WM_POWERBROADCAST => push(Event::Power),
        WM_QUERYENDSESSION => return LRESULT(1),
        WM_ENDSESSION => {
            // Signing out or shutting down: put the taskbar setting back now, there is no later.
            if wparam.0 != 0 {
                system::restore_taskbar();
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            push(Event::Quit);
            return LRESULT(0);
        }
        WM_DESTROY => {
            // A dock on another screen goes whenever that screen does; only the main window's
            // end is the app's.
            if me == MAIN_WINDOW.load(Ordering::Relaxed) {
                unsafe { PostQuitMessage(0) };
            }
            return LRESULT(0);
        }
        _ => {
            if msg != 0 && msg == SHELL_HOOK_MSG.load(Ordering::Relaxed) {
                push(Event::Shell(wparam.0, lparam.0));
                // This one question is answered on the spot: Windows is waiting for it.
                let answered = wparam.0 == HSHELL_GETMINRECT && unsafe { answer_min_rect(lparam) };
                return LRESULT(answered as isize);
            }
            if msg != 0 && msg == TASKBAR_CREATED_MSG.load(Ordering::Relaxed) {
                push(Event::TaskbarCreated);
            }
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// A window for the dock of a screen other than the first.
fn new_dock_window() -> Result<HWND> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            PCWSTR(wide(CLASS_NAME).as_ptr()),
            PCWSTR(wide("Bến").as_ptr()),
            WS_POPUP,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(instance),
            None,
        )
    }
}

// ------------------------------------------------------------------- menus

/// The menu of a dock icon (`item` = name, how many windows, pinned), of a divider the user put
/// in the dock (`divider`), or of the dock itself.
fn dock_menu(item: Option<(&str, usize, bool)>, divider: bool, taskbar_shown: bool, autostart: bool) -> Vec<Entry> {
    let mut rows = Vec::new();
    if divider {
        rows.push(Entry::item(cmd::REMOVE_DIVIDER, "Gỡ vách ngăn"));
        rows.push(Entry::separator());
    }
    if let Some((name, windows, pinned)) = item {
        rows.push(Entry::header(name));
        rows.push(Entry::item(cmd::OPEN, if windows == 0 { "Mở" } else { "Mở cửa sổ mới" }));
        rows.push(Entry::item(cmd::PIN, if pinned { "Gỡ khỏi dock" } else { "Giữ trong dock" }));
        if pinned {
            rows.push(Entry::item(cmd::DIVIDER, "Thêm vách ngăn phía sau"));
        }
        if windows > 0 {
            rows.push(Entry::item(cmd::CLOSE, if windows > 1 { "Đóng tất cả cửa sổ" } else { "Đóng cửa sổ" }));
        }
        rows.push(Entry::separator());
    }
    rows.push(Entry::item(cmd::SETTINGS, "Cài đặt Bến…"));
    rows.push(Entry::item(cmd::TASKBAR, "Hiện taskbar của Windows").checked(taskbar_shown));
    rows.push(Entry::item(cmd::AUTOSTART, "Khởi động cùng Windows").checked(autostart));
    rows.push(Entry::separator());
    rows.push(Entry::item(cmd::QUIT, "Thoát Bến"));
    rows
}

/// The menu of the island's power button.
fn power_menu() -> Vec<Entry> {
    vec![
        Entry::item(cmd::RESTART, "Khởi động lại…"),
        Entry::item(cmd::SHUT_DOWN, "Tắt máy…"),
        Entry::item(cmd::SIGN_OUT, "Đăng xuất…"),
        Entry::separator(),
        Entry::item(cmd::WINDOWS_SETTINGS, "Cài đặt Windows…"),
        Entry::item(cmd::SETTINGS, "Cài đặt Bến…"),
        Entry::separator(),
        Entry::item(cmd::QUIT, "Thoát Bến"),
    ]
}

// --------------------------------------------------------------- self-tests

/// How the app was started.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Run {
    Normal,
    /// A scripted self-test: an imaginary cursor sweeps across the dock while the material is
    /// switched, and the frame timing of each is written to ben.log.
    Bench,
    /// A scripted tour of every surface, for looking at them (or taking pictures of them).
    Demo,
}

struct Bench {
    saved: Config,
    started: Instant,
    /// Index into `BENCH_PHASES`; `None` until the dock has settled after start-up.
    phase: Option<usize>,
    finished: Option<&'static str>,
}

const BENCH_WARMUP: f32 = 3.0;
const BENCH_PHASE_SECONDS: f32 = 3.0;
type Tweak = fn(&mut Config);
const BENCH_PHASES: &[(&str, Tweak)] = &[("liquid glass", |c| c.style = 0.0), ("frosted glass", |c| c.style = 1.0), ("solid, no blur", |c| c.style = 3.0)];

/// The tour: at each time (seconds from the start) one thing happens.
struct Demo {
    started: Instant,
    step: usize,
}

#[derive(Clone, Copy, Debug)]
enum Act {
    SweepDock,
    RestOnIcon,
    LeaveDock,
    /// An app is dragged onto the dock from outside: it touches the glass,
    DragIn,
    /// is pushed in until the glass lets it through,
    PushIn,
    /// and is taken out again, so the tour leaves the pinned apps as they were.
    DragOut,
    AlertVolume,
    AlertTrack,
    OpenIsland,
    OpenPage(Page),
    CloseIsland,
    OpenMenu,
    CloseMenu,
    OpenTray,
    CloseTray,
    End,
}

const DEMO: &[(f32, Act)] = &[
    (2.0, Act::SweepDock),
    (5.0, Act::RestOnIcon),
    (7.5, Act::LeaveDock),
    (8.0, Act::DragIn),
    (9.4, Act::PushIn),
    (12.0, Act::DragOut),
    (12.5, Act::AlertVolume),
    (15.0, Act::AlertTrack),
    (17.5, Act::OpenIsland),
    (20.0, Act::OpenPage(Page::Wifi)),
    (22.5, Act::OpenPage(Page::Sound)),
    (25.0, Act::OpenPage(Page::Bluetooth)),
    (27.5, Act::OpenPage(Page::Battery)),
    (30.0, Act::OpenPage(Page::Calendar)),
    (32.5, Act::CloseIsland),
    (33.0, Act::OpenMenu),
    (35.5, Act::CloseMenu),
    (36.0, Act::OpenTray),
    (39.5, Act::CloseTray),
    (40.5, Act::End),
];

/// The picture of a window that has flown back out is kept up until the real window is there.
struct Release {
    window: HWND,
    /// Time left once the window is back.
    left: f32,
    waited: f32,
    /// Where the picture ended (the window's drawn part before it was minimised).
    ended: Option<RECT>,
}

/// An attempt to join a Wi-Fi network that Windows is still working on.
struct Joining {
    name: String,
    /// The network was saved in Windows for this attempt (so it is removed again if it fails).
    saved_now: bool,
    since: Instant,
}

/// A window that has just opened, watched for a moment (see `App::settle_newcomers`).
struct Newcomer {
    /// When it was first seen, not counting time it spent minimised.
    since: Instant,
    /// How often it has been made to float.
    times: u32,
    /// Whether it was maximised and where it was when last looked at, and since when that
    /// has been so.
    state: (bool, RECT),
    steady: Instant,
}

/// What one screen has: its dock and its island.
struct Screen {
    monitor: Monitor,
    /// The look for this screen (blur and shadow are sized in its pixels).
    theme: Theme,
    /// Keeps maximised windows out of the dock's strip. (Declared before the dock so that it
    /// gives the strip back while the dock's window still exists.)
    appbar: AppBar,
    dock: Dock,
    island: Option<Island>,
    /// Bottom edge of the dock's strip on screen, as last granted by the shell.
    bottom_line: i32,
    /// 0 = shown, 1 = out of the way of a full-screen app on this screen.
    hide: Spring,
}

pub struct App {
    /// The main window: the first screen's dock.
    hwnd: HWND,
    cfg: Config,
    motion: Motion,
    gfx: Gfx,
    scene: Scene,
    /// The first is the primary monitor's.
    screens: Vec<Screen>,

    bubble: Option<Bubble>,
    /// The screen each of the shared surfaces last opened on.
    bubble_screen: usize,
    launcher: Option<Launcher>,
    /// The background reader of the app list, started when the tray is first opened.
    scan: Option<AppScan>,
    preview: Option<Preview>,
    preview_screen: usize,
    fx: Option<Fx>,
    /// After a restore effect ends, how long to keep its picture up while the real window appears.
    fx_release: Option<Release>,

    tracker: WindowTracker,
    taskbar: Taskbar,
    /// The dock window whose mouse-leave is being tracked (0: none).
    tracking_mouse: isize,
    refresh_due: bool,
    replace_due: bool,
    /// The app window that was last in front (a menu of ours in front does not count).
    last_front: HWND,
    /// Last-modified time of ben.ini as last read, to notice edits made by hand.
    cfg_stamp: Option<SystemTime>,

    status: system::StatusReader,
    last_status: system::Status,
    clock: Clock,
    today: (i32, u32, u32),
    media: Option<Media>,
    playing: Option<NowPlaying>,

    // What the island's pages show.
    /// The Wi-Fi networks as last listed (the page's rows point into this).
    wifi_list: Vec<wifi::Network>,
    /// The network a password is being asked for.
    wifi_asking: Option<String>,
    /// A line about the last attempt to join.
    wifi_note: Option<String>,
    wifi_joining: Option<Joining>,
    /// The playback devices as last listed.
    outputs: Vec<audio::Output>,
    /// Bluetooth as last read (`None` until it has been).
    bluetooth: Option<bluetooth::Bluetooth>,
    /// Where the answer of a reading in progress arrives.
    bluetooth_reading: Option<Receiver<bluetooth::Bluetooth>>,

    /// The dock item the open menu belongs to (its screen and key), if it was opened on an icon.
    menu_item: Option<(usize, String)>,
    /// Where each open window was last seen on screen: its own rectangle and the part of it that
    /// is drawn. Not updated while the window is minimised.
    rects: HashMap<isize, (RECT, RECT)>,
    /// The screen each open window was last seen on while it was not minimised.
    homes: HashMap<isize, isize>,
    /// Every app window seen since the app started, so that only those that open later count
    /// as new. `seeded`: the first reading has been taken (those windows were open already).
    known: HashSet<isize>,
    seeded: bool,
    /// Windows that have just opened.
    newcomers: HashMap<isize, Newcomer>,
    /// Running app the cursor is resting on (its screen and key), and since when.
    resting_on: Option<(usize, String, Instant)>,
    /// When the cursor left the preview and its icon.
    preview_left: Option<Instant>,
    /// App that was just clicked: no preview for it until the cursor has moved off its icon.
    preview_block: Option<String>,
    /// The screen whose dock an app's tile, dragged out of the tray, is over.
    tray_drag: Option<usize>,
    hooks: Vec<HWINEVENTHOOK>,
    /// Last error from updating the visuals, so a repeating error is logged once.
    shown_error: Option<windows::core::Error>,

    bench: Option<Bench>,
    bench_last: Option<&'static str>,
    demo: Option<Demo>,
    /// An imaginary cursor over the first dock (window coordinates), for the tour.
    demo_cursor: Option<(f32, f32)>,

    /// An event came in since the last pass.
    touched: bool,
    /// The last pass left something moving.
    was_moving: bool,
    /// When the mouse last moved over, or clicked, one of the surfaces.
    last_input: Instant,
    /// When the open windows were last read.
    last_refresh: Instant,
    /// Passes of the main loop and readings of the open windows since the last note in the log.
    passes: u32,
    refreshes: u32,
    /// When something last moved or was touched.
    last_busy: Instant,
    /// Memory has been given back since then.
    rested: bool,
    last_memory_note: Instant,
    pub quit: bool,
}

impl App {
    pub fn new(hwnd: HWND, cfg: Config, run: Run) -> Result<Self> {
        MAIN_WINDOW.store(hwnd.0 as isize, Ordering::Relaxed);
        let gfx = Gfx::new(cfg.hardware_gpu)?;
        let scene = Scene::new(&gfx)?;
        let motion = Motion::new(cfg.anim_speed, cfg.elastic, system::reduce_motion());

        let mut app = App {
            hwnd,
            cfg,
            motion,
            gfx,
            scene,
            screens: Vec::new(),
            bubble: None,
            bubble_screen: 0,
            launcher: None,
            scan: None,
            preview: None,
            preview_screen: 0,
            fx: None,
            fx_release: None,
            tracker: WindowTracker::default(),
            taskbar: Taskbar::new(),
            tracking_mouse: 0,
            refresh_due: true,
            replace_due: false,
            last_front: HWND::default(),
            cfg_stamp: config::config_modified(),
            status: system::StatusReader::new(),
            last_status: system::Status::default(),
            clock: system::clock(),
            today: system::today(),
            media: None,
            playing: None,
            wifi_list: Vec::new(),
            wifi_asking: None,
            wifi_note: None,
            wifi_joining: None,
            outputs: Vec::new(),
            bluetooth: None,
            bluetooth_reading: None,
            menu_item: None,
            rects: HashMap::new(),
            homes: HashMap::new(),
            known: HashSet::new(),
            seeded: false,
            newcomers: HashMap::new(),
            resting_on: None,
            preview_left: None,
            preview_block: None,
            tray_drag: None,
            hooks: Vec::new(),
            shown_error: None,
            bench: None,
            bench_last: None,
            demo: None,
            demo_cursor: None,
            touched: true,
            was_moving: true,
            last_input: Instant::now(),
            last_refresh: Instant::now(),
            passes: 0,
            refreshes: 0,
            last_busy: Instant::now(),
            rested: false,
            last_memory_note: Instant::now(),
            quit: false,
        };

        if app.cfg.hide_taskbar {
            app.taskbar.hide();
        }
        // One dock per screen; the first screen's lives in the main window.
        for (index, monitor) in system::monitors().into_iter().enumerate() {
            let window = if index == 0 { Ok(hwnd) } else { new_dock_window() };
            match window.and_then(|w| app.make_screen(monitor, w, index != 0)) {
                Ok(screen) => app.screens.push(screen),
                Err(e) if index == 0 => return Err(e),
                Err(e) => crate::log!("no dock on screen {} ({e})", index + 1),
            }
        }
        let first = &app.screens[0];
        crate::log!(
            "graphics ready: {} screen(s), the first {}x{} px at scale {:.2}, {:?}, {}",
            app.screens.len(),
            first.monitor.width(),
            first.monitor.rect.bottom - first.monitor.rect.top,
            first.monitor.scale(),
            theme::Style::of(&app.cfg),
            if app.motion.reduced() { "animation effects are off in Windows" } else { "animated" }
        );

        match run {
            Run::Bench => app.bench = Some(Bench { saved: app.cfg.clone(), started: Instant::now(), phase: None, finished: None }),
            Run::Demo => app.demo = Some(Demo { started: Instant::now(), step: 0 }),
            Run::Normal => {}
        }
        app.place(true);

        unsafe {
            // Shell notifications make the docks react at once when windows open, close or flash.
            SHELL_HOOK_MSG.store(RegisterWindowMessageW(PCWSTR(wide("SHELLHOOK").as_ptr())), Ordering::Relaxed);
            TASKBAR_CREATED_MSG.store(RegisterWindowMessageW(PCWSTR(wide("TaskbarCreated").as_ptr())), Ordering::Relaxed);
            if !RegisterShellHookWindow(hwnd).as_bool() {
                crate::log!("RegisterShellHookWindow failed; falling back to polling only");
            }
            // The poll is the safety net: it also re-hides the taskbar if Explorer brings it
            // back, and moves the clock on.
            SetTimer(Some(hwnd), TIMER_ID, 1000, None);

            // Be told the moment a window is minimised, restored or moved.
            for (first, last) in [(EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND), (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND)] {
                let hook = SetWinEventHook(first, last, None, Some(win_event), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
                if hook.is_invalid() {
                    crate::log!("SetWinEventHook({first}) failed; minimise effects will not play");
                } else {
                    app.hooks.push(hook);
                }
            }
        }
        app.status.watch_volume(hwnd, WM_VOLUME_CHANGED);
        app.last_status = app.status.read();

        match Fx::new(&app.scene) {
            Ok(fx) => app.fx = Some(fx),
            Err(e) => crate::log!("effect window unavailable: {e}"),
        }
        match Preview::new(&app.scene, &app.screens[0].theme, &app.motion) {
            Ok(p) => app.preview = Some(p),
            Err(e) => crate::log!("window preview unavailable: {e}"),
        }
        // Versions before this one switched Windows' own minimise animation off while they
        // ran. This one leaves it alone; if such a run never put it back, do so now.
        system::restore_window_animations();
        app.setup_surfaces();
        system::limit_memory(app.cfg.ram_limit.round().max(0.0) as u32);
        Ok(app)
    }

    /// The dock (and the rest of what a screen has) for one monitor, in `window`.
    fn make_screen(&self, monitor: Monitor, window: HWND, owned: bool) -> Result<Screen> {
        let theme = theme::build(&self.cfg, system::system_is_dark(), system::accent_color(), monitor.scale());
        let dock = Dock::new(window, owned, &self.scene, &self.cfg, &theme, &self.motion, &monitor)?;
        dropzone::accept(window);
        let mut appbar = AppBar::new(window);
        if self.cfg.reserve_space {
            appbar.register(WM_APPBAR_CALLBACK);
        }
        // Everything rises into view when it first appears.
        let mut hide = Spring::with(1.0, self.motion.medium());
        hide.set(0.0);
        unsafe {
            let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
        }
        Ok(Screen { monitor, theme, appbar, dock, island: None, bottom_line: monitor.rect.bottom, hide })
    }

    /// Creates or removes the islands, the menus and the tray to match the settings.
    fn setup_surfaces(&mut self) {
        for screen in &mut self.screens {
            if self.cfg.island && screen.island.is_none() {
                match Island::new(&self.scene, &screen.theme, &self.motion) {
                    Ok(island) => screen.island = Some(island),
                    Err(e) => crate::log!("the island is unavailable: {e}"),
                }
            } else if !self.cfg.island {
                screen.island = None;
            }
            if let Some(island) = screen.island.as_mut() {
                island.place(&screen.monitor, &self.cfg, &screen.theme);
            }
            if screen.dock.set_launcher_button(self.cfg.launcher, &self.motion) {
                self.refresh_due = true;
            }
        }

        let want_media = self.cfg.island && self.cfg.island_media;
        if want_media && self.media.is_none() {
            self.media = Some(Media::start(self.hwnd, WM_MEDIA_CHANGED));
        } else if !want_media {
            self.media = None;
            self.playing = None;
        }

        let first = &self.screens[0].theme;
        if self.bubble.is_none() {
            match Bubble::new(&self.scene, first, &self.motion) {
                Ok(bubble) => self.bubble = Some(bubble),
                Err(e) => crate::log!("the app's own menus are unavailable ({e}); using Windows' menu"),
            }
        }
        if self.cfg.launcher && self.launcher.is_none() {
            match Launcher::new(&self.scene, first, &self.motion) {
                Ok(l) => self.launcher = Some(l),
                Err(e) => crate::log!("the app tray is unavailable: {e}"),
            }
        } else if !self.cfg.launcher {
            self.launcher = None;
            self.scan = None;
        }
    }

    fn screen_of_dock(&self, window: isize) -> Option<usize> {
        self.screens.iter().position(|s| s.dock.hwnd().0 as isize == window)
    }

    fn screen_of_island(&self, window: isize) -> Option<usize> {
        self.screens.iter().position(|s| s.island.as_ref().map(|i| i.hwnd().0 as isize) == Some(window))
    }

    /// The screen whose dock has an icon for a window, and that icon.
    fn owner_of(&self, window: HWND) -> Option<(usize, usize)> {
        self.screens.iter().enumerate().find_map(|(s, screen)| screen.dock.owner_of(window).map(|i| (s, i)))
    }

    // ------------------------------------------------------------- placement

    /// Sizes and positions the surfaces on their screens. With `reserve`, also asks the shell
    /// again for the strips of screen that maximised windows must stay out of; without it
    /// the strips are left as they are (used while a settings slider is being dragged, so
    /// that every window on screen is not resized on each mouse move).
    fn place(&mut self, reserve: bool) {
        for screen in &mut self.screens {
            let monitor = screen.monitor;
            // Where the bottom of the dock's strip is: the screen edge when the taskbar is
            // hidden, otherwise just above the taskbar.
            if reserve {
                let mut bottom = if self.taskbar.hidden { monitor.rect.bottom } else { monitor.work.bottom };
                if self.cfg.reserve_space {
                    let m = dock::metrics(&self.cfg, monitor.scale(), monitor.width());
                    let height = (m.panel_h + m.margin * 2.0).ceil() as i32;
                    if let Some(strip) = screen.appbar.reserve(monitor.rect, height) {
                        bottom = strip.bottom;
                    }
                }
                screen.bottom_line = bottom;
            }
            screen.dock.place(&self.cfg, &monitor, screen.bottom_line);
            if reserve {
                if let Some(island) = screen.island.as_mut() {
                    island.place(&monitor, &self.cfg, &screen.theme);
                }
            }
        }
        self.publish_min_rects();
    }

    /// A screen was plugged in, taken away, resized or rescaled.
    fn relayout_if_changed(&mut self) {
        let monitors = system::monitors();
        let same_screens = monitors.len() == self.screens.len() && monitors.iter().zip(&self.screens).all(|(m, s)| m.handle == s.monitor.handle);
        let same_shape = same_screens && monitors.iter().zip(&self.screens).all(|(m, s)| m.rect == s.monitor.rect && m.dpi == s.monitor.dpi);
        if same_shape {
            // At most the room left by other bars changed (the taskbar came or went, or the
            // app's own strips were granted): take the places again, nothing else.
            for (monitor, screen) in monitors.iter().zip(&mut self.screens) {
                screen.monitor = *monitor;
            }
            self.place(true);
            return;
        }
        crate::log!("screens changed: now {}", monitors.len());
        self.close_bubble();
        self.close_preview();
        if let Some(l) = self.launcher.as_mut() {
            l.close();
        }
        if same_screens {
            for (monitor, screen) in monitors.iter().zip(&mut self.screens) {
                screen.monitor = *monitor;
            }
        } else {
            // The first screen keeps the main window; the docks of the others are made again.
            self.screens.truncate(1);
            self.screens[0].monitor = monitors[0];
            self.screens[0].island = None;
            for monitor in monitors.into_iter().skip(1) {
                match new_dock_window().and_then(|w| self.make_screen(monitor, w, true)) {
                    Ok(screen) => self.screens.push(screen),
                    Err(e) => crate::log!("no dock on a screen ({e})"),
                }
            }
            self.share_pins(usize::MAX);
        }
        // Blur and shadow are in each screen's own pixels.
        self.retheme(true);
        self.setup_surfaces();
        self.place(true);
        self.refresh_due = true;
    }

    // ---------------------------------------------------------------- events

    pub fn handle(&mut self, ev: Event) {
        self.touched = true;
        if !matches!(ev, Event::Tick | Event::Shell(..) | Event::WindowsChanged | Event::Moved(_) | Event::Media | Event::Apps | Event::Power | Event::AppBar(_) | Event::Wake | Event::PageData) {
            self.busy();
        }
        if matches!(ev, Event::DragIn(..) | Event::DragOut(_) | Event::Dropped(_) | Event::MouseMove(_) | Event::LeftDown(..) | Event::LeftUp(..) | Event::MiddleUp(..) | Event::RightUp(..) | Event::PreviewClick(_) | Event::PreviewHover | Event::Island(..) | Event::Bubble(_) | Event::Launcher(_)) {
            self.last_input = Instant::now();
        }
        match ev {
            Event::MouseMove(window) => {
                if self.tracking_mouse != window {
                    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: HWND(window as *mut _), dwHoverTime: 0 };
                    if unsafe { TrackMouseEvent(&mut track) }.is_ok() {
                        self.tracking_mouse = window;
                    }
                }
            }
            Event::MouseLeave(window) => {
                if self.tracking_mouse == window {
                    self.tracking_mouse = 0;
                }
            }
            Event::LeftDown(window, x, y) => {
                let Some(s) = self.screen_of_dock(window) else { return };
                self.close_bubble();
                self.close_preview();
                let pressed = self.screens[s].dock.press(x as f32, y as f32);
                self.preview_block = pressed.map(|i| self.screens[s].dock.items[i].key.clone());
                unsafe {
                    SetCapture(HWND(window as *mut _));
                }
            }
            Event::LeftUp(window, x, y) => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                let Some(s) = self.screen_of_dock(window) else { return };
                match self.screens[s].dock.release(x as f32, y as f32) {
                    Released::Click(index) => self.click(s, index),
                    Released::Reordered => self.share_pins(s),
                    Released::Nothing => {}
                }
            }
            Event::MiddleUp(window, x, y) => {
                let Some(s) = self.screen_of_dock(window) else { return };
                self.close_bubble();
                if let Some(index) = self.screens[s].dock.app_at(x as f32, y as f32) {
                    self.launch(s, index);
                }
            }
            Event::RightUp(window, x, y) => {
                let Some(s) = self.screen_of_dock(window) else { return };
                self.close_bubble();
                self.close_preview();
                let dock = &self.screens[s].dock;
                let index = dock.app_at(x as f32, y as f32).or(dock.divider_at(x as f32, y as f32));
                self.show_dock_menu(s, index);
            }
            Event::DragIn(window, x) => {
                let Some(s) = self.screen_of_dock(window) else { return };
                self.close_bubble();
                self.close_preview();
                let dock = &mut self.screens[s].dock;
                dock.drag_in(&dropzone::carried(), (x - dock.win_x) as f32, &self.motion);
            }
            Event::DragOut(window) => {
                let Some(s) = self.screen_of_dock(window) else { return };
                self.screens[s].dock.drag_out();
                self.refresh_due = true;
            }
            Event::Dropped(window) => {
                let Some(s) = self.screen_of_dock(window) else { return };
                if self.screens[s].dock.drop_in(&dropzone::carried(), &self.motion) {
                    self.share_pins(s);
                }
            }
            Event::Tick => {
                self.refresh_due = true;
                self.taskbar.keep_hidden();
                self.read_status();
                self.clock = system::clock();
                self.today = system::today();
                if self.gfx.lost() {
                    self.rebuild_graphics();
                }
                if std::mem::take(&mut self.replace_due) {
                    self.place(true);
                }
                // ben.ini edited by hand: apply it without a restart.
                let stamp = config::config_modified();
                if stamp != self.cfg_stamp {
                    self.cfg_stamp = stamp;
                    let cfg = Config::load();
                    if cfg != self.cfg {
                        crate::log!("ben.ini changed; applying");
                        settings::sync(&cfg);
                        self.apply_config(cfg, true);
                    }
                }
                self.follow_wifi();
                // A page that is open keeps up with what it shows.
                for page in [Page::Wifi, Page::Sound, Page::Battery] {
                    self.refresh_pages(page);
                }
                self.housekeeping();
                self.note_stats();
            }
            Event::Shell(code, lparam) => {
                // A window was created (1), destroyed (2), activated (4, 0x8004), minimised
                // or restored (5), replaced (13, 14), moved to another monitor (16) or is
                // flashing. The rest, above all the title changes some apps make many times
                // a second (6), does not concern the docks.
                if matches!(code, 1 | 2 | 4 | 5 | 13 | 14 | 16 | 0x8004 | HSHELL_FLASH) {
                    self.refresh_due = true;
                }
                if code == HSHELL_FLASH && self.cfg.bounce {
                    // A window is asking for attention: hop three times, unless it is already in front.
                    let front = unsafe { GetForegroundWindow() };
                    if let Some((s, index)) = self.owner_of(HWND(lparam as *mut _)) {
                        let item = &mut self.screens[s].dock.items[index];
                        if !item.windows.contains(&front) {
                            item.hop(Some(3));
                        }
                    }
                }
            }
            Event::AppBar(code) => {
                // Another bar on this edge (usually the taskbar) changed. Re-placing is done on the
                // next timer tick, not here, so a burst of notifications costs one re-place.
                if code as u32 == ABN_POSCHANGED {
                    self.replace_due = true;
                }
            }
            Event::Relayout => self.relayout_if_changed(),
            Event::Theme => self.retheme(false),
            Event::TaskbarCreated => {
                // Explorer restarted: everything registered with it is gone.
                crate::log!("Explorer restarted; hiding the taskbar and reserving space again");
                if self.taskbar.hidden {
                    self.taskbar.hide();
                }
                for screen in &mut self.screens {
                    screen.appbar = AppBar::new(screen.dock.hwnd());
                    if self.cfg.reserve_space {
                        screen.appbar.register(WM_APPBAR_CALLBACK);
                    }
                }
                unsafe {
                    let _ = RegisterShellHookWindow(self.hwnd);
                }
                self.place(true);
            }
            Event::Minimized(id) => {
                let window = HWND(id as *mut _);
                // Unless the app started this minimise itself (then its own effect is playing).
                if self.fx.as_ref().and_then(|f| f.window()) != Some(window) {
                    if system::window_animations_on() {
                        // Windows is flying the window to its icon, which swallows it as it lands.
                        self.swallow(window, 0.11, 1.0);
                    } else {
                        // Windows' own animation is switched off: the app's effect is all there is.
                        self.play_effect(window, false);
                    }
                }
                self.refresh_due = true;
            }
            Event::Restored(id) => {
                let window = HWND(id as *mut _);
                if self.fx.as_ref().and_then(|f| f.window()) != Some(window) {
                    // The icon lets the window go.
                    self.swallow(window, 0.0, -0.6);
                }
                self.refresh_due = true;
            }
            Event::WindowsChanged => self.refresh_due = true,
            Event::Moved(id) => {
                // From here on the window's size is the user's business.
                self.newcomers.remove(&id);
                self.refresh_due = true;
            }
            // Nothing to do but look again, which every event leads to.
            Event::PreviewHover | Event::Wake => {}
            Event::PreviewClick(middle) => {
                let mut pt = POINT::default();
                let target = unsafe { GetCursorPos(&mut pt) }.ok().and_then(|_| self.preview.as_ref().and_then(|p| p.window_at(pt)));
                if let Some(window) = target {
                    if middle {
                        shell::close(window);
                    } else {
                        self.close_preview();
                        self.bring(window);
                    }
                    self.refresh_due = true;
                }
            }
            Event::Volume | Event::Power => self.read_status(),
            Event::Media => self.take_media(),
            Event::Apps => self.take_found_apps(),
            Event::PageData => {
                if let Some(read) = self.bluetooth_reading.as_ref().and_then(|rx| rx.try_recv().ok()) {
                    self.bluetooth = Some(read);
                    self.bluetooth_reading = None;
                    self.refresh_pages(Page::Bluetooth);
                }
            }
            Event::Island(window, input) => self.island_input(window, input),
            Event::Bubble(input) => {
                let outcome = self.bubble.as_mut().map(|b| b.handle(input)).unwrap_or(bubble::Outcome::Nothing);
                match outcome {
                    bubble::Outcome::Nothing => {}
                    bubble::Outcome::Closed => self.close_bubble(),
                    bubble::Outcome::Chosen(id) => {
                        let item = self.menu_item.clone();
                        self.close_bubble();
                        self.run_command(id, item);
                    }
                }
            }
            Event::Launcher(input) => {
                let chosen = self.launcher.as_mut().and_then(|l| l.handle(input));
                if let Some(target) = chosen {
                    shell::launch_soon(&target);
                    self.refresh_due = true;
                }
                self.carry_from_tray(input);
            }
            Event::Config(done) => {
                if let Some(cfg) = settings::current() {
                    self.apply_config(cfg, done);
                    if done {
                        // The settings window has just written the file itself.
                        self.cfg_stamp = config::config_modified();
                    }
                }
            }
            Event::Quit => self.quit = true,
        }
    }

    fn busy(&mut self) {
        self.last_busy = Instant::now();
        self.rested = false;
    }

    /// Wakes the main loop once, `seconds` from now. While nothing moves the loop sleeps; this
    /// is how something that only has to *wait* (for the cursor to have rested long enough)
    /// gets looked at again without keeping the loop turning in the meantime.
    fn wake_in(&self, seconds: f32) {
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_WAKE, (seconds.max(0.01) * 1000.0) as u32, None);
        }
    }

    /// Makes every island tell about something for a moment.
    fn alert(&mut self, alert: Alert) {
        if !self.cfg.island_alerts {
            return;
        }
        for screen in &mut self.screens {
            if let Some(island) = screen.island.as_mut() {
                island.alert(alert.clone());
            }
        }
    }

    /// Makes the icon of a window's app gulp, `delay` seconds from now: it takes the window in
    /// (`strength` above zero) or lets it go (below zero).
    fn swallow(&mut self, window: HWND, delay: f32, strength: f32) {
        if let Some((s, index)) = self.owner_of(window) {
            self.screens[s].dock.items[index].gulp(delay, strength);
        }
    }

    /// Tells the window procedure where each open window's icon is, for when Windows asks
    /// where a window goes as it is minimised.
    fn publish_min_rects(&self) {
        let mut rects = HashMap::new();
        for screen in &self.screens {
            screen.dock.min_rects(&mut rects);
        }
        MIN_RECTS.with(|table| *table.borrow_mut() = rects);
    }

    /// Reads battery, volume and network, and lets the islands tell about what changed.
    fn read_status(&mut self) {
        let now = self.status.read();
        let old = std::mem::replace(&mut self.last_status, now);
        if now.volume != old.volume {
            if let (Some((level, muted)), Some(_)) = (now.volume, old.volume) {
                self.alert(Alert::Volume { level, muted });
            }
            self.refresh_pages(Page::Sound);
        }
        match (old.battery, now.battery) {
            (Some((_, was)), Some((percent, plugged))) if was != plugged => self.alert(Alert::Power { percent, plugged }),
            // Told once on the way down past 20% and once past 10%.
            (Some((before, _)), Some((percent, false))) if (percent <= 20 && before > 20) || (percent <= 10 && before > 10) => self.alert(Alert::LowBattery(percent)),
            _ => {}
        }
        if now.online != old.online {
            self.alert(Alert::Network(now.online));
        }
    }

    /// Takes what the now-playing watcher has sent; a new track gets a moment on the islands.
    fn take_media(&mut self) {
        let Some(latest) = self.media.as_ref().and_then(|m| m.take()) else { return };
        if let Some(now) = &latest {
            let new_track = self.playing.as_ref().map(|old| old.title != now.title).unwrap_or(true);
            if new_track && now.playing && !now.title.is_empty() {
                self.alert(Alert::Track { title: now.title.clone(), artist: now.artist.clone() });
            }
        }
        self.playing = latest;
    }

    fn take_found_apps(&mut self) {
        if let (Some(scan), Some(launcher)) = (self.scan.as_ref(), self.launcher.as_mut()) {
            launcher.absorb(scan.found.try_iter(), &self.motion);
        }
    }

    // ----------------------------------------------------------- the island

    /// A click, a key, a roll of the wheel or a timer on an island.
    fn island_input(&mut self, window: isize, input: island::Input) {
        let Some(s) = self.screen_of_island(window) else { return };
        let action = match self.screens[s].island.as_mut() {
            Some(island) => island.handle(input),
            None => return,
        };
        let collapse = |app: &mut App| {
            if let Some(island) = app.screens[s].island.as_mut() {
                island.collapse();
            }
        };
        match action {
            Action::None => {}
            Action::Previous => self.send_media(media::Command::Previous),
            Action::PlayPause => self.send_media(media::Command::Toggle),
            Action::Next => self.send_media(media::Command::Next),
            Action::Volume(level) => {
                self.status.set_volume(level);
                if level > 0.0 && self.last_status.volume.map(|(_, muted)| muted).unwrap_or(false) {
                    self.status.set_muted(false);
                }
                self.volume_set(s);
            }
            Action::Mute => self.toggle_mute(s),
            Action::Settings => {
                collapse(self);
                settings::open(&self.cfg);
            }
            Action::Lock => {
                collapse(self);
                system::lock_screen();
            }
            Action::Sleep => {
                collapse(self);
                system::sleep();
            }
            Action::Power => {
                let Some((center, bottom)) = self.screens[s].island.as_ref().and_then(|i| i.power_anchor()) else { return };
                let gap = (6.0 * self.screens[s].monitor.scale()).round() as i32;
                if !self.open_bubble(s, power_menu(), bubble::Anchor::Below { center, top: bottom + gap }, None) {
                    self.show_native_menu(s, None, true);
                }
            }
            Action::Open(page) => self.open_page(s, page),
            Action::Row(page, id) => self.page_row(s, page, id),
            Action::Enter(Page::Wifi) => self.wifi_row(s, row::JOIN),
            Action::Enter(_) => {}
        }
    }

    fn send_media(&self, command: media::Command) {
        if let Some(media) = self.media.as_ref() {
            media.send(command);
        }
    }

    fn toggle_mute(&mut self, s: usize) {
        let muted = self.last_status.volume.map(|(_, muted)| muted).unwrap_or(false);
        self.status.set_muted(!muted);
        self.volume_set(s);
    }

    /// The volume was just set from the island of screen `s`: show it at once. (When Windows
    /// reports the change a moment later there is nothing new to tell.)
    fn volume_set(&mut self, s: usize) {
        self.last_status = self.status.read();
        if let (Some(island), Some((level, muted))) = (self.screens[s].island.as_mut(), self.last_status.volume) {
            if !island.is_open() && self.cfg.island_alerts {
                island.alert(Alert::Volume { level, muted });
            }
        }
        self.refresh_pages(Page::Sound);
    }

    /// What a card's page says right now: its title and its rows.
    fn page_rows(&mut self, page: Page) -> Option<(&'static str, Vec<Row>)> {
        match page {
            Page::Wifi => Some(("Wi-Fi", self.wifi_rows())),
            Page::Sound => Some(("Âm thanh", self.sound_rows())),
            Page::Bluetooth => Some(("Bluetooth", self.bluetooth_rows())),
            Page::Battery => Some(("Pin", self.battery_rows())),
            Page::Home | Page::Calendar => None,
        }
    }

    /// A card on the island of screen `s` was clicked: fill its page and show it.
    fn open_page(&mut self, s: usize, page: Page) {
        match page {
            Page::Wifi => {
                // Opened afresh: no half-typed password, and look for networks again.
                self.wifi_asking = None;
                self.wifi_note = None;
                wifi::refresh();
            }
            Page::Bluetooth => self.read_bluetooth(None),
            _ => {}
        }
        let Some((title, rows)) = self.page_rows(page) else { return };
        if let Some(island) = self.screens[s].island.as_mut() {
            island.show_page(page, title, rows);
        }
    }

    /// Hands new rows to every island that is showing `page`.
    fn refresh_pages(&mut self, page: Page) {
        let showing: Vec<usize> = self.screens.iter().enumerate().filter(|(_, screen)| screen.island.as_ref().and_then(|i| i.page()) == Some(page)).map(|(s, _)| s).collect();
        if showing.is_empty() {
            return;
        }
        let Some((title, rows)) = self.page_rows(page) else { return };
        for s in showing {
            if let Some(island) = self.screens[s].island.as_mut() {
                island.show_page(page, title, rows.clone());
            }
        }
    }

    /// A row of a page on the island of screen `s` was clicked.
    fn page_row(&mut self, s: usize, page: Page, id: usize) {
        let settings_page = |app: &mut App, uri: &str| {
            if let Some(island) = app.screens[s].island.as_mut() {
                island.collapse();
            }
            shell::launch_soon(uri);
        };
        match (page, id) {
            (Page::Wifi, row::SETTINGS) => settings_page(self, "ms-settings:network-wifi"),
            (Page::Wifi, row::LOCATION) => settings_page(self, "ms-settings:privacy-location"),
            (Page::Wifi, id) => self.wifi_row(s, id),
            (Page::Sound, row::SETTINGS) => settings_page(self, "ms-settings:sound"),
            (Page::Sound, row::MUTE) => self.toggle_mute(s),
            (Page::Sound, id) if id >= row::LISTED => {
                if let Some(output) = self.outputs.get(id - row::LISTED) {
                    if !output.is_default && !audio::set_default_output(&output.id) {
                        crate::log!("Windows did not switch the playback device");
                    }
                    // The volume now read is the new device's.
                    self.status.watch_volume(self.hwnd, WM_VOLUME_CHANGED);
                    self.last_status = self.status.read();
                }
                self.refresh_pages(Page::Sound);
            }
            (Page::Bluetooth, row::SETTINGS) => settings_page(self, "ms-settings:bluetooth"),
            (Page::Bluetooth, row::SWITCH) => {
                let on = self.bluetooth.as_ref().map(|b| b.on).unwrap_or(false);
                self.read_bluetooth(Some(!on));
                self.refresh_pages(Page::Bluetooth);
            }
            (Page::Battery, row::SETTINGS) => settings_page(self, "ms-settings:powersleep"),
            _ => {}
        }
    }

    /// The Wi-Fi page: the networks in range, or the question for a network's password.
    fn wifi_rows(&mut self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(name) = self.wifi_asking.clone() {
            rows.push(Row::note(&self.wifi_note.clone().unwrap_or_else(|| format!("Nhập mật khẩu của “{name}”"))));
            rows.push(Row::field(row::PASSWORD, "Mật khẩu"));
            rows.push(Row::item(row::JOIN, 0xE8FB, "Kết nối").hint("Enter"));
            rows.push(Row::item(row::CANCEL, 0xE711, "Huỷ"));
            return rows;
        }
        let scan = wifi::scan();
        self.wifi_list = match &scan {
            wifi::Scan::Found(list) => list.clone(),
            _ => Vec::new(),
        };
        if let Some(joining) = &self.wifi_joining {
            rows.push(Row::note(&format!("Đang kết nối “{}”…", joining.name)));
        } else if let Some(note) = &self.wifi_note {
            rows.push(Row::note(note));
        }
        match &scan {
            wifi::Scan::Found(list) => {
                if list.is_empty() {
                    rows.push(Row::note("Không thấy mạng nào quanh đây"));
                }
                for (index, network) in list.iter().enumerate().take(7) {
                    let bars = match network.signal {
                        0..=29 => 0xE872,
                        30..=54 => 0xE873,
                        55..=79 => 0xE874,
                        _ => 0xE701,
                    };
                    let mut line = Row::item(row::LISTED + index, bars, &network.name);
                    if network.connected {
                        line = line.hint("Đã kết nối").mark(TICK);
                    } else if network.secure {
                        line = line.mark(LOCK);
                    }
                    rows.push(line);
                }
                rows.push(Row::item(row::REFRESH, 0xE72C, "Tìm lại mạng"));
            }
            wifi::Scan::NoAdapter => {
                rows.push(Row::note("Máy này không có Wi-Fi"));
                rows.push(Row::note(if self.last_status.online { "Đang có Internet qua dây mạng" } else { "Không có Internet" }));
            }
            wifi::Scan::Denied => {
                rows.push(Row::note("Windows chỉ cho xem danh sách Wi-Fi khi Vị trí đang bật"));
                rows.push(Row::item(row::LOCATION, 0xE707, "Mở cài đặt Vị trí…"));
            }
            wifi::Scan::Failed(code) => rows.push(Row::note(&format!("Không đọc được danh sách Wi-Fi (lỗi {code})"))),
        }
        rows.push(Row::item(row::SETTINGS, GEAR, "Cài đặt mạng của Windows…"));
        rows
    }

    /// A click in the Wi-Fi page: join a network, ask for its password, or look again.
    fn wifi_row(&mut self, s: usize, id: usize) {
        let begin = |result: std::result::Result<(), u32>, name: &str, saved_now: bool| -> (Option<Joining>, Option<String>) {
            match result {
                Ok(()) => (Some(Joining { name: name.to_string(), saved_now, since: Instant::now() }), None),
                Err(code) => {
                    crate::log!("Wi-Fi: Windows refused to start joining a network (code {code})");
                    (None, Some(format!("Windows từ chối nối “{name}” (mã {code})")))
                }
            }
        };
        match id {
            row::CANCEL => {
                self.wifi_asking = None;
                self.wifi_note = None;
            }
            row::REFRESH => wifi::refresh(),
            row::JOIN => {
                let password = self.screens[s].island.as_ref().map(|i| i.typed().to_string()).unwrap_or_default();
                let network = self.wifi_asking.as_ref().and_then(|name| self.wifi_list.iter().find(|n| &n.name == name)).cloned();
                if let Some(network) = network {
                    if password.chars().count() < 8 {
                        self.wifi_note = Some("Mật khẩu Wi-Fi có ít nhất 8 ký tự".to_string());
                    } else {
                        // The password goes straight to Windows' Wi-Fi service; it is neither kept nor logged here.
                        let (joining, note) = begin(wifi::join_new(&network, &password), &network.name, true);
                        if joining.is_some() {
                            self.wifi_asking = None;
                        }
                        (self.wifi_joining, self.wifi_note) = (joining, note);
                    }
                }
            }
            id if id >= row::LISTED => {
                let Some(network) = self.wifi_list.get(id - row::LISTED).cloned() else { return };
                self.wifi_asking = None;
                if network.connected {
                    // Already on it: nothing to do.
                } else if network.known {
                    (self.wifi_joining, self.wifi_note) = begin(wifi::join_saved(&network), &network.name, false);
                } else if !network.secure {
                    (self.wifi_joining, self.wifi_note) = begin(wifi::join_new(&network, ""), &network.name, true);
                } else if network.takes_password() {
                    self.wifi_asking = Some(network.name.clone());
                    self.wifi_note = None;
                } else {
                    self.wifi_note = Some(format!("“{}” đòi đăng nhập bằng tài khoản, Bến chưa nối được loại mạng này", network.name));
                }
            }
            _ => {}
        }
        self.refresh_pages(Page::Wifi);
    }

    /// Follows a join in progress: done once the network shows as connected, given up after
    /// fifteen seconds (and a network saved just for this attempt is removed again).
    fn follow_wifi(&mut self) {
        let Some(joining) = self.wifi_joining.as_ref() else { return };
        let connected = matches!(wifi::scan(), wifi::Scan::Found(list) if list.iter().any(|n| n.connected && n.name == joining.name));
        if connected {
            crate::log!("Wi-Fi: joined a network after {:.1} s", joining.since.elapsed().as_secs_f32());
            self.wifi_joining = None;
            self.wifi_note = None;
        } else if joining.since.elapsed().as_secs_f32() > 15.0 {
            crate::log!("Wi-Fi: gave up joining a network after 15 s (saved for this attempt: {})", joining.saved_now);
            let name = joining.name.clone();
            if joining.saved_now {
                wifi::forget(&name);
                // Most likely the password: ask for it again.
                self.wifi_asking = Some(name.clone());
                self.wifi_note = Some(format!("Không nối được “{name}”. Kiểm tra lại mật khẩu"));
            } else {
                self.wifi_note = Some(format!("Không nối được “{name}”"));
            }
            self.wifi_joining = None;
        } else {
            return;
        }
        self.last_status = self.status.read();
    }

    /// The sound page: mute, and which device Windows plays through.
    fn sound_rows(&mut self) -> Vec<Row> {
        self.outputs = audio::outputs();
        let muted = self.last_status.volume.map(|(_, muted)| muted).unwrap_or(false);
        let mut rows = vec![Row::switch(row::MUTE, 0xE74F, "Tắt tiếng", muted), Row::note("Phát qua")];
        for (index, output) in self.outputs.iter().enumerate().take(7) {
            let line = Row::item(row::LISTED + index, 0xE7F5, &output.name);
            rows.push(if output.is_default { line.mark(TICK) } else { line });
        }
        if self.outputs.is_empty() {
            rows.push(Row::note("Không có thiết bị phát nào"));
        }
        rows.push(Row::item(row::SETTINGS, GEAR, "Cài đặt âm thanh của Windows…"));
        rows
    }

    /// The Bluetooth page: the switch and the paired devices.
    fn bluetooth_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        match &self.bluetooth {
            None => rows.push(Row::note("Đang đọc…")),
            Some(state) if !state.available => rows.push(Row::note("Máy này không có Bluetooth")),
            Some(state) => {
                let waiting = self.bluetooth_reading.is_some();
                rows.push(Row::switch(row::SWITCH, 0xE702, if waiting { "Bluetooth (đang đổi…)" } else { "Bluetooth" }, state.on));
                if !state.devices.is_empty() {
                    rows.push(Row::note("Thiết bị đã ghép đôi"));
                }
                for device in state.devices.iter().take(6) {
                    let line = Row::item(0, 0xE702, &device.name);
                    rows.push(if device.connected { line.hint("Đã kết nối").mark(TICK) } else { line });
                }
            }
        }
        rows.push(Row::item(row::SETTINGS, GEAR, "Cài đặt Bluetooth của Windows…"));
        rows
    }

    /// Reads Bluetooth on a thread of its own (it takes Windows a moment), after switching the
    /// radio if `switch` says so. The answer comes back as `Event::PageData`.
    fn read_bluetooth(&mut self, switch: Option<bool>) {
        if self.bluetooth_reading.is_some() {
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let window = self.hwnd.0 as isize;
        let spawned = std::thread::Builder::new().name("bluetooth".into()).spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            if let Some(on) = switch {
                bluetooth::set_on(on);
            }
            let _ = tx.send(bluetooth::read());
            unsafe {
                let _ = PostMessageW(Some(HWND(window as *mut _)), WM_PAGE_DATA, WPARAM(0), LPARAM(0));
            }
        });
        match spawned {
            Ok(_) => self.bluetooth_reading = Some(rx),
            Err(e) => crate::log!("could not start reading Bluetooth: {e}"),
        }
    }

    /// The battery page: how full it is and what the computer is running on.
    fn battery_rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        match system::battery_detail() {
            Some(b) => {
                let source = if b.charging {
                    "Đang sạc"
                } else if b.plugged {
                    "Đang cắm sạc"
                } else {
                    "Đang dùng pin"
                };
                rows.push(Row::item(0, island::battery_glyph(b.percent, b.plugged), &format!("Pin {}%", b.percent)).hint(source));
                if let Some(minutes) = b.minutes_left.filter(|_| !b.plugged) {
                    let (h, m) = (minutes / 60, minutes % 60);
                    rows.push(Row::note(&if h > 0 { format!("Còn khoảng {h} giờ {m} phút") } else { format!("Còn khoảng {m} phút") }));
                }
            }
            None => rows.push(Row::note("Máy này không có pin")),
        }
        rows.push(Row::item(row::SETTINGS, GEAR, "Cài đặt pin của Windows…"));
        rows
    }

    // ------------------------------------------------------------- the docks

    /// The pinned apps changed on screen `from`: the other screens' docks follow.
    fn share_pins(&mut self, from: usize) {
        let pins = config::load_pins().unwrap_or_default();
        for (s, screen) in self.screens.iter_mut().enumerate() {
            if s != from {
                screen.dock.adopt_pins(&pins, &self.motion);
            }
        }
        self.refresh_due = true;
    }

    /// Follows an app's tile as it is dragged out of the tray: over a dock it joins the dock's
    /// icons, and let go there it is pinned.
    fn carry_from_tray(&mut self, input: launcher::Input) {
        let Some(launcher) = self.launcher.as_mut() else { return };
        let let_go = matches!(input, launcher::Input::Click(..));
        let carried = if let_go { launcher.take_carried() } else { launcher.carried() };
        let mut pt = POINT::default();
        let have_cursor = unsafe { GetCursorPos(&mut pt) }.is_ok();
        // Over a dock: on its glass, or a little above it, where the swollen icons reach.
        let over = carried.as_ref().filter(|_| have_cursor).and_then(|_| {
            self.screens.iter().position(|screen| {
                let glass = screen.dock.panel_on_screen();
                let reach = screen.dock.m.icon as i32;
                pt.x >= glass.left - reach && pt.x < glass.right + reach && pt.y >= glass.top - reach && pt.y < screen.monitor.rect.bottom
            })
        });
        if let Some(s) = self.tray_drag.filter(|&s| Some(s) != over) {
            if let Some(screen) = self.screens.get_mut(s) {
                screen.dock.drag_out();
            }
            self.tray_drag = None;
            self.refresh_due = true;
        }
        let (Some(target), Some(s)) = (carried, over) else { return };
        let dock = &mut self.screens[s].dock;
        if let_go {
            self.tray_drag = None;
            if dock.drop_in(&[target], &self.motion) {
                self.share_pins(s);
            }
        } else {
            dock.drag_in(&[target], (pt.x - dock.win_x) as f32, &self.motion);
            self.tray_drag = Some(s);
        }
    }

    fn click(&mut self, s: usize, index: usize) {
        if self.screens[s].dock.items[index].key == LAUNCHER_KEY {
            self.toggle_launcher(s);
            return;
        }
        let front = unsafe { GetForegroundWindow() };
        let item = &self.screens[s].dock.items[index];
        if item.windows.is_empty() {
            self.launch(s, index);
        } else if item.windows.contains(&front) && !shell::is_minimized(front) {
            // (A window can be minimised and still count as the one in front; that one is
            // brought back like any other.)
            if item.windows.len() > 1 {
                // Already in front: step through this app's other windows, oldest first.
                let next = *item.windows.last().unwrap();
                self.bring(next);
            } else if self.cfg.click_minimize {
                // Start the effect first so the window's picture is up before the window goes.
                self.play_effect(front, false);
                shell::minimize(front);
            }
        } else {
            let first = item.windows[0];
            self.bring(first);
        }
        self.refresh_due = true;
    }

    /// Brings a window to the front. A minimised one flies out of its icon first.
    fn bring(&mut self, window: HWND) {
        if self.fx.as_ref().and_then(|f| f.window()) == Some(window) {
            // Already on its way.
            return;
        }
        if shell::is_minimized(window) && self.play_effect(window, true) {
            // The window itself is restored when the effect ends (see `update`).
            return;
        }
        shell::activate(window);
    }

    /// Plays the app's own minimise effect for a window (or the reverse, for restoring): it
    /// flows into its icon in the dock of the screen it is on. Returns false when the choice is
    /// Windows' own animation, or the effect cannot be played, so the caller carries on
    /// without it (and Windows animates the window to the same icon).
    fn play_effect(&mut self, window: HWND, restoring: bool) -> bool {
        let style = match self.cfg.minimize_effect.round() as i32 {
            1 => fx::Style::Shrink,
            2 => fx::Style::Genie,
            3 => fx::Style::Drop,
            _ => return false,
        };
        let what = if restoring { "restore" } else { "minimise" };
        let id = window.0 as isize;
        let Some((s, index)) = self.owner_of(window) else {
            crate::log!("effect ({what}) skipped: window {id:#x} is not one of the docks' apps");
            return false;
        };
        let place = match self.rects.get(&id).copied() {
            Some(r) => Some(r),
            // Minimised since before the app started: ask Windows where it will come back.
            None if restoring => shell::restored_rect(window).map(|r| (r, r)),
            // Not seen yet (it opened a moment ago): where it is right now, if it still is anywhere.
            None => shell::window_rect(window).filter(|r| r.left > -30000).map(|r| (r, shell::frame_rect(window).unwrap_or(r))),
        };
        let Some((whole, from)) = place else {
            crate::log!("effect ({what}) skipped: no known position for window {id:#x}");
            return false;
        };
        // The effect plays on the window's own screen; a window that hangs over onto another
        // screen flies from the part that is on this one.
        let mon = self.screens[s].monitor.rect;
        if from.right <= mon.left || from.left >= mon.right || from.bottom <= mon.top || from.top >= mon.bottom {
            crate::log!("effect ({what}) skipped: window {id:#x} is not on the screen of its dock ({from:?})");
            return false;
        }
        // The icon at its resting size, in screen pixels.
        let dock = &self.screens[s].dock;
        let (cx, bottom) = dock.icon_anchor(index);
        let (cx, bottom, icon) = (cx as f32, bottom as f32, dock.m.icon);
        let to = crate::warp::R { l: cx - icon * 0.5, t: bottom - icon, r: cx + icon * 0.5, b: bottom };
        let duration = self.cfg.minimize_time;
        self.fx_release = None;
        let started = match self.fx.as_mut() {
            Some(fx) => fx.start(window, whole, from, to, mon, style, restoring, duration),
            None => false,
        };
        let item = &mut self.screens[s].dock.items[index];
        if started {
            // The icon swallows the window as it lands, or lets it go as it leaves.
            if restoring {
                item.gulp(0.0, -0.6);
            } else {
                item.gulp(duration * 0.62, 1.0);
            }
        }
        if !started {
            crate::log!("effect ({what}, {style:?}) for '{}' could not start", item.name);
        }
        started
    }

    fn close_preview(&mut self) {
        if let Some(p) = self.preview.as_mut() {
            p.hide();
        }
        self.resting_on = None;
        self.preview_left = None;
    }

    /// Opens (or rebuilds) the window preview for an item of screen `s`'s dock.
    fn open_preview(&mut self, s: usize, index: usize) {
        let screen = &self.screens[s];
        let Some(item) = screen.dock.items.get(index) else { return };
        if item.windows.is_empty() {
            self.close_preview();
            return;
        }
        let layout = preview::Layout { anchor_x: screen.dock.icon_anchor(index).0, bottom_y: screen.dock.top_line(), monitor: screen.monitor.rect, scale: screen.monitor.scale(), size: self.cfg.preview_size };
        let (key, windows) = (item.key.clone(), item.windows.clone());
        if let Some(p) = self.preview.as_mut() {
            if let Err(e) = p.show(&self.scene, &self.gfx, &screen.theme, &key, &windows, &layout) {
                crate::log!("could not show the window preview: {e}");
                p.hide_now();
            }
        }
        self.preview_screen = s;
    }

    /// Decides whether the preview should open, stay or close.
    fn update_preview(&mut self, cursor: POINT) {
        if self.preview.is_none() {
            return;
        }
        if !self.cfg.preview {
            self.close_preview();
            return;
        }
        // The running app under the cursor, if any.
        let over = self.screens.iter().enumerate().find_map(|(s, screen)| screen.dock.hovered.filter(|&i| screen.dock.items.get(i).map(|it| it.is_app() && !it.windows.is_empty()).unwrap_or(false)).map(|i| (s, i)));
        let over_key = over.map(|(s, i)| (s, self.screens[s].dock.items[i].key.clone()));
        let open_key = self.preview.as_ref().and_then(|p| p.key().map(str::to_string)).map(|key| (self.preview_screen, key));

        match (&open_key, &over_key) {
            (Some(open), Some(key)) if open != key => {
                // Slid over to another running app: switch at once.
                let (s, i) = over.unwrap();
                self.open_preview(s, i);
                self.preview_left = None;
            }
            (Some(_), Some(_)) => self.preview_left = None,
            (Some(_), None) => {
                // Off the icon: stay open while the cursor is on the panel, or on its way there.
                let slack = (12.0 * self.screens[self.preview_screen.min(self.screens.len() - 1)].monitor.scale()) as i32;
                let on_panel = self.preview.as_ref().map(|p| p.contains(cursor, slack)).unwrap_or(false);
                if on_panel {
                    self.preview_left = None;
                } else {
                    match self.preview_left {
                        Some(left) if left.elapsed().as_secs_f32() > 0.3 => self.close_preview(),
                        Some(_) => {}
                        None => {
                            self.preview_left = Some(Instant::now());
                            self.wake_in(0.32);
                        }
                    }
                }
            }
            (None, Some((s, key))) => {
                let resting = match &self.resting_on {
                    Some((at, k, since)) if at == s && k == key => since.elapsed().as_secs_f32(),
                    _ => {
                        self.resting_on = Some((*s, key.clone(), Instant::now()));
                        self.wake_in(self.cfg.preview_delay + 0.02);
                        0.0
                    }
                };
                let busy = self.screens[*s].dock.pressed.is_some() || self.fx.as_ref().map(|f| f.is_playing()).unwrap_or(false) || self.bubble.as_ref().map(|b| b.is_open()).unwrap_or(false);
                let blocked = self.preview_block.as_ref() == Some(key);
                if !blocked {
                    self.preview_block = None;
                }
                if resting >= self.cfg.preview_delay && !busy && !blocked {
                    let (s, i) = over.unwrap();
                    self.open_preview(s, i);
                }
            }
            (None, None) => {
                self.resting_on = None;
                self.preview_block = None;
            }
        }
    }

    fn launch(&mut self, s: usize, index: usize) {
        if self.screens[s].dock.items[index].is_special() {
            self.toggle_launcher(s);
            return;
        }
        // The shell can take a second to start an app; the icon starts hopping at once.
        let item = &mut self.screens[s].dock.items[index];
        shell::launch_soon(&item.target);
        if self.cfg.bounce && item.windows.is_empty() {
            item.hop(None);
        }
    }

    /// The menu of a dock icon on screen `s`, or of the dock itself (`index` = None).
    fn show_dock_menu(&mut self, s: usize, index: Option<usize>) {
        let dock = &self.screens[s].dock;
        // The tray button is not an app: it only gets the dock's own menu.
        let index = index.filter(|&i| !dock.items[i].is_special());
        let divider = index.map(|i| dock.items[i].is_divider()).unwrap_or(false);
        let entries = dock_menu(index.filter(|_| !divider).map(|i| (dock.items[i].name.as_str(), dock.items[i].windows.len(), dock.items[i].pinned)), divider, !self.taskbar.hidden, system::autostart_enabled());
        // Standing on the icon at its largest, or at the cursor on the dock's own glass.
        let (center, bottom) = match index {
            Some(i) => (dock.icon_anchor(i).0, dock.top_line()),
            None => {
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                }
                (pt.x, dock.panel_on_screen().top - (8.0 * self.screens[s].monitor.scale()).round() as i32)
            }
        };
        let key = index.map(|i| dock.items[i].key.clone());
        if !self.open_bubble(s, entries, bubble::Anchor::Above { center, bottom }, key) {
            self.show_native_menu(s, index, false);
        }
    }

    /// Shows the app's own menu with these rows on screen `s`. Returns false if it could not be shown.
    fn open_bubble(&mut self, s: usize, entries: Vec<Entry>, anchor: bubble::Anchor, item: Option<String>) -> bool {
        self.close_preview();
        let screen = &self.screens[s];
        let Some(bubble) = self.bubble.as_mut() else { return false };
        if let Err(e) = bubble.open(&self.scene, &self.gfx, &screen.theme, entries, anchor, screen.monitor.rect, screen.monitor.scale()) {
            crate::log!("could not show the menu: {e}");
            bubble.close_now();
            return false;
        }
        self.bubble_screen = s;
        self.menu_item = item.map(|key| (s, key));
        true
    }

    fn close_bubble(&mut self) {
        if let Some(bubble) = self.bubble.as_mut() {
            bubble.close();
        }
        self.menu_item = None;
    }

    /// Carries out a menu command. `item` is the dock item the menu was opened on (its screen and key).
    fn run_command(&mut self, id: usize, item: Option<(usize, String)>) {
        let target = item.and_then(|(s, key)| self.screens.get(s).and_then(|screen| screen.dock.index_of(&key)).map(|i| (s, i)));
        match id {
            cmd::OPEN => {
                if let Some((s, i)) = target {
                    self.launch(s, i);
                }
            }
            cmd::PIN => {
                if let Some((s, i)) = target {
                    self.screens[s].dock.toggle_pin(i);
                    self.share_pins(s);
                }
            }
            cmd::DIVIDER => {
                if let Some((s, i)) = target {
                    self.screens[s].dock.add_divider(i, &self.motion);
                    self.share_pins(s);
                }
            }
            cmd::REMOVE_DIVIDER => {
                if let Some((s, i)) = target {
                    self.screens[s].dock.remove_divider(i);
                    self.share_pins(s);
                }
            }
            cmd::CLOSE => {
                if let Some((s, i)) = target {
                    for &w in &self.screens[s].dock.items[i].windows {
                        shell::close(w);
                    }
                }
            }
            cmd::TASKBAR => {
                let mut cfg = self.cfg.clone();
                cfg.hide_taskbar = !self.taskbar.hidden;
                cfg.save();
                self.cfg_stamp = config::config_modified();
                settings::sync(&cfg);
                self.apply_config(cfg, true);
            }
            cmd::AUTOSTART => {
                system::set_autostart(!system::autostart_enabled());
            }
            cmd::SETTINGS => settings::open(&self.cfg),
            cmd::QUIT => self.quit = true,
            cmd::WINDOWS_SETTINGS => {
                shell::launch_soon("ms-settings:");
            }
            cmd::RESTART => {
                if system::confirm("Khởi động lại máy ngay bây giờ?") {
                    system::run("shutdown.exe", "/r /t 0");
                }
            }
            cmd::SHUT_DOWN => {
                if system::confirm("Tắt máy ngay bây giờ?") {
                    system::run("shutdown.exe", "/s /t 0");
                }
            }
            cmd::SIGN_OUT => {
                if system::confirm("Đăng xuất ngay bây giờ?") {
                    system::run("shutdown.exe", "/l");
                }
            }
            _ => {}
        }
        self.refresh_due = true;
    }

    /// Windows' own menu, for when the app's menu cannot be made. `below`: open it downwards
    /// from the cursor (from the island), not upwards.
    fn show_native_menu(&mut self, s: usize, index: Option<usize>, below: bool) {
        let window = self.screens[s].dock.hwnd();
        let chosen = unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let add = |flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
                let _ = AppendMenuW(menu, flags, id, PCWSTR(wide(text).as_ptr()));
            };
            if index.map(|i| self.screens[s].dock.items[i].is_divider()).unwrap_or(false) {
                add(MF_STRING, cmd::REMOVE_DIVIDER, "Gỡ vách ngăn");
                add(MF_SEPARATOR, 0, "");
            } else if let Some(item) = index.map(|i| &self.screens[s].dock.items[i]) {
                add(MF_STRING | MF_GRAYED, 0, &item.name);
                add(MF_SEPARATOR, 0, "");
                add(MF_STRING, cmd::OPEN, if item.windows.is_empty() { "Mở" } else { "Mở cửa sổ mới" });
                add(MF_STRING, cmd::PIN, if item.pinned { "Gỡ khỏi dock" } else { "Giữ trong dock" });
                if item.pinned {
                    add(MF_STRING, cmd::DIVIDER, "Thêm vách ngăn phía sau");
                }
                if !item.windows.is_empty() {
                    add(MF_STRING, cmd::CLOSE, if item.windows.len() > 1 { "Đóng tất cả cửa sổ" } else { "Đóng cửa sổ" });
                }
                add(MF_SEPARATOR, 0, "");
            }
            let check = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
            add(MF_STRING, cmd::SETTINGS, "Cài đặt Bến…");
            add(check(!self.taskbar.hidden), cmd::TASKBAR, "Hiện taskbar của Windows");
            add(check(system::autostart_enabled()), cmd::AUTOSTART, "Khởi động cùng Windows");
            add(MF_SEPARATOR, 0, "");
            add(MF_STRING, cmd::QUIT, "Thoát Bến");

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // A popup menu only closes on an outside click if its owner is the foreground window.
            let _ = SetForegroundWindow(window);
            let align = if below { TPM_TOPALIGN } else { TPM_BOTTOMALIGN };
            let picked = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | align | TPM_LEFTALIGN | TPM_RIGHTBUTTON, pt.x, pt.y, None, window, None);
            let _ = PostMessageW(Some(window), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            picked.0 as usize
        };
        let item = index.map(|i| (s, self.screens[s].dock.items[i].key.clone()));
        self.run_command(chosen, item);
    }

    /// Opens the app tray on screen `s` (or closes it if it is open).
    fn toggle_launcher(&mut self, s: usize) {
        self.close_preview();
        self.close_bubble();
        if self.scan.is_none() && self.launcher.is_some() {
            // First time: start reading the list of installed apps. Names arrive within a
            // second; icons are read as their page is shown.
            self.scan = Some(shell::scan_apps(self.hwnd, WM_APPS_FOUND));
        }
        self.take_found_apps();
        let screen = &self.screens[s];
        let rect = screen.monitor.rect;
        let from_x = screen.dock.index_of(LAUNCHER_KEY).map(|i| screen.dock.icon_anchor(i).0).unwrap_or((rect.left + rect.right) / 2);
        let bottom = screen.dock.panel_on_screen().top - (10.0 * screen.monitor.scale()).round() as i32;
        let Some(launcher) = self.launcher.as_mut() else { return };
        if launcher.is_open() {
            launcher.close();
        } else if let Err(e) = launcher.open(&self.scene, &screen.theme, &self.motion, rect, screen.monitor.scale(), self.cfg.launcher_icon, bottom, from_x) {
            crate::log!("could not open the app tray: {e}");
        }
    }

    /// Switches to new settings while running. `done` is false while a slider is still moving.
    fn apply_config(&mut self, cfg: Config, done: bool) {
        let old = std::mem::replace(&mut self.cfg, cfg);
        let cfg = self.cfg.clone();

        if cfg.hide_taskbar != self.taskbar.hidden {
            if cfg.hide_taskbar {
                self.taskbar.hide();
            } else {
                self.taskbar.restore();
            }
        }
        if cfg.reserve_space != old.reserve_space {
            for screen in &mut self.screens {
                if cfg.reserve_space {
                    screen.appbar.register(WM_APPBAR_CALLBACK);
                } else {
                    screen.appbar.remove();
                }
            }
        }
        self.retheme(false);
        if cfg.preview_size != old.preview_size || !cfg.preview {
            self.close_preview();
        }
        if done || cfg.island != old.island || cfg.launcher != old.launcher || cfg.island_media != old.island_media {
            self.setup_surfaces();
        }
        if cfg.ram_limit != old.ram_limit && done {
            system::limit_memory(cfg.ram_limit.round().max(0.0) as u32);
        }

        // If sizes changed, lay the surfaces out again. The screen strips that other windows
        // must stay out of are only re-requested once the change is final.
        let resized = self.screens.iter().any(|screen| dock::metrics(&cfg, screen.monitor.scale(), screen.monitor.width()) != screen.dock.m);
        if resized || done {
            self.place(done);
        }
        self.refresh_due = true;
    }

    /// Works each screen's look out again and hands it to the surfaces if it changed.
    /// `force`: a screen's scale changed, so do it even if the colours are the same.
    fn retheme(&mut self, force: bool) {
        self.motion = Motion::new(self.cfg.anim_speed, self.cfg.elastic, system::reduce_motion());
        let (dark, accent) = (system::system_is_dark(), system::accent_color());
        let mut changed = false;
        for screen in &mut self.screens {
            let theme = theme::build(&self.cfg, dark, accent, screen.monitor.scale());
            if theme == screen.theme && !force {
                continue;
            }
            changed = true;
            screen.theme = theme;
            screen.dock.set_theme(&screen.theme);
            if let Some(island) = screen.island.as_mut() {
                island.set_theme(&screen.theme);
            }
        }
        if !changed {
            return;
        }
        let first = &self.screens[0].theme;
        if let Some(bubble) = self.bubble.as_mut() {
            bubble.set_theme(first);
        }
        if let Some(launcher) = self.launcher.as_mut() {
            launcher.set_theme(first);
        }
        if let Some(preview) = self.preview.as_mut() {
            preview.set_theme(first);
        }
        self.menu_item = None;
    }

    // ---------------------------------------------------------------- update

    /// Re-reads which apps have windows, on which screen, and which one is in front.
    fn refresh_windows(&mut self) {
        self.refresh_due = false;
        self.last_refresh = Instant::now();
        self.refreshes += 1;
        let windows = self.tracker.scan(self.hwnd);
        self.settle_newcomers(&windows);
        let real_front = unsafe { GetForegroundWindow() };
        // An open menu must not change which app counts as being in front; and if something
        // else came to the front without the menu being told, the menu goes.
        if let Some(open) = self.bubble.as_ref().filter(|b| b.is_open()).map(|b| b.hwnd()) {
            if real_front != open {
                crate::log!("menu closed: another window is in front");
                self.close_bubble();
            }
        }
        if !shell::is_own_window(real_front) {
            self.last_front = real_front;
        }
        let front = self.last_front;

        // Each dock gets the windows on its own screen (a window on a screen without a dock
        // goes to the first).
        let handles: Vec<isize> = self.screens.iter().map(|s| s.monitor.handle).collect();
        let mut shares: Vec<Vec<shell::AppWindow>> = vec![Vec::new(); self.screens.len()];
        for window in windows {
            let id = window.hwnd.0 as isize;
            // A minimised window stays with the dock of the screen it was last seen on: it
            // cannot move while it is away, and asking Windows mid-way gives odd answers.
            let parked = shell::is_parked(window.hwnd);
            let monitor = match self.homes.get(&id) {
                Some(home) if parked => *home,
                _ => shell::monitor_of(window.hwnd),
            };
            if !parked {
                self.homes.insert(id, monitor);
            }
            let at = handles.iter().position(|h| *h == monitor).unwrap_or(0);
            shares[at].push(window);
        }
        for (screen, share) in self.screens.iter_mut().zip(shares) {
            screen.dock.sync(share, front, &self.motion);
        }

        // Remember where every open window is, for the minimise effect.
        let mut seen = Vec::new();
        for screen in &self.screens {
            for item in &screen.dock.items {
                for &w in &item.windows {
                    let id = w.0 as isize;
                    seen.push(id);
                    if !shell::is_minimized(w) {
                        if let Some(whole) = shell::window_rect(w) {
                            self.rects.insert(id, (whole, shell::frame_rect(w).unwrap_or(whole)));
                        }
                    }
                }
            }
        }
        self.rects.retain(|id, _| seen.contains(id));
        self.homes.retain(|id, _| seen.contains(id));
        self.publish_min_rects();

        // If the previewed app's windows changed, rebuild the preview (or close it).
        let open = self.preview.as_ref().and_then(|p| p.key().map(|k| (k.to_string(), p.windows())));
        if let Some((key, shown)) = open {
            let s = self.preview_screen;
            match self.screens.get(s).and_then(|screen| screen.dock.index_of(&key)) {
                Some(index) if !self.screens[s].dock.items[index].windows.is_empty() => {
                    let now = &self.screens[s].dock.items[index].windows;
                    if shown.len() != now.len() || shown.iter().any(|w| !now.contains(w)) {
                        self.open_preview(s, index);
                    }
                }
                _ => self.close_preview(),
            }
        }

        // Step out of the way of games, videos and presentations, on the screen they fill.
        let filled = (self.cfg.hide_on_fullscreen && shell::is_fullscreen(real_front)).then(|| shell::monitor_of(real_front));
        let mut changed = false;
        for screen in &mut self.screens {
            let hidden = filled == Some(screen.monitor.handle);
            changed |= (screen.hide.target == 1.0) != hidden;
            screen.hide.set(if hidden { 1.0 } else { 0.0 });
            if hidden {
                if let Some(island) = screen.island.as_mut() {
                    island.collapse();
                }
            }
        }
        if changed {
            self.busy();
            if filled.is_some() {
                self.close_preview();
            }
        }
    }

    /// Notices the windows that have just opened and, if the settings say so, makes them
    /// float: not maximised, in the middle of their screen (see `shell::float_window`). Each
    /// window is only looked at for a moment after it opens; what the user does with it
    /// afterwards is left alone.
    fn settle_newcomers(&mut self, windows: &[shell::AppWindow]) {
        // Windows on other virtual desktops are not new when their desktop comes up.
        self.known.extend(self.tracker.elsewhere().iter().copied());
        for window in windows {
            let id = window.hwnd.0 as isize;
            if self.known.insert(id) && self.seeded {
                self.newcomers.insert(id, Newcomer { since: Instant::now(), times: 0, state: (false, RECT::default()), steady: Instant::now() });
            }
        }
        self.seeded = true;
        self.known.retain(|id| shell::exists(HWND(*id as *mut _)));
        if !self.cfg.float_windows {
            self.newcomers.clear();
        }
        if self.newcomers.is_empty() {
            return;
        }
        // Where the windows that are not new stand (a new one's own place does not count).
        let mut taken: Vec<(i32, i32)> = self.rects.iter().filter(|(id, _)| !self.newcomers.contains_key(*id)).map(|(_, (_, frame))| (frame.left, frame.top)).collect();
        let share = self.cfg.float_size;
        let mut look_again = false;
        self.newcomers.retain(|id, new| {
            let hwnd = HWND(*id as *mut _);
            if !shell::exists(hwnd) {
                return false;
            }
            if shell::is_minimized(hwnd) {
                // Opened minimised: its moment starts when it is first brought up.
                new.since = Instant::now();
                return true;
            }
            if new.since.elapsed().as_secs_f32() > NEWCOMER_FOR {
                return false;
            }
            look_again = true;
            // Only a window that has stood still for a moment is touched.
            let state = (shell::is_maximized(hwnd), shell::window_rect(hwnd).unwrap_or_default());
            if state != new.state {
                (new.state, new.steady) = (state, Instant::now());
                return true;
            }
            if new.steady.elapsed().as_secs_f32() < STEADY_FOR {
                return true;
            }
            if let Some(corner) = shell::float_window(hwnd, share, &taken) {
                taken.push(corner);
                new.times += 1;
            }
            new.times < 2
        });
        if look_again {
            // An app may still maximise its window a moment after showing it.
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_LOOK, LOOK_EVERY, None);
            }
        }
    }

    /// Advances all motion by `dt` seconds and updates every surface.
    /// Returns true while something is still moving (another frame is needed).
    pub fn update(&mut self, dt: f32) -> bool {
        self.run_demo();
        self.passes += 1;
        // Windows often reports several changes at once (an app opening three windows): read
        // them together, a few times a second at most.
        if self.refresh_due && self.last_refresh.elapsed().as_secs_f32() >= REFRESH_EVERY {
            self.refresh_windows();
        }

        // Cursor, read directly every frame so the docks track it with no lag.
        let mut pt = POINT::default();
        let have_cursor = unsafe { GetCursorPos(&mut pt) }.is_ok();
        let fake = self.bench_cursor().or(self.demo_cursor);

        let mut moving = false;
        let mut errors: Vec<windows::core::Error> = Vec::new();
        let previewing = self.preview.as_ref().map(|p| p.is_open()).unwrap_or(false);
        let info = island::Info { time: &self.clock.time, day: &self.clock.day, date: &self.clock.date, today: self.today, status: self.last_status, media: self.playing.as_ref() };
        for (s, screen) in self.screens.iter_mut().enumerate() {
            let mut cursor = have_cursor.then(|| ((pt.x - screen.dock.win_x) as f32, (pt.y - screen.dock.win_y) as f32));
            if s == 0 && fake.is_some() {
                cursor = fake;
            }
            // Coming back into view it bounces a little; leaving, it just goes.
            screen.hide.tune(if screen.hide.target == 0.0 { self.motion.medium() } else { self.motion.calm() });
            screen.hide.step(dt);
            moving |= !screen.hide.settled(1e-3);

            let count = screen.dock.count();
            let quiet = previewing && self.preview_screen == s;
            match screen.dock.update(&self.scene, &self.gfx, &self.cfg, &screen.theme, &self.motion, cursor, &screen.hide, quiet, dt) {
                Ok(more) => moving |= more,
                Err(e) => errors.push(e),
            }
            if screen.dock.count() != count {
                // An app left the dock: read the windows again.
                self.refresh_due = true;
            }
            if let Some(island) = screen.island.as_mut() {
                match island.update(&self.scene, &self.gfx, &self.cfg, &screen.theme, &self.motion, &info, &screen.hide, dt) {
                    Ok(more) => moving |= more,
                    Err(e) => errors.push(e),
                }
            }
        }

        // Window effects.
        let scene = &self.scene;
        if let Some(done) = self.fx.as_mut().and_then(|f| f.step(scene, dt)) {
            if done.restoring {
                // The picture has reached full size: bring the real window back underneath it,
                // then take the picture away a moment later.
                shell::activate(done.window);
                let ended = self.rects.get(&(done.window.0 as isize)).map(|(_, frame)| *frame);
                self.fx_release = Some(Release { window: done.window, left: 0.12, waited: 0.0, ended });
                self.refresh_due = true;
            }
        }
        if let Some(release) = self.fx_release.as_mut() {
            release.waited += dt;
            // The countdown only runs once the real window is back; a slow app gets up to 0.6 s.
            let back = !shell::is_minimized(release.window);
            if back {
                release.left -= dt;
            }
            if (back && release.left <= 0.0) || release.waited >= 0.6 {
                let (window, waited, before) = (release.window, release.waited, release.ended);
                self.fx_release = None;
                if !back {
                    crate::log!("picture taken away {:.0} ms after the window was asked back, though it is still minimised (it was at {:?}, is at {:?})", waited * 1000.0, before, shell::frame_rect(window));
                }
                if let Some(fx) = self.fx.as_mut() {
                    fx.stop();
                }
            }
        }

        self.update_preview(pt);
        let last = self.screens.len() - 1;
        let theme = &self.screens[self.preview_screen.min(last)].theme;
        if let Some(preview) = self.preview.as_mut() {
            match preview.update(theme, &self.motion, pt, dt) {
                Ok(more) => moving |= more,
                Err(e) => errors.push(e),
            }
        }
        let theme = &self.screens[self.bubble_screen.min(last)].theme;
        if let Some(bubble) = self.bubble.as_mut() {
            match bubble.update(theme, &self.motion, dt) {
                Ok(more) => moving |= more,
                Err(e) => errors.push(e),
            }
        }
        if let Some(launcher) = self.launcher.as_mut() {
            match launcher.update(&self.scene, &self.gfx, &self.screens[0].theme, &self.motion, self.scan.as_ref(), dt) {
                Ok(more) => moving |= more,
                Err(e) => errors.push(e),
            }
        }
        for e in errors {
            self.note_error(e);
        }

        moving = moving
            || self.bench.is_some()
            || self.demo_cursor.is_some()
            || self.last_input.elapsed().as_secs_f32() < PACE_AFTER_INPUT
            || self.fx.as_ref().map(|f| f.is_playing()).unwrap_or(false)
            || self.fx_release.is_some()
            || self.refresh_due;

        // Everything changed above goes to the compositor as one batch. The compositor
        // answers each batch with a message to this thread, and a message wakes the loop;
        // so a batch is only sent when something may have changed (something is moving, has
        // just stopped, or an event came in). Sent on every pass, the answers alone would
        // keep the loop turning at the display's pace for ever.
        let touched = std::mem::take(&mut self.touched);
        if moving || self.was_moving || touched {
            self.scene.commit();
        }
        self.was_moving = moving;
        if moving {
            self.busy();
        }
        moving
    }

    /// Logs an error from updating visuals, once per distinct error (it would repeat every frame).
    fn note_error(&mut self, e: windows::core::Error) {
        if self.shown_error.as_ref() != Some(&e) {
            crate::log!("could not update visuals: {e}");
            self.shown_error = Some(e);
        }
    }

    /// Once the app has been at rest for a while, gives back what it does not need: caches
    /// for painting, the tray's icons, and every memory page not in use this instant. The
    /// first frame after waking pays a moment for it.
    fn housekeeping(&mut self) {
        if self.last_busy.elapsed().as_secs_f32() < REST_AFTER {
            return;
        }
        let mut dropped = false;
        if let Some(launcher) = self.launcher.as_mut() {
            if launcher.idle_for().map(|t| t > KEEP_TRAY_ICONS).unwrap_or(false) {
                dropped |= launcher.release();
            }
        }
        if !self.rested || dropped {
            let before = system::memory_use().unwrap_or_default();
            self.rested = true;
            self.gfx.trim();
            self.scene.trim();
            system::release_memory();
            if self.cfg.stats {
                let after = system::memory_use().unwrap_or_default();
                crate::log!("at rest: gave memory back, {:.0} MB in RAM (was {:.0}), {:.0} MB private (was {:.0})", after.in_ram, before.in_ram, after.private, before.private);
            }
        }
    }

    /// With `stats = true`: once a minute, how much memory the app holds and how often it
    /// woke up. At rest it should wake about once a second, for the clock and the safety poll.
    fn note_stats(&mut self) {
        let seconds = self.last_memory_note.elapsed().as_secs_f32();
        if !self.cfg.stats || seconds < 60.0 {
            return;
        }
        self.last_memory_note = Instant::now();
        let now = system::memory_use().unwrap_or_default();
        crate::log!("in the last {seconds:.0} s: {} passes of the loop, {} readings of the windows | {:.0} MB in RAM, {:.0} MB private", self.passes, self.refreshes, now.in_ram, now.private);
        (self.passes, self.refreshes) = (0, 0);
    }

    pub fn stats_enabled(&self) -> bool {
        self.cfg.stats || self.bench.is_some()
    }

    pub fn item_count(&self) -> usize {
        self.screens.iter().map(|s| s.dock.count()).sum()
    }

    /// Name of a self-test phase that has just ended (so its timing can be logged under that name).
    pub fn bench_phase_finished(&mut self) -> Option<&'static str> {
        self.bench_last.take().or_else(|| self.bench.as_mut().and_then(|b| b.finished.take()))
    }

    /// Advances the self-test and returns where its imaginary cursor is, in the first dock's window.
    fn bench_cursor(&mut self) -> Option<(f32, f32)> {
        let bench = self.bench.as_mut()?;
        let t = bench.started.elapsed().as_secs_f32() - BENCH_WARMUP;
        if t < 0.0 {
            return None;
        }
        let phase = (t / BENCH_PHASE_SECONDS) as usize;
        if bench.phase != Some(phase) {
            if let Some(previous) = bench.phase {
                bench.finished = Some(BENCH_PHASES[previous].0);
            }
            if phase >= BENCH_PHASES.len() {
                let saved = self.bench.take().map(|b| b.saved)?;
                let memory = system::memory_use().unwrap_or_default();
                crate::log!("bench: finished; {:.0} MB in RAM, {:.0} MB private", memory.in_ram, memory.private);
                self.apply_config(saved, false);
                // `finished` was dropped with the bench; report the last phase through a fresh marker.
                self.bench_last = Some(BENCH_PHASES[BENCH_PHASES.len() - 1].0);
                return None;
            }
            bench.phase = Some(phase);
            let mut cfg = bench.saved.clone();
            (BENCH_PHASES[phase].1)(&mut cfg);
            self.apply_config(cfg, false);
        }
        // Sweep left and right across the icons, about one pass per 1.4 s.
        Some(self.screens[0].dock.sweep_point(t))
    }

    /// Plays the next scene of the tour when its time has come. It plays on the first screen.
    fn run_demo(&mut self) {
        let Some(demo) = self.demo.as_mut() else { return };
        let t = demo.started.elapsed().as_secs_f32();
        // While sweeping, the imaginary cursor keeps moving.
        if let Some((_, Act::SweepDock)) = demo.step.checked_sub(1).and_then(|i| DEMO.get(i)) {
            self.demo_cursor = Some(self.screens[0].dock.sweep_point(t));
        }
        let Some(&(at, act)) = DEMO.get(demo.step) else { return };
        if t < at {
            return;
        }
        demo.step += 1;
        crate::log!("demo: {act:?}");
        let dock = &self.screens[0].dock;
        // The second app icon, or whichever there is.
        let icon = (0..dock.count()).filter(|&i| dock.items[i].is_app() && !dock.items[i].is_special()).nth(1).or(Some(0)).filter(|&i| i < dock.count());
        match act {
            Act::SweepDock => self.demo_cursor = Some(dock.sweep_point(t)),
            Act::RestOnIcon => {
                if let Some(i) = icon {
                    let (l, _, r, _) = dock.icon_rect(i);
                    self.demo_cursor = Some(((l + r) * 0.5, dock.m.panel_top + dock.m.panel_h * 0.5));
                }
            }
            Act::LeaveDock => self.demo_cursor = None,
            Act::DragIn => {
                // An app that is unlikely to be in the dock already, held at the top edge of
                // the glass, between two icons.
                let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
                let x = icon.map(|i| dock.icon_rect(i).2 + (dock.m.slot - dock.m.icon) * 0.5).unwrap_or(dock.m.win_w as f32 * 0.5);
                self.demo_cursor = Some((x, dock.m.panel_top - 1.0));
                self.screens[0].dock.drag_in(&[format!("{windir}\\System32\\Taskmgr.exe")], x, &self.motion);
            }
            Act::PushIn => {
                if let Some((x, _)) = self.demo_cursor {
                    self.demo_cursor = Some((x, dock.m.panel_top + dock.m.panel_h * 0.5));
                }
            }
            Act::DragOut => {
                self.screens[0].dock.drag_out();
                self.demo_cursor = None;
            }
            Act::AlertVolume => self.alert(Alert::Volume { level: 0.62, muted: false }),
            Act::AlertTrack => self.alert(Alert::Track { title: "Bài hát đang phát".to_string(), artist: "Nghệ sĩ".to_string() }),
            Act::OpenIsland => {
                if let Some(island) = self.screens[0].island.as_mut() {
                    island.open();
                }
            }
            // The month is a page the island keeps by itself; the others are filled here.
            Act::OpenPage(Page::Calendar) => {
                if let Some(island) = self.screens[0].island.as_mut() {
                    island.show_page(Page::Calendar, "", Vec::new());
                }
            }
            Act::OpenPage(page) => self.open_page(0, page),
            Act::CloseIsland => {
                if let Some(island) = self.screens[0].island.as_mut() {
                    island.collapse();
                }
            }
            Act::OpenMenu => self.show_dock_menu(0, icon),
            Act::CloseMenu => self.close_bubble(),
            Act::OpenTray => {
                if !self.launcher.as_ref().map(|l| l.is_open()).unwrap_or(true) {
                    self.toggle_launcher(0);
                }
            }
            Act::CloseTray => {
                if let Some(l) = self.launcher.as_mut() {
                    l.close();
                }
            }
            Act::End => {
                self.demo = None;
                self.demo_cursor = None;
                let memory = system::memory_use().unwrap_or_default();
                crate::log!("demo: finished; {:.0} MB in RAM, {:.0} MB private", memory.in_ram, memory.private);
            }
        }
        self.busy();
    }

    /// True while a scripted run is in charge (the loop then keeps turning without input).
    pub fn scripted(&self) -> bool {
        self.demo.is_some() || self.bench.is_some()
    }

    /// The GPU device was reset: make a new one and repaint everything on it.
    fn rebuild_graphics(&mut self) {
        match Gfx::new(self.cfg.hardware_gpu).and_then(|g| self.scene.set_rendering_device(&g).map(|_| g)) {
            Ok(g) => {
                self.gfx = g;
                crate::log!("graphics device was reset; rebuilt");
            }
            Err(e) => {
                crate::log!("graphics rebuild failed: {e}");
                return;
            }
        }
        for screen in &mut self.screens {
            screen.dock.forget_labels();
            screen.dock.forget_icons();
            if let Some(island) = screen.island.as_mut() {
                island.forget();
            }
        }
        if let Some(fx) = self.fx.as_mut() {
            fx.stop();
        }
        if let Some(preview) = self.preview.as_mut() {
            preview.hide_now();
        }
        self.resting_on = None;
        if let Some(bubble) = self.bubble.as_mut() {
            bubble.forget();
        }
        if let Some(launcher) = self.launcher.as_mut() {
            launcher.close();
            launcher.release();
        }
        self.refresh_due = true;
    }

    // --------------------------------------------------------------- cleanup

    /// Undoes everything the app changed in Windows. Called once, on exit.
    pub fn shutdown(&mut self) {
        // From here on Explorer's answer stands again.
        MIN_RECTS.with(|table| table.borrow_mut().clear());
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            let _ = DeregisterShellHookWindow(self.hwnd);
            for hook in self.hooks.drain(..) {
                let _ = UnhookWinEvent(hook);
            }
        }
        self.preview = None;
        self.fx = None;
        self.launcher = None;
        self.scan = None;
        self.bubble = None;
        self.media = None;
        system::restore_window_animations();
        // Each screen's dock and island give their strip of screen back as they go.
        self.screens.clear();
        if self.taskbar.hidden {
            self.taskbar.restore();
        }
    }
}
