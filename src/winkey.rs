//! The Windows key opens Bến's app tray instead of the Start menu.
//!
//! A tap of the key on its own is taken; the key used with another one
//! (Win+D, Win+E, Win+Shift+S...) is left to Windows. The Start menu opens when
//! the key comes up, unless something else was pressed while it was down; so on a
//! tap a harmless key is pressed and released first, which is what stops it.
//! Nothing is swallowed, so the key can never be left held down.
//!
//! The hook has a thread of its own: Windows drops a keyboard hook whose thread
//! does not answer within a fraction of a second, and the thread that animates the
//! dock is busy now and then for longer than that.

use std::cell::Cell;
use std::thread::JoinHandle;
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Tags the key presses sent from here, so the hook can tell them from the user's.
const OWN: usize = 0x42_45_4E;
/// A key that exists on no keyboard and does nothing. It is what makes the Start menu
/// think the Windows key was used for something.
const MASK: u16 = 0xE8;

/// What has been seen of the Windows key so far.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Keys {
    /// The key is down.
    held: bool,
    /// Another key went down while it was.
    used: bool,
}

impl Keys {
    /// A key went down (`down`) or up. Returns true for the end of a tap of the Windows key
    /// on its own.
    fn step(&mut self, key: u32, down: bool) -> bool {
        let windows = key == VK_LWIN.0 as u32 || key == VK_RWIN.0 as u32;
        match (windows, down) {
            (true, true) => {
                // Held down, the key repeats: only the first press counts.
                if !self.held {
                    *self = Keys { held: true, used: false };
                }
                false
            }
            (true, false) => {
                let tap = self.held && !self.used;
                *self = Keys::default();
                tap
            }
            (false, true) => {
                self.used |= self.held;
                false
            }
            (false, false) => false,
        }
    }
}

thread_local! {
    static KEYS: Cell<Keys> = const { Cell::new(Keys { held: false, used: false }) };
    /// The window that is told, and with which message.
    static TELL: Cell<(isize, u32)> = const { Cell::new((0, 0)) };
}

unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 && lparam.0 != 0 {
        let key = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if key.dwExtraInfo != OWN {
            let message = wparam.0 as u32;
            let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
            if down || message == WM_KEYUP || message == WM_SYSKEYUP {
                let mut keys = KEYS.get();
                let tap = keys.step(key.vkCode, down);
                KEYS.set(keys);
                if tap {
                    press_mask();
                    let (window, message) = TELL.get();
                    unsafe {
                        let _ = PostMessageW(Some(HWND(window as *mut _)), message, WPARAM(0), LPARAM(0));
                    }
                }
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Presses and releases the key that does nothing.
fn press_mask() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(MASK), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: OWN } },
    };
    let keys = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    unsafe {
        SendInput(&keys, std::mem::size_of::<INPUT>() as i32);
    }
}

/// The hook, for as long as this is kept.
pub struct WinKey {
    thread: u32,
    runner: Option<JoinHandle<()>>,
}

/// Starts watching the Windows key. `window` is sent `message` for each tap of it on its
/// own. `None` if the hook could not be made (the Start menu then stays as it is).
pub fn start(window: HWND, message: u32) -> Option<WinKey> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let window = window.0 as isize;
    let run = move || unsafe {
        TELL.set((window, message));
        let hooked = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), None, 0) {
            Ok(hooked) => hooked,
            Err(e) => {
                crate::log!("could not watch the Windows key ({e}); the Start menu stays as it is");
                let _ = sender.send(None);
                return;
            }
        };
        let _ = sender.send(Some(GetCurrentThreadId()));
        // Keyboard events reach the hook through this thread's messages.
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {}
        let _ = UnhookWindowsHookEx(hooked);
    };
    let runner = std::thread::Builder::new().name("winkey".into()).stack_size(128 * 1024).spawn(run).ok()?;
    let thread = receiver.recv().ok().flatten()?;
    Some(WinKey { thread, runner: Some(runner) })
}

impl Drop for WinKey {
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

    const WIN: u32 = 91;
    const RIGHT_WIN: u32 = 92;
    const D: u32 = 0x44;

    fn play(events: &[(u32, bool)]) -> Vec<bool> {
        let mut keys = Keys::default();
        events.iter().map(|&(key, down)| keys.step(key, down)).collect()
    }

    #[test]
    fn a_tap_of_the_key_on_its_own_is_taken_when_it_comes_up() {
        assert_eq!(play(&[(WIN, true), (WIN, false)]), [false, true]);
        assert_eq!(play(&[(RIGHT_WIN, true), (RIGHT_WIN, false)]), [false, true]);
    }

    #[test]
    fn holding_it_down_does_not_change_that() {
        assert_eq!(play(&[(WIN, true), (WIN, true), (WIN, true), (WIN, false)]), [false, false, false, true]);
    }

    #[test]
    fn the_key_used_with_another_is_left_to_windows() {
        // Win+D.
        assert_eq!(play(&[(WIN, true), (D, true), (D, false), (WIN, false)]), [false, false, false, false]);
        // Another key still held when it is pressed, and let go first or last.
        assert_eq!(play(&[(D, true), (WIN, true), (D, false), (WIN, false)]), [false, false, false, true]);
        // The next tap is a tap again.
        assert_eq!(play(&[(WIN, true), (D, true), (WIN, false), (D, false), (WIN, true), (WIN, false)]), [false, false, false, false, false, true]);
    }

    #[test]
    fn a_key_coming_up_alone_is_nothing() {
        assert_eq!(play(&[(WIN, false), (D, false)]), [false, false]);
    }
}
