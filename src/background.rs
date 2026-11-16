//! The apps running in the background: the icons programs keep in the taskbar's
//! notification area, which went out of sight with the taskbar. The island shows them on
//! its first page (see `island.rs`), and a click there reaches the program as a click on
//! its icon in the taskbar would.
//!
//! A program hands its icon over by looking for a window of the taskbar's class and
//! sending it the icon's data (that is all `Shell_NotifyIcon` does). So a hidden window of
//! that class is kept in front of the real one. It reads what goes by and passes every
//! message on to the real taskbar unchanged, which answers as it always did: nothing is
//! taken out of Windows' own list, and when Bến goes the programs are talking to the
//! taskbar again. The programs that are already running are asked what they have the way
//! Explorer asks them when it has restarted.
//!
//! The window has a thread of its own, like the Windows-key hook: every program that
//! changes its icon waits for the answer, and the thread that animates the dock is busy
//! now and then. What an icon's tip or balloon says is never written to the log.

use crate::shell::{self, IconPixels};
use crate::util::{from_wide, wide};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Shell::{NIF_GUID, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_STATE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NIN_SELECT, NIS_HIDDEN};
use windows::Win32::UI::WindowsAndMessaging::*;

/// The class of the taskbar's window, by which programs find it, and of the part of it
/// that holds the icons (some programs look for that to know where their icon is).
const TASKBAR_CLASS: &str = "Shell_TrayWnd";
const AREA_CLASS: &str = "TrayNotifyWnd";
/// Marks the stand-in, for whoever looks for the real taskbar.
const STAND_IN: &str = "BenStandIn";
/// Tags the question put to the running programs, so that this program's own window does
/// not take it for Explorer having restarted.
pub const OWN: usize = 0x42_45_4E;
/// What a `WM_COPYDATA` sent to the taskbar is about: this one is an order about an icon.
const ICON_ORDER: usize = 1;
/// How often the stand-in checks that it is still the first window of its class, in
/// milliseconds, and after how many such checks the icons whose programs are gone are dropped.
const LOOK_EVERY: u32 = 250;
const SWEEP_EVERY: u32 = 8;
/// How long the real taskbar gets to answer what is passed on to it, in milliseconds.
const PATIENCE: u32 = 3000;

/// What `WM_COPYDATA` carries.
#[repr(C)]
struct CopyData {
    kind: usize,
    len: u32,
    bytes: *const core::ffi::c_void,
}

/// What `Shell_NotifyIcon` sends the taskbar: a mark, the order (add, modify, delete...)
/// and the caller's `NOTIFYICONDATAW` with its handles cut to 32 bits.
#[repr(C)]
#[derive(Clone, Copy)]
struct Order {
    mark: u32,
    order: u32,
    size: u32,
    window: u32,
    id: u32,
    flags: u32,
    message: u32,
    icon: u32,
    tip: [u16; 128],
    state: u32,
    state_mask: u32,
    info: [u16; 256],
    version: u32,
    info_title: [u16; 64],
    info_flags: u32,
    guid: GUID,
    balloon_icon: u32,
}

/// An icon as its program has described it so far.
struct Entry {
    serial: u64,
    window: u32,
    id: u32,
    guid: GUID,
    /// The message its program wants to be told of clicks with, and the way it wants them
    /// told (see `wrap`): 0 until the program says otherwise.
    message: u32,
    version: u32,
    hidden: bool,
    tip: String,
    picture: Option<Arc<IconPixels>>,
    drawn: u32,
    /// The program's name, for when the icon has no tip, and to put the icons in order.
    program: String,
}

impl Entry {
    fn is(&self, order: &Order) -> bool {
        let named = order.flags & NIF_GUID.0 != 0 && order.guid != GUID::zeroed();
        (named && self.guid == order.guid) || (order.window != 0 && self.window == order.window && self.id == order.id)
    }

    /// On show: an icon with a picture that can be clicked, and that its program has not hidden.
    fn shown(&self) -> bool {
        !self.hidden && self.message != 0 && self.picture.is_some()
    }
}

/// One app running in the background, as the island shows it.
#[derive(Clone)]
pub struct Resident {
    /// Tells it from the others for as long as it stays.
    pub id: u64,
    /// What its icon says under the cursor, or else the program's name.
    pub name: String,
    pub picture: Arc<IconPixels>,
    /// Goes up each time the picture changes.
    pub drawn: u32,
    window: u32,
    icon: u32,
    message: u32,
    version: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Resident {
    /// One that no program stands behind, for trying the island's page out.
    #[cfg(test)]
    pub fn made_up(id: u64, name: &str, picture: IconPixels) -> Resident {
        Resident { id, name: name.to_string(), picture: Arc::new(picture), drawn: 1, window: 0, icon: 0, message: 0, version: 0 }
    }

    /// The same, as the icon of the program this window belongs to.
    #[cfg(test)]
    fn of(mut self, window: HWND) -> Resident {
        self.window = window.0 as usize as u32;
        self
    }

    /// What its program has up over the other windows right now: menus, flyouts, tips.
    /// Taken before a click, it tells afterwards whether the click brought something up
    /// (see `showing`).
    pub fn popups(&self) -> Vec<isize> {
        let mut pid = 0u32;
        let mut windows: Vec<HWND> = Vec::new();
        unsafe {
            GetWindowThreadProcessId(HWND(self.window as usize as *mut _), Some(&mut pid));
            let _ = EnumWindows(Some(collect_window), LPARAM(&mut windows as *mut _ as isize));
        }
        windows.retain(|&window| pid != 0 && unsafe { passing(window, pid) });
        windows.into_iter().map(|window| window.0 as isize).collect()
    }

    /// Whether its program is showing a menu or the like that is not among `before`: what
    /// a click on its icon has brought up, for as long as that stays.
    pub fn showing(&self, before: &[isize]) -> bool {
        self.popups().iter().any(|window| !before.contains(window))
    }

    /// Clicks its icon the way the taskbar would. `twice`: this is the second click of a
    /// double click. `at`: where on the screen.
    pub fn click(&self, button: Button, twice: bool, at: (i32, i32)) {
        unsafe {
            let window = HWND(self.window as usize as *mut _);
            // The program may bring its window or its menu to the front.
            let mut pid = 0u32;
            GetWindowThreadProcessId(window, Some(&mut pid));
            let _ = AllowSetForegroundWindow(pid);
            for event in clicks(button, twice, self.version) {
                let (wparam, lparam) = wrap(event, self.icon, self.version, at);
                let _ = SendNotifyMessageW(window, self.message, WPARAM(wparam), LPARAM(lparam));
            }
        }
    }
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    list.push(hwnd);
    windows::core::BOOL(1)
}

/// Whether a window is one the program `pid` has put up for a moment over the others, and
/// is in view: a menu of Windows' own making, or a window that keeps out of the taskbar,
/// is not taken to the front, or belongs to another window. (A program's main window is
/// none of these: a click that only opens it leaves nothing to wait for.)
unsafe fn passing(window: HWND, pid: u32) -> bool {
    unsafe {
        let mut owner = 0u32;
        GetWindowThreadProcessId(window, Some(&mut owner));
        let mut at = RECT::default();
        if owner != pid || !IsWindowVisible(window).as_bool() || GetWindowRect(window, &mut at).is_err() || at.right - at.left < 8 || at.bottom - at.top < 8 {
            return false;
        }
        let ex = WINDOW_EX_STYLE(GetWindowLongW(window, GWL_EXSTYLE) as u32);
        shell::class_name(window) == "#32768" || ex.contains(WS_EX_TOOLWINDOW) || ex.contains(WS_EX_NOACTIVATE) || GetWindow(window, GW_OWNER).is_ok_and(|owner| !owner.is_invalid())
    }
}

/// What the taskbar tells a program for one click on its icon. From the third way of
/// telling on, a click is also named for what it means: the icon chosen, its menu wanted.
fn clicks(button: Button, twice: bool, version: u32) -> Vec<u32> {
    let mut events = match button {
        Button::Left => vec![if twice { WM_LBUTTONDBLCLK } else { WM_LBUTTONDOWN }, WM_LBUTTONUP],
        Button::Middle => vec![WM_MBUTTONDOWN, WM_MBUTTONUP],
        Button::Right => vec![WM_RBUTTONDOWN, WM_RBUTTONUP],
    };
    if version >= 3 {
        match button {
            Button::Left => events.push(NIN_SELECT),
            Button::Right => events.push(WM_CONTEXTMENU),
            Button::Middle => {}
        }
    }
    events
}

/// How an event is packed for the program. The fourth way of telling gives the place on
/// the screen and puts the icon's number beside the event; the earlier ones give the
/// number and the event alone.
fn wrap(event: u32, icon: u32, version: u32, at: (i32, i32)) -> (usize, isize) {
    if version > 3 {
        let place = ((at.1 as u16 as usize) << 16) | at.0 as u16 as usize;
        (place, (((icon & 0xFFFF) << 16) | (event & 0xFFFF)) as isize)
    } else {
        (icon as usize, event as isize)
    }
}

/// What an icon's tip says, on one line.
fn one_line(tip: &str) -> String {
    tip.lines().map(str::trim).filter(|line| !line.is_empty()).collect::<Vec<_>>().join(" · ")
}

/// Takes note of an order about an icon. `picture`: the icon it names, already read.
/// Returns true if what is on show has changed.
fn note(list: &mut Vec<Entry>, order: &Order, picture: Option<IconPixels>, serial: &mut u64) -> bool {
    let at = list.iter().position(|entry| entry.is(order));
    if order.order == NIM_DELETE.0 {
        return at.map(|at| list.remove(at)).is_some_and(|entry| entry.shown());
    }
    if order.order == NIM_SETVERSION.0 {
        if let Some(at) = at {
            list[at].version = order.version;
        }
        return false;
    }
    if order.order != NIM_ADD.0 && order.order != NIM_MODIFY.0 {
        return false;
    }
    // (A change to an icon that was never heard of is taken for the icon itself: its
    // program was running before this was.)
    let at = at.unwrap_or_else(|| {
        *serial += 1;
        list.push(Entry { serial: *serial, window: order.window, id: order.id, guid: GUID::zeroed(), message: 0, version: 0, hidden: false, tip: String::new(), picture: None, drawn: 0, program: String::new() });
        list.len() - 1
    });
    let entry = &mut list[at];
    let before = entry.shown();
    let mut changed = false;
    if order.window != 0 {
        entry.window = order.window;
        entry.id = order.id;
    }
    if order.flags & NIF_GUID.0 != 0 {
        entry.guid = order.guid;
    }
    if order.flags & NIF_MESSAGE.0 != 0 {
        entry.message = order.message;
    }
    // (Only the fourth way of telling has a use for this flag.)
    if order.flags & NIF_SHOWTIP.0 != 0 && entry.version == 0 {
        entry.version = 4;
    }
    if order.flags & NIF_STATE.0 != 0 && order.state_mask & NIS_HIDDEN.0 != 0 {
        entry.hidden = order.state & NIS_HIDDEN.0 != 0;
    }
    if order.flags & NIF_TIP.0 != 0 {
        let tip = from_wide(&order.tip);
        changed |= tip != entry.tip;
        entry.tip = tip;
    }
    if order.flags & NIF_ICON.0 != 0 {
        let same = match (entry.picture.as_ref(), picture.as_ref()) {
            (Some(old), Some(new)) => old.size == new.size && old.bgra == new.bgra,
            (None, None) => true,
            _ => false,
        };
        if !same {
            entry.picture = picture.map(Arc::new);
            entry.drawn += 1;
            changed = true;
        }
    }
    entry.shown() != before || (changed && entry.shown())
}

/// The windows of the taskbar's class, the first to be found first.
fn taskbars() -> Vec<HWND> {
    let class = wide(TASKBAR_CLASS);
    let mut found = Vec::new();
    let mut after: Option<HWND> = None;
    unsafe {
        while let Ok(window) = FindWindowExW(None, after, PCWSTR(class.as_ptr()), PCWSTR::null()) {
            if window.is_invalid() {
                break;
            }
            found.push(window);
            after = Some(window);
        }
    }
    found
}

/// Whether a window of the taskbar's class is the stand-in (of this program, or of another
/// copy of it) and not the taskbar.
fn is_stand_in(window: HWND) -> bool {
    let mark = wide(STAND_IN);
    unsafe { !GetPropW(window, PCWSTR(mark.as_ptr())).is_invalid() || shell::is_own_window(window) }
}

/// The taskbar's own window.
pub fn taskbar() -> Option<HWND> {
    taskbars().into_iter().find(|&window| !is_stand_in(window))
}

/// The name of the program a window belongs to.
fn program_of(window: HWND) -> String {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(window, Some(&mut pid));
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let have_path = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        if !have_path || len == 0 {
            return String::new();
        }
        shell::display_name(&String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// What the stand-in's thread keeps.
struct Home {
    list: Vec<Entry>,
    serial: u64,
    looks: u32,
    out: Sender<Vec<Resident>>,
    /// The window has been told that there is something to take, and has not taken it yet.
    told: Arc<AtomicBool>,
    window: isize,
    message: u32,
}

impl Home {
    /// Sends the app what is on show now: in the order of the programs' names, so that each
    /// icon is found where it was the last time.
    fn tell(&self) {
        let mut shown: Vec<&Entry> = self.list.iter().filter(|entry| entry.shown()).collect();
        shown.sort_by_cached_key(|entry| (entry.program.to_lowercase(), entry.serial));
        let residents = shown
            .into_iter()
            .map(|entry| {
                let tip = one_line(&entry.tip);
                Resident {
                    id: entry.serial,
                    name: if tip.is_empty() { entry.program.clone() } else { tip },
                    picture: entry.picture.clone().unwrap(),
                    drawn: entry.drawn,
                    window: entry.window,
                    icon: entry.id,
                    message: entry.message,
                    version: entry.version,
                }
            })
            .collect();
        let _ = self.out.send(residents);
        if !self.told.swap(true, Ordering::AcqRel) {
            unsafe {
                let _ = PostMessageW(Some(HWND(self.window as *mut _)), self.message, WPARAM(0), LPARAM(0));
            }
        }
    }
}

thread_local! {
    static HOME: RefCell<Option<Home>> = const { RefCell::new(None) };
}

/// Reads the order about an icon out of a `WM_COPYDATA`, if that is what it carries.
unsafe fn order_in(lparam: LPARAM) -> Option<Order> {
    unsafe {
        let data = (lparam.0 as *const CopyData).as_ref()?;
        // (Up to the icon's handle at least: the rest may be left out, and is then nothing.)
        if data.kind != ICON_ORDER || data.bytes.is_null() || (data.len as usize) < 32 {
            return None;
        }
        let mut order: Order = std::mem::zeroed();
        std::ptr::copy_nonoverlapping(data.bytes as *const u8, &mut order as *mut Order as *mut u8, (data.len as usize).min(std::mem::size_of::<Order>()));
        Some(order)
    }
}

/// Hands a message on to the real taskbar, and brings back its answer.
unsafe fn pass_on(msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        let Some(real) = taskbar() else {
            // No taskbar to ask (Explorer is not running): an icon was noted all the same.
            return LRESULT((msg == WM_COPYDATA) as isize);
        };
        if msg != WM_COPYDATA && !InSendMessage().as_bool() {
            // Posted: nobody is waiting for an answer.
            let _ = PostMessageW(Some(real), msg, wparam, lparam);
            return LRESULT(0);
        }
        let mut answer = 0usize;
        SendMessageTimeoutW(real, msg, wparam, lparam, SMTO_ABORTIFHUNG, PATIENCE, Some(&mut answer));
        LRESULT(answer as isize)
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_COPYDATA => {
                if let Some(order) = order_in(lparam) {
                    // The picture is taken now: the program may throw its icon away as soon
                    // as it has its answer.
                    let wanted = order.flags & NIF_ICON.0 != 0 && order.icon != 0 && (order.order == NIM_ADD.0 || order.order == NIM_MODIFY.0);
                    let picture = if wanted { shell::icon_pixels(HICON(order.icon as usize as *mut _)) } else { None };
                    HOME.with_borrow_mut(|home| {
                        let Some(home) = home.as_mut() else { return };
                        let changed = note(&mut home.list, &order, picture, &mut home.serial);
                        for entry in home.list.iter_mut().filter(|entry| entry.program.is_empty()) {
                            entry.program = program_of(HWND(entry.window as usize as *mut _));
                        }
                        if changed {
                            home.tell();
                        }
                    });
                }
                // (Nothing of this thread's is held while the taskbar is asked: other
                // messages come in meanwhile.)
                pass_on(msg, wparam, lparam)
            }
            WM_TIMER => {
                // Explorer puts its taskbar back on top now and then; programs must go on
                // finding this window first.
                let class = wide(TASKBAR_CLASS);
                if FindWindowW(PCWSTR(class.as_ptr()), PCWSTR::null()).ok() != Some(hwnd) {
                    let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
                HOME.with_borrow_mut(|home| {
                    let Some(home) = home.as_mut() else { return };
                    home.looks += 1;
                    if home.looks % SWEEP_EVERY != 0 {
                        return;
                    }
                    // A program that ended without taking its icon back: Windows drops the
                    // icon when the cursor comes to it; here it goes within two seconds.
                    let before = home.list.len();
                    home.list.retain(|entry| IsWindow(Some(HWND(entry.window as usize as *mut _))).as_bool());
                    if home.list.len() != before {
                        home.tell();
                    }
                });
                LRESULT(0)
            }
            // What is meant for the taskbar goes to the taskbar: its commands (show the
            // desktop, for one), and its own messages. Closing it is not for this window either.
            WM_COMMAND | WM_CLOSE => pass_on(msg, wparam, lparam),
            _ if (WM_USER..0xC000).contains(&msg) => pass_on(msg, wparam, lparam),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

unsafe extern "system" fn plain(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Makes the window that stands in for the taskbar: of its class, where it is and as
/// large, never shown, and in front of it. `None` if there is not exactly one taskbar to
/// stand in for (Explorer is not running, or another program stands in for it already).
unsafe fn stand_in() -> Option<HWND> {
    unsafe {
        let all = taskbars();
        if all.len() != 1 {
            crate::log!("{} windows of the taskbar's class where one was expected; the apps in the background are left to it", all.len());
            return None;
        }
        let real = all[0];
        let instance: HINSTANCE = GetModuleHandleW(None).ok()?.into();
        let class_name = wide(TASKBAR_CLASS);
        let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: PCWSTR(class_name.as_ptr()), ..Default::default() };
        RegisterClassExW(&class);
        // A program that asks where the taskbar is gets the answer it always got.
        let mut at = RECT::default();
        let _ = GetWindowRect(real, &mut at);
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST, PCWSTR(class_name.as_ptr()), PCWSTR::null(), WS_POPUP, at.left, at.top, at.right - at.left, at.bottom - at.top, None, None, Some(instance), None);
        let hwnd = match hwnd {
            Ok(hwnd) => hwnd,
            Err(e) => {
                crate::log!("no window to stand in for the taskbar ({e}); the apps in the background are not shown");
                return None;
            }
        };
        let mark = wide(STAND_IN);
        let _ = SetPropW(hwnd, PCWSTR(mark.as_ptr()), Some(HANDLE(1 as *mut _)));
        // The part that holds the icons, for the programs that look for it.
        let area_name = wide(AREA_CLASS);
        if let Ok(area) = FindWindowExW(Some(real), None, PCWSTR(area_name.as_ptr()), PCWSTR::null()) {
            let mut inner = RECT::default();
            if !area.is_invalid() && GetWindowRect(area, &mut inner).is_ok() {
                let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(plain), hInstance: instance, lpszClassName: PCWSTR(area_name.as_ptr()), ..Default::default() };
                RegisterClassExW(&class);
                let _ = CreateWindowExW(WINDOW_EX_STYLE(0), PCWSTR(area_name.as_ptr()), PCWSTR::null(), WS_CHILD, inner.left - at.left, inner.top - at.top, inner.right - inner.left, inner.bottom - inner.top, Some(hwnd), None, Some(instance), None);
            }
        }
        SetTimer(Some(hwnd), 1, LOOK_EVERY, None);
        Some(hwnd)
    }
}

/// The watch on the apps in the background, for as long as this is kept.
pub struct Background {
    thread: u32,
    runner: Option<JoinHandle<()>>,
    rx: Receiver<Vec<Resident>>,
    told: Arc<AtomicBool>,
}

/// Starts watching. `window` gets `message` whenever there is something new to `take`.
/// `None` if the stand-in could not be made (the island then shows none).
pub fn start(window: HWND, message: u32) -> Option<Background> {
    let (sender, receiver) = channel();
    let (out, rx) = channel();
    let told = Arc::new(AtomicBool::new(false));
    let window = window.0 as isize;
    let run = {
        let told = told.clone();
        move || unsafe {
            let Some(hwnd) = stand_in() else {
                let _ = sender.send(None);
                return;
            };
            HOME.set(Some(Home { list: Vec::new(), serial: 0, looks: 0, out, told, window, message }));
            let _ = sender.send(Some(GetCurrentThreadId()));
            // The programs that are running already say what they have when they are asked
            // the way Explorer asks them after it has restarted.
            let asked = RegisterWindowMessageW(PCWSTR(wide("TaskbarCreated").as_ptr()));
            if asked != 0 {
                let _ = SendNotifyMessageW(HWND_BROADCAST, asked, WPARAM(OWN), LPARAM(0));
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                DispatchMessageW(&msg);
            }
            let _ = DestroyWindow(hwnd);
            HOME.set(None);
        }
    };
    let runner = std::thread::Builder::new().name("background".into()).stack_size(256 * 1024).spawn(run).ok()?;
    let thread = receiver.recv().ok().flatten()?;
    Some(Background { thread, runner: Some(runner), rx, told })
}

impl Background {
    /// If anything changed since the last call: the apps in the background as they are now.
    pub fn take(&self) -> Option<Vec<Resident>> {
        self.told.store(false, Ordering::Release);
        self.rx.try_iter().last()
    }
}

impl Drop for Background {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(runner) = self.runner.take() {
            let _ = runner.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order(what: u32, window: u32, id: u32, flags: u32) -> Order {
        let mut order: Order = unsafe { std::mem::zeroed() };
        order.order = what;
        order.window = window;
        order.id = id;
        order.flags = flags;
        order.message = 0x8001;
        order
    }

    fn tipped(mut order: Order, tip: &str) -> Order {
        for (to, from) in order.tip.iter_mut().zip(tip.encode_utf16()) {
            *to = from;
        }
        order
    }

    fn picture(shade: u8) -> Option<IconPixels> {
        Some(IconPixels { size: 2, bgra: vec![shade; 16] })
    }

    const WHOLE: u32 = NIF_MESSAGE.0 | NIF_ICON.0 | NIF_TIP.0;

    #[test]
    fn the_order_is_laid_out_as_windows_sends_it() {
        assert_eq!(std::mem::size_of::<Order>(), 964);
        assert_eq!(std::mem::offset_of!(Order, tip), 32);
        assert_eq!(std::mem::offset_of!(Order, version), 808);
        assert_eq!(std::mem::offset_of!(Order, guid), 944);
    }

    #[test]
    fn an_icon_comes_changes_and_goes() {
        let (mut list, mut serial) = (Vec::new(), 0);
        assert!(note(&mut list, &tipped(order(NIM_ADD.0, 7, 1, WHOLE), "One"), picture(1), &mut serial));
        assert_eq!((list.len(), list[0].tip.as_str(), list[0].drawn), (1, "One", 1));
        // The same picture and the same words again: nothing to tell.
        assert!(!note(&mut list, &tipped(order(NIM_MODIFY.0, 7, 1, NIF_ICON.0 | NIF_TIP.0), "One"), picture(1), &mut serial));
        assert!(note(&mut list, &tipped(order(NIM_MODIFY.0, 7, 1, NIF_TIP.0), "Two"), None, &mut serial));
        assert!(note(&mut list, &order(NIM_MODIFY.0, 7, 1, NIF_ICON.0), picture(9), &mut serial));
        assert_eq!((list.len(), list[0].tip.as_str(), list[0].drawn), (1, "Two", 2));
        // Another icon of the same window is another icon.
        assert!(note(&mut list, &order(NIM_ADD.0, 7, 2, WHOLE), picture(1), &mut serial));
        assert_eq!(list.len(), 2);
        assert!(note(&mut list, &order(NIM_DELETE.0, 7, 1, 0), None, &mut serial));
        assert_eq!((list.len(), list[0].id), (1, 2));
        assert!(!note(&mut list, &order(NIM_DELETE.0, 7, 1, 0), None, &mut serial));
    }

    #[test]
    fn a_hidden_icon_is_kept_but_not_shown() {
        let (mut list, mut serial) = (Vec::new(), 0);
        note(&mut list, &order(NIM_ADD.0, 7, 1, WHOLE), picture(1), &mut serial);
        let mut hide = order(NIM_MODIFY.0, 7, 1, NIF_STATE.0);
        (hide.state, hide.state_mask) = (NIS_HIDDEN.0, NIS_HIDDEN.0);
        assert!(note(&mut list, &hide, None, &mut serial));
        assert!(!list[0].shown());
        // Its words change meanwhile: nothing to tell, it is not there to be seen.
        assert!(!note(&mut list, &tipped(order(NIM_MODIFY.0, 7, 1, NIF_TIP.0), "Unseen"), None, &mut serial));
        hide.state = 0;
        assert!(note(&mut list, &hide, None, &mut serial));
        assert!(list[0].shown());
    }

    #[test]
    fn an_icon_that_cannot_be_clicked_or_seen_is_not_shown() {
        let (mut list, mut serial) = (Vec::new(), 0);
        assert!(!note(&mut list, &order(NIM_ADD.0, 7, 1, NIF_ICON.0), picture(1), &mut serial));
        assert!(!note(&mut list, &order(NIM_ADD.0, 8, 1, NIF_MESSAGE.0), None, &mut serial));
        assert!(list.iter().all(|entry| !entry.shown()));
        assert!(note(&mut list, &order(NIM_MODIFY.0, 7, 1, NIF_MESSAGE.0), None, &mut serial));
    }

    #[test]
    fn a_change_to_an_unknown_icon_brings_it_in() {
        let (mut list, mut serial) = (Vec::new(), 0);
        assert!(note(&mut list, &order(NIM_MODIFY.0, 7, 1, WHOLE), picture(1), &mut serial));
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn an_icon_is_known_by_its_name_when_it_has_one() {
        let (mut list, mut serial) = (Vec::new(), 0);
        let mut add = order(NIM_ADD.0, 7, 1, WHOLE | NIF_GUID.0);
        add.guid = GUID::from_u128(42);
        note(&mut list, &add, picture(1), &mut serial);
        // Named again without its window: still the same icon.
        let mut gone = order(NIM_DELETE.0, 0, 0, NIF_GUID.0);
        gone.guid = GUID::from_u128(42);
        assert!(note(&mut list, &gone, None, &mut serial));
        assert!(list.is_empty());
    }

    #[test]
    fn the_way_of_telling_is_what_the_program_asked_for() {
        let (mut list, mut serial) = (Vec::new(), 0);
        note(&mut list, &order(NIM_ADD.0, 7, 1, WHOLE), picture(1), &mut serial);
        assert_eq!(list[0].version, 0);
        let mut set = order(NIM_SETVERSION.0, 7, 1, 0);
        set.version = 4;
        assert!(!note(&mut list, &set, None, &mut serial));
        assert_eq!(list[0].version, 4);
    }

    #[test]
    fn a_click_is_told_the_way_the_taskbar_tells_it() {
        assert_eq!(clicks(Button::Left, false, 0), [WM_LBUTTONDOWN, WM_LBUTTONUP]);
        assert_eq!(clicks(Button::Left, true, 0), [WM_LBUTTONDBLCLK, WM_LBUTTONUP]);
        assert_eq!(clicks(Button::Left, false, 4), [WM_LBUTTONDOWN, WM_LBUTTONUP, NIN_SELECT]);
        assert_eq!(clicks(Button::Right, false, 0), [WM_RBUTTONDOWN, WM_RBUTTONUP]);
        assert_eq!(clicks(Button::Right, false, 3), [WM_RBUTTONDOWN, WM_RBUTTONUP, WM_CONTEXTMENU]);
        assert_eq!(clicks(Button::Middle, false, 4), [WM_MBUTTONDOWN, WM_MBUTTONUP]);
    }

    #[test]
    fn an_event_is_packed_for_the_way_of_telling() {
        assert_eq!(wrap(WM_LBUTTONUP, 5, 0, (100, 200)), (5, WM_LBUTTONUP as isize));
        assert_eq!(wrap(WM_LBUTTONUP, 5, 3, (100, 200)), (5, WM_LBUTTONUP as isize));
        assert_eq!(wrap(WM_CONTEXTMENU, 5, 4, (100, 200)), ((200 << 16) | 100, ((5 << 16) | WM_CONTEXTMENU) as isize));
        // A screen to the left of the main one: the place is still two halves of 16 bits.
        assert_eq!(wrap(NIN_SELECT, 0, 4, (-10, 20)).0, (20 << 16) | 0xFFF6);
    }

    #[test]
    fn a_tip_of_several_lines_is_put_on_one() {
        assert_eq!(one_line("Docker Desktop\r\n  Running \n"), "Docker Desktop · Running");
        assert_eq!(one_line(""), "");
    }

    /// An icon of `side` by `side` pixels (BGRA, top row first). `gone` says which of them
    /// its mask leaves out.
    fn icon_of(side: i32, bgra: &[u8], gone: &[bool]) -> HICON {
        use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject};
        // The mask has one bit for each pixel, in rows of whole 16 bits.
        let (side_n, stride) = (side as usize, (side as usize).div_ceil(16) * 2);
        let mut mask = vec![0u8; stride * side_n];
        for (i, _) in gone.iter().enumerate().filter(|(_, gone)| **gone) {
            mask[(i / side_n) * stride + (i % side_n) / 8] |= 0x80 >> ((i % side_n) % 8);
        }
        unsafe {
            let colour = CreateBitmap(side, side, 1, 32, Some(bgra.as_ptr() as *const _));
            let mask = CreateBitmap(side, side, 1, 1, Some(mask.as_ptr() as *const _));
            let icon = CreateIconIndirect(&ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: colour }).unwrap();
            let _ = DeleteObject(colour.into());
            let _ = DeleteObject(mask.into());
            icon
        }
    }

    #[test]
    fn an_icon_that_says_how_solid_each_pixel_is_is_read_so() {
        let mut bgra = [0u8, 0, 255, 128].repeat(16);
        bgra[..4].fill(0);
        let icon = icon_of(4, &bgra, &[false; 16]);
        let read = shell::icon_pixels(icon);
        unsafe {
            let _ = DestroyIcon(icon);
        }
        let read = read.unwrap();
        assert_eq!((read.size, &read.bgra[..4]), (4, &[0u8, 0, 0, 0][..]));
        // Half there, and its colour weighted by that.
        let p = &read.bgra[4..8];
        assert_eq!((p[0], p[1], p[3]), (0, 0, 128));
        assert!((126..=130).contains(&p[2]), "{p:?}");
    }

    #[test]
    fn an_icon_of_the_old_kind_is_cut_out_by_its_mask() {
        let bgra = [10u8, 20, 30, 0].repeat(16);
        let gone: Vec<bool> = (0..16).map(|i| i % 4 < 2).collect();
        let icon = icon_of(4, &bgra, &gone);
        let read = shell::icon_pixels(icon);
        unsafe {
            let _ = DestroyIcon(icon);
        }
        let read = read.unwrap();
        for (i, p) in read.bgra.chunks_exact(4).enumerate() {
            assert_eq!(p, if i % 4 < 2 { [0, 0, 0, 0] } else { [10, 20, 30, 255] }, "pixel {i}");
        }
    }

    #[test]
    fn what_a_program_puts_up_for_a_moment_is_told_from_its_main_window() {
        // (The windows are made where no screen is: nothing of this is seen.)
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
            let class_name = wide("BenPopupTest");
            RegisterClassExW(&WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(plain), hInstance: instance, lpszClassName: PCWSTR(class_name.as_ptr()), ..Default::default() });
            let make = |ex: WINDOW_EX_STYLE, owner: Option<HWND>| CreateWindowExW(ex, PCWSTR(class_name.as_ptr()), PCWSTR::null(), WS_POPUP, -8000, 0, 60, 40, owner, None, Some(instance), None).unwrap();
            let home = make(WINDOW_EX_STYLE(0), None);
            let app = Resident::made_up(1, "Test", IconPixels { size: 1, bgra: vec![0; 4] }).of(home);
            let before = app.popups();
            assert!(!app.showing(&before));
            // Its main window coming into view is nothing to wait for.
            let _ = ShowWindow(home, SW_SHOWNOACTIVATE);
            assert!(!app.showing(&before));
            // A window that keeps out of the taskbar is, while it is in view.
            let tool = make(WS_EX_TOOLWINDOW, None);
            assert!(!app.showing(&before));
            let _ = ShowWindow(tool, SW_SHOWNOACTIVATE);
            assert!(app.showing(&before));
            // What was up already before the click is not what the click brought.
            assert!(!app.showing(&app.popups()));
            let _ = ShowWindow(tool, SW_HIDE);
            assert!(!app.showing(&before));
            // So is one that belongs to another window, and one that is not taken to the front.
            for passing in [make(WINDOW_EX_STYLE(0), Some(home)), make(WS_EX_NOACTIVATE, None)] {
                let _ = ShowWindow(passing, SW_SHOWNOACTIVATE);
                assert!(app.showing(&before));
                let _ = DestroyWindow(passing);
                assert!(!app.showing(&before));
            }
            let _ = DestroyWindow(tool);
            let _ = DestroyWindow(home);
        }
    }

    /// The real thing, for a few seconds: the stand-in goes in front of the taskbar, the
    /// running programs are asked for their icons (every program with an icon in the
    /// notification area hears what Explorer asks after a restart), and an icon of the
    /// test's own is added, clicked, hidden and taken away. Nothing is shown and nothing
    /// takes the focus. Only how many icons there are is printed, not what they say. Not
    /// while Bến itself is running with this switched on: there is one stand-in at a time.
    /// Run by hand: cargo test --release the_notification_area_is_read -- --ignored --nocapture
    #[test]
    #[ignore]
    fn the_notification_area_is_read() {
        use std::time::{Duration, Instant};
        use windows::Win32::UI::Shell::{Shell_NotifyIconW, NOTIFYICONDATAW, NOTIFYICON_VERSION_4};

        const TOLD: u32 = WM_APP + 77;
        thread_local! {
            static HEARD: RefCell<Vec<(usize, isize)>> = const { RefCell::new(Vec::new()) };
        }
        unsafe extern "system" fn hear(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            if msg == TOLD {
                HEARD.with_borrow_mut(|heard| heard.push((wparam.0, lparam.0)));
                return LRESULT(0);
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        fn wait(ms: u64) {
            let until = Instant::now() + Duration::from_millis(ms);
            let mut msg = MSG::default();
            while Instant::now() < until {
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        DispatchMessageW(&msg);
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        fn tip(data: &mut NOTIFYICONDATAW, text: &str) {
            data.szTip.fill(0);
            for (to, from) in data.szTip.iter_mut().zip(text.encode_utf16()) {
                *to = from;
            }
        }

        unsafe {
            let class_name = wide(TASKBAR_CLASS);
            let first = || FindWindowW(PCWSTR(class_name.as_ptr()), PCWSTR::null()).ok();
            let real = taskbar().expect("a taskbar");
            assert_eq!(first(), Some(real));
            let watch = start(HWND::default(), WM_APP).expect("the stand-in");
            assert!(first().is_some_and(|window| window != real), "programs find the stand-in first");
            assert_eq!(taskbar(), Some(real));
            wait(2000);
            let found = watch.take().unwrap_or_default();
            println!("{} apps in the background answered", found.len());
            for resident in &found {
                println!("  an icon of {0}x{0} pixels, told of clicks the way of version {1}", resident.picture.size, resident.version);
            }

            // An icon of the test's own goes through the stand-in to the taskbar.
            let instance: HINSTANCE = GetModuleHandleW(None).unwrap().into();
            let own_class = wide("BenIconTest");
            RegisterClassExW(&WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(hear), hInstance: instance, lpszClassName: PCWSTR(own_class.as_ptr()), ..Default::default() });
            let window = CreateWindowExW(WS_EX_TOOLWINDOW, PCWSTR(own_class.as_ptr()), PCWSTR::null(), WS_POPUP, 0, 0, 1, 1, None, None, Some(instance), None).unwrap();
            let icon = icon_of(16, &[0u8, 128, 255, 255].repeat(256), &[false; 256]);
            let mut data = NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: window, uID: 7, uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP, uCallbackMessage: TOLD, hIcon: icon, ..Default::default() };
            tip(&mut data, "Ben test\nline two");
            assert!(Shell_NotifyIconW(NIM_ADD, &data).as_bool(), "the taskbar takes an icon handed to the stand-in");
            wait(300);
            let mine = watch.take().expect("told of the new icon").into_iter().find(|resident| resident.name == "Ben test · line two").expect("the test's icon is there");
            assert_eq!((mine.picture.size, &mine.picture.bgra[..4]), (16, &[0u8, 128, 255, 255][..]));

            // A click reaches the icon's program the old way: the icon's number, then the event.
            mine.click(Button::Left, false, (100, 200));
            wait(300);
            assert_eq!(HEARD.take(), [(7, WM_LBUTTONDOWN as isize), (7, WM_LBUTTONUP as isize)]);
            mine.click(Button::Left, true, (100, 200));
            wait(300);
            assert_eq!(HEARD.take(), [(7, WM_LBUTTONDBLCLK as isize), (7, WM_LBUTTONUP as isize)]);

            // And the new way once the program has asked for it: the place, then the event with the number.
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            assert!(Shell_NotifyIconW(NIM_SETVERSION, &data).as_bool());
            data.uFlags = NIF_TIP;
            tip(&mut data, "Ben test two");
            assert!(Shell_NotifyIconW(NIM_MODIFY, &data).as_bool());
            wait(300);
            let mine = watch.take().expect("told of the new words").into_iter().find(|resident| resident.name == "Ben test two").expect("the test's icon is still there");
            mine.click(Button::Right, false, (100, 200));
            wait(300);
            let place = (200 << 16) | 100;
            let with = |event: u32| ((7 << 16) | event) as isize;
            assert_eq!(HEARD.take(), [(place, with(WM_RBUTTONDOWN)), (place, with(WM_RBUTTONUP)), (place, with(WM_CONTEXTMENU))]);

            // Hidden by its program, it is off the list; shown again, it is back; deleted, it is gone.
            data.uFlags = NIF_STATE;
            (data.dwState, data.dwStateMask) = (NIS_HIDDEN, NIS_HIDDEN);
            assert!(Shell_NotifyIconW(NIM_MODIFY, &data).as_bool());
            wait(300);
            assert!(watch.take().expect("told that it is hidden").iter().all(|resident| resident.id != mine.id));
            data.dwState = Default::default();
            assert!(Shell_NotifyIconW(NIM_MODIFY, &data).as_bool());
            wait(300);
            assert!(watch.take().expect("told that it is back").iter().any(|resident| resident.id == mine.id));
            assert!(Shell_NotifyIconW(NIM_DELETE, &data).as_bool());
            wait(300);
            assert!(watch.take().expect("told that it is gone").iter().all(|resident| resident.id != mine.id));

            // The stand-in goes, and programs find the taskbar itself again.
            drop(watch);
            assert_eq!(first(), Some(real));
            let _ = DestroyWindow(window);
            let _ = DestroyIcon(icon);
        }
    }
}
