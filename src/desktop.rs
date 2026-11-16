//! The desktop's own windows, which belong to Explorer.
//!
//! What lies behind everything on screen is not one window. Since Windows 11
//! 24H2 it is `Progman` with two children: the icons (`SHELLDLL_DefView`) on
//! top and, under them, a `WorkerW` that Windows draws the wallpaper picture
//! into. A window of ours put between those two is a wallpaper that moves
//! (see `wallpaper.rs`). Before 24H2 the icons live in a top-level `WorkerW`
//! of their own, and the wallpaper in the `WorkerW` that follows it.
//!
//! "Show desktop" (Win+D) minimises nothing: it lifts the desktop above every
//! ordinary window. Something that is meant to lie on the desktop, as the
//! widgets do, has to be lifted with it, and let down again after (see
//! `Mark`). This is how Rainmeter keeps its skins on the desktop.

use crate::shell;
use crate::util::wide;
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const MARK_CLASS: &str = "BenDesktopMark";
/// Asks `Progman` to make the `WorkerW` that the wallpaper is drawn into, if it has not yet.
const SPAWN_WORKER: u32 = 0x052C;

/// Where a wallpaper of ours goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Behind {
    /// The window it is a child of.
    pub parent: HWND,
    /// The icons' window, when that is a child of the same parent: the wallpaper has to be
    /// put right under it (the layout since 24H2). `None`: the parent holds nothing else.
    pub icons: Option<HWND>,
}

fn child(parent: HWND, class: &str) -> Option<HWND> {
    unsafe { FindWindowExW(Some(parent), None, PCWSTR(wide(class).as_ptr()), PCWSTR::null()) }.ok().filter(|w| !w.is_invalid())
}

/// Finds where a wallpaper of ours goes: under the icons, over the picture Windows draws.
pub fn behind_icons() -> Option<Behind> {
    unsafe {
        let progman = FindWindowW(PCWSTR(wide("Progman").as_ptr()), PCWSTR::null()).ok().filter(|w| !w.is_invalid())?;
        let mut answer = 0usize;
        SendMessageTimeoutW(progman, SPAWN_WORKER, WPARAM(0xD), LPARAM(0x1), SMTO_NORMAL, 1000, Some(&mut answer));
        // Since 24H2 `Progman` has no surface of its own and holds both the icons and the
        // picture; that it has none is how the layout is told.
        let style = WINDOW_EX_STYLE(GetWindowLongW(progman, GWL_EXSTYLE) as u32);
        if style.contains(WS_EX_NOREDIRECTIONBITMAP) {
            if let Some(icons) = child(progman, "SHELLDLL_DefView") {
                return Some(Behind { parent: progman, icons: Some(icons) });
            }
        }
        // Before: the icons are in a top-level `WorkerW`, and the one after it is the picture's.
        unsafe extern "system" fn each(window: HWND, found: LPARAM) -> BOOL {
            if child(window, "SHELLDLL_DefView").is_some() {
                let after = unsafe { FindWindowExW(None, Some(window), PCWSTR(wide("WorkerW").as_ptr()), PCWSTR::null()) };
                if let Ok(worker) = after {
                    unsafe { *(found.0 as *mut isize) = worker.0 as isize };
                }
            }
            BOOL(1)
        }
        let mut worker = 0isize;
        let _ = EnumWindows(Some(each), LPARAM(&mut worker as *mut isize as isize));
        (worker != 0).then(|| Behind { parent: HWND(worker as *mut _), icons: None })
    }
}

/// The top-level window the icons are in: what "show desktop" lifts.
pub fn icons_window() -> Option<HWND> {
    unsafe {
        let shell_window = GetShellWindow();
        if !shell_window.is_invalid() && child(shell_window, "SHELLDLL_DefView").is_some() {
            return Some(shell_window);
        }
        unsafe extern "system" fn each(window: HWND, found: LPARAM) -> BOOL {
            if shell::class_name(window) == "WorkerW" && child(window, "SHELLDLL_DefView").is_some() {
                unsafe { *(found.0 as *mut isize) = window.0 as isize };
                return BOOL(0);
            }
            BOOL(1)
        }
        let mut host = 0isize;
        let _ = EnumWindows(Some(each), LPARAM(&mut host as *mut isize as isize));
        (host != 0).then(|| HWND(host as *mut _))
    }
}

/// True if what is at this point of the screen is the desktop itself: its picture or its
/// icons, with no window over them.
pub fn is_at(point: POINT) -> bool {
    unsafe {
        let window = WindowFromPoint(point);
        if window.is_invalid() {
            return false;
        }
        let top = GetAncestor(window, GA_ROOT);
        let class = shell::class_name(if top.is_invalid() { window } else { top });
        class == "Progman" || class == "WorkerW"
    }
}

/// A window that is never shown and always kept under every other one. Where the desktop
/// stands compared to it tells whether the desktop has been lifted ("show desktop").
pub struct Mark {
    hwnd: HWND,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Mark {
    pub fn new() -> windows::core::Result<Self> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class_name = wide(MARK_CLASS);
            let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: PCWSTR(class_name.as_ptr()), ..Default::default() };
            RegisterClassExW(&class);
            let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, PCWSTR(class_name.as_ptr()), PCWSTR::null(), WS_POPUP | WS_DISABLED, 0, 0, 0, 0, None, None, Some(instance), None)?;
            let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            Ok(Mark { hwnd })
        }
    }

    /// True while the desktop is lifted above the ordinary windows: the mark, which is under
    /// all of them, is then under the desktop too.
    pub fn desktop_lifted(&self) -> bool {
        let Some(icons) = icons_window() else { return false };
        unsafe {
            if !IsWindowVisible(icons).as_bool() {
                return false;
            }
            // Every top-level window of the mark's class that lies under the icons' window.
            let mut under = FindWindowExW(None, Some(icons), PCWSTR(wide(MARK_CLASS).as_ptr()), PCWSTR::null()).ok();
            while let Some(window) = under.filter(|w| !w.is_invalid()) {
                if window == self.hwnd {
                    return true;
                }
                under = FindWindowExW(None, Some(window), PCWSTR(wide(MARK_CLASS).as_ptr()), PCWSTR::null()).ok();
            }
            false
        }
    }

    /// Puts the mark back under everything (other windows are sent to the bottom too).
    pub fn sink(&self) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

impl Drop for Mark {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The lowest of the windows that stay on top of all others, counted up from the icons'
/// window: a window put right after it is above the lifted desktop and under every bar.
pub fn lowest_topmost() -> Option<HWND> {
    let mut window = icons_window()?;
    unsafe {
        for _ in 0..512 {
            window = GetWindow(window, GW_HWNDPREV).ok().filter(|w| !w.is_invalid())?;
            if WINDOW_EX_STYLE(GetWindowLongW(window, GWL_EXSTYLE) as u32).contains(WS_EX_TOPMOST) {
                return Some(window);
            }
        }
    }
    None
}
