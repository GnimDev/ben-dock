//! The "window flies into its dock icon" effect (and back out again).
//!
//! How it works, in three steps:
//!
//! 1. Windows keeps a live picture of every window, including minimised ones,
//!    and can show it anywhere as a "thumbnail". The effect window puts that
//!    picture exactly where the real window is (or just was), so nothing
//!    appears to change on screen.
//! 2. Two frames later that part of the screen is copied once into a surface
//!    owned by the dock.
//! 3. From then on the effect is ordinary composition visuals: the picture is
//!    cut into up to a few hundred horizontal slices, each a row or more
//!    high, and the slices are squeezed along a funnel (see `warp.rs`). This
//!    is the same machinery that moves the dock's icons, and just as cheap.
//!
//! The picture taken when a window is minimised is kept, and replayed
//! backwards when the window is brought back from the dock.

use crate::glass::{Glass, Level, Slice, Stage};
use crate::shell;
use crate::util::wide;
use crate::warp::{self, Strip, R};
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "BenEffectWindow";
/// Most slices the funnel is cut into. More gives smoother edges.
const MAX_SLICES: usize = 320;
/// How many minimised windows' pictures are kept for the way back.
const KEPT_PICTURES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Shrink,
    Funnel,
    Drop,
}

/// What just finished playing.
pub struct Finished {
    pub window: HWND,
    /// True when the window was flying out of the dock (it now has to be restored for real).
    pub restoring: bool,
}

/// A window's picture and the part of the screen it was taken from.
struct Picture {
    /// The picture at full size, then pre-shrunk copies of it.
    levels: Vec<Level>,
    area: R,
}

enum Phase {
    /// The live picture is up; waiting for it to reach the screen so it can be copied.
    Settling(u32),
    Playing,
    /// No copy could be made: the live picture itself is scaled down instead.
    LiveOnly,
}

struct Playing {
    window: HWND,
    phase: Phase,
    /// The live picture, while it is shown.
    thumb: Option<isize>,
    /// Frames to keep the live picture up after the slices have appeared underneath it.
    thumb_frames: u32,
    picture: Option<Picture>,
    slices: Vec<Slice>,
    /// Where the live picture is shown at its true size, and the drawn part of the window
    /// that is on this monitor.
    full: R,
    from: R,
    to: R,
    style: Style,
    restoring: bool,
    elapsed: f32,
    duration: f32,
    /// Set once the end has been reported, so it is reported only once.
    reported: bool,
    strips: Vec<Strip>,
}

pub struct Fx {
    hwnd: HWND,
    stage: Stage,
    /// Top-left of the effect window on screen.
    origin: (i32, i32),
    playing: Option<Playing>,
    /// Pictures of minimised windows, newest last.
    kept: Vec<(isize, Picture)>,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn to_rect(r: R) -> RECT {
    RECT { left: r.l.round() as i32, top: r.t.round() as i32, right: r.r.round() as i32, bottom: r.b.round() as i32 }
}

impl Fx {
    pub fn new(glass: &Glass) -> windows::core::Result<Self> {
        unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None)?.into();
            let class_name = wide(CLASS_NAME);
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&class);
            // No surface of its own (so it is fully transparent), never takes focus, not in Alt+Tab.
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
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
            let stage = glass.stage(hwnd)?;
            Ok(Fx { hwnd, stage, origin: (0, 0), playing: None, kept: Vec::new() })
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// The window an effect is currently playing for.
    pub fn window(&self) -> Option<HWND> {
        self.playing.as_ref().map(|p| p.window)
    }

    /// Starts flying `window` between its place on screen and its icon (`to`), all in screen
    /// pixels. `whole` is the window's own rectangle, `frame` the part of it that is drawn, and
    /// `bounds` the monitor. Returns false if nothing can be shown.
    pub fn start(&mut self, glass: &Glass, window: HWND, whole: RECT, frame: RECT, to: R, bounds: RECT, style: Style, restoring: bool, duration: f32) -> bool {
        self.stop();
        let full = R { l: whole.left as f32, t: whole.top as f32, r: whole.right as f32, b: whole.bottom as f32 };
        // The drawn part of the window that is on this monitor: that is what gets pictured.
        let visible = R {
            l: (frame.left.max(bounds.left)) as f32,
            t: (frame.top.max(bounds.top)) as f32,
            r: (frame.right.min(bounds.right)) as f32,
            b: (frame.bottom.min(bounds.bottom)) as f32,
        };
        if visible.w() < 8.0 || visible.h() < 8.0 {
            return false;
        }
        // The whole drawn part, also where it runs off the monitor.
        let drawn = R { l: frame.left as f32, t: frame.top as f32, r: frame.right as f32, b: frame.bottom as f32 };
        let id = window.0 as isize;
        let kept = self.kept.iter().position(|(w, _)| *w == id).map(|i| self.kept.remove(i).1);

        // Cover the path of the picture, and nothing else, so the rest of the screen stays clickable.
        let left = (visible.l.min(to.l).floor() as i32).max(bounds.left);
        let top = (visible.t.min(to.t).floor() as i32).max(bounds.top);
        let right = (visible.r.max(to.r).ceil() as i32).min(bounds.right);
        let bottom = (visible.b.max(to.b).ceil() as i32).min(bounds.bottom);
        if right - left < 2 || bottom - top < 2 {
            return false;
        }
        unsafe {
            if SetWindowPos(self.hwnd, Some(HWND_TOPMOST), left, top, right - left, bottom - top, SWP_NOACTIVATE).is_err() {
                return false;
            }
        }
        self.origin = (left, top);
        let _ = self.stage.set_opacity(1.0);

        let mut playing = Playing {
            window,
            phase: Phase::Playing,
            thumb: None,
            thumb_frames: 0,
            picture: None,
            slices: Vec::new(),
            full,
            from: visible,
            to,
            style,
            restoring,
            elapsed: 0.0,
            duration: duration.max(0.05),
            reported: false,
            strips: Vec::new(),
        };

        match (restoring, kept) {
            // Coming back: replay the picture taken when it went in, if it is of the same place.
            (true, Some(picture)) if (picture.area.w() - visible.w()).abs() < 2.0 && (picture.area.h() - visible.h()).abs() < 2.0 => {
                playing.picture = Some(Picture { area: visible, ..picture });
            }
            // Coming back without a picture: scale the live picture up instead.
            (true, _) => {
                let Some((thumb, at)) = self.live_picture(window, full, drawn, false) else { return false };
                playing.thumb = Some(thumb);
                playing.full = at;
                playing.phase = Phase::LiveOnly;
                playing.style = if style == Style::Funnel { Style::Shrink } else { style };
            }
            // Going in: show the live picture where the window is, to be copied in two frames.
            (false, _) => {
                let Some((thumb, at)) = self.live_picture(window, full, drawn, true) else { return false };
                playing.thumb = Some(thumb);
                playing.full = at;
                playing.phase = Phase::Settling(0);
            }
        }
        self.playing = Some(playing);
        // Place everything before the window becomes visible, so the first frame is already right.
        self.apply(glass);
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        true
    }

    /// Registers the live picture of `window` on the effect window and works out which
    /// rectangle it has to be shown over to be exactly as large as the window is on screen.
    /// Windows makes the live picture either as large as the whole window or only as large as
    /// its drawn part (the invisible edges left out). Shown over the wrong one it is stretched
    /// by a few pixels, and the copy made from it does not line up with the real window.
    fn live_picture(&self, window: HWND, whole: R, drawn: R, show: bool) -> Option<(isize, R)> {
        unsafe {
            let thumb = match DwmRegisterThumbnail(self.hwnd, window) {
                Ok(t) => t,
                Err(e) => {
                    crate::log!("no live picture available for the window: {e}");
                    return None;
                }
            };
            let at = match DwmQueryThumbnailSourceSize(thumb) {
                Ok(size) => {
                    let (w, h) = (size.cx as f32, size.cy as f32);
                    let fits = |r: R| (r.w() - w).abs() <= 1.0 && (r.h() - h).abs() <= 1.0;
                    let (at, over) = if fits(drawn) {
                        (drawn, "the drawn part")
                    } else if fits(whole) {
                        (whole, "the whole window")
                    } else {
                        (drawn, "the drawn part, though it matches neither")
                    };
                    crate::log!("live picture is {w}x{h}; whole window {}x{}, drawn part {}x{}: shown over {over}", whole.w(), whole.h(), drawn.w(), drawn.h());
                    at
                }
                Err(e) => {
                    crate::log!("size of the live picture unknown ({e}); shown over the whole window");
                    whole
                }
            };
            if show {
                self.place_live(thumb, at, 255);
            }
            Some((thumb, at))
        }
    }

    fn place_live(&self, thumb: isize, at: R, alpha: u8) {
        let dest = to_rect(R { l: at.l - self.origin.0 as f32, t: at.t - self.origin.1 as f32, r: at.r - self.origin.0 as f32, b: at.b - self.origin.1 as f32 });
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: dest,
            rcSource: RECT::default(),
            opacity: alpha,
            fVisible: BOOL((dest.right > dest.left && dest.bottom > dest.top && alpha > 0) as i32),
            fSourceClientAreaOnly: BOOL(0),
        };
        unsafe {
            let _ = DwmUpdateThumbnailProperties(thumb, &props);
        }
    }

    /// Advances the effect by one frame. Returns what finished, once.
    pub fn step(&mut self, glass: &Glass, dt: f32) -> Option<Finished> {
        let playing = self.playing.as_mut()?;
        if playing.reported {
            return None;
        }
        match playing.phase {
            Phase::Settling(frames) if frames < 2 => {
                // Give the live picture two frames to be on screen for certain.
                playing.phase = Phase::Settling(frames + 1);
                return None;
            }
            Phase::Settling(_) => {
                let area = playing.from;
                let started = std::time::Instant::now();
                let copied = shell::capture_screen(to_rect(area)).and_then(|(w, h, pixels)| match glass.picture_levels(w, h, &pixels) {
                    Ok(levels) => {
                        crate::log!("window picture {w}x{h} copied in {:.0} ms ({} sizes kept)", started.elapsed().as_secs_f32() * 1000.0, levels.len());
                        Some(Picture { levels, area })
                    }
                    Err(e) => {
                        crate::log!("could not store the window's picture: {e}");
                        None
                    }
                });
                // The first slice is made right here: if the picture cannot be shown at all,
                // the live picture is still up and can be scaled instead.
                let ready = copied.and_then(|picture| match glass.slice(&self.stage, &picture.levels) {
                    Ok(slice) => Some((picture, slice)),
                    Err(e) => {
                        crate::log!("could not create a picture slice: {e}");
                        None
                    }
                });
                match ready {
                    Some((picture, slice)) => {
                        playing.slices.clear();
                        playing.slices.push(slice);
                        playing.picture = Some(picture);
                        playing.phase = Phase::Playing;
                        // The slices appear under the live picture first; then it is taken away.
                        playing.thumb_frames = 2;
                    }
                    None => {
                        crate::log!("scaling the live picture instead");
                        playing.phase = Phase::LiveOnly;
                        playing.style = if playing.style == Style::Funnel { Style::Shrink } else { playing.style };
                    }
                }
                self.apply(glass);
                return None;
            }
            _ => {}
        }

        if playing.thumb_frames > 0 {
            playing.thumb_frames -= 1;
            if playing.thumb_frames == 0 {
                if let Some(thumb) = playing.thumb.take() {
                    unsafe {
                        let _ = DwmUnregisterThumbnail(thumb);
                    }
                }
            }
        }

        playing.elapsed += dt;
        if playing.elapsed >= playing.duration {
            playing.reported = true;
            playing.elapsed = playing.duration;
            let done = Finished { window: playing.window, restoring: playing.restoring };
            if done.restoring {
                // Leave the picture up at full size: the dock restores the real window underneath
                // and calls `stop` a moment later, so there is no flash of empty desktop.
                self.apply(glass);
            } else {
                self.stop();
            }
            return Some(done);
        }
        self.apply(glass);
        None
    }

    /// Puts every slice where it belongs at the current time.
    fn apply(&mut self, glass: &Glass) {
        let (ox, oy) = (self.origin.0 as f32, self.origin.1 as f32);
        let Some(playing) = self.playing.as_mut() else { return };
        let t = (playing.elapsed / playing.duration).clamp(0.0, 1.0);
        // 0 = the window where it is, 1 = inside the icon.
        let p = if playing.restoring { 1.0 - t } else { t };
        let alpha = warp::opacity(p);

        if let Phase::LiveOnly = playing.phase {
            // Fallback: one live picture, scaled as a whole.
            let at = match playing.style {
                Style::Drop => warp::drop_down(playing.full, playing.to, p),
                _ => warp::shrink(playing.full, playing.to, p),
            };
            if let Some(thumb) = playing.thumb {
                let props = DWM_THUMBNAIL_PROPERTIES {
                    dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY,
                    rcDestination: to_rect(R { l: at.l - ox, t: at.t - oy, r: at.r - ox, b: at.b - oy }),
                    rcSource: RECT::default(),
                    opacity: (alpha * 255.0).round() as u8,
                    fVisible: BOOL(1),
                    fSourceClientAreaOnly: BOOL(0),
                };
                unsafe {
                    let _ = DwmUpdateThumbnailProperties(thumb, &props);
                }
            }
            return;
        }
        let Some(picture) = playing.picture.as_ref() else { return };

        match playing.style {
            Style::Shrink => {
                playing.strips.clear();
                playing.strips.push(Strip { src_top: 0.0, src_bottom: 1.0, dst: warp::shrink(playing.from, playing.to, p) });
            }
            Style::Drop => {
                playing.strips.clear();
                playing.strips.push(Strip { src_top: 0.0, src_bottom: 1.0, dst: warp::drop_down(playing.from, playing.to, p) });
            }
            Style::Funnel => warp::funnel(playing.from, playing.to, p, MAX_SLICES, &mut playing.strips),
        }

        // One visual per slice, created as the funnel needs them.
        while playing.slices.len() < playing.strips.len() {
            match glass.slice(&self.stage, &picture.levels) {
                Ok(slice) => playing.slices.push(slice),
                Err(e) => {
                    crate::log!("could not create a picture slice: {e}");
                    break;
                }
            }
        }
        for (index, slice) in playing.slices.iter_mut().enumerate() {
            let shown = match playing.strips.get(index) {
                Some(strip) if strip.dst.w() > 0.01 && strip.dst.h() > 0.01 => {
                    slice.show(&picture.levels, strip.dst.l - ox, strip.dst.t - oy, strip.dst.w(), strip.dst.h(), strip.src_top, strip.src_bottom)
                }
                _ => slice.hide(),
            };
            if let Err(e) = shown {
                crate::log!("could not move a picture slice: {e}");
                break;
            }
        }
        let _ = self.stage.set_opacity(alpha);
    }

    /// Ends the effect and hides the effect window.
    pub fn stop(&mut self) {
        let Some(mut playing) = self.playing.take() else { return };
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            if let Some(thumb) = playing.thumb.take() {
                let _ = DwmUnregisterThumbnail(thumb);
            }
        }
        playing.slices.clear();
        // Keep the picture of a window that went into the dock, for when it comes back.
        if !playing.restoring {
            if let Some(picture) = playing.picture.take() {
                self.kept.retain(|(w, _)| unsafe { IsWindow(Some(HWND(*w as *mut _))).as_bool() });
                self.kept.push((playing.window.0 as isize, picture));
                if self.kept.len() > KEPT_PICTURES {
                    self.kept.remove(0);
                }
            }
        }
    }

    /// Forgets every kept picture (they were painted on a GPU device that has been reset).
    pub fn forget_pictures(&mut self) {
        self.stop();
        self.kept.clear();
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        self.stop();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
