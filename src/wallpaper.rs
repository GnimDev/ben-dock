//! The live wallpaper: water, seen from above, in place of the desktop's picture.
//!
//! From the back: a colour that deepens towards the bottom; light playing on
//! it, which is one small picture laid side by side with itself twice over,
//! at two sizes, each sliding its own way (the compositor moves them, the app
//! does nothing for it); and on top the rings (see `water.rs`), which are
//! painted only while the water moves.
//!
//! The water moves for three reasons: the cursor brushes it wherever the
//! desktop itself is under it, a click drops something in, and sound stirs
//! it: a beat is a drop somewhere. How loud the sound is comes from the
//! needle of Windows' own level meter (see `audio::Meter`); nothing is
//! recorded.
//!
//! Or a video, in place of the water's colour and light: one file, played
//! without sound and without end, filling every screen. It plays while the
//! desktop itself is in view and stands still otherwise. The rings are laid
//! over it as over the water, so the cursor and the music stir it too.
//!
//! The window is a child of the desktop's, between its icons and its picture
//! (see `desktop.rs`). A window inside another program's shares that
//! program's turn at the mouse and keyboard, so it has a thread of its own
//! that does nothing else: the desktop never waits for the dock, nor the dock
//! for the desktop.

use crate::audio::Meter;
use crate::config::Config;
use crate::desktop::{self, Behind};
use crate::reel;
use crate::scene::{rgba, Bead, Drift, Scene, Sprite, Stage};
use crate::system;
use crate::util::{data_dir, wide};
use crate::water::{self, Water};
use std::path::PathBuf;
use std::sync::mpsc::channel;
use std::time::Instant;
use windows::core::{Result, PCWSTR};
use windows::Media::Playback::{MediaPlaybackState, MediaPlayer, MediaPlayerSurface};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Color;
use windows::UI::Composition::{CompositionDrawingSurface, VisualCollection};

const CLASS_NAME: &str = "BenWallpaperWindow";
/// The value of the setting that asks for a video in place of the water.
pub const VIDEO: u32 = 3;
/// Side of one cell of the water in screen pixels on a screen 1080 pixels high, and the
/// most cells the sheet may have (a very large desktop gets larger cells).
const CELL: f32 = 5.0;
const MOST_CELLS: f32 = 170_000.0;
/// Side of the picture of light, in its own pixels.
const TILE: usize = 512;

/// What the water is like: its colour at the top and at the bottom, the colour of the light
/// that plays on it, and how strong each of the two layers of that light is.
struct Look {
    top: (f32, f32, f32),
    bottom: (f32, f32, f32),
    light: (f32, f32, f32),
    layers: [f32; 2],
}

/// A pool at night, and the sea on a clear day.
const DARK: Look = Look { top: (0.020, 0.043, 0.066), bottom: (0.008, 0.020, 0.034), light: (0.50, 0.78, 0.86), layers: [0.16, 0.11] };
const BLUE: Look = Look { top: (0.035, 0.300, 0.540), bottom: (0.016, 0.140, 0.330), light: (0.72, 0.93, 1.00), layers: [0.30, 0.20] };

/// The file that says which video to play: its first line is the video's full path.
pub fn video_file() -> PathBuf {
    data_dir().join("video.txt")
}

/// The video that was chosen, if one was.
pub fn video_path() -> Option<String> {
    named(&std::fs::read_to_string(video_file()).ok()?)
}

/// Keeps the path of the video that was chosen.
pub fn choose_video(path: &str) {
    if let Err(e) = std::fs::write(video_file(), format!("{path}\n")) {
        crate::log!("could not save video.txt: {e}");
    }
}

/// The path that the text of video.txt names: its first line that says anything, without
/// the quotation marks that "Copy as path" puts round one.
fn named(text: &str) -> Option<String> {
    text.lines().map(|line| line.trim().trim_start_matches('\u{feff}').trim_matches('"').trim()).find(|line| !line.is_empty() && !line.starts_with('#')).map(str::to_string)
}

/// The size a video's pictures are drawn at: the video's own shape, and no larger than it
/// takes to fill a screen of `screen` pixels (a video four times the size of the screen is
/// not drawn in full only to be made small again), nor larger than the video is.
fn drawn_size(video: (u32, u32), screen: (f32, f32)) -> (f32, f32) {
    let (w, h) = (video.0.max(1) as f32, video.1.max(1) as f32);
    let fill = (screen.0 / w).max(screen.1 / h).min(1.0);
    ((w * fill).round().max(1.0), (h * fill).round().max(1.0))
}

/// A video in place of the water's colour and light.
struct Film {
    player: MediaPlayer,
    /// What the player shows, and a sprite of it on every screen: made once the video has
    /// said how large it is.
    surface: Option<MediaPlayerSurface>,
    sprites: Vec<Sprite>,
    /// Where the sprites go, and where each screen is there (left, top, width, height).
    content: VisualCollection,
    screens: Vec<(f32, f32, f32, f32)>,
    /// The file it plays.
    path: String,
    /// The player has been told to play (false: to stand still).
    playing: bool,
    /// The video's first pictures are there, and the sprites show them.
    shown: bool,
    /// When it was asked for, and whether it has been given up on (said once).
    began: Instant,
    given_up: bool,
}

impl Film {
    /// A player for the video at `path`, without sound and without end. `content`: where
    /// its pictures will be shown, under everything else there; `screens`: where each
    /// screen is in the window (left, top, width, height).
    fn new(content: &VisualCollection, path: String, screens: &[(f32, f32, f32, f32)]) -> Result<Self> {
        let player = MediaPlayer::new()?;
        // It is a wallpaper: nothing for the media keys, nor for whoever shows what is
        // playing (the island, for one), to take for music.
        player.CommandManager()?.SetIsEnabled(false)?;
        player.SetIsMuted(true)?;
        player.SetIsLoopingEnabled(true)?;
        player.SetSource(&reel::source(&path)?)?;
        Ok(Film { player, surface: None, sprites: Vec::new(), content: content.clone(), screens: screens.to_vec(), path, playing: false, shown: false, began: Instant::now(), given_up: false })
    }

    /// Has the video play (`wanted`) or stand still, and brings it into view once its
    /// first pictures are there. Until then the desktop keeps its own picture.
    fn tend(&mut self, scene: &Scene, wanted: bool) -> Result<()> {
        // (Until its first pictures are there it plays whatever lies over the desktop, so
        // that what little of the desktop shows, shows the video and not the old picture.)
        let wanted = wanted || !self.shown;
        if wanted != self.playing {
            self.playing = wanted;
            if wanted {
                self.player.Play()?;
            } else {
                self.player.Pause()?;
            }
        }
        if self.shown {
            return Ok(());
        }
        let session = self.player.PlaybackSession()?;
        let video = (session.NaturalVideoWidth()?, session.NaturalVideoHeight()?);
        if video.0 == 0 || video.1 == 0 {
            if !self.given_up && self.began.elapsed().as_secs() >= 15 {
                self.given_up = true;
                crate::log!("wallpaper: the video has not begun to play (a file that is gone, or of a kind Windows cannot play?); the desktop keeps its own picture");
            }
            return Ok(());
        }
        if self.surface.is_none() {
            // Now that it has said how large it is: its pictures are drawn in its own
            // shape, and a sprite on every screen cuts off what does not fit that screen.
            let largest = self.screens.iter().map(|s| (s.2, s.3)).fold((1.0f32, 1.0f32), |most, s| if s.0 * s.1 > most.0 * most.1 { s } else { most });
            let (w, h) = drawn_size(video, largest);
            let surface = scene.film(&self.player, w, h)?;
            for (x, y, wide, high) in &self.screens {
                let mut sprite = scene.film_in(&self.content, &surface)?;
                sprite.place(*x, *y, *wide, *high)?;
                sprite.set_opacity(0.0)?;
                self.sprites.push(sprite);
            }
            self.surface = Some(surface);
            crate::log!("wallpaper: the video is {}x{} px, drawn at {w}x{h} on {} screen(s)", video.0, video.1, self.screens.len());
        }
        // Into view once it is under way (its first pictures have been drawn by then).
        if session.PlaybackState()? == MediaPlaybackState::Playing && session.Position()?.Duration >= 1_500_000 {
            for sprite in &mut self.sprites {
                sprite.set_opacity(1.0)?;
            }
            self.shown = true;
            crate::log!("wallpaper: the video is under way and in view");
        }
        Ok(())
    }
}

/// What the water is told every pass.
pub struct Feel {
    /// False while no part of the desktop can be seen: nothing moves then.
    pub showing: bool,
    /// On some screen no window is spread over the whole desktop: sound stirs the water.
    /// (Under a window that fills the screen only the strips along the edges show, and
    /// those are left to the cursor.)
    pub open: bool,
    /// The cursor on the screen, while the desktop itself is under it.
    pub cursor: Option<(i32, i32)>,
    /// The left mouse button is down.
    pub down: bool,
}

pub struct Wallpaper {
    hwnd: HWND,
    thread: u32,
    /// The icons' window, where it is a child of the same window as ours: ours belongs
    /// right under it (see `desktop::Behind`).
    icons: Option<HWND>,
    stage: Option<Stage>,
    /// The water's colour and the light on it; or, in their place, a video.
    _base: Option<Bead>,
    lights: Vec<Drift>,
    film: Option<Film>,
    rings: Sprite,
    surface: CompositionDrawingSurface,
    _light: Option<CompositionDrawingSurface>,
    water: Water,
    pixels: Vec<u8>,
    /// Which water this is (the setting it was made for).
    look: u32,
    /// The window's top-left corner on screen, its size, and the side of a cell, in pixels.
    origin: (i32, i32),
    size: (i32, i32),
    cell: f32,
    /// How far the two layers of light slide in one round, and how long a round takes.
    rounds: [(f32, f32, f32); 2],
    sliding: bool,
    /// Rings are on show.
    painted: bool,
    cursor: Option<(f32, f32)>,
    down: bool,
    meter: Option<Meter>,
    meter_sought: Instant,
    heard: Instant,
    /// The level of the sound evened out over about a second, and the seconds since the last
    /// drop it made.
    usual: f32,
    since_drop: f32,
    /// Chooses where the drops of the music fall.
    dice: u32,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // Not there, as far as the mouse is concerned.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Makes the window as a child of `parent` on a thread of its own, which then only keeps it
/// company. Returns the window and the thread.
fn make_window(parent: HWND, size: (i32, i32)) -> Option<(HWND, u32)> {
    let (send, made) = channel();
    let parent = parent.0 as isize;
    let run = move || unsafe {
        let instance: HINSTANCE = match GetModuleHandleW(None) {
            Ok(module) => module.into(),
            Err(_) => return,
        };
        let class_name = wide(CLASS_NAME);
        let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: PCWSTR(class_name.as_ptr()), ..Default::default() };
        RegisterClassExW(&class);
        // WS_EX_NOREDIRECTIONBITMAP: only what the compositor draws, like every window of the app.
        let window = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP, PCWSTR(class_name.as_ptr()), PCWSTR::null(), WS_CHILD, 0, 0, size.0, size.1, Some(HWND(parent as *mut _)), None, Some(instance), None);
        let Ok(window) = window else {
            let _ = send.send(None);
            return;
        };
        let _ = send.send(Some((window.0 as isize, GetCurrentThreadId())));
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            DispatchMessageW(&msg);
        }
        let _ = DestroyWindow(window);
    };
    std::thread::Builder::new().name("wallpaper".into()).stack_size(128 * 1024).spawn(run).ok()?;
    let (window, thread) = made.recv().ok()??;
    Some((HWND(window as *mut _), thread))
}

fn tone(c: (f32, f32, f32), alpha: f32) -> Color {
    rgba(c.0, c.1, c.2, alpha)
}

impl Wallpaper {
    /// Puts the water behind the desktop's icons. `cfg.wallpaper` says which water, or
    /// that it is to be a video: `video` is then the file to play.
    pub fn new(scene: &Scene, cfg: &Config, video: Option<String>) -> Result<Self> {
        let failed = || windows::core::Error::from(E_FAIL);
        let behind: Behind = desktop::behind_icons().ok_or_else(failed)?;
        let mut client = RECT::default();
        let mut corner = POINT::default();
        unsafe {
            GetClientRect(behind.parent, &mut client)?;
            let _ = ClientToScreen(behind.parent, &mut corner);
        }
        let size = (client.right.max(1), client.bottom.max(1));
        let (hwnd, thread) = make_window(behind.parent, size).ok_or_else(failed)?;
        // From here on the window is given back if anything fails.
        let built = Self::build(scene, cfg, video, hwnd, thread, behind, (corner.x, corner.y), size);
        if built.is_err() {
            unsafe {
                let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        built
    }

    fn build(scene: &Scene, cfg: &Config, video: Option<String>, hwnd: HWND, thread: u32, behind: Behind, origin: (i32, i32), size: (i32, i32)) -> Result<Self> {
        let look_of = cfg.wallpaper.round() as u32;
        let look = if look_of == 2 { &BLUE } else { &DARK };
        let (w, h) = (size.0 as f32, size.1 as f32);
        let stage = scene.stage(hwnd)?;
        let content = stage.content()?;
        // Sized for the screen, so the water looks the same on a small one and a large one.
        let zoom = (h / 1080.0).clamp(0.6, 3.0);
        let mut lights = Vec::new();
        let mut rounds = [(0.0, 0.0, 0.0); 2];
        let (mut base, mut light, mut film) = (None, None, None);

        if look_of == VIDEO {
            // A video, where the water's colour and its light would be.
            // Each screen has the whole video to itself. (Where the screens cannot be told,
            // the whole desktop is one.)
            let mut screens: Vec<(f32, f32, f32, f32)> = system::monitors().iter().map(|monitor| monitor.rect).map(|r| ((r.left - origin.0) as f32, (r.top - origin.1) as f32, (r.right - r.left) as f32, (r.bottom - r.top) as f32)).collect();
            if screens.is_empty() {
                screens.push((0.0, 0.0, w, h));
            }
            film = Some(Film::new(&content, video.ok_or_else(|| windows::core::Error::from(E_INVALIDARG))?, &screens)?);
        } else {
            // The colour of the water. (A little larger than the window: the edge of this
            // shape is a line of its own, which is kept out of sight.)
            let mut colour = scene.bead_in(&content)?;
            colour.place(-2.0, -2.0, w + 4.0, h + 4.0, 0.0)?;
            colour.set_colors(tone(look.top, 1.0), tone(look.bottom, 1.0), tone(look.bottom, 0.0))?;
            colour.set_opacity(1.0)?;
            base = Some(colour);

            // The light: one picture, premultiplied, twice.
            let pattern = water::caustics(TILE);
            let mut pixels = Vec::with_capacity(TILE * TILE * 4);
            for value in &pattern {
                let byte = |c: f32| (c * value * 255.0).round().clamp(0.0, 255.0) as u8;
                pixels.extend_from_slice(&[byte(look.light.2), byte(look.light.1), byte(look.light.0), byte(1.0)]);
            }
            let picture = scene.surface_from_bgra(TILE as u32, TILE as u32, &pixels)?;
            for (index, (scale, seconds, across)) in [(1.5 * zoom, 96.0, 1.0), (2.35 * zoom, 150.0, -1.0)].into_iter().enumerate() {
                let mut layer = scene.drift_in(&content, &picture, scale)?;
                layer.place(0.0, 0.0, w, h)?;
                layer.set_opacity(look.layers[index])?;
                lights.push(layer);
                // One round is one whole picture across and one down, so it starts over unseen.
                let side = TILE as f32 * scale;
                rounds[index] = (side * across, side, seconds);
            }
            light = Some(picture);
        }

        // The rings: a sheet of cells stretched over the whole window.
        let cell = (CELL * zoom).max((w * h / MOST_CELLS).sqrt());
        let water = Water::new((w / cell).ceil() as usize, (h / cell).ceil() as usize);
        let (cells_w, cells_h) = water.size();
        let pixels = vec![0u8; cells_w * cells_h * 4];
        let surface = scene.surface_from_bgra(cells_w as u32, cells_h as u32, &pixels)?;
        let mut rings = scene.sprite_in(&content)?;
        rings.set_surface(&surface)?;
        rings.place(0.0, 0.0, cells_w as f32 * cell, cells_h as f32 * cell)?;
        rings.set_opacity(0.0)?;

        unsafe {
            // Under the icons, where they share a parent; then into view.
            if let Some(icons) = behind.icons {
                let _ = SetWindowPos(hwnd, Some(icons), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            let _ = ShowWindow(hwnd, SW_SHOWNA);
        }
        crate::log!("wallpaper: {}, {}x{} px in {}x{} cells, behind the icons of {:?}", ["", "dark water", "blue water", "a video"].get(look_of as usize).copied().unwrap_or("dark water"), size.0, size.1, cells_w, cells_h, behind.parent.0);
        Ok(Wallpaper {
            hwnd,
            thread,
            icons: behind.icons,
            stage: Some(stage),
            _base: base,
            lights,
            film,
            rings,
            surface,
            _light: light,
            water,
            pixels,
            look: look_of,
            origin,
            size,
            cell,
            rounds,
            sliding: false,
            painted: false,
            cursor: None,
            down: false,
            meter: None,
            meter_sought: Instant::now() - std::time::Duration::from_secs(60),
            heard: Instant::now(),
            usual: 0.0,
            since_drop: 0.0,
            dice: 0x2545_F491,
        })
    }

    /// Which water this is (the value of the setting it was made for).
    pub fn look(&self) -> u32 {
        self.look
    }

    /// The video it plays, if it is one.
    pub fn video(&self) -> Option<&str> {
        self.film.as_ref().map(|film| film.path.as_str())
    }

    /// False once the window is gone (Explorer restarted and took it along), or no longer
    /// fits the desktop (a screen came or went): the wallpaper has to be made again.
    pub fn fits(&self) -> bool {
        unsafe {
            if !IsWindow(Some(self.hwnd)).as_bool() {
                return false;
            }
            let mut client = RECT::default();
            match GetParent(self.hwnd) {
                Ok(parent) => GetClientRect(parent, &mut client).is_ok() && (client.right, client.bottom) == self.size,
                Err(_) => false,
            }
        }
    }

    /// Puts the window back right under the icons when the picture Windows draws has come
    /// to lie over it (see `desktop::picture_over`): the wallpaper would go on behind that
    /// picture, where nobody sees it.
    fn keep_place(&self) {
        let Some(icons) = self.icons else { return };
        if desktop::picture_over(self.hwnd) && unsafe { SetWindowPos(self.hwnd, Some(icons), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }.is_ok() {
            crate::log!("wallpaper: Windows had laid its own picture over it; it is back under the icons");
        }
    }

    /// A drop falls in at a point of the screen.
    pub fn splash(&mut self, x: i32, y: i32, strength: f32) {
        let (cx, cy) = self.cells(x as f32, y as f32);
        self.water.drop(cx, cy, 3.2, strength);
    }

    /// A point of the screen, in cells of the water.
    fn cells(&self, x: f32, y: f32) -> (f32, f32) {
        ((x - self.origin.0 as f32) / self.cell, (y - self.origin.1 as f32) / self.cell)
    }

    /// The next of a row of numbers between 0 and 1 that look like chance.
    fn roll(&mut self) -> f32 {
        self.dice = self.dice.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((self.dice >> 8) & 0xffff) as f32 / 65535.0
    }

    /// Lets sound stir the water: a beat is a drop somewhere, and sound without a beat
    /// still lets one fall now and then. `strength`: the setting.
    fn listen(&mut self, strength: f32) {
        let passed = self.heard.elapsed().as_secs_f32().min(0.5);
        self.heard = Instant::now();
        // The meter belongs to one device. Without one, or while it is silent, look again
        // now and then for the device Windows plays through: it may have changed.
        let level = self.meter.as_ref().and_then(|meter| meter.peak());
        if level.unwrap_or(0.0) <= 0.0 && self.meter_sought.elapsed().as_secs() >= 4 {
            self.meter_sought = Instant::now();
            self.meter = Meter::new();
        }
        let Some(level) = level else { return };
        self.since_drop += passed;
        let beat = level > 0.05 && level > self.usual * 1.28 + 0.03 && self.since_drop > 0.16;
        let stir = level > 0.03 && self.since_drop > 1.1;
        if beat || stir {
            let force = if beat { ((level - self.usual) * 6.0).clamp(0.9, 3.0) } else { 0.6 + level };
            let (cells_w, cells_h) = self.water.size();
            // Anywhere but the very edges.
            let (x, y) = (self.roll(), self.roll());
            self.water.drop((0.08 + 0.84 * x) * cells_w as f32, (0.1 + 0.8 * y) * cells_h as f32, if beat { 3.0 } else { 2.4 }, force * strength);
            self.since_drop = 0.0;
        }
        // What is usual just now: the level, evened out over about a second.
        self.usual += (level - self.usual) * (passed / 0.9).min(1.0);
    }

    /// Moves the water on. Returns true while it is moving (another frame is needed).
    pub fn update(&mut self, scene: &Scene, cfg: &Config, feel: &Feel, dt: f32) -> Result<bool> {
        self.keep_place();
        if let Some(film) = self.film.as_mut() {
            // A video plays while the desktop itself is in view on some screen. Under
            // windows that cover it, it stands still: at most the strips along the edges
            // show, and what cannot be seen is not worth the work of playing it.
            film.tend(scene, feel.showing && feel.open)?;
        }
        // The light plays only while there is something of the desktop to see.
        if feel.showing != self.sliding {
            self.sliding = feel.showing;
            for (layer, (dx, dy, seconds)) in self.lights.iter_mut().zip(self.rounds) {
                if feel.showing {
                    layer.slide(dx, dy, seconds)?;
                } else {
                    layer.rest()?;
                }
            }
        }
        if !feel.showing {
            self.cursor = None;
            return Ok(false);
        }
        let strength = cfg.wallpaper_strength;
        if cfg.wallpaper_cursor {
            let cursor = feel.cursor.map(|(x, y)| (x as f32, y as f32));
            if let (Some((x, y)), Some((from_x, from_y))) = (cursor, self.cursor) {
                // The cursor brushes the water along its way: a dent every few pixels, and
                // a fainter one for a slower move.
                let moved = ((x - from_x).powi(2) + (y - from_y).powi(2)).sqrt();
                if moved > 0.5 {
                    let dents = (moved / 9.0).ceil().clamp(1.0, 12.0);
                    let force = 0.075 * strength * (moved / 9.0 / dents).min(1.0);
                    for dent in 1..=dents as usize {
                        let along = dent as f32 / dents;
                        let (cx, cy) = self.cells(from_x + (x - from_x) * along, from_y + (y - from_y) * along);
                        self.water.touch(cx, cy, 3.4, force);
                    }
                }
            }
            if let (Some((x, y)), true, false) = (cursor, feel.down, self.down) {
                // A click: something drops in.
                let (cx, cy) = self.cells(x, y);
                self.water.drop(cx, cy, 3.2, 3.2 * strength);
            }
            self.cursor = cursor;
        } else {
            self.cursor = None;
        }
        self.down = feel.down;
        if cfg.wallpaper_music && feel.open {
            self.listen(strength);
        }

        if self.water.advance(dt) {
            let (cells_w, cells_h) = self.water.size();
            self.water.paint(&mut self.pixels);
            scene.repaint(&self.surface, cells_w as u32, cells_h as u32, &self.pixels)?;
            self.rings.set_opacity(1.0)?;
            self.painted = true;
        }
        if self.water.calm() && self.painted {
            self.rings.set_opacity(0.0)?;
            self.painted = false;
        }
        Ok(!self.water.calm())
    }
}

impl Drop for Wallpaper {
    fn drop(&mut self) {
        // What is drawn goes first, then the window, on the thread that made it.
        if let Some(film) = self.film.take() {
            let _ = film.player.Close();
        }
        self.stage = None;
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
            let _ = PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gfx::Gfx;
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};

    #[test]
    fn the_file_names_one_video() {
        assert_eq!(named("C:\\Videos\\sea.mp4\n").as_deref(), Some("C:\\Videos\\sea.mp4"));
        // As "Copy as path" gives it, after a remark and an empty line.
        assert_eq!(named("\u{feff}# the video\n\n  \"D:\\My films\\sea at night.mkv\"  \nanother.mp4\n").as_deref(), Some("D:\\My films\\sea at night.mkv"));
        assert_eq!(named(""), None);
        assert_eq!(named("\n   \n# nothing\n"), None);
    }

    #[test]
    fn a_video_is_drawn_in_its_own_shape_and_no_larger_than_needed() {
        // The size of the screen: as it is.
        assert_eq!(drawn_size((1920, 1080), (1920.0, 1080.0)), (1920.0, 1080.0));
        // Twice the screen: drawn at the screen's size.
        assert_eq!(drawn_size((3840, 2160), (1920.0, 1080.0)), (1920.0, 1080.0));
        // Smaller than the screen: as it is (the sprite makes it larger).
        assert_eq!(drawn_size((1280, 720), (1920.0, 1080.0)), (1280.0, 720.0));
        // Another shape than the screen: enough to fill it both ways, and still its own shape.
        assert_eq!(drawn_size((4000, 1000), (1920.0, 1080.0)), (4000.0, 1000.0));
        assert_eq!(drawn_size((4000, 3000), (1920.0, 1080.0)), (1920.0, 1440.0));
        assert_eq!(drawn_size((0, 0), (1920.0, 1080.0)), (1.0, 1.0));
    }

    /// Plays a video in a small window of its own for a few seconds: the player has to move
    /// on through the file, and what is on the screen where the window is has to be a
    /// picture of the window's own, and another picture a moment later. Also says how large
    /// the video is, and whether playing it keeps the display from going to sleep. No sound
    /// is made.
    ///
    /// The window is the size of a thumbnail and stands near the bottom left corner of the
    /// first screen, over everything, because a video's pictures cannot be had in a quieter
    /// way: a window that is out of sight gives a black picture when asked for one. Where
    /// the screen itself cannot be looked at (it is locked: what is copied off it is the
    /// lock screen), the test says so and goes by the player alone.
    /// Run by hand: BEN_VIDEO=<file> cargo test --profile quick a_video_plays -- --ignored --nocapture
    /// (BEN_OUT names a folder for the two pictures.)
    #[test]
    #[ignore]
    fn a_video_plays() {
        use windows::Win32::System::Power::{CallNtPowerInformation, SystemExecutionState};
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let path = std::env::var("BEN_VIDEO").expect("BEN_VIDEO names a video file");
        // What the system is kept awake for: 1 = the computer, 2 = the display.
        let awake = || unsafe {
            let mut state = 0u32;
            let _ = CallNtPowerInformation(SystemExecutionState, None, 0, Some(&mut state as *mut u32 as *mut _), 4);
            state
        };
        let before = awake();
        let gfx = Gfx::new(false).expect("a graphics device");
        let scene = Scene::new(&gfx).expect("a compositor");
        let (w, h) = (96, 54);
        let (x, y) = (240, unsafe { GetSystemMetrics(SM_CYSCREEN) } - h - 6);
        // What is on the screen where the window is.
        let seen = || unsafe {
            use windows::Win32::Graphics::Gdi::*;
            let screen = GetDC(None);
            let memory = CreateCompatibleDC(Some(screen));
            let bitmap = CreateCompatibleBitmap(screen, w, h);
            let previous = SelectObject(memory, bitmap.into());
            let _ = BitBlt(memory, 0, 0, w, h, Some(screen), x, y, SRCCOPY);
            SelectObject(memory, previous);
            let mut info = BITMAPINFO { bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() }, ..Default::default() };
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            GetDIBits(memory, bitmap, 0, h as u32, Some(pixels.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
            let _ = DeleteObject(bitmap.into());
            let _ = DeleteDC(memory);
            ReleaseDC(None, screen);
            pixels
        };
        // How many pixels of two such pictures are not the same colour, give or take `slack`.
        let unlike = |a: &[u8], b: &[u8], slack: u8| a.chunks(4).zip(b.chunks(4)).filter(|(a, b)| (0..3).any(|c| a[c].abs_diff(b[c]) > slack)).count();
        let hwnd = unsafe {
            let instance: HINSTANCE = GetModuleHandleW(None).expect("a module").into();
            let class_name = wide("BenVideoTestWindow");
            let class = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: instance, lpszClassName: PCWSTR(class_name.as_ptr()), ..Default::default() };
            RegisterClassExW(&class);
            let hwnd = CreateWindowExW(WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST, PCWSTR(class_name.as_ptr()), PCWSTR::null(), WS_POPUP, x, y, w, h, None, None, Some(instance), None).expect("a window");
            let _ = ShowWindow(hwnd, SW_SHOWNA);
            hwnd
        };
        let stage = scene.stage(hwnd).expect("a stage");
        let content = stage.content().expect("its content");
        let mut msg = MSG::default();
        let mut turn = |seconds: f32, each: &mut dyn FnMut()| {
            let from = Instant::now();
            while from.elapsed().as_secs_f32() < seconds {
                unsafe {
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        DispatchMessageW(&msg);
                    }
                }
                each();
                scene.commit();
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        // First: can the screen be looked at at all? The window all red for a moment.
        let all = (w * h) as usize;
        let red = scene.backdrop_in(&content, rgba(1.0, 0.0, 0.0, 1.0)).expect("a backdrop");
        turn(0.5, &mut || {});
        let reddened = seen();
        let looking = reddened.chunks(4).filter(|pixel| pixel[2] > 200 && pixel[1] < 60 && pixel[0] < 60).count() * 2 > all;
        content.Remove(&red).expect("the backdrop taken away");

        let memory = || crate::system::memory_use().map(|m| (m.in_ram, m.private)).unwrap_or((0.0, 0.0));
        let light = memory();
        let mut film = Film::new(&content, path, &[(0.0, 0.0, w as f32, h as f32)]).expect("a player");
        let into = |film: &Film| film.player.PlaybackSession().and_then(|session| session.Position()).map(|at| at.Duration as f32 / 1e7).unwrap_or(-1.0);
        let started = Instant::now();
        while !film.shown && started.elapsed().as_secs_f32() < 10.0 {
            turn(0.05, &mut || film.tend(&scene, true).expect("the player tended"));
        }
        assert!(film.shown, "the video never began to play");
        let came = started.elapsed().as_secs_f32();
        // It plays for a while, stands still, is looked at; and the same once more.
        let mut pictures: Vec<(f32, Vec<u8>)> = Vec::new();
        let mut during = 0;
        for _ in 0..2 {
            turn(1.2, &mut || film.tend(&scene, true).expect("the player tended"));
            during = awake();
            turn(0.7, &mut || film.tend(&scene, false).expect("the player tended"));
            pictures.push((into(&film), seen()));
        }
        let session = film.player.PlaybackSession().expect("a session");
        println!("the video is {}x{} px and came into view {came:.1} s after it was asked for", session.NaturalVideoWidth().unwrap_or(0), session.NaturalVideoHeight().unwrap_or(0));
        println!("it stood still {:.1} s into the video, and then {:.1} s into it", pictures[0].0, pictures[1].0);
        println!("kept awake before: {before:#x}; while it played: {during:#x} (1 = the computer, 2 = the display)");
        let heavy = memory();
        println!("memory before there was a player: {:.0} MB in RAM, {:.0} MB private; after playing: {:.0} MB in RAM, {:.0} MB private", light.0, light.1, heavy.0, heavy.1);
        assert!(pictures[1].0 > pictures[0].0 + 0.5, "the player has not moved on");
        if looking {
            let (own, moved) = (unlike(&reddened, &pictures[0].1, 12), unlike(&pictures[0].1, &pictures[1].1, 2));
            println!("on the screen: {} % of the window is no longer red but a picture; {} % of that picture is different the second time", own * 100 / all, moved * 100 / all);
            if let Ok(out) = std::env::var("BEN_OUT") {
                for (index, (_, pixels)) in pictures.iter().enumerate() {
                    crate::camera::save_picture(&std::path::Path::new(&out).join(format!("video{index}.bmp")), w, h, pixels);
                }
            }
            assert!(own * 2 > all, "the window shows nothing of the video");
            assert!(moved > 0, "the same picture later on: on the screen the video stands still");
        } else {
            println!("the screen cannot be looked at now (locked?): what the window shows was not checked");
        }
        drop(film);
        drop(stage);
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }

    /// Not a check of anything: puts the wallpaper on the real desktop for a few seconds,
    /// drops a few things in and draws the cursor across it, so that it can be looked at.
    /// Run by hand: cargo test --release live_on_the_desktop -- --ignored
    /// (BEN_WATER=2 for the blue water, BEN_SECONDS for how long. BEN_WATER=3 with
    /// BEN_VIDEO naming a file for a video.)
    #[test]
    #[ignore]
    fn live_on_the_desktop() {
        let number = |name: &str, otherwise: f32| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(otherwise);
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let gfx = Gfx::new(false).expect("a graphics device");
        let scene = Scene::new(&gfx).expect("a compositor");
        let mut cfg = Config::default();
        cfg.wallpaper = number("BEN_WATER", 1.0);
        let mut wallpaper = Wallpaper::new(&scene, &cfg, std::env::var("BEN_VIDEO").ok()).expect("the wallpaper");
        assert!(wallpaper.fits());
        let (w, h) = (wallpaper.size.0 as f32, wallpaper.size.1 as f32);
        let (seconds, started) = (number("BEN_SECONDS", 6.0), Instant::now());
        let mut last = Instant::now();
        let mut dropped = 0;
        let mut frames = 0;
        let mut msg = MSG::default();
        loop {
            let t = started.elapsed().as_secs_f32();
            if t >= seconds {
                break;
            }
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            // Three drops, half a second apart, and the cursor drawn across from the first second on.
            for (index, (at, x, y, strength)) in [(0.6, 0.30, 0.45, 3.2), (1.1, 0.64, 0.33, 2.4), (1.6, 0.52, 0.72, 1.6)].into_iter().enumerate() {
                if t >= at && dropped == index {
                    wallpaper.splash(wallpaper.origin.0 + (w * x) as i32, wallpaper.origin.1 + (h * y) as i32, strength);
                    dropped += 1;
                }
            }
            let stroke = ((t - 1.0) / 1.6).clamp(0.0, 1.0);
            let cursor = (t > 1.0 && stroke < 1.0).then(|| (wallpaper.origin.0 + (w * (0.15 + 0.5 * stroke)) as i32, wallpaper.origin.1 + (h * (0.85 - 0.3 * stroke)) as i32));
            let feel = Feel { showing: true, open: std::env::var("BEN_SOUND").is_ok(), cursor, down: false };
            let dt = last.elapsed().as_secs_f32();
            last = Instant::now();
            wallpaper.update(&scene, &cfg, &feel, dt).expect("an update");
            scene.commit();
            frames += 1;
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        println!("{frames} frames in {seconds} s; calm at the end: {}", wallpaper.water.calm());
        // Windows lays its own picture over it now and then. Put under everything else in
        // the desktop's window, it has to be back over the picture after the next pass.
        if wallpaper.icons.is_some() {
            unsafe {
                let _ = SetWindowPos(wallpaper.hwnd, Some(HWND_BOTTOM), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
            assert!(desktop::picture_over(wallpaper.hwnd), "the window could not be put under the picture");
            wallpaper.update(&scene, &cfg, &Feel { showing: true, open: false, cursor: None, down: false }, 0.0).expect("an update");
            assert!(!desktop::picture_over(wallpaper.hwnd), "the window was left under the picture");
            println!("put under Windows' picture, it was back over it after one pass");
        }
    }
}
