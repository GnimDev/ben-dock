//! The icon carried out of the app tray: a bubble of glass around it that follows the
//! cursor, so that a drag can be seen while the tray itself has gone.
//!
//! The bubble trails the cursor a little and swings back, like a drop on a finger. Over a dock
//! it gives way to the icon that the dock itself squeezes in (see `dock.rs`); let go there it
//! shrinks into the spot, let go anywhere else it swells and bursts. Like the island it is a
//! window of its own; the mouse goes straight through it, and while the button is down the
//! tray's window is the one that hears it.

use crate::motion::Motion;
use crate::scene::{Glass, Scene, Sprite, Stage};
use crate::spring::Spring;
use crate::theme::Theme;
use crate::util::wide;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::CompositionDrawingSurface;

const CLASS_NAME: &str = "BenCarryWindow";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Not on screen.
    Idle,
    /// Held under the cursor.
    Held,
    /// Let go: on its way out.
    Vanishing,
}

pub struct Carry {
    hwnd: HWND,
    _stage: Stage,
    glass: Glass,
    icon: Sprite,
    state: State,
    /// How big the bubble is (1 = as made) and how much of it shows.
    size: Spring,
    fade: Spring,
    /// How far the bubble lags behind the window that follows the cursor, in pixels.
    lag: (Spring, Spring),
    /// Over a dock: the dock shows the icon, so the bubble steps aside.
    over_dock: bool,
    last: Option<POINT>,
    placed: Option<(i32, i32)>,
    /// The bubble's diameter and the window's side, in pixels.
    d: f32,
    side: f32,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        // Not there, as far as the mouse is concerned.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

impl Carry {
    pub fn new(scene: &Scene, theme: &Theme, motion: &Motion) -> Result<Self> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class_name = wide(CLASS_NAME);
            let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: PCWSTR(class_name.as_ptr()), ..Default::default() };
            RegisterClassExW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
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
            let stage = scene.stage(hwnd)?;
            let glass = scene.glass_in(&stage.content()?, &theme.island)?;
            let icon = scene.sprite_in(&glass.inner()?)?;
            Ok(Carry {
                hwnd,
                _stage: stage,
                glass,
                icon,
                state: State::Idle,
                size: Spring::with(0.5, motion.medium()),
                fade: Spring::with(0.0, motion.fade()),
                lag: (Spring::with(0.0, motion.medium()), Spring::with(0.0, motion.medium())),
                over_dock: false,
                last: None,
                placed: None,
                d: 1.0,
                side: 1.0,
            })
        }
    }

    /// True while an icon is held (not once it has been let go).
    pub fn is_held(&self) -> bool {
        self.state == State::Held
    }

    /// Picks the icon up: the bubble swells out of the cursor. `icon` is the icon's size in
    /// pixels, `scale` the screen's.
    pub fn lift(&mut self, surface: &CompositionDrawingSurface, icon: f32, scale: f32, theme: &Theme, motion: &Motion) -> Result<()> {
        let px = |v: f32| (v * scale).round();
        self.d = (icon * 1.5).round();
        // Room for the bubble swelling to a burst, and for its shadow.
        self.side = (self.d * 1.5 + 2.0 * (theme.island.shadow_reach + px(10.0))).round();
        self.glass.set_material(&theme.island);
        self.icon.set_surface(surface)?;
        let pad = ((self.d - icon) * 0.5).round();
        self.icon.place(pad, pad, icon, icon)?;
        self.icon.set_opacity(1.0)?;
        self.size = Spring::with(0.45, motion.medium());
        self.fade = Spring::with(0.0, motion.fade());
        self.lag = (Spring::with(0.0, motion.medium()), Spring::with(0.0, motion.medium()));
        self.state = State::Held;
        self.over_dock = false;
        self.last = None;
        self.placed = None;
        let mut at = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut at);
            let half = (self.side * 0.5) as i32;
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), at.x - half, at.y - half, self.side as i32, self.side as i32, SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
        self.placed = Some((at.x - (self.side * 0.5) as i32, at.y - (self.side * 0.5) as i32));
        Ok(())
    }

    /// Whether the cursor is over a dock, which shows the icon itself.
    pub fn over_dock(&mut self, over: bool) {
        self.over_dock = over;
    }

    /// The button came up. `landed`: the icon was pinned to a dock, so the bubble shrinks into
    /// the spot; otherwise it swells and bursts.
    pub fn vanish(&mut self, landed: bool, motion: &Motion) {
        if self.state != State::Held {
            return;
        }
        self.state = State::Vanishing;
        self.size.tune(motion.small());
        self.size.set(if landed { 0.3 } else { 1.5 });
        self.fade.tune(motion.calm());
        self.fade.set(0.0);
    }

    /// Follows the cursor and advances the motion. Returns true while the bubble is on screen.
    pub fn update(&mut self, scene: &Scene, motion: &Motion, dt: f32) -> Result<bool> {
        if self.state == State::Idle {
            return Ok(false);
        }
        let mut at = POINT::default();
        if unsafe { GetCursorPos(&mut at) }.is_ok() {
            // The window keeps up with the cursor; the bubble in it falls behind, then swings back.
            if let Some(last) = self.last {
                let most = self.d * 0.35;
                self.lag.0.pos = (self.lag.0.pos - (at.x - last.x) as f32).clamp(-most, most);
                self.lag.1.pos = (self.lag.1.pos - (at.y - last.y) as f32).clamp(-most, most);
            }
            self.last = Some(at);
            let half = (self.side * 0.5) as i32;
            let place = (at.x - half, at.y - half);
            if self.placed != Some(place) {
                self.placed = Some(place);
                unsafe {
                    let _ = SetWindowPos(self.hwnd, None, place.0, place.1, 0, 0, SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER);
                }
            }
        }
        if self.state == State::Held {
            self.size.tune(motion.medium());
            self.size.set(if self.over_dock { 0.5 } else { 1.0 });
            self.fade.set(if self.over_dock { 0.0 } else { 1.0 });
        }
        for lag in [&mut self.lag.0, &mut self.lag.1] {
            lag.tune(motion.medium());
            lag.set(0.0);
            lag.step(dt);
        }
        self.size.step(dt);
        self.fade.step(dt);

        let d = self.d;
        let (gx, gy) = ((self.side - d) * 0.5 + self.lag.0.pos, (self.side - d) * 0.5 + self.lag.1.pos);
        self.glass.set_rect(scene, gx, gy, d, d, d * 0.5)?;
        let scale = self.size.pos.max(0.05);
        self.glass.set_scale(scale, scale, d * 0.5, d * 0.5)?;
        self.glass.set_opacity(self.fade.pos.clamp(0.0, 1.0))?;

        if self.state == State::Vanishing && self.fade.pos < 0.02 {
            self.state = State::Idle;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            return Ok(false);
        }
        Ok(true)
    }
}

impl Drop for Carry {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
