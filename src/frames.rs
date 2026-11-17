//! Other programs' windows in the colours of the theme.
//!
//! Windows draws the title bar and the border of a window, and lets them be
//! given colours: by the window's own program, and by any other. A window
//! that has a title bar gets the colour of the theme's glass for the bar and
//! the colour of the theme's writing for its title; the border of the window
//! in front is the accent colour, that of every other one the glass with a
//! little of its bright edge. A program that draws its own title bar (a
//! browser, most new apps) shows only the border.
//!
//! Windows keeps none of this: a window that closes takes its colours along,
//! and `undress` gives every window back what Windows would give it. A window
//! of a program that runs as administrator cannot be given anything by a
//! program that does not; it stays as it is.
//!
//! The glass of File Explorer itself is left to ExplorerBlurMica (see
//! `blurmica`): it cannot be had from outside the window, and from inside it
//! means putting code into Explorer, which Bến does not do.

use crate::config::Config;
use crate::shell;
use crate::theme::Theme;
use std::collections::HashMap;
use windows::core::BOOL;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWINDOWATTRIBUTE};
use windows::Win32::UI::WindowsAndMessaging::*;

/// In place of a colour: what Windows would give the window (`DWMWA_COLOR_DEFAULT`).
const AS_WINDOWS_HAS_IT: u32 = 0xFFFF_FFFF;

/// What a window's frame is given: the colour of its title bar, of its title and of its
/// border, as Windows takes them (0x00BBGGRR). `None`: what Windows would give it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Dress {
    bar: Option<u32>,
    title: Option<u32>,
    border: Option<u32>,
}

/// A colour as Windows takes it: 0x00BBGGRR.
fn colorref(r: f32, g: f32, b: f32) -> u32 {
    let part = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    part(r) | part(g) << 8 | part(b) << 16
}

/// What the settings and the theme give a window: one in front of the others, or another.
pub fn dress_of(cfg: &Config, theme: &Theme, front: bool) -> Dress {
    let glass = (theme.dock.tint.R as f32 / 255.0, theme.dock.tint.G as f32 / 255.0, theme.dock.tint.B as f32 / 255.0);
    let rim = (theme.dock.rim.R as f32 / 255.0, theme.dock.rim.G as f32 / 255.0, theme.dock.rim.B as f32 / 255.0);
    // The border of a window that is not in front: the glass, a little of its bright edge.
    let quiet = |glass: f32, rim: f32| glass + (rim - glass) * 0.26;
    Dress {
        bar: cfg.frame_titles.then(|| colorref(glass.0, glass.1, glass.2)),
        title: cfg.frame_titles.then(|| colorref(theme.text.r, theme.text.g, theme.text.b)),
        border: cfg.frame_borders.then(|| if front { colorref(theme.accent.0, theme.accent.1, theme.accent.2) } else { colorref(quiet(glass.0, rim.0), quiet(glass.1, rim.1), quiet(glass.2, rim.2)) }),
    }
}

/// Gives a window its frame's colours. (What cannot be given, to a window of a program that
/// runs as administrator for one, is left as it is.)
fn give(hwnd: HWND, dress: Dress) {
    let set = |attribute: DWMWINDOWATTRIBUTE, colour: Option<u32>| {
        let colour = colour.unwrap_or(AS_WINDOWS_HAS_IT);
        unsafe {
            let _ = DwmSetWindowAttribute(hwnd, attribute, &colour as *const u32 as *const _, 4);
        }
    };
    set(DWMWA_CAPTION_COLOR, dress.bar);
    set(DWMWA_TEXT_COLOR, dress.title);
    set(DWMWA_BORDER_COLOR, dress.border);
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    list.push(hwnd);
    BOOL(1)
}

/// The windows that have a frame of Windows' to colour: in view, with a title bar or an
/// edge to size them by, and not the app's own.
fn framed_windows() -> Vec<HWND> {
    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect_window), LPARAM(&mut handles as *mut _ as isize));
    }
    handles.retain(|hwnd| unsafe {
        let style = WINDOW_STYLE(GetWindowLongW(*hwnd, GWL_STYLE) as u32);
        IsWindowVisible(*hwnd).as_bool() && (style.contains(WS_CAPTION) || style.contains(WS_THICKFRAME)) && !shell::is_own_window(*hwnd)
    });
    handles
}

/// The windows that wear the theme, and what each was given.
#[derive(Default)]
pub struct Frames {
    given: HashMap<isize, Dress>,
}

impl Frames {
    /// Gives every window with a frame the colours the settings ask for (and takes them
    /// back when they ask for none). `front`: the window in front of the others. A window
    /// is only touched when what it is to wear has changed.
    pub fn tend(&mut self, cfg: &Config, theme: &Theme, front: HWND) {
        if !cfg.frame_borders && !cfg.frame_titles {
            self.strip();
            return;
        }
        for hwnd in framed_windows() {
            let dress = dress_of(cfg, theme, hwnd == front);
            if self.given.insert(hwnd.0 as isize, dress) != Some(dress) {
                give(hwnd, dress);
            }
        }
        self.given.retain(|id, _| shell::exists(HWND(*id as *mut _)));
    }

    /// Takes the colours back.
    fn strip(&mut self) {
        for (id, _) in self.given.drain() {
            give(HWND(id as *mut _), Dress::default());
        }
    }

    /// Gives every window back what Windows would give it.
    pub fn undress(&mut self) {
        self.strip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;
    use crate::util::wide;
    use windows::core::PCWSTR;
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;

    fn themed(scheme: usize) -> (Config, Theme) {
        let mut cfg = Config::default();
        theme::choose(scheme, &mut cfg);
        let theme = theme::build(&cfg, true, (0.0, 0.47, 0.84), 1.0);
        (cfg, theme)
    }

    #[test]
    fn a_colour_is_written_blue_first() {
        assert_eq!(colorref(1.0, 0.5, 0.0), 0x0000_80FF);
        assert_eq!(colorref(0.0, 0.0, 1.0), 0x00FF_0000);
        assert_eq!(colorref(2.0, -1.0, 1.0), 0x00FF_00FF);
    }

    #[test]
    fn a_window_wears_only_what_the_settings_ask_for() {
        let (mut cfg, theme) = themed(1);
        // Nothing asked for: nothing given.
        assert_eq!(dress_of(&cfg, &theme, true), Dress::default());
        // The borders alone: the accent colour in front, a quieter one behind.
        cfg.frame_borders = true;
        let (front, behind) = (dress_of(&cfg, &theme, true), dress_of(&cfg, &theme, false));
        assert_eq!(front.border, Some(colorref(theme.accent.0, theme.accent.1, theme.accent.2)));
        assert!(behind.border.is_some() && behind.border != front.border);
        assert_eq!((front.bar, front.title), (None, None));
        // The title bars too: the glass, and what is written on it.
        cfg.frame_titles = true;
        let dress = dress_of(&cfg, &theme, false);
        assert_eq!(dress.bar, Some(theme.dock.tint.R as u32 | (theme.dock.tint.G as u32) << 8 | (theme.dock.tint.B as u32) << 16));
        assert_eq!(dress.title, Some(0x00FF_FFFF));
        // A light theme: dark words on light glass.
        let (mut cfg, theme) = themed(2);
        cfg.frame_titles = true;
        let dress = dress_of(&cfg, &theme, true);
        assert_eq!(dress.title, Some(0));
        assert!(dress.bar.unwrap() & 0xFF > 200);
        assert_eq!(dress.border, None);
    }

    /// Gives a window of the test's own the colours of a theme and takes them back, and
    /// reads the title bar off the window's own picture each time. The window stands where
    /// no screen is and takes no focus.
    /// Run by hand: cargo test a_window_wears_the_theme -- --ignored --nocapture
    /// (BEN_SCHEME: which theme, 1 without it. BEN_OUT: a folder for a picture of the window.)
    #[test]
    #[ignore]
    fn a_window_wears_the_theme() {
        unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        let scheme = std::env::var("BEN_SCHEME").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        let (mut cfg, theme) = themed(scheme);
        (cfg.frame_borders, cfg.frame_titles) = (true, true);
        let (w, h) = (460, 300);
        let hwnd = unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).expect("a module").into();
            let class_name = wide("BenFrameTestWindow");
            let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: PCWSTR(class_name.as_ptr()), hbrBackground: windows::Win32::Graphics::Gdi::HBRUSH(6 as *mut _), ..Default::default() };
            RegisterClassExW(&class);
            let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, PCWSTR(class_name.as_ptr()), PCWSTR(wide("Một cửa sổ để thử").as_ptr()), WS_OVERLAPPEDWINDOW, -20000, 200, w, h, None, None, Some(instance), None).expect("a window");
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            hwnd
        };
        let mut msg = MSG::default();
        let mut look = |name: &str| {
            let from = std::time::Instant::now();
            while from.elapsed().as_secs_f32() < 0.5 {
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        DispatchMessageW(&msg);
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            let pixels = unsafe { crate::camera::print(hwnd, w, h) }.expect("a picture of the window");
            if let Ok(out) = std::env::var("BEN_OUT") {
                crate::camera::save_picture(&std::path::Path::new(&out).join(format!("frame-{name}.bmp")), w, h, &pixels);
            }
            // The title bar a third of the way along, clear of the title and of the buttons.
            let at = ((14 * w + w / 3) * 4) as usize;
            (pixels[at + 2] as u32) | (pixels[at + 1] as u32) << 8 | (pixels[at] as u32) << 16
        };
        let plain = look("plain");
        let dress = dress_of(&cfg, &theme, true);
        give(hwnd, dress);
        let worn = look("worn");
        give(hwnd, Dress::default());
        let back = look("back");
        println!("the title bar: {plain:#08x} as Windows has it, {worn:#08x} in the theme (asked for {:#08x}), {back:#08x} given back", dress.bar.unwrap());
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
        let near = |a: u32, b: u32| (0..3).all(|part| ((a >> (8 * part)) & 0xFF).abs_diff((b >> (8 * part)) & 0xFF) <= 3);
        assert!(near(worn, dress.bar.unwrap()), "the title bar did not take the theme's colour");
        assert!(near(back, plain), "the title bar was not given back");
    }
}
