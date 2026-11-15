//! The dock: its items, how they move, how they are drawn and what clicks do.

use crate::bar::{self, Bar};
use crate::calendar::{self, Calendar};
use crate::config::{self, Config};
use crate::fx::{self, Fx};
use crate::launcher::{self, Launcher};
use crate::gfx::{color, Gfx};
use crate::glass::{rgba, Glass, Palette, PanelRect, Pill, Sprite};
use crate::layout::{layout, Magnifier, Slot};
use crate::menu::{self, Menu};
use crate::menus;
use crate::preview::{self, Preview};
use crate::settings;
use crate::shell::{self, IconPixels, WindowTracker};
use crate::spring::Spring;
use crate::system::{self, AppBar, Taskbar};
use crate::util::wide;
use crate::wifi;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Instant, SystemTime};
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::ABN_POSCHANGED;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Color;
use windows::UI::Composition::CompositionDrawingSurface;

pub const CLASS_NAME: &str = "BenDockWindow";
const WM_APPBAR_CALLBACK: u32 = WM_APP + 1;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_DPICHANGED_MSG: u32 = 0x02E0;
const WM_DWMCOLORIZATIONCOLORCHANGED_MSG: u32 = 0x0320;
const TIMER_ID: usize = 1;
const HSHELL_FLASH: usize = 0x8006;
/// Identity of the dock's own launcher button (not an app, never written to pins.txt).
const LAUNCHER_KEY: &str = "::launcher";


// ------------------------------------------------------------------ events
//
// The window procedure only records what happened. The main loop handles the
// events afterwards, so no dock code ever runs re-entrantly (for example
// while a context menu is pumping messages).

#[derive(Clone, Copy, Debug)]
pub enum Event {
    MouseMove,
    MouseLeave,
    LeftDown(i32, i32),
    LeftUp(i32, i32),
    MiddleUp(i32, i32),
    RightUp(i32, i32),
    Tick,
    Shell(usize, isize),
    AppBar(usize),
    Relayout,
    Theme,
    TaskbarCreated,
    /// A window started to minimise (reported by Windows).
    Minimized(isize),
    /// A window was moved, resized or restored: positions need re-reading.
    WindowsChanged,
    /// Click in the window preview. `true` = middle button.
    PreviewClick(bool),
    /// The cursor moved over the top bar (x in bar pixels; negative = it left).
    BarMove(i32),
    BarClick(i32),
    Launcher(launcher::Input),
    Menu(menu::Input),
    /// The panel beside the Wi-Fi list (the password).
    Side(menu::Input),
    Calendar(menu::Input),
    /// The settings window changed something. `true` once the change is final (control released).
    Config(bool),
    Quit,
}

thread_local! {
    static EVENTS: RefCell<VecDeque<Event>> = const { RefCell::new(VecDeque::new()) };
}
static SHELL_HOOK_MSG: AtomicU32 = AtomicU32::new(0);
static TASKBAR_CREATED_MSG: AtomicU32 = AtomicU32::new(0);

fn push(ev: Event) {
    EVENTS.with(|q| {
        let mut q = q.borrow_mut();
        if matches!(ev, Event::MouseMove) && matches!(q.back(), Some(Event::MouseMove)) {
            return;
        }
        q.push_back(ev);
    });
}

/// Queues an event for the dock from elsewhere in the app (the settings window).
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
        _ => push(Event::WindowsChanged),
    }
}

fn point(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32)
}

pub unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => push(Event::MouseMove),
        WM_MOUSELEAVE => push(Event::MouseLeave),
        WM_LBUTTONDOWN => {
            let (x, y) = point(lparam);
            push(Event::LeftDown(x, y));
        }
        WM_LBUTTONUP => {
            let (x, y) = point(lparam);
            push(Event::LeftUp(x, y));
        }
        WM_MBUTTONUP => {
            let (x, y) = point(lparam);
            push(Event::MiddleUp(x, y));
        }
        WM_RBUTTONUP => {
            let (x, y) = point(lparam);
            push(Event::RightUp(x, y));
        }
        WM_TIMER => push(Event::Tick),
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        WM_DISPLAYCHANGE | WM_DPICHANGED_MSG => push(Event::Relayout),
        WM_SETTINGCHANGE => {
            push(Event::Relayout);
            push(Event::Theme);
        }
        WM_DWMCOLORIZATIONCOLORCHANGED_MSG => push(Event::Theme),
        WM_APPBAR_CALLBACK => push(Event::AppBar(wparam.0)),
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
            unsafe { PostQuitMessage(0) };
            return LRESULT(0);
        }
        _ => {
            if msg != 0 && msg == SHELL_HOOK_MSG.load(Ordering::Relaxed) {
                push(Event::Shell(wparam.0, lparam.0));
                return LRESULT(0);
            }
            if msg != 0 && msg == TASKBAR_CREATED_MSG.load(Ordering::Relaxed) {
                push(Event::TaskbarCreated);
            }
        }
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

// ----------------------------------------------------------------- metrics

/// Every size the dock uses, in physical pixels for the current display scale.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Metrics {
    scale: f32,
    icon: f32,
    /// Width of one icon's slot at rest (icon + gap).
    slot: f32,
    separator: f32,
    pad_x: f32,
    pad_bottom: f32,
    panel_h: f32,
    radius: f32,
    margin: f32,
    win_w: i32,
    win_h: i32,
    /// Y of the icons' bottom edge, in window coordinates.
    baseline: f32,
    panel_top: f32,
    /// Extra scale at the cursor.
    amp: f32,
    mag_radius: f32,
    bounce_h: f32,
    label_h: f32,
    label_gap: f32,
    font: f32,
}

fn metrics(cfg: &Config, scale: f32, monitor_w: i32) -> Metrics {
    let px = |v: f32| (v * scale).round();
    let icon = px(cfg.icon_size);
    let gap = px(cfg.gap);
    let pad_top = px(cfg.padding);
    // The running indicator lives in the bottom padding, so it gets a little more room.
    let pad_bottom = px(cfg.padding + if cfg.indicator { 3.0 } else { 0.0 });
    let panel_h = icon + pad_top + pad_bottom;
    let margin = px(cfg.margin_bottom);
    let amp = cfg.magnification - 1.0;
    let overflow = icon * amp;
    let bounce_h = (icon * cfg.bounce_height).round();
    let label_h = px(cfg.label_size * 2.0);
    let label_gap = px(10.0);
    let headroom = overflow + bounce_h.max(label_gap + label_h) + px(8.0);
    let win_h = (margin + panel_h + headroom).ceil();
    Metrics {
        scale,
        icon,
        slot: icon + gap,
        separator: px(13.0),
        pad_x: (px(cfg.padding + 1.0) - gap * 0.5).max(0.0),
        pad_bottom,
        panel_h,
        radius: (panel_h * cfg.corner_radius / 100.0).round(),
        margin,
        win_w: monitor_w,
        win_h: win_h as i32,
        baseline: win_h - margin - pad_bottom,
        panel_top: win_h - margin - panel_h,
        amp,
        mag_radius: cfg.spread * (icon + gap),
        bounce_h,
        label_h,
        label_gap,
        font: cfg.label_size * scale,
    }
}

struct Monitor {
    rect: RECT,
    work: RECT,
    dpi: u32,
}

fn primary_monitor() -> Monitor {
    unsafe {
        let handle = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(handle, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        Monitor { rect: info.rcMonitor, work: info.rcWork, dpi: dx.max(48) }
    }
}

// ------------------------------------------------------------------- items

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    App,
    Separator,
}

struct Bounce {
    t: f32,
    /// `Some(n)`: stop after n hops. `None`: keep going until the app's window shows up.
    hops: Option<u32>,
}

/// An app's name, painted once.
struct Label {
    surface: CompositionDrawingSurface,
    width: f32,
    height: f32,
}

struct Item {
    kind: Kind,
    key: String,
    target: String,
    name: String,
    pinned: bool,
    /// Open windows, front-most first.
    windows: Vec<HWND>,
    pixels: IconPixels,
    /// The icon pre-scaled to a few sizes between "at rest" and "fully magnified", smallest first.
    /// Showing the nearest size up keeps the icon sharp at every size, and pixel-exact at rest.
    surfaces: Vec<(f32, CompositionDrawingSurface)>,
    /// Which of `surfaces` the sprite currently shows.
    level: Option<usize>,
    sprite: Option<Sprite>,
    /// Running indicator for apps, the divider line for the separator.
    mark: Option<Pill>,
    label: Option<Label>,
    /// 0 = absent, 1 = fully in the dock. Drives the slide-in / slide-out.
    presence: Spring,
    /// Running indicator visibility.
    running: Spring,
    /// 1 while this app owns the foreground window.
    active: Spring,
    /// 1 while the mouse button is held on the icon.
    press: Spring,
    /// Sideways offset from the icon's slot, in pixels. Non-zero while the icon is being dragged
    /// to a new place, and while a neighbour glides over to make room.
    shift: Spring,
    bounce: Option<Bounce>,
    bounce_y: f32,
}

impl Item {
    fn app(key: String, target: String, pinned: bool, present: bool) -> Self {
        let name = shell::display_name(&target);
        let pixels = shell::load_icon(&target, 256).unwrap_or_else(|| {
            crate::log!("no icon for '{target}'; using a plain tile");
            shell::placeholder_icon(96)
        });
        let mut presence = Spring::new(if present { 1.0 } else { 0.0 }, 18.0, 1.0);
        presence.set(1.0);
        Item {
            kind: Kind::App,
            key,
            target,
            name,
            pinned,
            windows: Vec::new(),
            pixels,
            surfaces: Vec::new(),
            level: None,
            sprite: None,
            mark: None,
            label: None,
            presence,
            running: Spring::new(0.0, 22.0, 1.0),
            active: Spring::new(0.0, 22.0, 0.75),
            press: Spring::new(0.0, 45.0, 1.0),
            shift: Spring::new(0.0, 24.0, 0.85),
            bounce: None,
            bounce_y: 0.0,
        }
    }

    /// The dock's own button that opens the app launcher.
    fn launcher_button(accent: (f32, f32, f32)) -> Self {
        let mut item = Item::separator();
        item.kind = Kind::App;
        item.key = LAUNCHER_KEY.to_string();
        item.target = LAUNCHER_KEY.to_string();
        item.name = "Ứng dụng".to_string();
        item.pinned = true;
        item.pixels = shell::launcher_icon(256, accent);
        item.presence = Spring::new(1.0, 18.0, 1.0);
        item
    }

    fn is_special(&self) -> bool {
        self.key.starts_with("::")
    }

    fn separator() -> Self {
        Item {
            kind: Kind::Separator,
            key: String::new(),
            target: String::new(),
            name: String::new(),
            pinned: false,
            windows: Vec::new(),
            pixels: IconPixels { size: 0, bgra: Vec::new() },
            surfaces: Vec::new(),
            level: None,
            sprite: None,
            mark: None,
            label: None,
            presence: Spring::new(0.0, 18.0, 1.0),
            running: Spring::new(0.0, 22.0, 1.0),
            active: Spring::new(0.0, 22.0, 1.0),
            press: Spring::new(0.0, 45.0, 1.0),
            shift: Spring::new(0.0, 24.0, 0.85),
            bounce: None,
            bounce_y: 0.0,
        }
    }

    fn is_app(&self) -> bool {
        self.kind == Kind::App
    }

    fn settled(&self) -> bool {
        self.presence.settled(1e-3) && self.running.settled(1e-3) && self.active.settled(1e-3) && self.press.settled(1e-3) && self.shift.settled(0.05) && self.bounce.is_none()
    }
}

// -------------------------------------------------------------------- dock

/// A scripted self-test: an imaginary cursor sweeps across the dock while a few rendering
/// variants are switched in turn, and the frame timing of each is written to ben.log. This is
/// how "which part is slow on this machine" gets answered with numbers.
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
const BENCH_PHASES: &[(&str, Tweak)] = &[("blur on", |c| c.blur = true), ("blur off", |c| c.blur = false)];

struct Colors {
    dark: bool,
    accent: (f32, f32, f32),
}

/// An attempt to join a Wi-Fi network that Windows is still working on.
struct Joining {
    name: String,
    /// The network was saved in Windows for this attempt (so it is removed again if it fails).
    saved_now: bool,
    since: Instant,
}

/// The picture of a window that has flown back out is kept up until the real window is there.
struct Release {
    window: HWND,
    /// Time left once the window is back.
    left: f32,
    waited: f32,
    /// Where the picture ended (the window's drawn part before it was minimised).
    ended: Option<RECT>,
}

pub struct Dock {
    hwnd: HWND,
    cfg: Config,
    m: Metrics,
    gfx: Gfx,
    glass: Glass,
    text_format: Option<IDWriteTextFormat>,
    label_bg: Option<Pill>,
    label_text: Option<Sprite>,
    /// Item whose name the label sprite currently shows.
    label_shown: Option<String>,
    colors: Colors,

    items: Vec<Item>,
    widths: Vec<f32>,
    slots: Vec<Slot>,
    panel: PanelRect,

    /// 0 = flat, 1 = fully magnified.
    mag: Spring,
    cursor_x: f32,
    hovering: bool,
    hovered: Option<usize>,
    pressed: Option<usize>,
    label_alpha: Spring,
    label_index: Option<usize>,
    /// 0 = shown, 1 = tucked below the screen edge.
    hide: Spring,

    tracker: WindowTracker,
    taskbar: Taskbar,
    appbar: AppBar,
    monitor_rect: RECT,
    dpi: u32,
    win_x: i32,
    win_y: i32,
    region: Option<(i32, i32, i32, i32)>,
    tracking_mouse: bool,
    refresh_due: bool,
    replace_due: bool,
    /// Bottom edge of the dock's strip on screen, as last granted by the shell.
    bottom_line: i32,
    /// Last-modified time of ben.ini as last read, to notice edits made by hand.
    cfg_stamp: Option<SystemTime>,
    bench: Option<Bench>,
    bench_last: Option<&'static str>,
    bar: Option<Bar>,
    /// Blur strength the bar and the launcher were made with.
    popup_sigma: Option<f32>,
    status: system::StatusReader,
    last_status: system::Status,
    clock: String,
    /// Name shown in the top bar: the app in front.
    front_name: String,
    launcher: Option<Launcher>,
    /// The dock's own drop-down / context menu.
    menu: Option<Menu>,
    side: Option<Menu>,
    /// The date panel under the clock.
    calendar: Option<Calendar>,
    /// The top bar part the open menu hangs from, if it was opened from the bar.
    menu_zone: Option<bar::Zone>,
    /// The dock item the open menu belongs to (its key), if it was opened on an icon.
    menu_item: Option<String>,
    /// The window that was in front when the menu opened: menu commands act on it.
    menu_target: HWND,
    /// The Wi-Fi networks as last listed in the Wi-Fi menu (its rows point into this).
    wifi_list: Vec<wifi::Network>,
    /// The network a password is being asked for.
    wifi_asking: Option<String>,
    /// A line about the last attempt to join, shown at the top of the Wi-Fi menu.
    wifi_note: Option<String>,
    /// A join in progress.
    wifi_joining: Option<Joining>,
    /// Apps arriving from the background scan, for the launcher.
    app_scan: Option<std::sync::mpsc::Receiver<shell::AppEntry>>,
    fx: Option<Fx>,
    /// After a restore effect ends, how long to keep its picture up while the real window appears.
    fx_release: Option<Release>,
    /// Where each open window was last seen on screen: its own rectangle and the part of it that
    /// is drawn. Not updated while the window is minimised.
    rects: HashMap<isize, (RECT, RECT)>,
    preview: Option<Preview>,
    /// Running app the cursor is resting on, and since when.
    resting_on: Option<(String, Instant)>,
    /// When the cursor left the preview and its icon.
    preview_left: Option<Instant>,
    /// Set while an icon is being dragged to a new place: how far from the icon's centre it was grabbed.
    drag: Option<f32>,
    /// Where the mouse button went down, in window pixels.
    press_x: f32,
    /// App that was just clicked: no preview for it until the cursor has moved off its icon.
    preview_block: Option<String>,
    hooks: Vec<HWINEVENTHOOK>,
    /// Last error from updating the visuals, so a repeating error is logged once.
    shown_error: Option<windows::core::Error>,
    pub quit: bool,
}

impl Dock {
    pub fn new(hwnd: HWND, cfg: Config, bench: bool) -> Result<Self> {
        let monitor = primary_monitor();
        let scale = monitor.dpi as f32 / 96.0;
        let m = metrics(&cfg, scale, monitor.rect.right - monitor.rect.left);
        let colors = Colors { dark: is_dark(&cfg), accent: system::accent_color() };

        let gfx = Gfx::new()?;
        let sigma = (cfg.blur && cfg.blur_radius > 0.0).then_some(cfg.blur_radius * scale);
        let glass = Glass::new(hwnd, &gfx, m.win_w as f32, m.win_h as f32, sigma, &palette(&cfg, &colors))?;
        crate::log!("graphics ready: {}x{} px, scale {:.2}, blur {}", m.win_w, m.win_h, scale, if glass.blur_active { "on" } else { "off" });

        let mut hide = Spring::new(1.0, 13.0, 1.0);
        hide.set(0.0);
        let mag_speed = cfg.mag_speed;

        let mut dock = Dock {
            hwnd,
            cfg,
            m,
            gfx,
            glass,
            text_format: None,
            label_bg: None,
            label_text: None,
            label_shown: None,
            colors,
            items: Vec::new(),
            widths: Vec::new(),
            slots: Vec::new(),
            panel: PanelRect { x: 0.0, y: 0.0, w: 0.0, h: 0.0, radius: 0.0 },
            mag: Spring::new(0.0, mag_speed, 1.0),
            cursor_x: m.win_w as f32 * 0.5,
            hovering: false,
            hovered: None,
            pressed: None,
            label_alpha: Spring::new(0.0, 32.0, 1.0),
            label_index: None,
            hide,
            tracker: WindowTracker::default(),
            taskbar: Taskbar::new(),
            appbar: AppBar::new(hwnd),
            monitor_rect: monitor.rect,
            dpi: monitor.dpi,
            win_x: 0,
            win_y: 0,
            region: None,
            tracking_mouse: false,
            refresh_due: true,
            replace_due: false,
            bottom_line: monitor.rect.bottom,
            cfg_stamp: config::config_modified(),
            bench: None,
            bench_last: None,
            bar: None,
            popup_sigma: None,
            status: system::StatusReader::new(),
            last_status: system::Status::default(),
            clock: system::clock_text(),
            front_name: String::new(),
            launcher: None,
            menu: None,
            side: None,
            calendar: None,
            menu_zone: None,
            menu_item: None,
            menu_target: HWND::default(),
            wifi_list: Vec::new(),
            wifi_asking: None,
            wifi_note: None,
            wifi_joining: None,
            app_scan: None,
            fx: None,
            fx_release: None,
            rects: HashMap::new(),
            preview: None,
            resting_on: None,
            preview_left: None,
            drag: None,
            press_x: 0.0,
            preview_block: None,
            hooks: Vec::new(),
            shown_error: None,
            quit: false,
        };

        if bench {
            dock.bench = Some(Bench { saved: dock.cfg.clone(), started: Instant::now(), phase: None, finished: None });
        }
        dock.load_pins();
        if dock.cfg.hide_taskbar {
            dock.taskbar.hide();
        }
        if dock.cfg.reserve_space {
            dock.appbar.register(WM_APPBAR_CALLBACK);
        }
        dock.place_window(&monitor, true);

        unsafe {
            // Shell notifications make the dock react at once when windows open, close or flash.
            SHELL_HOOK_MSG.store(RegisterWindowMessageW(PCWSTR(wide("SHELLHOOK").as_ptr())), Ordering::Relaxed);
            TASKBAR_CREATED_MSG.store(RegisterWindowMessageW(PCWSTR(wide("TaskbarCreated").as_ptr())), Ordering::Relaxed);
            if !RegisterShellHookWindow(hwnd).as_bool() {
                crate::log!("RegisterShellHookWindow failed; falling back to polling only");
            }
            // The poll is the safety net: it also re-hides the taskbar if Explorer brings it back.
            SetTimer(Some(hwnd), TIMER_ID, 500, None);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);

            // Be told the moment a window is minimised, restored or moved.
            for (first, last) in [(EVENT_SYSTEM_MINIMIZESTART, EVENT_SYSTEM_MINIMIZEEND), (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND)] {
                let hook = SetWinEventHook(first, last, None, Some(win_event), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS);
                if hook.is_invalid() {
                    crate::log!("SetWinEventHook({first}) failed; minimise effects will not play");
                } else {
                    dock.hooks.push(hook);
                }
            }
        }
        match Fx::new(&dock.glass) {
            Ok(fx) => dock.fx = Some(fx),
            Err(e) => crate::log!("effect window unavailable: {e}"),
        }
        let sigma = (dock.cfg.blur && dock.cfg.blur_radius > 0.0).then_some(dock.cfg.blur_radius * dock.m.scale);
        match Preview::new(&dock.glass, sigma, &palette(&dock.cfg, &dock.colors)) {
            Ok(p) => dock.preview = Some(p),
            Err(e) => crate::log!("window preview unavailable: {e}"),
        }
        if dock.cfg.minimize_effect >= 0.5 {
            system::disable_window_animations();
        }
        dock.setup_bar_and_launcher();
        Ok(dock)
    }

    fn load_pins(&mut self) {
        let pins = config::load_pins().unwrap_or_else(|| {
            // First run: start with File Explorer so the dock is not empty.
            let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
            let pins = vec![format!("{windir}\\explorer.exe")];
            config::save_pins(&pins);
            pins
        });
        if self.cfg.launcher {
            self.items.push(Item::launcher_button(self.colors.accent));
        }
        for target in pins {
            let key = shell::key_for_target(&target);
            if self.items.iter().any(|i| i.key == key) {
                continue;
            }
            self.items.push(Item::app(key, target, true, true));
        }
        self.items.push(Item::separator());
    }

    fn save_pins(&self) {
        let pins: Vec<String> = self.items.iter().filter(|i| i.is_app() && i.pinned && !i.is_special()).map(|i| i.target.clone()).collect();
        config::save_pins(&pins);
    }

    // ------------------------------------------------------------- placement

    /// Sizes and positions the window for the given monitor. With `reserve`, also asks the shell
    /// again for the strip of screen that maximised windows must stay out of; without it the
    /// strip is left as it is (used while a settings slider is being dragged, so that every
    /// window on screen is not resized on each mouse move).
    fn place_window(&mut self, monitor: &Monitor, reserve: bool) {
        let scale = monitor.dpi as f32 / 96.0;
        self.monitor_rect = monitor.rect;
        self.dpi = monitor.dpi;
        let old = self.m;
        self.m = metrics(&self.cfg, scale, monitor.rect.right - monitor.rect.left);
        let m = self.m;

        // Where the bottom of the dock's strip is: the screen edge when the taskbar is hidden,
        // otherwise just above the taskbar.
        if reserve {
            let mut bottom = if self.taskbar.hidden { monitor.rect.bottom } else { monitor.work.bottom };
            if self.cfg.reserve_space {
                let height = (m.panel_h + m.margin * 2.0).ceil() as i32;
                if let Some(strip) = self.appbar.reserve(monitor.rect, height) {
                    bottom = strip.bottom;
                }
            }
            self.bottom_line = bottom;
        }
        let bottom = self.bottom_line;
        self.win_x = monitor.rect.left;
        self.win_y = bottom - m.win_h;
        unsafe {
            if let Err(e) = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), self.win_x, self.win_y, m.win_w, m.win_h, SWP_NOACTIVATE) {
                crate::log!("SetWindowPos failed: {e}");
            }
        }
        if let Err(e) = self.glass.resize(m.win_w as f32, m.win_h as f32) {
            crate::log!("resize failed: {e}");
        }
        // Text and pre-scaled icons are made for specific sizes: redo them only if those changed.
        if old.font != m.font {
            self.forget_labels();
        }
        if icon_levels(&old) != icon_levels(&m) {
            for item in &mut self.items {
                item.surfaces.clear();
                item.level = None;
            }
        }
        self.region = None;
        if old.win_w != m.win_w {
            self.cursor_x = m.win_w as f32 * 0.5;
        }
    }

    fn relayout_if_changed(&mut self) {
        let monitor = primary_monitor();
        if monitor.rect != self.monitor_rect || monitor.dpi != self.dpi {
            crate::log!("display changed: {:?} @ {} dpi", monitor.rect, monitor.dpi);
            self.place_window(&monitor, true);
            if let Some(bar) = self.bar.as_mut() {
                bar.place(monitor.rect, self.m.scale, self.cfg.bar_height, self.cfg.reserve_space);
            }
            if let Some(l) = self.launcher.as_mut() {
                l.close();
            }
        }
    }

    // ---------------------------------------------------------------- events

    pub fn handle(&mut self, ev: Event) {
        match ev {
            Event::MouseMove => {
                if !self.tracking_mouse {
                    let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                    self.tracking_mouse = unsafe { TrackMouseEvent(&mut track) }.is_ok();
                }
            }
            Event::MouseLeave => self.tracking_mouse = false,
            Event::LeftDown(x, y) => {
                self.close_menu();
                self.close_preview();
                self.pressed = self.app_at(x as f32, y as f32);
                self.press_x = x as f32;
                self.drag = None;
                self.preview_block = self.pressed.map(|i| self.items[i].key.clone());
                unsafe {
                    SetCapture(self.hwnd);
                }
            }
            Event::LeftUp(x, y) => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                if let Some(index) = self.pressed.take() {
                    if self.drag.take().is_some() {
                        // Dropped after dragging: keep the new order, do not treat it as a click.
                        if self.items.get(index).map(|i| i.pinned).unwrap_or(false) {
                            self.save_pins();
                        }
                    } else if self.app_at(x as f32, y as f32) == Some(index) {
                        self.click(index);
                    }
                }
            }
            Event::MiddleUp(x, y) => {
                self.close_menu();
                if let Some(index) = self.app_at(x as f32, y as f32) {
                    self.launch(index);
                }
            }
            Event::RightUp(x, y) => {
                self.close_menu();
                self.close_preview();
                let index = self.app_at(x as f32, y as f32);
                self.show_menu(index);
            }
            Event::Tick => {
                self.refresh_due = true;
                self.taskbar.keep_hidden();
                if self.bar.is_some() {
                    self.last_status = self.status.read();
                    self.clock = system::clock_text();
                }
                self.take_scanned_apps();
                self.follow_wifi();
                if self.gfx.lost() {
                    self.rebuild_graphics();
                }
                if std::mem::take(&mut self.replace_due) {
                    let monitor = primary_monitor();
                    self.place_window(&monitor, true);
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
            }
            Event::Shell(code, lparam) => {
                self.refresh_due = true;
                if code == HSHELL_FLASH && self.cfg.bounce {
                    // A window is asking for attention: hop three times, unless it is already in front.
                    let flashing = lparam;
                    let front = unsafe { GetForegroundWindow() };
                    if let Some(item) = self.items.iter_mut().find(|i| i.windows.iter().any(|w| w.0 as isize == flashing)) {
                        if item.bounce.is_none() && !item.windows.contains(&front) {
                            item.bounce = Some(Bounce { t: 0.0, hops: Some(3) });
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
            Event::Theme => {
                let colors = Colors { dark: is_dark(&self.cfg), accent: system::accent_color() };
                if colors.dark != self.colors.dark || colors.accent != self.colors.accent {
                    self.colors = colors;
                    self.forget_labels();
                    if let Err(e) = self.glass.set_palette(&palette(&self.cfg, &self.colors)) {
                        crate::log!("palette update failed: {e}");
                    }
                    self.repaint_bar_and_launcher();
                }
            }
            Event::TaskbarCreated => {
                // Explorer restarted: everything registered with it is gone.
                crate::log!("Explorer restarted; hiding the taskbar and reserving space again");
                if self.taskbar.hidden {
                    self.taskbar.hide();
                }
                if self.cfg.reserve_space {
                    self.appbar = AppBar::new(self.hwnd);
                    self.appbar.register(WM_APPBAR_CALLBACK);
                }
                unsafe {
                    let _ = RegisterShellHookWindow(self.hwnd);
                }
                let monitor = primary_monitor();
                self.place_window(&monitor, true);
            }
            Event::Minimized(id) => {
                let window = HWND(id as *mut _);
                // Unless the dock started this minimise itself (then the effect is already playing).
                if self.fx.as_ref().and_then(|f| f.window()) != Some(window) {
                    self.play_effect(window, false);
                }
                self.refresh_due = true;
            }
            Event::WindowsChanged => self.refresh_due = true,
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
            Event::BarMove(x) => {
                if let Some(bar) = self.bar.as_mut() {
                    bar.set_hover((x >= 0).then_some(x as f32));
                }
                // With a menu open, sliding along the bar opens the menu under the cursor.
                let over = self.bar.as_ref().and_then(|b| b.zone_at(x as f32)).filter(|z| z.is_menu());
                if let (Some(open), Some(over)) = (self.menu_zone, over) {
                    if open != over {
                        self.open_bar_menu(over);
                    }
                }
            }
            Event::BarClick(x) => {
                let zone = self.bar.as_ref().and_then(|b| b.zone_at(x as f32));
                let was_open = self.menu_zone;
                let calendar_was_open = self.calendar.as_ref().map(|c| c.is_open()).unwrap_or(false);
                self.close_menu();
                match zone {
                    // Clicking the title of the open menu only closes it.
                    Some(zone) if zone.is_menu() => {
                        if was_open != Some(zone) {
                            if zone == bar::Zone::Network {
                                // Opened afresh: no half-typed password, and look for networks again.
                                self.wifi_asking = None;
                                wifi::refresh();
                            }
                            self.open_bar_menu(zone);
                        }
                    }
                    Some(bar::Zone::Search) => self.toggle_launcher(),
                    // Clicking the clock while its panel is open only closes it.
                    Some(bar::Zone::Clock) if !calendar_was_open => self.open_calendar(),
                    _ => {}
                }
            }
            Event::Menu(input) => {
                let outcome = self.menu.as_mut().map(|m| m.handle(input)).unwrap_or(menu::Outcome::Nothing);
                match outcome {
                    menu::Outcome::Nothing => {}
                    menu::Outcome::Closed => self.close_menu(),
                    // Choosing in the Wi-Fi list keeps the menu open: it shows what happens next.
                    menu::Outcome::Chosen(id) if menus::is_wifi(id) => self.wifi_command(id),
                    menu::Outcome::Entered(_) => self.wifi_command(menus::WIFI_JOIN),
                    menu::Outcome::Chosen(id) => {
                        let item = self.menu_item.clone();
                        self.close_menu();
                        self.run_command(id, item);
                    }
                    menu::Outcome::Value(id, value) => {
                        if id == menus::VOLUME {
                            self.status.set_volume(value);
                            // The speaker symbol in the bar follows at once.
                            self.last_status = self.status.read();
                        }
                    }
                    menu::Outcome::Step(dir) => {
                        if let Some(zone) = self.menu_zone {
                            self.open_bar_menu(zone.neighbour(dir));
                        }
                    }
                }
            }
            Event::Side(input) => {
                let outcome = self.side.as_mut().map(|m| m.handle(input)).unwrap_or(menu::Outcome::Nothing);
                match outcome {
                    menu::Outcome::Closed => {
                        // The focus went back to the Wi-Fi list (a click on it): only the
                        // panel goes away. Anywhere else (or Esc): everything closes.
                        let main = self.menu.as_ref().filter(|m| m.is_open()).map(|m| m.hwnd());
                        if main.is_some() && main == Some(unsafe { GetForegroundWindow() }) {
                            self.wifi_asking = None;
                            if let Some(side) = self.side.as_mut() {
                                side.close();
                            }
                            if let Some(menu) = self.menu.as_mut() {
                                menu.pin(None);
                                menu.rearm();
                            }
                        } else {
                            self.close_menu();
                        }
                    }
                    menu::Outcome::Chosen(id) if menus::is_wifi(id) => self.wifi_command(id),
                    menu::Outcome::Entered(_) => self.wifi_command(menus::WIFI_JOIN),
                    _ => {}
                }
            }
            Event::Calendar(input) => {
                let outcome = self.calendar.as_mut().map(|c| c.handle(input)).unwrap_or(calendar::Outcome::Nothing);
                match outcome {
                    calendar::Outcome::Nothing => {}
                    calendar::Outcome::Closed => self.close_menu(),
                    calendar::Outcome::Notifications => {
                        self.close_menu();
                        system::press_windows_key('n');
                    }
                }
            }
            Event::Launcher(input) => {
                let chosen = self.launcher.as_mut().and_then(|l| l.handle(input));
                if let Some(target) = chosen {
                    shell::launch(&target);
                    self.refresh_due = true;
                }
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

    /// Index of the app icon at a point in window coordinates.
    fn app_at(&self, x: f32, y: f32) -> Option<usize> {
        let index = self.slots.iter().position(|s| x >= s.left && x < s.right)?;
        let item = self.items.get(index)?;
        if !item.is_app() || item.presence.target < 0.5 {
            return None;
        }
        let (_, top, _, _) = self.icon_rect(index);
        (y >= top.min(self.m.panel_top) - 2.0).then_some(index)
    }

    /// (left, top, right, bottom) of an item's icon as shown this frame.
    fn icon_rect(&self, index: usize) -> (f32, f32, f32, f32) {
        match (self.slots.get(index), self.items.get(index)) {
            (Some(&slot), Some(item)) => icon_rect_of(&self.m, slot, item),
            _ => (0.0, 0.0, 0.0, 0.0),
        }
    }

    fn click(&mut self, index: usize) {
        if self.items[index].key == LAUNCHER_KEY {
            self.toggle_launcher();
            return;
        }
        let front = unsafe { GetForegroundWindow() };
        let item = &self.items[index];
        if item.windows.is_empty() {
            self.launch(index);
        } else if item.windows.contains(&front) {
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

    /// Plays the minimise effect for a window (or the reverse, for restoring). Returns false
    /// when the effect is off or cannot be played, so the caller can carry on without it.
    fn play_effect(&mut self, window: HWND, restoring: bool) -> bool {
        let style = match self.cfg.minimize_effect.round() as i32 {
            1 => fx::Style::Shrink,
            2 => fx::Style::Funnel,
            3 => fx::Style::Drop,
            _ => return false,
        };
        let what = if restoring { "restore" } else { "minimise" };
        let id = window.0 as isize;
        let Some(index) = self.items.iter().position(|i| i.windows.contains(&window)) else {
            crate::log!("effect ({what}) skipped: window {id:#x} is not one of the dock's apps");
            return false;
        };
        let place = match self.rects.get(&id).copied() {
            Some(r) => Some(r),
            // Minimised since before the dock started: ask Windows where it will come back.
            None if restoring => shell::restored_rect(window).map(|r| (r, r)),
            None => None,
        };
        let Some((whole, from)) = place else {
            crate::log!("effect ({what}) skipped: no known position for window {id:#x}");
            return false;
        };
        // Only on the dock's own monitor, and not when the window is entirely off it.
        let mon = self.monitor_rect;
        if from.right <= mon.left || from.left >= mon.right || from.bottom <= mon.top || from.top >= mon.bottom {
            crate::log!("effect ({what}) skipped: window {id:#x} is on another screen ({from:?})");
            return false;
        }
        // The icon at its resting size, in screen pixels.
        let (l, _, r, _) = self.icon_rect(index);
        let cx = (l + r) * 0.5 + self.win_x as f32;
        let bottom = self.m.baseline + self.win_y as f32;
        let to = crate::warp::R { l: cx - self.m.icon * 0.5, t: bottom - self.m.icon, r: cx + self.m.icon * 0.5, b: bottom };
        let duration = self.cfg.minimize_time;
        self.fx_release = None;
        let started = match self.fx.as_mut() {
            Some(fx) => fx.start(&self.glass, window, whole, from, to, mon, style, restoring, duration),
            None => false,
        };
        crate::log!("effect ({what}, {style:?}) for '{}' from ({}, {}, {}, {}): {}", self.items[index].name, from.left, from.top, from.right, from.bottom, if started { "playing" } else { "could not start" });
        started
    }

    fn close_preview(&mut self) {
        if let Some(p) = self.preview.as_mut() {
            p.hide();
        }
        self.resting_on = None;
        self.preview_left = None;
    }

    /// Opens (or rebuilds) the window preview for an item.
    fn open_preview(&mut self, index: usize) {
        let Some(item) = self.items.get(index) else { return };
        if item.windows.is_empty() {
            self.close_preview();
            return;
        }
        let (l, _, r, _) = self.icon_rect(index);
        let m = self.m;
        let layout = preview::Layout {
            anchor_x: self.win_x + ((l + r) * 0.5).round() as i32,
            // Clear of the icon at its largest.
            bottom_y: self.win_y + (m.baseline - m.icon * (1.0 + m.amp) - 10.0 * m.scale).round() as i32,
            monitor: self.monitor_rect,
            scale: m.scale,
            size: self.cfg.preview_size,
            dark: self.colors.dark,
        };
        let (key, windows) = (item.key.clone(), item.windows.clone());
        if let Some(p) = self.preview.as_mut() {
            match p.show(&self.glass, &self.gfx, &key, &windows, &layout) {
                Ok(()) => crate::log!("preview opened for '{key}' ({} windows)", windows.len()),
                Err(e) => {
                    crate::log!("could not show the window preview: {e}");
                    p.hide();
                }
            }
        }
    }

    /// Decides each frame whether the preview should open, stay or close.
    fn update_preview(&mut self, cursor: POINT, dt: f32) {
        if self.preview.is_none() {
            return;
        }
        if !self.cfg.preview {
            self.close_preview();
            return;
        }
        // The running app under the cursor, if any.
        let over = self.hovered.filter(|&i| self.items.get(i).map(|it| it.is_app() && !it.windows.is_empty()).unwrap_or(false));
        let over_key = over.map(|i| self.items[i].key.clone());
        let open_key = self.preview.as_ref().and_then(|p| p.key().map(str::to_string));

        match (&open_key, &over_key) {
            (Some(open), Some(key)) if open != key => {
                // Slid over to another running app: switch at once.
                self.open_preview(over.unwrap());
                self.preview_left = None;
            }
            (Some(_), Some(_)) => self.preview_left = None,
            (Some(_), None) => {
                // Off the icon: stay open while the cursor is on the panel, or on its way there.
                let on_panel = self.preview.as_ref().map(|p| p.contains(cursor, (12.0 * self.m.scale) as i32)).unwrap_or(false);
                if on_panel {
                    self.preview_left = None;
                } else if self.preview_left.get_or_insert_with(Instant::now).elapsed().as_secs_f32() > 0.3 {
                    self.close_preview();
                }
            }
            (None, Some(key)) => {
                let resting = match &self.resting_on {
                    Some((k, since)) if k == key => since.elapsed().as_secs_f32(),
                    _ => {
                        self.resting_on = Some((key.clone(), Instant::now()));
                        0.0
                    }
                };
                let busy = self.pressed.is_some() || self.fx.as_ref().map(|f| f.is_playing()).unwrap_or(false);
                let blocked = self.preview_block.as_ref() == Some(key);
                if !blocked {
                    self.preview_block = None;
                }
                if resting >= self.cfg.preview_delay && !busy && !blocked {
                    self.open_preview(over.unwrap());
                }
            }
            (None, None) => {
                self.resting_on = None;
                self.preview_block = None;
            }
        }
        if let Some(p) = self.preview.as_mut() {
            p.update(cursor, dt);
        }
    }

    fn launch(&mut self, index: usize) {
        if self.items[index].is_special() {
            self.toggle_launcher();
            return;
        }
        let item = &mut self.items[index];
        let started = shell::launch(&item.target);
        if started && self.cfg.bounce && item.windows.is_empty() && item.bounce.is_none() {
            item.bounce = Some(Bounce { t: 0.0, hops: None });
        }
    }

    /// The menu of a dock icon, or of the dock itself (`index` = None).
    fn show_menu(&mut self, index: Option<usize>) {
        // The launcher button is not an app: it only gets the dock's own menu.
        let index = index.filter(|&i| !self.items[i].is_special());
        let entries = menus::dock(index.map(|i| (self.items[i].name.as_str(), self.items[i].windows.len(), self.items[i].pinned)), !self.taskbar.hidden, system::autostart_enabled());
        let m = self.m;
        // Standing on the icon at its largest, or at the cursor over the dock's own glass.
        let center = match index {
            Some(i) => {
                let (l, _, r, _) = self.icon_rect(i);
                self.win_x + ((l + r) * 0.5).round() as i32
            }
            None => {
                let mut pt = POINT::default();
                unsafe {
                    let _ = GetCursorPos(&mut pt);
                }
                pt.x
            }
        };
        let bottom = self.win_y + (m.baseline - m.icon * (1.0 + m.amp) - 10.0 * m.scale).round() as i32;
        let key = index.map(|i| self.items[i].key.clone());
        if !self.open_menu(entries, menu::Anchor::Above { center, bottom }, None, key) {
            self.show_native_menu(index, false);
        }
    }

    /// Opens the menu that hangs from a part of the top bar.
    fn open_bar_menu(&mut self, zone: bar::Zone) {
        let Some((left, top)) = self.bar.as_ref().and_then(|b| b.drop_point(zone)) else { return };
        self.note_menu_target();
        let target = self.menu_target;
        let has_window = self.items.iter().any(|i| i.windows.contains(&target));
        let entries = match zone {
            bar::Zone::Logo => menus::logo(!self.taskbar.hidden, system::autostart_enabled()),
            bar::Zone::App => menus::app(&self.front_name, has_window),
            bar::Zone::Title(index) => menus::titled(index, has_window),
            bar::Zone::Volume => {
                self.last_status = self.status.read();
                let (level, muted) = self.last_status.volume.unwrap_or((0.0, true));
                menus::sound(level, muted)
            }
            bar::Zone::Battery => menus::battery(system::battery_detail()),
            bar::Zone::Network => {
                let scan = wifi::scan();
                self.wifi_list = match &scan {
                    wifi::Scan::Found(list) => list.clone(),
                    _ => Vec::new(),
                };
                menus::wifi(&menus::WifiView {
                    scan: &scan,
                    online: self.last_status.online,
                    asking: self.wifi_asking.as_deref(),
                    joining: self.wifi_joining.as_ref().map(|j| j.name.as_str()),
                    note: self.wifi_note.as_deref(),
                })
            }
            _ => return,
        };
        let gap = (2.0 * self.m.scale).round() as i32;
        if !self.open_menu(entries, menu::Anchor::Below { left, top: top + gap }, Some(zone), None) && zone == bar::Zone::Logo {
            self.show_native_menu(None, true);
        }
        if zone == bar::Zone::Network {
            self.show_wifi_side(None);
        } else if let Some(side) = self.side.as_mut() {
            side.close();
        }
    }

    /// Opens (or closes) the password panel beside the Wi-Fi list, level with the network it
    /// is for. `typed` is what was already typed, if the panel is being shown again.
    fn show_wifi_side(&mut self, typed: Option<&str>) {
        let network = self.wifi_asking.as_ref().and_then(|name| self.wifi_list.iter().position(|n| &n.name == name));
        let row = network.map(|i| menus::WIFI_NETWORK | i);
        let rect = row.and_then(|id| self.menu.as_ref().and_then(|m| m.row_rect(id)));
        let (Some(row), Some((left, top, right, _))) = (row, rect) else {
            if let Some(side) = self.side.as_mut() {
                side.close();
            }
            if let Some(menu) = self.menu.as_mut() {
                menu.pin(None);
            }
            return;
        };
        let name = self.wifi_asking.clone().unwrap_or_default();
        let entries = menus::wifi_password(&name, self.wifi_note.as_deref());
        let Some(side) = self.side.as_mut() else { return };
        if let Err(e) = side.open(&self.glass, &self.gfx, entries, menu::Anchor::Beside { left, right, top }, self.monitor_rect, self.m.scale, self.colors.dark, self.colors.accent) {
            crate::log!("could not show the password panel: {e}");
            side.close();
            return;
        }
        if let Some(text) = typed {
            side.set_typed(text);
        }
        if let Some(menu) = self.menu.as_mut() {
            menu.pin(Some(row));
        }
    }

    /// A click in the Wi-Fi list: join a network, ask for its password, or look again.
    fn wifi_command(&mut self, id: usize) {
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
            menus::WIFI_CANCEL => self.wifi_asking = None,
            menus::WIFI_REFRESH => {
                wifi::refresh();
                self.wifi_note = Some("Đang tìm lại, mở lại menu sau vài giây".to_string());
            }
            menus::WIFI_JOIN | menus::WIFI_PASSWORD => {
                let password = self.side.as_ref().map(|m| m.typed()).unwrap_or_default();
                let network = self.wifi_asking.as_ref().and_then(|name| self.wifi_list.iter().find(|n| &n.name == name)).cloned();
                if let Some(network) = network {
                    if password.chars().count() < 8 {
                        // Only the panel changes: what was typed stays.
                        self.wifi_note = Some("Mật khẩu Wi-Fi có ít nhất 8 ký tự".to_string());
                        self.show_wifi_side(Some(&password));
                        return;
                    } else {
                        let (joining, note) = begin(wifi::join_new(&network, &password), &network.name, true);
                        if joining.is_some() {
                            self.wifi_asking = None;
                        }
                        (self.wifi_joining, self.wifi_note) = (joining, note);
                    }
                }
            }
            id => {
                let Some(network) = menus::wifi_index(id).and_then(|i| self.wifi_list.get(i)).cloned() else { return };
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
        }
        self.open_bar_menu(bar::Zone::Network);
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
        if self.menu_zone == Some(bar::Zone::Network) {
            self.open_bar_menu(bar::Zone::Network);
        }
    }

    /// Remembers the window in front, unless it is one of the dock's own (an open menu).
    fn note_menu_target(&mut self) {
        let front = unsafe { GetForegroundWindow() };
        if !shell::is_own_window(front) {
            self.menu_target = front;
        }
    }

    /// Shows the dock's menu with these rows. Returns false if it could not be shown.
    fn open_menu(&mut self, entries: Vec<menu::Entry>, anchor: menu::Anchor, zone: Option<bar::Zone>, item: Option<String>) -> bool {
        self.note_menu_target();
        self.close_preview();
        let Some(menu) = self.menu.as_mut() else { return false };
        if let Err(e) = menu.open(&self.glass, &self.gfx, entries, anchor, self.monitor_rect, self.m.scale, self.colors.dark, self.colors.accent) {
            crate::log!("could not show the menu: {e}");
            menu.close();
            return false;
        }
        self.menu_zone = zone;
        self.menu_item = item;
        if let Some(bar) = self.bar.as_mut() {
            bar.set_open(zone);
        }
        true
    }

    /// Opens the date panel under the clock.
    fn open_calendar(&mut self) {
        let Some((_, right, bottom)) = self.bar.as_ref().and_then(|b| b.span(bar::Zone::Clock)) else { return };
        self.note_menu_target();
        self.close_preview();
        let Some(calendar) = self.calendar.as_mut() else {
            // No panel of our own: Windows' notifications and calendar.
            system::press_windows_key('n');
            return;
        };
        let gap = (2.0 * self.m.scale).round() as i32;
        let edge = right - (4.0 * self.m.scale).round() as i32;
        match calendar.open(&self.glass, &self.gfx, system::today(), edge, bottom + gap, self.monitor_rect, self.m.scale, self.colors.dark, self.colors.accent) {
            Ok(()) => {
                if let Some(bar) = self.bar.as_mut() {
                    bar.set_open(Some(bar::Zone::Clock));
                }
            }
            Err(e) => {
                crate::log!("could not show the date panel: {e}");
                calendar.close();
            }
        }
    }

    /// Closes whichever drop-down is open: a menu or the date panel.
    fn close_menu(&mut self) {
        if let Some(menu) = self.menu.as_mut() {
            menu.close();
        }
        if let Some(side) = self.side.as_mut() {
            side.close();
        }
        if let Some(calendar) = self.calendar.as_mut() {
            calendar.close();
        }
        self.menu_zone = None;
        self.menu_item = None;
        if let Some(bar) = self.bar.as_mut() {
            bar.set_open(None);
        }
    }

    /// Carries out a menu command. `item` is the key of the dock item the menu was opened on.
    fn run_command(&mut self, id: usize, item: Option<String>) {
        let index = item.and_then(|key| self.items.iter().position(|i| i.key == key));
        let target = self.menu_target;
        let target_ok = !target.is_invalid() && unsafe { IsWindow(Some(target)).as_bool() };
        match id {
            0 => {}
            menus::OPEN => {
                if let Some(i) = index {
                    self.launch(i);
                }
            }
            menus::PIN => {
                if let Some(i) = index {
                    self.toggle_pin(i);
                }
            }
            menus::CLOSE => {
                if let Some(i) = index {
                    for &w in &self.items[i].windows {
                        shell::close(w);
                    }
                }
            }
            menus::TASKBAR => {
                let mut cfg = self.cfg.clone();
                cfg.hide_taskbar = !self.taskbar.hidden;
                cfg.save();
                self.cfg_stamp = config::config_modified();
                settings::sync(&cfg);
                self.apply_config(cfg, true);
            }
            menus::AUTOSTART => {
                system::set_autostart(!system::autostart_enabled());
            }
            menus::SETTINGS => settings::open(&self.cfg),
            menus::QUIT => self.quit = true,
            menus::WINDOWS_SETTINGS => {
                shell::launch("ms-settings:");
            }
            menus::LOCK => system::lock_screen(),
            menus::SLEEP => system::sleep(),
            menus::RESTART => {
                if system::confirm("Khởi động lại máy ngay bây giờ?") {
                    system::run("shutdown.exe", "/r /t 0");
                }
            }
            menus::SHUT_DOWN => {
                if system::confirm("Tắt máy ngay bây giờ?") {
                    system::run("shutdown.exe", "/s /t 0");
                }
            }
            menus::SIGN_OUT => {
                if system::confirm("Đăng xuất ngay bây giờ?") {
                    system::run("shutdown.exe", "/l");
                }
            }
            menus::MUTE => {
                let muted = self.status.read().volume.map(|(_, muted)| muted).unwrap_or(false);
                self.status.set_muted(!muted);
                self.last_status = self.status.read();
            }
            menus::SOUND_SETTINGS => {
                shell::launch("ms-settings:sound");
            }
            menus::BATTERY_SETTINGS => {
                shell::launch("ms-settings:powersleep");
            }
            menus::LOCATION_SETTINGS => {
                shell::launch("ms-settings:privacy-location");
            }
            menus::NETWORK_SETTINGS => {
                shell::launch("ms-settings:network");
            }
            menus::LAUNCHER => self.toggle_launcher(),
            menus::BEN_FOLDER => {
                shell::launch(&crate::util::data_dir().to_string_lossy());
            }
            menus::WINDOW_MINIMIZE if target_ok => shell::minimize(target),
            menus::WINDOW_MAXIMIZE if target_ok => {
                shell::activate(target);
                shell::toggle_maximize(target);
            }
            menus::WINDOW_CLOSE if target_ok => shell::close(target),
            menus::APP_QUIT if target_ok => {
                if let Some(app) = self.items.iter().find(|i| i.windows.contains(&target)) {
                    for &w in &app.windows {
                        shell::close(w);
                    }
                }
            }
            id => {
                if let Some((mods, key)) = menus::shortcut(id) {
                    // The shortcut goes to the window that was in front before the menu opened.
                    if target_ok {
                        shell::activate(target);
                    }
                    system::press_keys(mods, key);
                }
            }
        }
        self.refresh_due = true;
    }

    /// Windows' own menu, for when the dock's menu cannot be made. `below`: open it downwards
    /// from the cursor (from the top bar), not upwards.
    fn show_native_menu(&mut self, index: Option<usize>, below: bool) {
        use menus::{AUTOSTART, CLOSE, OPEN, PIN, QUIT, SETTINGS, TASKBAR};

        let chosen = unsafe {
            let Ok(menu) = CreatePopupMenu() else { return };
            let add = |flags: MENU_ITEM_FLAGS, id: usize, text: &str| {
                let _ = AppendMenuW(menu, flags, id, PCWSTR(wide(text).as_ptr()));
            };
            if let Some(item) = index.map(|i| &self.items[i]) {
                add(MF_STRING | MF_GRAYED, 0, &item.name);
                add(MF_SEPARATOR, 0, "");
                add(MF_STRING, OPEN, if item.windows.is_empty() { "Mở" } else { "Mở cửa sổ mới" });
                add(MF_STRING, PIN, if item.pinned { "Gỡ khỏi Bến" } else { "Giữ trong Bến" });
                if !item.windows.is_empty() {
                    add(MF_STRING, CLOSE, if item.windows.len() > 1 { "Đóng tất cả cửa sổ" } else { "Đóng" });
                }
                add(MF_SEPARATOR, 0, "");
            }
            let check = |on: bool| if on { MF_STRING | MF_CHECKED } else { MF_STRING };
            add(MF_STRING, SETTINGS, "Cài đặt…");
            add(check(!self.taskbar.hidden), TASKBAR, "Hiện taskbar của Windows");
            add(check(system::autostart_enabled()), AUTOSTART, "Khởi động cùng Windows");
            add(MF_SEPARATOR, 0, "");
            add(MF_STRING, QUIT, "Thoát Bến");

            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // A popup menu only closes on an outside click if its owner is the foreground window.
            let _ = SetForegroundWindow(self.hwnd);
            let align = if below { TPM_TOPALIGN } else { TPM_BOTTOMALIGN };
            let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | align | TPM_LEFTALIGN | TPM_RIGHTBUTTON, pt.x, pt.y, None, self.hwnd, None);
            let _ = PostMessageW(Some(self.hwnd), WM_NULL, WPARAM(0), LPARAM(0));
            let _ = DestroyMenu(menu);
            cmd.0 as usize
        };

        let item = index.map(|i| self.items[i].key.clone());
        self.run_command(chosen, item);
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
            if cfg.reserve_space {
                self.appbar.register(WM_APPBAR_CALLBACK);
            } else {
                self.appbar.remove();
            }
        }
        if cfg.blur != old.blur || cfg.blur_radius != old.blur_radius {
            let sigma = (cfg.blur && cfg.blur_radius > 0.0).then_some(cfg.blur_radius * self.m.scale);
            self.glass.set_blur(sigma);
        }
        let colors = Colors { dark: is_dark(&cfg), accent: system::accent_color() };
        if colors.dark != self.colors.dark {
            self.forget_labels();
        }
        self.colors = colors;
        if let Err(e) = self.glass.set_palette(&palette(&cfg, &self.colors)) {
            crate::log!("palette update failed: {e}");
        }
        if let Some(p) = &self.preview {
            p.set_palette(&palette(&cfg, &self.colors));
        }
        if (cfg.minimize_effect >= 0.5) != (old.minimize_effect >= 0.5) {
            if cfg.minimize_effect >= 0.5 {
                system::disable_window_animations();
            } else {
                system::restore_window_animations();
            }
        }
        if cfg.preview_size != old.preview_size || !cfg.preview {
            self.close_preview();
        }
        // The bar's and the launcher's glass is made with the blur built in. If the blur they
        // were made with is no longer the one in the settings, make them again (once the
        // slider has been released, not on every step of it).
        let sigma = (cfg.blur && cfg.blur_radius > 0.0).then_some(cfg.blur_radius * self.m.scale);
        if done && sigma != self.popup_sigma {
            self.close_menu();
            self.menu = None;
            self.side = None;
            self.calendar = None;
            self.bar = None;
            self.launcher = None;
            self.app_scan = None;
        }
        if done || cfg.top_bar != old.top_bar || cfg.launcher != old.launcher {
            self.setup_bar_and_launcher();
        }
        if let Some(bar) = &self.bar {
            bar.set_palette(&bar_palette(&cfg, &self.colors));
        }

        // If sizes changed, lay the window out again. The screen strip that other windows must
        // stay out of is only re-requested once the change is final.
        let monitor = primary_monitor();
        let scale = monitor.dpi as f32 / 96.0;
        let resized = metrics(&cfg, scale, monitor.rect.right - monitor.rect.left) != self.m;
        if resized || done {
            self.place_window(&monitor, done);
        }
        self.refresh_due = true;
    }

    fn toggle_pin(&mut self, index: usize) {
        let mut item = self.items.remove(index);
        item.pinned = !item.pinned;
        if item.pinned {
            // Pinned apps live to the left of the separator.
            let sep = self.items.iter().position(|i| i.kind == Kind::Separator).unwrap_or(self.items.len());
            self.items.insert(sep, item);
        } else {
            self.items.push(item);
        }
        self.hovered = None;
        self.pressed = None;
        self.label_index = None;
        self.save_pins();
    }

    // ---------------------------------------------------------------- update

    /// Re-reads which apps have windows and which one is in front.
    fn refresh_windows(&mut self) {
        self.refresh_due = false;
        let windows = self.tracker.scan(self.hwnd);
        let mut front = unsafe { GetForegroundWindow() };
        let mut popups: Vec<HWND> = Vec::new();
        popups.extend(self.menu.as_ref().filter(|m| m.is_open()).map(|m| m.hwnd()));
        popups.extend(self.side.as_ref().filter(|m| m.is_open()).map(|m| m.hwnd()));
        popups.extend(self.calendar.as_ref().filter(|c| c.is_open()).map(|c| c.hwnd()));
        if !popups.is_empty() {
            if popups.contains(&front) {
                // An open menu must not change which app counts as being in front.
                front = self.menu_target;
            } else {
                // Something else came to the front without the menu being told: close it.
                crate::log!("menu closed: another window is in front");
                self.close_menu();
            }
        }

        for item in &mut self.items {
            item.windows.clear();
        }
        for w in windows {
            match self.items.iter_mut().find(|i| i.is_app() && i.key == w.key) {
                Some(item) => item.windows.push(w.hwnd),
                None => {
                    let mut item = Item::app(w.key, w.target, false, false);
                    item.windows.push(w.hwnd);
                    self.items.push(item);
                }
            }
        }

        let mut pinned = 0;
        let mut unpinned = 0;
        for item in self.items.iter_mut().filter(|i| i.is_app()) {
            let running = !item.windows.is_empty();
            item.running.set(if running { 1.0 } else { 0.0 });
            item.active.set(if item.windows.contains(&front) { 1.0 } else { 0.0 });
            let stays = item.pinned || running || item.bounce.is_some();
            item.presence.set(if stays { 1.0 } else { 0.0 });
            if item.pinned {
                pinned += 1;
            } else if stays {
                unpinned += 1;
            }
        }
        if let Some(sep) = self.items.iter_mut().find(|i| i.kind == Kind::Separator) {
            sep.presence.set(if pinned > 0 && unpinned > 0 { 1.0 } else { 0.0 });
        }

        // Remember where every open window is, for the minimise effect.
        let mut seen = Vec::new();
        for item in &self.items {
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
        self.rects.retain(|id, _| seen.contains(id));

        // If the previewed app's windows changed, rebuild the preview (or close it).
        let open = self.preview.as_ref().and_then(|p| p.key().map(|k| (k.to_string(), p.windows())));
        if let Some((key, shown)) = open {
            match self.items.iter().position(|i| i.key == key) {
                Some(index) if !self.items[index].windows.is_empty() => {
                    let now = &self.items[index].windows;
                    if shown.len() != now.len() || shown.iter().any(|w| !now.contains(w)) {
                        self.open_preview(index);
                    }
                }
                _ => self.close_preview(),
            }
        }

        // The top bar names the app in front.
        self.front_name = self.items.iter().find(|i| i.is_app() && i.windows.contains(&front)).map(|i| i.name.clone()).unwrap_or_else(|| "Màn hình nền".to_string());

        // Step out of the way of games, videos and presentations.
        let fullscreen = self.cfg.hide_on_fullscreen && front != self.hwnd && shell::is_fullscreen(front);
        self.hide.set(if fullscreen { 1.0 } else { 0.0 });
        if fullscreen {
            self.close_preview();
        }
    }

    /// Advances all motion by `dt` seconds and updates the glass panel.
    /// Returns true while something is still moving (another frame is needed).
    pub fn update(&mut self, dt: f32) -> bool {
        if self.refresh_due {
            self.refresh_windows();
        }
        let m = self.m;

        // Cursor, read directly every frame so magnification tracks it with no lag.
        let mut pt = POINT::default();
        let mut have_cursor = unsafe { GetCursorPos(&mut pt) }.is_ok();
        let mut cx = (pt.x - self.win_x) as f32;
        let mut cy = (pt.y - self.win_y) as f32;
        if let Some((x, y)) = self.bench_cursor() {
            (cx, cy, have_cursor) = (x, y, true);
        }
        let shown = self.hide.target == 0.0;
        self.hovering = have_cursor && shown && self.cursor_in_dock(cx, cy);
        if self.hovering {
            self.cursor_x = cx;
        }
        let speed = self.cfg.anim_speed;
        self.mag.omega = self.cfg.mag_speed;
        self.hide.omega = 13.0 * speed;
        self.mag.set(if self.hovering { 1.0 } else { 0.0 });
        self.mag.step(dt);
        self.hide.step(dt);

        // Per-item motion.
        let bounce_h = m.bounce_h;
        let hop = self.cfg.bounce_time.max(0.05);
        for (index, item) in self.items.iter_mut().enumerate() {
            item.presence.omega = 18.0 * speed;
            item.running.omega = 22.0 * speed;
            item.active.omega = 22.0 * speed;
            item.presence.step(dt);
            item.running.step(dt);
            item.active.step(dt);
            item.press.set(if self.pressed == Some(index) { 1.0 } else { 0.0 });
            item.press.step(dt);
            if !(self.pressed == Some(index) && self.drag.is_some()) {
                item.shift.step(dt);
            }
            if let Some(b) = &mut item.bounce {
                let before = (b.t / hop).floor();
                b.t += dt;
                let after = (b.t / hop).floor();
                let mut done = false;
                if after > before {
                    // Only stop when the icon is back on the ground.
                    done = match b.hops {
                        Some(n) => after as u32 >= n,
                        None => !item.windows.is_empty() || b.t > 10.0,
                    };
                }
                if done {
                    item.bounce = None;
                    item.bounce_y = 0.0;
                } else {
                    let f = (b.t / hop).fract();
                    item.bounce_y = bounce_h * 4.0 * f * (1.0 - f);
                }
            }
        }

        // Drop apps that have finished sliding out.
        let before = self.items.len();
        self.items.retain(|item| !(item.is_app() && !item.pinned && item.presence.target == 0.0 && item.presence.pos < 0.004));
        if self.items.len() != before {
            self.pressed = None;
            self.label_index = None;
            self.refresh_due = true;
        }

        // Dragging an icon sideways moves it to a new place among its neighbours.
        let moved = self.drag_icon(cx);

        // Layout: rest widths, then the magnifier stretches them around the cursor.
        self.widths.clear();
        for item in &self.items {
            let base = if item.is_app() { m.slot } else { m.separator };
            self.widths.push(base * item.presence.pos.clamp(0.0, 1.0));
        }
        let magnifier = Magnifier { amp: m.amp * self.mag.pos, radius: m.mag_radius, center: self.cursor_x };
        layout(&self.widths, m.win_w as f32 * 0.5, &magnifier, &mut self.slots);

        // An icon that just changed places starts from where it was and glides to its new slot.
        for (index, was_at) in moved {
            if let (Some(slot), Some(item)) = (self.slots.get(index), self.items.get_mut(index)) {
                item.shift.pos = was_at - slot.center();
                item.shift.vel = 0.0;
            }
        }
        // The dragged icon itself stays under the cursor.
        if let (Some(index), Some(grab)) = (self.pressed, self.drag) {
            if let (Some(slot), Some(item)) = (self.slots.get(index), self.items.get_mut(index)) {
                item.shift.pos = (cx - grab) - slot.center();
                item.shift.vel = 0.0;
            }
        }

        self.hovered = if self.hovering && self.drag.is_none() { self.app_at(cx, cy) } else { None };
        if let Some(h) = self.hovered {
            self.label_index = Some(h);
        }
        // Window effects.
        let glass = &self.glass;
        if let Some(done) = self.fx.as_mut().and_then(|f| f.step(glass, dt)) {
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
                let now = shell::frame_rect(window);
                crate::log!("picture taken away {:.0} ms after the window was asked back; window is {} at {:?}, the picture ended at {:?}", waited * 1000.0, if back { "back" } else { "still minimised" }, now, before);
                if let Some(fx) = self.fx.as_mut() {
                    fx.stop();
                }
            }
        }
        self.update_preview(pt, dt);
        let previewing = self.preview.as_ref().map(|p| p.is_open()).unwrap_or(false);

        // Top bar and launcher.
        let mut extra_motion = false;
        if let Some(bar) = self.bar.as_mut() {
            let hidden = self.hide.target == 1.0;
            match bar.update(&self.glass, &self.gfx, &self.front_name, &self.last_status, &self.clock, self.colors.dark, hidden, dt) {
                Ok(moving) => extra_motion |= moving,
                Err(e) => self.note_error(e),
            }
        }
        if let Some(menu) = self.menu.as_mut() {
            if let Err(e) = menu.update(&self.glass, &self.gfx) {
                self.note_error(e);
            }
        }
        if let Some(side) = self.side.as_mut() {
            if let Err(e) = side.update(&self.glass, &self.gfx) {
                self.note_error(e);
            }
        }
        if let Some(calendar) = self.calendar.as_mut() {
            if let Err(e) = calendar.update(&self.glass, &self.gfx) {
                self.note_error(e);
            }
        }
        if let Some(launcher) = self.launcher.as_mut() {
            if launcher.is_visible() {
                if let Some(rx) = self.app_scan.as_ref() {
                    launcher.absorb(rx.try_iter());
                }
            }
            match launcher.update(&self.glass, &self.gfx, pt, dt) {
                Ok(visible) => extra_motion |= visible,
                Err(e) => self.note_error(e),
            }
        }

        // The name label gives way to the preview.
        self.label_alpha.set(if self.hovered.is_some() && self.cfg.labels && !previewing { 1.0 } else { 0.0 });
        self.label_alpha.step(dt);

        // Glass panel: hugs the icons and grows with them.
        let (left, right) = match (self.slots.first(), self.slots.last()) {
            (Some(a), Some(b)) => (a.left, b.right),
            _ => (m.win_w as f32 * 0.5, m.win_w as f32 * 0.5),
        };
        // Optionally the panel also rises with the tallest icon.
        let peak = self.slots.iter().zip(&self.items).filter(|(_, i)| i.is_app()).map(|(s, _)| s.scale).fold(1.0, f32::max);
        let rise = (peak - 1.0) * m.icon * self.cfg.panel_grow;
        self.panel = PanelRect { x: left - m.pad_x, y: m.panel_top - rise, w: (right - left) + m.pad_x * 2.0, h: m.panel_h + rise, radius: m.radius };
        let slide = (self.hide.pos * (m.panel_h + m.margin + 6.0 * m.scale)).round();

        // Everything below goes to the compositor as one batch: panel, icons, indicators and
        // label always change in the same frame.
        if let Err(e) = self.glass.set_panel(self.panel).and_then(|_| self.glass.set_slide(slide)).and_then(|_| self.show_items()).and_then(|_| self.show_label()) {
            self.note_error(e);
        }
        self.glass.commit();

        self.update_region();

        self.bench.is_some()
            || extra_motion
            || previewing
            || self.resting_on.is_some()
            || self.pressed.is_some()
            || self.fx.as_ref().map(|f| f.is_playing()).unwrap_or(false)
            || self.hovering
            || self.refresh_due
            || !self.mag.settled(1e-3)
            || !self.hide.settled(1e-3)
            || !self.label_alpha.settled(1e-3)
            || self.items.iter().any(|i| !i.settled())
    }

    /// Handles dragging an icon sideways. Returns the icons that changed places this frame, as
    /// (new index, where its centre was on screen), so they can glide instead of jumping.
    fn drag_icon(&mut self, cx: f32) -> Vec<(usize, f32)> {
        let mut moved = Vec::new();
        let Some(mut index) = self.pressed else { return moved };
        if index >= self.items.len() || index >= self.slots.len() {
            return moved;
        }
        if self.drag.is_none() {
            // A press turns into a drag once the cursor has clearly moved.
            if (cx - self.press_x).abs() < 6.0 * self.m.scale {
                return moved;
            }
            if self.items[index].is_special() {
                return moved;
            }
            self.drag = Some(self.press_x - self.slots[index].center());
            self.close_preview();
        }
        let grab = self.drag.unwrap_or(0.0);
        let centre = cx - grab;
        // Swap with a neighbour each time the dragged icon passes its middle. Pinned apps stay
        // among pinned apps, open ones among open ones.
        loop {
            let pinned = self.items[index].pinned;
            let same_group = |item: &Item| item.is_app() && !item.is_special() && item.pinned == pinned && item.presence.target > 0.5;
            let right = index + 1 < self.items.len() && index + 1 < self.slots.len() && same_group(&self.items[index + 1]) && centre > self.slots[index + 1].center();
            let left = index > 0 && same_group(&self.items[index - 1]) && centre < self.slots[index - 1].center();
            let other = if right {
                index + 1
            } else if left {
                index - 1
            } else {
                break;
            };
            // The neighbour takes the dragged icon's slot; remember where it was drawn.
            let was_at = self.slots[other].center() + self.items[other].shift.pos;
            self.items.swap(index, other);
            self.slots.swap(index, other);
            moved.push((index, was_at));
            index = other;
            self.pressed = Some(index);
            if moved.len() > self.items.len() {
                break;
            }
        }
        if !moved.is_empty() {
            self.label_index = None;
        }
        moved
    }

    /// Logs an error from updating visuals, once per distinct error (it would repeat every frame).
    fn note_error(&mut self, e: windows::core::Error) {
        if self.shown_error.as_ref() != Some(&e) {
            crate::log!("could not update visuals: {e}");
            self.shown_error = Some(e);
        }
    }

    /// Whether the cursor (window coordinates) is over the dock: the panel, the strip below it,
    /// or the part of a magnified icon that sticks out above the panel.
    fn cursor_in_dock(&self, cx: f32, cy: f32) -> bool {
        if self.slots.is_empty() || cx < self.panel.x - 2.0 || cx > self.panel.x + self.panel.w + 2.0 || cy >= self.m.win_h as f32 {
            return false;
        }
        let mut top = self.panel.y;
        if let Some(index) = self.slots.iter().position(|s| cx >= s.left && cx < s.right) {
            if self.items.get(index).map(|i| i.is_app()).unwrap_or(false) {
                top = top.min(self.icon_rect(index).1);
            }
        }
        cy >= top - 1.0
    }

    /// Limits which part of the window takes mouse input. At rest only the panel does, so clicks
    /// just above the dock reach the windows behind it. While icons are magnified or bouncing the
    /// area grows to cover them.
    fn update_region(&mut self) {
        let m = self.m;
        let hidden = self.hide.target == 1.0 && self.hide.pos > 0.97;
        let lively = self.mag.pos > 0.001 || self.label_alpha.pos > 0.001 || self.items.iter().any(|i| i.bounce.is_some());
        let wanted = if hidden {
            (0, 0, 0, 0)
        } else if lively {
            let grow = m.amp * m.mag_radius * 0.5 + 24.0 * m.scale;
            let rest_left = m.win_w as f32 * 0.5 - self.widths.iter().sum::<f32>() * 0.5 - m.pad_x;
            let rest_right = m.win_w as f32 - rest_left;
            ((rest_left - grow).floor() as i32, 0, (rest_right + grow).ceil() as i32, m.win_h)
        } else {
            ((self.panel.x - 2.0).floor() as i32, (m.panel_top - 2.0).floor() as i32, (self.panel.x + self.panel.w + 2.0).ceil() as i32, m.win_h)
        };
        if self.region == Some(wanted) {
            return;
        }
        self.region = Some(wanted);
        unsafe {
            // The system takes ownership of the region handle.
            let rgn = CreateRectRgn(wanted.0, wanted.1, wanted.2, wanted.3);
            SetWindowRgn(self.hwnd, Some(rgn), false);
        }
    }

    pub fn stats_enabled(&self) -> bool {
        self.cfg.stats || self.bench.is_some()
    }

    pub fn item_count(&self) -> usize {
        self.items.len()
    }

    /// Name of a self-test phase that has just ended (so its timing can be logged under that name).
    pub fn bench_phase_finished(&mut self) -> Option<&'static str> {
        self.bench_last.take().or_else(|| self.bench.as_mut().and_then(|b| b.finished.take()))
    }

    /// Advances the self-test and returns where its imaginary cursor is, in window coordinates.
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
                crate::log!("bench: finished");
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
        let centre = self.panel.x + self.panel.w * 0.5;
        let reach = (self.panel.w * 0.5 - self.m.icon * 0.5).max(1.0);
        let x = centre + reach * (t * 2.2).sin();
        Some((x, self.m.panel_top + self.m.panel_h * 0.5))
    }

    // --------------------------------------------------------------- visuals

    /// Positions every icon, indicator and the separator for this frame.
    fn show_items(&mut self) -> Result<()> {
        let m = self.m;
        let dark = self.colors.dark;
        let (ar, ag, ab) = self.colors.accent;
        let neutral = if dark { (1.0, 1.0, 1.0, 0.72) } else { (0.0, 0.0, 0.0, 0.55) };
        let levels = icon_levels(&m);
        let show_indicator = self.cfg.indicator;
        let Dock { items, slots, glass, .. } = self;

        for (index, item) in items.iter_mut().enumerate() {
            let Some(&slot) = slots.get(index) else { break };
            let presence = item.presence.pos.clamp(0.0, 1.0);
            let cx = slot.center();

            if item.kind == Kind::Separator {
                if item.mark.is_none() {
                    item.mark = Some(glass.pill(false)?);
                }
                let line = item.mark.as_mut().unwrap();
                let c = if dark { 1.0 } else { 0.0 };
                let inset = m.panel_h * 0.22;
                let w = m.scale.max(1.0).round();
                line.place((cx - w * 0.5).round(), m.panel_top + inset, w, m.panel_h - inset * 2.0, 0.0)?;
                line.set_color(rgba(c, c, c, if dark { 0.22 } else { 0.18 }))?;
                line.set_opacity(presence)?;
                continue;
            }

            // Paint the icon at each size it will be shown at, once.
            if item.surfaces.is_empty() {
                for &size in &levels {
                    let surface = glass.surface_from_pixels(&shell::resample(&item.pixels, size))?;
                    item.surfaces.push((size as f32, surface));
                }
                item.level = None;
            }
            if item.sprite.is_none() {
                item.sprite = Some(glass.sprite(false)?);
                item.level = None;
            }
            if item.mark.is_none() {
                item.mark = Some(glass.pill(false)?);
            }

            let (mut l, mut t, r, _) = icon_rect_of(&m, slot, item);
            let mut size = r - l;
            if (size - m.icon).abs() < 0.02 && item.bounce_y == 0.0 {
                // Standing still: sit on whole pixels so the icon is shown 1:1, perfectly sharp.
                l = l.round();
                t = t.round();
                size = m.icon;
            }
            let level = item.surfaces.iter().position(|(s, _)| *s >= size - 0.5).unwrap_or(item.surfaces.len().saturating_sub(1));
            let sprite = item.sprite.as_mut().unwrap();
            if item.level != Some(level) {
                if let Some((_, surface)) = item.surfaces.get(level) {
                    sprite.set_surface(surface)?;
                    item.level = Some(level);
                }
            }
            sprite.place(l, t, size, size)?;
            sprite.set_opacity(presence * (1.0 - 0.42 * item.press.pos.clamp(0.0, 1.0)))?;

            // Running indicator: a small pill under the icon that widens and takes the accent
            // colour while the app is in front.
            let mark = item.mark.as_mut().unwrap();
            let running = if show_indicator { item.running.pos.clamp(0.0, 1.0) } else { 0.0 };
            if running > 0.01 {
                let active = item.active.pos.clamp(0.0, 1.2);
                let mix = active.min(1.0);
                let w = (5.0 + 11.0 * active) * m.scale * (0.4 + 0.6 * running);
                let h = 3.0 * m.scale;
                let y = m.baseline + (m.pad_bottom - h) * 0.5 + 0.5 * m.scale;
                mark.place(cx - w * 0.5, y, w, h, h * 0.5)?;
                mark.set_color(rgba(
                    neutral.0 + (ar - neutral.0) * mix,
                    neutral.1 + (ag - neutral.1) * mix,
                    neutral.2 + (ab - neutral.2) * mix,
                    neutral.3 + (1.0 - neutral.3) * mix,
                ))?;
                mark.set_opacity(running * presence)?;
            } else {
                mark.set_opacity(0.0)?;
            }
        }
        Ok(())
    }

    /// App name in a pill above the hovered icon.
    fn show_label(&mut self) -> Result<()> {
        let alpha = self.label_alpha.pos.clamp(0.0, 1.0);
        let index = self.label_index.filter(|&i| alpha > 0.01 && i < self.items.len() && i < self.slots.len() && self.items[i].is_app());
        let Some(index) = index else {
            if let Some(bg) = self.label_bg.as_mut() {
                bg.set_opacity(0.0)?;
            }
            if let Some(text) = self.label_text.as_mut() {
                text.set_opacity(0.0)?;
            }
            return Ok(());
        };
        let m = self.m;
        let dark = self.colors.dark;

        if self.items[index].label.is_none() {
            unsafe {
                if self.text_format.is_none() {
                    let format = self.gfx.dwrite.CreateTextFormat(
                        PCWSTR(wide("Segoe UI Variable Text").as_ptr()),
                        None,
                        DWRITE_FONT_WEIGHT_MEDIUM,
                        DWRITE_FONT_STYLE_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        m.font,
                        PCWSTR(wide("").as_ptr()),
                    )?;
                    format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                    self.text_format = Some(format);
                }
                let text: Vec<u16> = self.items[index].name.encode_utf16().collect();
                let layout = self.gfx.dwrite.CreateTextLayout(&text, self.text_format.as_ref().unwrap(), 2000.0, 200.0)?;
                let mut tm = DWRITE_TEXT_METRICS::default();
                layout.GetMetrics(&mut tm)?;
                let (width, height) = (tm.width.ceil().max(1.0), tm.height.ceil().max(1.0));
                let fg = if dark { 1.0 } else { 0.08 };
                let surface = self.glass.surface_from_text(&layout, width, height, color(fg, fg, fg, 1.0))?;
                self.items[index].label = Some(Label { surface, width, height });
            }
        }
        if self.label_bg.is_none() {
            self.label_bg = Some(self.glass.pill(true)?);
        }
        if self.label_text.is_none() {
            self.label_text = Some(self.glass.sprite(true)?);
            self.label_shown = None;
        }

        let (l, t, r, _) = self.icon_rect(index);
        let item = &self.items[index];
        let label = item.label.as_ref().unwrap();
        let pad = (0.85 * m.font).round();
        let w = label.width + pad * 2.0;
        let h = m.label_h;
        let x = ((l + r) * 0.5 - w * 0.5).clamp(4.0, (m.win_w as f32 - w - 4.0).max(4.0)).round();
        let y = (t - m.label_gap - h).max(2.0).round();

        let bg = self.label_bg.as_mut().unwrap();
        bg.place(x, y, w, h, h * 0.5)?;
        bg.set_color(if dark { rgba(0.11, 0.11, 0.125, 0.94) } else { rgba(0.98, 0.98, 0.985, 0.96) })?;
        bg.set_opacity(alpha)?;

        let text = self.label_text.as_mut().unwrap();
        if self.label_shown.as_deref() != Some(item.key.as_str()) {
            text.set_surface(&label.surface)?;
            self.label_shown = Some(item.key.clone());
        }
        text.place(x + pad, (y + (h - label.height) * 0.5).round(), label.width, label.height)?;
        text.set_opacity(alpha)
    }

    /// Throws away painted app names (they are painted for one font size and one theme).
    fn forget_labels(&mut self) {
        self.text_format = None;
        self.label_shown = None;
        for item in &mut self.items {
            item.label = None;
        }
    }

    /// The GPU device was reset: make a new one and repaint every icon and label on it.
    fn rebuild_graphics(&mut self) {
        match Gfx::new().and_then(|g| self.glass.set_rendering_device(&g).map(|_| g)) {
            Ok(g) => {
                self.gfx = g;
                crate::log!("graphics device was reset; rebuilt");
            }
            Err(e) => {
                crate::log!("graphics rebuild failed: {e}");
                return;
            }
        }
        self.forget_labels();
        for item in &mut self.items {
            item.surfaces.clear();
            item.level = None;
        }
        if let Some(fx) = self.fx.as_mut() {
            fx.forget_pictures();
        }
        self.close_preview();
        self.repaint_bar_and_launcher();
        self.refresh_due = true;
    }

    // ------------------------------------------------- top bar and launcher

    /// Creates or removes the top bar and the launcher to match the settings.
    fn setup_bar_and_launcher(&mut self) {
        let sigma = (self.cfg.blur && self.cfg.blur_radius > 0.0).then_some(self.cfg.blur_radius * self.m.scale);
        self.popup_sigma = sigma;
        if self.cfg.top_bar && self.bar.is_none() {
            match Bar::new(&self.glass, sigma, &bar_palette(&self.cfg, &self.colors)) {
                Ok(bar) => {
                    self.bar = Some(bar);
                    self.last_status = self.status.read();
                }
                Err(e) => crate::log!("top bar unavailable: {e}"),
            }
        } else if !self.cfg.top_bar {
            self.bar = None;
        }
        if let Some(bar) = self.bar.as_mut() {
            bar.place(self.monitor_rect, self.m.scale, self.cfg.bar_height, self.cfg.reserve_space);
        }

        if self.menu.is_none() {
            match Menu::new(&self.glass, sigma, &menu_palette(&self.colors)) {
                Ok(menu) => self.menu = Some(menu),
                Err(e) => crate::log!("the dock's own menus are unavailable ({e}); using Windows' menu"),
            }
        }
        if self.side.is_none() && self.menu.is_some() {
            match Menu::new(&self.glass, sigma, &menu_palette(&self.colors)) {
                Ok(side) => {
                    side.make_side();
                    self.side = Some(side);
                }
                Err(e) => crate::log!("the panel beside the Wi-Fi list is unavailable ({e})"),
            }
        }
        if self.calendar.is_none() {
            match Calendar::new(&self.glass, sigma, &menu_palette(&self.colors)) {
                Ok(calendar) => self.calendar = Some(calendar),
                Err(e) => crate::log!("the date panel is unavailable ({e}); the clock opens Windows' own"),
            }
        }
        if self.launcher.is_none() {
            match Launcher::new(&self.glass, sigma.map(|s| s * 1.4), &launcher_palette(&self.colors)) {
                Ok(l) => self.launcher = Some(l),
                Err(e) => crate::log!("app launcher unavailable: {e}"),
            }
        }
        if self.app_scan.is_none() && self.launcher.is_some() {
            // Read the list of installed apps in the background; it takes a few seconds.
            let icon = (self.cfg.launcher_icon * self.m.scale).round().max(16.0) as u32;
            self.app_scan = Some(shell::scan_apps(icon));
        }

        // The launcher's button at the start of the dock.
        let has_button = self.items.iter().any(|i| i.key == LAUNCHER_KEY);
        if self.cfg.launcher && !has_button {
            self.items.insert(0, Item::launcher_button(self.colors.accent));
        } else if !self.cfg.launcher && has_button {
            self.items.retain(|i| i.key != LAUNCHER_KEY);
        }
        if self.cfg.launcher != has_button {
            self.pressed = None;
            self.hovered = None;
            self.label_index = None;
            self.drag = None;
            self.refresh_due = true;
        }
    }

    fn repaint_bar_and_launcher(&mut self) {
        if let Some(bar) = self.bar.as_mut() {
            bar.set_palette(&bar_palette(&self.cfg, &self.colors));
            bar.forget();
        }
        if let Some(l) = self.launcher.as_mut() {
            l.set_palette(&launcher_palette(&self.colors));
            l.forget();
        }
        self.close_menu();
        if let Some(menu) = self.menu.as_mut() {
            menu.set_palette(&menu_palette(&self.colors));
            menu.forget();
        }
        if let Some(side) = self.side.as_mut() {
            side.set_palette(&menu_palette(&self.colors));
            side.forget();
        }
        if let Some(calendar) = self.calendar.as_mut() {
            calendar.set_palette(&menu_palette(&self.colors));
            calendar.forget();
        }
    }

    fn take_scanned_apps(&mut self) {
        let (Some(rx), Some(launcher)) = (self.app_scan.as_ref(), self.launcher.as_mut()) else { return };
        launcher.absorb(rx.try_iter());
    }

    fn toggle_launcher(&mut self) {
        self.close_preview();
        self.take_scanned_apps();
        let (monitor, scale, icon, dark, own) = (self.monitor_rect, self.m.scale, self.cfg.launcher_icon, self.colors.dark, self.hwnd);
        let Some(launcher) = self.launcher.as_mut() else { return };
        if launcher.is_open() {
            launcher.close();
        } else {
            launcher.open(monitor, scale, icon, dark);
            // Keep the dock in front of the launcher's sheet.
            unsafe {
                let _ = SetWindowPos(own, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    // --------------------------------------------------------------- cleanup

    /// Undoes everything the dock changed in Windows. Called once, on exit.
    pub fn shutdown(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_ID);
            let _ = DeregisterShellHookWindow(self.hwnd);
            for hook in self.hooks.drain(..) {
                let _ = UnhookWinEvent(hook);
            }
        }
        self.close_preview();
        self.preview = None;
        self.fx = None;
        self.bar = None;
        self.launcher = None;
        self.menu = None;
        self.side = None;
        self.calendar = None;
        system::restore_window_animations();
        self.appbar.remove();
        if self.taskbar.hidden {
            self.taskbar.restore();
        }
    }
}

fn icon_rect_of(m: &Metrics, slot: Slot, item: &Item) -> (f32, f32, f32, f32) {
    let size = m.icon * slot.scale * item.presence.pos.clamp(0.0, 1.0);
    let cx = slot.center() + item.shift.pos;
    let bottom = m.baseline - item.bounce_y;
    (cx - size * 0.5, bottom - size, cx + size * 0.5, bottom)
}

fn is_dark(cfg: &Config) -> bool {
    cfg.is_dark_forced().unwrap_or_else(system::system_is_dark)
}

/// Pixel sizes each icon is pre-scaled to: at rest, fully magnified, and steps in between so
/// that no size is ever drawn from a bitmap more than about 1.4x larger than it.
fn icon_levels(m: &Metrics) -> Vec<u32> {
    let rest = m.icon.round().max(1.0);
    let max = (m.icon * (1.0 + m.amp)).ceil().max(rest);
    let mut levels = vec![rest as u32];
    let steps = ((max / rest).ln() / 1.4f32.ln()).ceil().max(1.0) as i32;
    for i in 1..=steps {
        let size = (rest * (max / rest).powf(i as f32 / steps as f32)).ceil() as u32;
        if size > *levels.last().unwrap() {
            levels.push(size);
        }
    }
    levels
}

/// The top bar: the dock's glass, a little clearer.
fn bar_palette(cfg: &Config, colors: &Colors) -> Palette {
    let mut p = palette(cfg, colors);
    p.tint.A = ((p.tint.A as f32) * 0.85) as u8;
    p
}

/// Menus: more solid than the dock, so every row reads clearly over anything.
fn menu_palette(colors: &Colors) -> Palette {
    if colors.dark {
        Palette { tint: Color { A: 205, R: 36, G: 36, B: 40 }, border_inner: Color { A: 46, R: 255, G: 255, B: 255 }, border_outer: Color { A: 110, R: 0, G: 0, B: 0 } }
    } else {
        Palette { tint: Color { A: 222, R: 246, G: 246, B: 248 }, border_inner: Color { A: 160, R: 255, G: 255, B: 255 }, border_outer: Color { A: 44, R: 0, G: 0, B: 0 } }
    }
}

/// The launcher's full-screen sheet: darker and more solid than the dock, so names stay readable.
fn launcher_palette(colors: &Colors) -> Palette {
    if colors.dark {
        Palette { tint: Color { A: 150, R: 18, G: 18, B: 22 }, border_inner: Color { A: 0, R: 0, G: 0, B: 0 }, border_outer: Color { A: 0, R: 0, G: 0, B: 0 } }
    } else {
        Palette { tint: Color { A: 165, R: 238, G: 238, B: 242 }, border_inner: Color { A: 0, R: 0, G: 0, B: 0 }, border_outer: Color { A: 0, R: 0, G: 0, B: 0 } }
    }
}

fn palette(cfg: &Config, colors: &Colors) -> Palette {
    // With blur off there is nothing to soften what shows through, so make the panel more solid.
    let opacity = if cfg.blur { cfg.tint_opacity } else { cfg.tint_opacity.max(0.78) };
    let a = (opacity * 255.0).round() as u8;
    if colors.dark {
        Palette {
            tint: Color { A: a, R: 26, G: 26, B: 30 },
            border_inner: Color { A: 40, R: 255, G: 255, B: 255 },
            border_outer: Color { A: 90, R: 0, G: 0, B: 0 },
        }
    } else {
        Palette {
            tint: Color { A: a, R: 244, G: 244, B: 247 },
            border_inner: Color { A: 150, R: 255, G: 255, B: 255 },
            border_outer: Color { A: 34, R: 0, G: 0, B: 0 },
        }
    }
}
