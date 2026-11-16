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
//! The window is a child of the desktop's, between its icons and its picture
//! (see `desktop.rs`). A window inside another program's shares that
//! program's turn at the mouse and keyboard, so it has a thread of its own
//! that does nothing else: the desktop never waits for the dock, nor the dock
//! for the desktop.

use crate::audio::Meter;
use crate::config::Config;
use crate::desktop::{self, Behind};
use crate::scene::{rgba, Bead, Drift, Scene, Sprite, Stage};
use crate::util::wide;
use crate::water::{self, Water};
use std::sync::mpsc::channel;
use std::time::Instant;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Color;
use windows::UI::Composition::CompositionDrawingSurface;

const CLASS_NAME: &str = "BenWallpaperWindow";
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
    stage: Option<Stage>,
    _base: Bead,
    lights: Vec<Drift>,
    rings: Sprite,
    surface: CompositionDrawingSurface,
    _light: CompositionDrawingSurface,
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
    /// Puts the water behind the desktop's icons. `cfg.wallpaper` says which water.
    pub fn new(scene: &Scene, cfg: &Config) -> Result<Self> {
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
        let built = Self::build(scene, cfg, hwnd, thread, behind, (corner.x, corner.y), size);
        if built.is_err() {
            unsafe {
                let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
        built
    }

    fn build(scene: &Scene, cfg: &Config, hwnd: HWND, thread: u32, behind: Behind, origin: (i32, i32), size: (i32, i32)) -> Result<Self> {
        let look_of = cfg.wallpaper.round() as u32;
        let look = if look_of == 2 { &BLUE } else { &DARK };
        let (w, h) = (size.0 as f32, size.1 as f32);
        let stage = scene.stage(hwnd)?;
        let content = stage.content()?;

        // The colour of the water. (A little larger than the window: the edge of this shape
        // is a line of its own, which is kept out of sight.)
        let mut base = scene.bead_in(&content)?;
        base.place(-2.0, -2.0, w + 4.0, h + 4.0, 0.0)?;
        base.set_colors(tone(look.top, 1.0), tone(look.bottom, 1.0), tone(look.bottom, 0.0))?;
        base.set_opacity(1.0)?;

        // The light: one picture, premultiplied, twice.
        let pattern = water::caustics(TILE);
        let mut pixels = Vec::with_capacity(TILE * TILE * 4);
        for value in &pattern {
            let byte = |c: f32| (c * value * 255.0).round().clamp(0.0, 255.0) as u8;
            pixels.extend_from_slice(&[byte(look.light.2), byte(look.light.1), byte(look.light.0), byte(1.0)]);
        }
        let light = scene.surface_from_bgra(TILE as u32, TILE as u32, &pixels)?;
        // Sized for the screen, so the water looks the same on a small one and a large one.
        let zoom = (h / 1080.0).clamp(0.6, 3.0);
        let mut lights = Vec::new();
        let mut rounds = [(0.0, 0.0, 0.0); 2];
        for (index, (scale, seconds, across)) in [(1.5 * zoom, 96.0, 1.0), (2.35 * zoom, 150.0, -1.0)].into_iter().enumerate() {
            let mut layer = scene.drift_in(&content, &light, scale)?;
            layer.place(0.0, 0.0, w, h)?;
            layer.set_opacity(look.layers[index])?;
            lights.push(layer);
            // One round is one whole picture across and one down, so it starts over unseen.
            let side = TILE as f32 * scale;
            rounds[index] = (side * across, side, seconds);
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
        crate::log!("wallpaper: {} water, {}x{} px in {}x{} cells, behind the icons of {:?}", if look_of == 2 { "blue" } else { "dark" }, size.0, size.1, cells_w, cells_h, behind.parent.0);
        Ok(Wallpaper {
            hwnd,
            thread,
            stage: Some(stage),
            _base: base,
            lights,
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

    /// Not a check of anything: puts the wallpaper on the real desktop for a few seconds,
    /// drops a few things in and draws the cursor across it, so that it can be looked at.
    /// Run by hand: cargo test --release live_on_the_desktop -- --ignored
    /// (BEN_WATER=2 for the blue water, BEN_SECONDS for how long.)
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
        let mut wallpaper = Wallpaper::new(&scene, &cfg).expect("the wallpaper");
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
    }
}
