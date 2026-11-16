//! A window that only holds shadows.
//!
//! The dock and the island take the mouse only where their glass is: their
//! windows are cut down to that rectangle, so that a click beside the glass
//! reaches whatever lies behind. But what a window is cut down to is also all
//! it may show, and a shadow lies exactly outside the glass. So each of them
//! has a second window of the same size, under the first, that shows nothing
//! but the shadow and lets every click through.

use crate::scene::{Scene, Stage};
use crate::util::wide;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::VisualCollection;

const CLASS_NAME: &str = "BenShadeWindow";

pub struct Shade {
    hwnd: HWND,
    stage: Stage,
    placed: Option<(i32, i32, i32, i32)>,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // Not there, as far as the mouse is concerned.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

impl Shade {
    pub fn new(scene: &Scene) -> Result<Self> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class_name = wide(CLASS_NAME);
            let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: PCWSTR(class_name.as_ptr()), ..Default::default() };
            RegisterClassExW(&class);
            // WS_EX_LAYERED | WS_EX_TRANSPARENT: mouse input goes straight through to the windows below.
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(class_name.as_ptr()),
                PCWSTR::null(),
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
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            match scene.stage(hwnd) {
                Ok(stage) => Ok(Shade { hwnd, stage, placed: None }),
                Err(e) => {
                    let _ = DestroyWindow(hwnd);
                    Err(e)
                }
            }
        }
    }

    /// Where the shadows go. Coordinates are the same as in the window this one lies under.
    pub fn content(&self) -> Result<VisualCollection> {
        self.stage.content()
    }

    /// Puts the window exactly under `above`, which is at (`x`, `y`) and `w` by `h` on screen.
    pub fn place(&mut self, above: HWND, x: i32, y: i32, w: i32, h: i32) {
        if self.placed == Some((x, y, w, h)) {
            return;
        }
        self.placed = Some((x, y, w, h));
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(above), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    /// Puts the window right under `above` again, where it already is on screen: for when
    /// `above` has changed its place among the other windows (the widgets on the desktop do).
    pub fn tuck_under(&self, above: HWND) {
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(above), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// Moves everything in the window (when the surface above slides out of view).
    pub fn set_offset(&mut self, x: f32, y: f32) -> Result<()> {
        self.stage.set_offset(x, y)
    }
}

impl Drop for Shade {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
