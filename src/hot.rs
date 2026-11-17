//! Hot corners: the cursor pushed into a bottom corner of the screen sends every window
//! out of the way, as a click on the far end of the taskbar does; pushed there again, it
//! brings them back.
//!
//! A corner is a window of a few pixels in the very corner of the screen, with nothing
//! drawn in it. It lies over the glass that may stand in the same corner, so the glass
//! keeps everything but the corner itself. The cursor has to stay a moment: one that only
//! passes through does nothing, and each visit counts once.

use crate::app::{self, Event};
use crate::util::wide;
use std::cell::Cell;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "BenHotCorner";
const WM_MOUSELEAVE: u32 = 0x02A3;
/// How far the corner reaches along each edge, in logical pixels.
const REACH: f32 = 3.0;
/// How long the cursor has to stay, in milliseconds.
const STAY_MS: u32 = 160;
const TIMER_STAY: usize = 1;

thread_local! {
    /// The corner the cursor is in (0: none), and whether the app has been told of this visit.
    static VISIT: Cell<(isize, bool)> = const { Cell::new((0, false)) };
}

/// What a corner does with what its window hears. Kept apart from the window so that it
/// can be tried without a cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Heard {
    /// The cursor moved inside the corner.
    Move,
    Leave,
    /// It has stayed long enough.
    Stayed,
    /// A click in the very corner: it does not wait (it is what the end of the taskbar did).
    Click,
}

/// What to do about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Then {
    Nothing,
    /// A visit began: start waiting, and listen for the cursor leaving.
    Wait,
    StopWaiting,
    /// Show the desktop (and stop waiting).
    Tell,
}

/// One step: the visit so far (the corner the cursor is in, whether it has been told
/// about), what corner `me` heard, and from that the visit as it is now and what to do.
fn step(visit: (isize, bool), me: isize, heard: Heard) -> ((isize, bool), Then) {
    match heard {
        Heard::Move if visit.0 != me => ((me, false), Then::Wait),
        Heard::Move => (visit, Then::Nothing),
        Heard::Leave if visit.0 == me => ((0, false), Then::StopWaiting),
        Heard::Leave => (visit, Then::StopWaiting),
        Heard::Stayed | Heard::Click if visit == (me, false) => ((me, true), Then::Tell),
        Heard::Stayed | Heard::Click => (visit, Then::StopWaiting),
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let heard = match msg {
        WM_MOUSEMOVE => Heard::Move,
        WM_MOUSELEAVE => Heard::Leave,
        WM_TIMER => Heard::Stayed,
        WM_LBUTTONUP => Heard::Click,
        // A click here must not take the keyboard from the app in use.
        WM_MOUSEACTIVATE => return LRESULT(MA_NOACTIVATE as isize),
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    };
    let (visit, then) = step(VISIT.get(), hwnd.0 as isize, heard);
    unsafe {
        match then {
            Then::Nothing => {}
            Then::Wait => {
                let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                if TrackMouseEvent(&mut track).is_err() {
                    // Without word of the cursor leaving, a visit would never end.
                    return LRESULT(0);
                }
                SetTimer(Some(hwnd), TIMER_STAY, STAY_MS, None);
            }
            Then::StopWaiting => {
                let _ = KillTimer(Some(hwnd), TIMER_STAY);
            }
            Then::Tell => {
                let _ = KillTimer(Some(hwnd), TIMER_STAY);
                app::post(Event::HotCorner);
            }
        }
    }
    VISIT.set(visit);
    LRESULT(0)
}

pub struct Hot {
    hwnd: HWND,
    left: bool,
    /// Where it is on screen (left, top), how large, and whether it is there at all.
    placed: Option<(i32, i32, i32, bool)>,
}

impl Hot {
    /// A corner for the left or the right end of a screen's bottom edge.
    pub fn new(left: bool) -> Result<Self> {
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
            // No surface of its own and nothing drawn in it: it is there for the mouse only.
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST, PCWSTR(class_name.as_ptr()), PCWSTR::null(), WS_POPUP, 0, 0, 1, 1, None, None, Some(instance), None)?;
            Ok(Hot { hwnd, left, placed: None })
        }
    }

    pub fn left(&self) -> bool {
        self.left
    }

    /// Where the corner is on a screen: left, top and size. `screen`: the whole screen.
    fn spot(left: bool, screen: RECT, scale: f32) -> (i32, i32, i32) {
        let reach = (REACH * scale).round().max(2.0) as i32;
        (if left { screen.left } else { screen.right - reach }, screen.bottom - reach, reach)
    }

    /// Puts the corner in its corner of a screen, or takes it away (`shown` false: a
    /// full-screen app has the screen, and its corners with it).
    pub fn place(&mut self, screen: RECT, scale: f32, shown: bool) {
        let (x, y, reach) = Self::spot(self.left, screen, scale);
        if self.placed == Some((x, y, reach, shown)) {
            return;
        }
        self.placed = Some((x, y, reach, shown));
        unsafe {
            if shown {
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, reach, reach, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            } else {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }

    /// Puts it over the other surfaces again. They are topmost too, and whichever was shown
    /// last lies on top: the glass in the same corner, for one.
    pub fn raise(&self) {
        if matches!(self.placed, Some((.., true))) {
            unsafe {
                let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }
}

impl Drop for Hot {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_visit_tells_once_and_only_after_a_stay() {
        let none = (0, false);
        // The cursor comes in: wait. Moving about inside changes nothing.
        let (visit, then) = step(none, 7, Heard::Move);
        assert_eq!((visit, then), ((7, false), Then::Wait));
        assert_eq!(step(visit, 7, Heard::Move), (visit, Then::Nothing));
        // It has stayed: tell. Staying on, or clicking after that, does not tell again.
        let (visit, then) = step(visit, 7, Heard::Stayed);
        assert_eq!((visit, then), ((7, true), Then::Tell));
        assert_eq!(step(visit, 7, Heard::Click).1, Then::StopWaiting);
        assert_eq!(step(visit, 7, Heard::Stayed).1, Then::StopWaiting);
        // It leaves and comes back: a new visit.
        let (visit, then) = step(visit, 7, Heard::Leave);
        assert_eq!((visit, then), (none, Then::StopWaiting));
        assert_eq!(step(visit, 7, Heard::Move).1, Then::Wait);
    }

    #[test]
    fn a_cursor_that_only_passes_through_does_nothing() {
        let (visit, _) = step((0, false), 7, Heard::Move);
        let (visit, then) = step(visit, 7, Heard::Leave);
        assert_eq!((visit, then), ((0, false), Then::StopWaiting));
        // A timer that still rings after that finds no visit to tell about.
        assert_eq!(step(visit, 7, Heard::Stayed).1, Then::StopWaiting);
    }

    #[test]
    fn a_click_does_not_wait_and_another_corner_starts_its_own_visit() {
        let (visit, _) = step((0, false), 7, Heard::Move);
        assert_eq!(step(visit, 7, Heard::Click), ((7, true), Then::Tell));
        // The cursor is in corner 7; corner 9 hears a move (it jumped there): 9's visit.
        assert_eq!(step((7, true), 9, Heard::Move), ((9, false), Then::Wait));
        // A leave from 7 that arrives late does not end 9's visit.
        assert_eq!(step((9, false), 7, Heard::Leave), ((9, false), Then::StopWaiting));
    }

    /// Puts a corner in each bottom corner of the first screen for a moment and asks Windows
    /// which window is at the very corner: it has to be the corner's own, though nothing is
    /// drawn there, and no longer once the corner is taken away. The cursor is not moved.
    /// Run by hand: cargo test --profile quick the_corners_take_the_mouse -- --ignored
    #[test]
    #[ignore]
    fn the_corners_take_the_mouse() {
        let screen = unsafe { RECT { left: 0, top: 0, right: GetSystemMetrics(SM_CXSCREEN), bottom: GetSystemMetrics(SM_CYSCREEN) } };
        for left in [true, false] {
            let mut hot = Hot::new(left).expect("a corner");
            hot.place(screen, 1.0, true);
            let (x, y, reach) = Hot::spot(left, screen, 1.0);
            // The outermost pixel of the screen, where a cursor pushed into the corner ends up.
            let outermost = POINT { x: if left { x } else { x + reach - 1 }, y: y + reach - 1 };
            assert_eq!(unsafe { WindowFromPoint(outermost) }, hot.hwnd, "the {} corner", if left { "left" } else { "right" });
            hot.raise();
            assert_eq!(unsafe { WindowFromPoint(outermost) }, hot.hwnd);
            hot.place(screen, 1.0, false);
            assert_ne!(unsafe { WindowFromPoint(outermost) }, hot.hwnd);
        }
    }

    #[test]
    fn a_corner_is_in_the_very_corner_of_its_screen() {
        let screen = RECT { left: 1920, top: 0, right: 3840, bottom: 1080 };
        assert_eq!(Hot::spot(true, screen, 1.0), (1920, 1077, 3));
        assert_eq!(Hot::spot(false, screen, 1.0), (3837, 1077, 3));
        // Larger on a screen that is scaled, and never less than two pixels.
        assert_eq!(Hot::spot(false, screen, 2.0), (3834, 1074, 6));
        assert_eq!(Hot::spot(true, screen, 0.5).2, 2);
    }
}
