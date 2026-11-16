//! Takes a picture of any window, shown or minimised, without anything
//! appearing on screen and without holding up the main thread.
//!
//! Windows will show the live picture of a window (a "thumbnail") on a window
//! of ours, and will draw a window of ours, with everything composed on it,
//! into a bitmap (`PrintWindow` with "full content"). Put together: a helper
//! window that is cloaked (composed like any other, but never shown) is given
//! the live picture of the window to photograph and is then printed. That
//! works for a minimised window too, which cannot be printed itself, and it
//! is never fooled by what happens to lie in front of the window.
//!
//! The print has no transparency, and a window's corners are round. So the
//! helper is white, and under the picture it has a black strip on which the
//! picture's four corners are shown once more. A pixel that is lighter over
//! white than over black is see-through by exactly that much.
//!
//! One print takes a few hundredths of a second for a large window (most of
//! it waiting for the compositor), so it runs on a thread of its own, which
//! also makes the smaller copies of the picture (see `Scene::picture_level`).

use crate::scene::halve;
use crate::util::wide;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::Instant;
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Posted to the camera's thread: take a picture (wParam = the request's number, lParam = the window).
const WM_SHOOT: u32 = WM_APP + 20;
/// PW_RENDERFULLCONTENT: the desktop compositor draws the window, with everything it composes on it.
const FULL_CONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);
/// How many sizes of a picture are made: full size and five halvings (down to 1/32).
const MOST_LEVELS: usize = 6;
/// Side of the square looked at in each corner for transparency, in pixels.
const CORNER: i32 = 24;

/// A window's picture.
pub struct Shot {
    /// The picture at full size, then halved again and again: (width, height, pixels).
    /// Premultiplied BGRA, top row first.
    pub levels: Vec<(u32, u32, Vec<u8>)>,
    /// How long taking it took, in milliseconds.
    pub took: f32,
}

pub struct Camera {
    thread: u32,
    got: Receiver<(u64, Option<Shot>)>,
    last: u64,
}

impl Camera {
    /// Starts the camera's thread. `None` if it cannot work on this system.
    pub fn start() -> Option<Self> {
        let (send, got) = channel();
        let (ready, started) = channel();
        std::thread::Builder::new().name("camera".into()).spawn(move || run(send, ready)).ok()?;
        let thread = started.recv().ok()?;
        (thread != 0).then_some(Camera { thread, got, last: 0 })
    }

    /// Asks for a picture of a window. Returns the number to collect it by.
    pub fn ask(&mut self, window: HWND) -> Option<u64> {
        self.last += 1;
        unsafe { PostThreadMessageW(self.thread, WM_SHOOT, WPARAM(self.last as usize), LPARAM(window.0 as isize)) }.ok()?;
        Some(self.last)
    }

    /// The answer to request `id`, once it is there; `Some(None)` if no picture could be
    /// taken. Answers to earlier requests are thrown away.
    pub fn take(&mut self, id: u64) -> Option<Option<Shot>> {
        while let Ok((answered, shot)) = self.got.try_recv() {
            if answered == id {
                return Some(shot);
            }
        }
        None
    }
}

impl Drop for Camera {
    fn drop(&mut self) {
        unsafe {
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

/// The camera's thread: an ordinary message loop (its helper is a top-level window, and a
/// top-level window whose thread does not answer messages holds up every program that sends
/// one to all windows), with requests arriving as messages to the thread.
fn run(send: Sender<(u64, Option<Shot>)>, ready: Sender<u32>) {
    unsafe {
        // Looking at the queue creates it, so that requests posted from now on are kept.
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE);
        let studio = Studio::new();
        let _ = ready.send(if studio.is_some() { GetCurrentThreadId() } else { 0 });
        let Some(studio) = studio else { return };
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            if !msg.hwnd.is_invalid() || msg.message != WM_SHOOT {
                DispatchMessageW(&msg);
                continue;
            }
            // Only the newest request matters: the app plays one effect at a time.
            let (mut id, mut window) = (msg.wParam.0 as u64, msg.lParam.0);
            let mut newer = MSG::default();
            while PeekMessageW(&mut newer, None, WM_SHOOT, WM_SHOOT, PM_REMOVE).as_bool() {
                if newer.hwnd.is_invalid() {
                    let _ = send.send((id, None));
                    (id, window) = (newer.wParam.0 as u64, newer.lParam.0);
                }
            }
            if send.send((id, studio.shoot(HWND(window as *mut _)))).is_err() {
                break;
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// The helper window pictures are taken on.
struct Studio {
    hwnd: HWND,
}

impl Studio {
    unsafe fn new() -> Option<Self> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).ok()?.into();
            let class_name = wide("BenCameraWindow");
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                // White wherever nothing else is drawn.
                hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&class);
            let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, PCWSTR(class_name.as_ptr()), PCWSTR::null(), WS_POPUP, 0, 0, 16, 16, None, None, Some(instance), None).ok()?;
            // Cloaked: composed like any window, but never put on screen. Without this the
            // helper would flash up with every picture, so without it there is no camera.
            let on = BOOL(1);
            if DwmSetWindowAttribute(hwnd, DWMWA_CLOAK, &on as *const BOOL as *const _, std::mem::size_of::<BOOL>() as u32).is_err() {
                let _ = DestroyWindow(hwnd);
                return None;
            }
            Some(Studio { hwnd })
        }
    }

    /// Takes the picture of a window and makes its smaller copies.
    unsafe fn shoot(&self, window: HWND) -> Option<Shot> {
        unsafe {
            let started = Instant::now();
            let mut thumbs = Vec::with_capacity(5);
            thumbs.push(DwmRegisterThumbnail(self.hwnd, window).ok()?);
            let size = DwmQueryThumbnailSourceSize(thumbs[0]).ok().filter(|s| s.cx >= 8 && s.cy >= 8 && s.cx <= 16384 && s.cy <= 16384);
            let pixels = size.and_then(|size| self.expose(window, size.cx, size.cy, &mut thumbs));
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            for thumb in thumbs {
                let _ = DwmUnregisterThumbnail(thumb);
            }
            let (w, h, pixels) = pixels?;

            let mut levels = vec![(w, h, pixels)];
            while levels.len() < MOST_LEVELS {
                let (lw, lh, last) = levels.last().unwrap();
                if (*lw).min(*lh) < 32 {
                    break;
                }
                let next = halve(*lw, *lh, last);
                levels.push(next);
            }
            Some(Shot { levels, took: started.elapsed().as_secs_f32() * 1000.0 })
        }
    }

    /// Lays the window's live picture out on the helper, prints the helper, and returns the
    /// picture with its corners' transparency worked out. `thumbs` holds the picture already
    /// registered and receives the others, for the caller to let go of.
    unsafe fn expose(&self, window: HWND, w: i32, h: i32, thumbs: &mut Vec<isize>) -> Option<(u32, u32, Vec<u8>)> {
        unsafe {
            // The whole picture at the top, over white. Below it a black strip with its four
            // corners once more, side by side.
            let c = CORNER.min(w / 2).min(h / 2);
            let (sheet_w, sheet_h) = (w.max(4 * c), h + c);
            let corners = [(0, 0), (w - c, 0), (0, h - c), (w - c, h - c)];
            let _ = SetWindowPos(self.hwnd, None, 0, 0, sheet_w, sheet_h, SWP_NOZORDER | SWP_NOACTIVATE);
            place(thumbs[0], None, RECT { left: 0, top: 0, right: w, bottom: h });
            for (i, &(x, y)) in corners.iter().enumerate() {
                let thumb = DwmRegisterThumbnail(self.hwnd, window).ok()?;
                thumbs.push(thumb);
                let i = i as i32;
                place(thumb, Some(RECT { left: x, top: y, right: x + c, bottom: y + c }), RECT { left: i * c, top: h, right: (i + 1) * c, bottom: h + c });
            }
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            // Painted now, not when the thread next has nothing to do: white all over (the
            // window's own background), then the strip black.
            let _ = RedrawWindow(Some(self.hwnd), None, None, RDW_INVALIDATE | RDW_ERASE | RDW_UPDATENOW);
            let dc = GetDC(Some(self.hwnd));
            FillRect(dc, &RECT { left: 0, top: h, right: sheet_w, bottom: sheet_h }, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            ReleaseDC(Some(self.hwnd), dc);

            let sheet = print(self.hwnd, sheet_w, sheet_h)?;
            let mut pixels = Vec::with_capacity((w * h * 4) as usize);
            for row in 0..h {
                let at = (row * sheet_w * 4) as usize;
                pixels.extend_from_slice(&sheet[at..at + (w * 4) as usize]);
            }
            // A window that may not be photographed (protected content) leaves the sheet as
            // it was, white, or comes out all black.
            let plain = |value: u8| pixels.chunks_exact(4).step_by(61).all(|px| px[0] == value && px[1] == value && px[2] == value);
            if plain(255) || plain(0) {
                return None;
            }
            for px in pixels.chunks_exact_mut(4) {
                px[3] = 255;
            }
            for (i, &(x, y)) in corners.iter().enumerate() {
                for row in 0..c {
                    for col in 0..c {
                        let black = (((h + row) * sheet_w + i as i32 * c + col) * 4) as usize;
                        let white = (((y + row) * w + x + col) * 4) as usize;
                        // Over white a see-through pixel is lighter than over black by exactly
                        // how see-through it is. Its colour over black is the colour to keep.
                        let gap = (0..3).map(|k| pixels[white + k] as i32 - sheet[black + k] as i32).max().unwrap_or(0);
                        pixels[white..white + 3].copy_from_slice(&sheet[black..black + 3]);
                        pixels[white + 3] = (255 - gap).clamp(0, 255) as u8;
                    }
                }
            }
            Some((w as u32, h as u32, pixels))
        }
    }
}

impl Drop for Studio {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// Shows a live picture (or the part `source` of it) in `dest` of the window it is registered on.
unsafe fn place(thumb: isize, source: Option<RECT>, dest: RECT) {
    let mut flags = DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY;
    if source.is_some() {
        flags |= DWM_TNP_RECTSOURCE;
    }
    let props = DWM_THUMBNAIL_PROPERTIES { dwFlags: flags, rcDestination: dest, rcSource: source.unwrap_or_default(), opacity: 255, fVisible: BOOL(1), fSourceClientAreaOnly: BOOL(0) };
    unsafe {
        let _ = DwmUpdateThumbnailProperties(thumb, &props);
    }
}

/// Has the compositor draw a window of ours into a bitmap. BGRA, top row first.
pub(crate) unsafe fn print(hwnd: HWND, w: i32, h: i32) -> Option<Vec<u8>> {
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative = top row first
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut pixels = None;
        if let Ok(bitmap) = CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            let previous = SelectObject(memory, bitmap.into());
            if PrintWindow(hwnd, memory, FULL_CONTENT).as_bool() && !bits.is_null() {
                pixels = Some(std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec());
            }
            SelectObject(memory, previous);
            let _ = DeleteObject(bitmap.into());
        }
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        pixels
    }
}
