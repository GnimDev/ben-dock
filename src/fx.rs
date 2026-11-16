//! The "window flows into its dock icon" effect (and back out again), played
//! when a window is minimised or brought back with a click on the dock.
//!
//! Two things can fly.
//!
//! The live picture: Windows keeps a live picture of every window, minimised
//! ones included, and will show it anywhere on a window of ours (a
//! "thumbnail"). Told every frame which rectangle to fill, it is the whole
//! effect for the plain shapes (shrink, drop): nothing is copied, and what
//! flies is the window itself, still playing its video.
//!
//! The picture cut into slices: the genie bends the window's sides, which one
//! rectangle cannot do, and showing the live picture a few hundred times over
//! (once per slice) brings the desktop compositor down to a third of the
//! display's frame rate. So the genie takes a photograph of the window (see
//! `camera.rs`: off screen, on another thread), and shows bands of that one
//! picture as ordinary composition visuals, which cost next to nothing. While
//! the photograph is being taken, which is a few hundredths of a second, the
//! live picture stands in for the window, so nothing is seen of the wait.
//!
//! A window minimised by other means (its own button, the keyboard) gets this
//! effect too: Windows' own animation is kept off for the windows that are
//! open (see `keep_windows_still` in app.rs), so the window is gone at once
//! and the effect plays over where it was. Restored by other means (Alt+Tab)
//! it is animated by Windows, which the app only tells where the window's icon
//! is (see `MIN_RECTS` in app.rs). While this effect plays for a window,
//! Windows' own animation is switched off for that one window so the two never
//! play on top of each other.

use crate::camera::Camera;
use crate::scene::{Level, Scene, Slice, Stage};
use crate::shell;
use crate::util::wide;
use crate::warp::{self, Strip, R};
use std::collections::VecDeque;
use std::time::Instant;
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

const CLASS_NAME: &str = "BenEffectWindow";
/// Most slices the genie is cut into. More gives smoother sides.
const MAX_SLICES: usize = 320;
/// How long the genie waits for the window's photograph before going on without it. Seconds.
const WAIT_FOR_PICTURE: f32 = 0.25;
/// Frames the live picture stays up over the slices that take its place, so that no frame
/// goes by with neither on screen.
const HANDOVER_FRAMES: u32 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    /// The whole picture scales down into the icon.
    Shrink,
    /// The picture is drawn into the icon through a funnel with curved sides.
    Genie,
    /// The picture shrinks a little and slides down to the icon.
    Drop,
}

impl Style {
    /// Where the live picture, flying as one piece, is at progress `p`: 0 = the window,
    /// 1 = inside the icon. (The genie without its photograph keeps its outline.)
    fn at(self, from: R, to: R, p: f32) -> R {
        match self {
            Style::Shrink => warp::shrink(from, to, p),
            Style::Genie => warp::pour(from, to, p),
            Style::Drop => warp::drop_down(from, to, p),
        }
    }
}

/// What just finished playing.
pub struct Finished {
    pub window: HWND,
    /// True when the window was flying out of the dock (it now has to be restored for real).
    pub restoring: bool,
}

struct Playing {
    window: HWND,
    /// The live picture of the window, while it is in use.
    thumb: Option<isize>,
    /// Where the picture is at its true size, and the icon it flies to.
    from: R,
    to: R,
    style: Style,
    restoring: bool,
    elapsed: f32,
    duration: f32,
    /// Set once the end has been reported, so it is reported only once.
    reported: bool,

    // The genie's photograph.
    /// It has been asked for and is not there yet: the request, and since when.
    waiting: Option<(u64, Instant)>,
    /// Its sizes still to be handed to the compositor, largest first (one a frame).
    pending: VecDeque<(u32, u32, Vec<u8>)>,
    levels: Vec<Level>,
    slices: Vec<Slice>,
    strips: Vec<Strip>,
    /// Frames left before the live picture is taken off the slices.
    handover: u32,

    // For the log.
    frames: u32,
    /// When the last frame of the flight was shown, and the longest time between two.
    last_frame: Option<Instant>,
    longest_gap: f32,
    /// The longest a frame's work on the effect took, in seconds.
    worst: f32,
    most_slices: usize,
    /// The photograph: its size, how long the effect stood still for it, how long the camera
    /// took, and how long handing its full size to the compositor took (milliseconds).
    picture: (u32, u32),
    held: f32,
    camera: f32,
    upload: f32,
}

pub struct Fx {
    hwnd: HWND,
    stage: Stage,
    /// Top-left of the effect window on screen.
    origin: (i32, i32),
    playing: Option<Playing>,
    /// Takes the genie's photographs. `None` if that is not possible on this system.
    camera: Option<Camera>,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn to_r(r: RECT) -> R {
    R { l: r.left as f32, t: r.top as f32, r: r.right as f32, b: r.bottom as f32 }
}

impl Fx {
    pub fn new(scene: &Scene) -> windows::core::Result<Self> {
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
            let stage = scene.stage(hwnd)?;
            // The slices are stretched and squeezed: smooth their edges.
            stage.soften()?;
            let camera = Camera::start();
            if camera.is_none() {
                crate::log!("no camera for window pictures; the genie will fly the live picture as one piece");
            }
            Ok(Fx { hwnd, stage, origin: (0, 0), playing: None, camera })
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
    /// `bounds` the monitor it plays on. Returns false if nothing can be shown.
    pub fn start(&mut self, window: HWND, whole: RECT, frame: RECT, to: R, bounds: RECT, style: Style, restoring: bool, duration: f32) -> bool {
        self.stop();
        let (whole, drawn) = (to_r(whole), to_r(frame));
        let (thumb, from) = unsafe {
            let thumb = match DwmRegisterThumbnail(self.hwnd, window) {
                Ok(t) => t,
                Err(e) => {
                    crate::log!("no live picture available for the window: {e}");
                    return false;
                }
            };
            // Windows makes the live picture either as large as the whole window or only as
            // large as its drawn part (the invisible edges left out). Shown over the wrong one
            // it would be stretched by a few pixels and jump as the effect starts.
            let from = match DwmQueryThumbnailSourceSize(thumb) {
                Ok(size) if size.cx > 0 && size.cy > 0 => {
                    let (w, h) = (size.cx as f32, size.cy as f32);
                    let fits = |r: R| (r.w() - w).abs() <= 1.0 && (r.h() - h).abs() <= 1.0;
                    if fits(drawn) || !fits(whole) {
                        drawn
                    } else {
                        whole
                    }
                }
                _ => {
                    crate::log!("the window has no live picture to show");
                    let _ = DwmUnregisterThumbnail(thumb);
                    return false;
                }
            };
            (thumb, from)
        };
        // Cover the path of the picture on this monitor, and nothing else.
        let mut path = from.union(to);
        for step in 0..=16 {
            path = path.union(style.at(from, to, step as f32 / 16.0));
        }
        let left = (path.l.floor() as i32).max(bounds.left);
        let top = (path.t.floor() as i32).max(bounds.top);
        let right = (path.r.ceil() as i32).min(bounds.right);
        let bottom = (path.b.ceil() as i32).min(bounds.bottom);
        let placed = from.w() >= 8.0 && from.h() >= 8.0 && right - left >= 2 && bottom - top >= 2 && unsafe { SetWindowPos(self.hwnd, Some(HWND_TOPMOST), left, top, right - left, bottom - top, SWP_NOACTIVATE).is_ok() };
        if !placed {
            unsafe {
                let _ = DwmUnregisterThumbnail(thumb);
            }
            return false;
        }
        self.origin = (left, top);
        // The genie needs a photograph to cut up; until it is there, nothing moves.
        let waiting = match (style, self.camera.as_mut()) {
            (Style::Genie, Some(camera)) => camera.ask(window).map(|id| (id, Instant::now())),
            _ => None,
        };
        self.playing = Some(Playing {
            window,
            thumb: Some(thumb),
            from,
            to,
            style,
            restoring,
            elapsed: 0.0,
            duration: duration.max(0.05),
            reported: false,
            waiting,
            pending: VecDeque::new(),
            levels: Vec::new(),
            slices: Vec::new(),
            strips: Vec::new(),
            handover: 0,
            frames: 0,
            last_frame: None,
            longest_gap: 0.0,
            worst: 0.0,
            most_slices: 0,
            picture: (0, 0),
            held: 0.0,
            camera: 0.0,
            upload: 0.0,
        });
        let _ = self.stage.set_opacity(1.0);
        // This window is ours to animate until the effect is over.
        shell::set_transitions(window, false);
        // Place the picture before the effect window becomes visible, so the first frame is already right.
        self.place_thumb();
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        true
    }

    /// Advances the effect by one frame. Returns what finished, once.
    pub fn step(&mut self, scene: &Scene, dt: f32) -> Option<Finished> {
        let playing = self.playing.as_mut()?;
        if playing.reported {
            return None;
        }

        // The genie waits for its photograph. Minimising, the live picture stands where the
        // window was meanwhile; coming back, there is nothing to see yet.
        if let Some((id, since)) = playing.waiting {
            let answer = self.camera.as_mut().and_then(|camera| camera.take(id));
            let waited = since.elapsed().as_secs_f32();
            match answer {
                None if waited < WAIT_FOR_PICTURE => return None,
                None | Some(None) => {
                    crate::log!("no photograph of the window after {:.0} ms; its live picture flies as one piece", waited * 1000.0);
                    playing.waiting = None;
                }
                Some(Some(shot)) => {
                    playing.waiting = None;
                    (playing.held, playing.camera) = (waited * 1000.0, shot.took);
                    let started = Instant::now();
                    let mut sizes: VecDeque<(u32, u32, Vec<u8>)> = shot.levels.into();
                    // The full size now; the smaller ones follow, one a frame.
                    let first = sizes.pop_front().and_then(|(w, h, pixels)| {
                        playing.picture = (w, h);
                        scene.picture_level(w, h, &pixels).ok()
                    });
                    let slice = first.and_then(|level| {
                        playing.levels.push(level);
                        scene.slice(&self.stage, &playing.levels).ok()
                    });
                    match slice {
                        Some(slice) => {
                            playing.slices.push(slice);
                            playing.pending = sizes;
                            playing.upload = started.elapsed().as_secs_f32() * 1000.0;
                            if playing.restoring {
                                // Nothing was showing; the photograph comes out of the icon.
                                if let Some(thumb) = playing.thumb.take() {
                                    unsafe {
                                        let _ = DwmUnregisterThumbnail(thumb);
                                    }
                                }
                            } else {
                                playing.handover = HANDOVER_FRAMES;
                            }
                        }
                        None => {
                            crate::log!("the window's photograph could not be shown; its live picture flies as one piece");
                            playing.levels.clear();
                        }
                    }
                }
            }
            // The clock starts with the next frame: this one shows the effect's first picture.
            self.apply(scene);
            return None;
        }

        // The live picture stays over the slices for a moment; the clock waits until it is gone.
        if playing.handover > 0 {
            playing.handover -= 1;
            if playing.handover == 0 {
                if let Some(thumb) = playing.thumb.take() {
                    unsafe {
                        let _ = DwmUnregisterThumbnail(thumb);
                    }
                }
            }
            self.apply(scene);
            return None;
        }

        // One more size of the photograph a frame. The small ones are only needed once the
        // picture has shrunk, so they need not hold up its start.
        if let Some((w, h, pixels)) = playing.pending.pop_front() {
            match scene.picture_level(w, h, &pixels) {
                Ok(level) => playing.levels.push(level),
                Err(_) => playing.pending.clear(),
            }
        }

        playing.frames += 1;
        let now = Instant::now();
        if let Some(last) = playing.last_frame.replace(now) {
            playing.longest_gap = playing.longest_gap.max((now - last).as_secs_f32());
        }
        playing.elapsed += dt;
        if playing.elapsed >= playing.duration {
            playing.reported = true;
            playing.elapsed = playing.duration;
            let done = Finished { window: playing.window, restoring: playing.restoring };
            if done.restoring {
                // Leave the picture up at full size: the app restores the real window underneath
                // and calls `stop` a moment later, so there is no flash of empty desktop.
                self.apply(scene);
            } else {
                self.stop();
            }
            return Some(done);
        }
        self.apply(scene);
        None
    }

    /// Puts the live picture where it belongs at the current time.
    fn place_thumb(&mut self) {
        let (ox, oy) = (self.origin.0 as f32, self.origin.1 as f32);
        let Some(playing) = self.playing.as_mut() else { return };
        let Some(thumb) = playing.thumb else { return };
        let t = (playing.elapsed / playing.duration).clamp(0.0, 1.0);
        // 0 = the window where it is, 1 = inside the icon.
        let p = if playing.restoring { 1.0 - t } else { t };
        let (at, shown, opacity) = if playing.waiting.is_some() {
            // Standing in for the window until its photograph is there. Coming back there is
            // nothing to stand in for.
            (playing.from, !playing.restoring, 1.0)
        } else if !playing.levels.is_empty() {
            // Over the slices that are taking its place.
            (playing.from, true, 1.0)
        } else {
            (playing.style.at(playing.from, playing.to, p), true, warp::opacity(p))
        };
        let dest = RECT { left: (at.l - ox).round() as i32, top: (at.t - oy).round() as i32, right: (at.r - ox).round() as i32, bottom: (at.b - oy).round() as i32 };
        let opacity = (opacity * 255.0).round() as u8;
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_OPACITY | DWM_TNP_VISIBLE | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: dest,
            rcSource: RECT::default(),
            opacity,
            fVisible: BOOL((shown && dest.right > dest.left && dest.bottom > dest.top && opacity > 0) as i32),
            fSourceClientAreaOnly: BOOL(0),
        };
        unsafe {
            let _ = DwmUpdateThumbnailProperties(thumb, &props);
        }
    }

    /// Puts everything where it belongs at the current time.
    fn apply(&mut self, scene: &Scene) {
        let started = Instant::now();
        self.place_thumb();
        let (ox, oy) = (self.origin.0 as f32, self.origin.1 as f32);
        let Some(playing) = self.playing.as_mut() else { return };
        if !playing.levels.is_empty() {
            let t = (playing.elapsed / playing.duration).clamp(0.0, 1.0);
            let p = if playing.restoring { 1.0 - t } else { t };
            warp::funnel(playing.from, playing.to, p, MAX_SLICES, &mut playing.strips);
            // One visual per slice, created as the funnel needs them.
            while playing.slices.len() < playing.strips.len() {
                match scene.slice(&self.stage, &playing.levels) {
                    Ok(slice) => playing.slices.push(slice),
                    Err(e) => {
                        crate::log!("could not create a picture slice: {e}");
                        break;
                    }
                }
            }
            for (index, slice) in playing.slices.iter_mut().enumerate() {
                let shown = match playing.strips.get(index) {
                    Some(strip) if strip.dst.w() > 0.01 && strip.dst.h() > 0.01 && strip.foot.1 - strip.foot.0 > 0.01 => {
                        slice.show(&playing.levels, strip.dst.l - ox, strip.dst.t - oy, strip.dst.w(), strip.dst.h(), strip.foot.0 - ox, strip.foot.1 - strip.foot.0, strip.src_top, strip.src_bottom)
                    }
                    _ => slice.hide(),
                };
                if let Err(e) = shown {
                    crate::log!("could not move a picture slice: {e}");
                    break;
                }
            }
            let _ = self.stage.set_opacity(warp::opacity(p));
            playing.most_slices = playing.most_slices.max(playing.strips.len());
        }
        playing.worst = playing.worst.max(started.elapsed().as_secs_f32());
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
        playing.levels.clear();
        // From here on the window is Windows' to animate again.
        shell::set_transitions(playing.window, true);
        let what = if playing.restoring { "restore" } else { "minimise" };
        if playing.picture.0 > 0 {
            crate::log!(
                "effect ({what}, {:?}): {} frames in {:.0} ms (longest wait for one {:.1} ms), {} slices at most, the app's work on a frame {:.1} ms at most; photograph {}x{} there after {:.0} ms (camera {:.0} ms), handed over in {:.1} ms",
                playing.style,
                playing.frames,
                playing.elapsed * 1000.0,
                playing.longest_gap * 1000.0,
                playing.most_slices,
                playing.worst * 1000.0,
                playing.picture.0,
                playing.picture.1,
                playing.held,
                playing.camera,
                playing.upload
            );
        } else {
            crate::log!("effect ({what}, {:?}): {} frames in {:.0} ms (longest wait for one {:.1} ms)", playing.style, playing.frames, playing.elapsed * 1000.0, playing.longest_gap * 1000.0);
        }
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
