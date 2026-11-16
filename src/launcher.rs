//! The app tray: a sheet of glass that grows up out of the dock's first
//! button, holding every installed app in pages, the most popular first (see
//! `ranking.rs`). Type to search (by name, initials, short name...), scroll or
//! use the arrow keys to change page, click an app to open it, drag it onto a
//! dock to pin it, click outside or press Esc to leave.
//!
//! The list of apps is read from the start; only the icons of the page on show
//! are painted (they come from `shell::scan_apps`, mostly from the file kept by
//! `icon_cache.rs`); a while after it is closed it lets them go again.

use crate::app::{self, Event};
use crate::gfx::Gfx;
use crate::layout::Grid;
use crate::motion::{Glide, Motion};
use crate::ranking::{self, Hay, Query, Usage};
use crate::scene::{Bead, Glass, Pill, Scene, Sprite, Stage};
use crate::shell::{self, AppScan, Found, IconPixels};
use crate::spring::Spring;
use crate::text::{self, glyph, Line, ICON_FONT, UI_FONT};
use crate::theme::Theme;
use crate::util::wide;
use std::cell::Cell;
use std::time::Instant;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::CompositionDrawingSurface;

const CLASS_NAME: &str = "BenLauncherWindow";
const WM_MOUSELEAVE: u32 = 0x02A3;
/// How many icons and names are painted per frame at most, so opening never stutters.
const PAINTS_PER_FRAME: usize = 12;

/// What the tray's window reports to the app.
#[derive(Clone, Copy, Debug)]
pub enum Input {
    Move(i32, i32),
    Leave,
    /// The mouse button went down.
    Press(i32, i32),
    Click(i32, i32),
    /// Mouse wheel; positive = away from the user.
    Wheel(i32),
    /// A typed character (UTF-16 unit).
    Char(u16),
    /// A key that is not a character (arrows).
    Key(u16),
    /// Another window was activated.
    Lost,
}

struct Tile {
    name: String,
    /// Lower-case name, for ordering.
    key: String,
    /// What a search looks at.
    hay: Hay,
    /// How popular the app is (see `ranking.rs`); the most popular are on the first page.
    score: f32,
    target: String,
    /// The icon, read but not painted yet.
    pixels: Option<IconPixels>,
    /// The icon has been asked for.
    asked: bool,
    icon: Option<Sprite>,
    surface: Option<CompositionDrawingSurface>,
    label: Option<Line>,
    shown: bool,
    /// 0 = a stand-in tile, 1 = the app's own icon, fully there. It swells in as it arrives.
    arrive: Spring,
}

pub struct Launcher {
    hwnd: HWND,
    _stage: Stage,
    glass: Glass,
    /// The drop of light under the cursor.
    hot: Bead,
    hot_at: Glide,
    search_box: Pill,
    search_icon: Line,
    search_text: Line,
    /// Said where the apps would be when there are none to show.
    empty: Line,
    dots: Vec<Pill>,
    /// Every app, the most popular first.
    tiles: Vec<Tile>,
    /// Indices into `tiles` that match the search, in display order.
    matches: Vec<usize>,
    query: String,
    /// What was started and what was picked for which search.
    usage: Usage,
    /// The tray has been closed since this was last asked.
    just_closed: bool,
    /// What a tile shows until its icon has been read.
    blank: Option<CompositionDrawingSurface>,
    open: bool,
    /// The window is on screen (open, or shrinking away).
    visible: bool,
    /// 0 = closed, 1 = fully open.
    show: Spring,
    /// Current page, as a position that glides to `page`.
    scroll: Spring,
    page: usize,
    grid: Grid,
    icon: f32,
    scale: f32,
    size: (f32, f32),
    /// Room around the glass inside the window, for its shadow.
    margin: f32,
    /// The point the sheet grows out of, in its own coordinates.
    root: (f32, f32),
    cursor: Option<(f32, f32)>,
    /// The tile the mouse button went down on, and where (sheet pixels).
    held: Option<(usize, f32, f32)>,
    /// The app whose tile is being dragged out of the tray, towards a dock.
    carrying: Option<String>,
    label_font: Option<IDWriteTextFormat>,
    search_font: Option<IDWriteTextFormat>,
    icon_font: Option<IDWriteTextFormat>,
    /// When it was last closed (its icons are let go a while later).
    closed: Option<Instant>,
}

thread_local! {
    static TRACKING: Cell<bool> = const { Cell::new(false) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let send = |input: Input| app::post(Event::Launcher(input));
    let (x, y) = ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_MOUSEMOVE => {
            if !TRACKING.get() {
                let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                TRACKING.set(unsafe { TrackMouseEvent(&mut track) }.is_ok());
            }
            send(Input::Move(x, y));
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            TRACKING.set(false);
            send(Input::Leave);
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            // A tile can be dragged out of the tray: keep hearing from the mouse out there.
            unsafe {
                SetCapture(hwnd);
            }
            send(Input::Press(x, y));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            send(Input::Click(x, y));
            return LRESULT(0);
        }
        WM_MOUSEWHEEL => {
            send(Input::Wheel(((wparam.0 >> 16) & 0xffff) as i16 as i32));
            return LRESULT(0);
        }
        WM_CHAR => {
            send(Input::Char(wparam.0 as u16));
            return LRESULT(0);
        }
        WM_KEYDOWN => {
            send(Input::Key(wparam.0 as u16));
            return LRESULT(0);
        }
        WM_ACTIVATE => {
            if (wparam.0 & 0xffff) == 0 {
                send(Input::Lost);
            }
            return LRESULT(0);
        }
        WM_CLOSE => {
            // Alt+F4 on the tray closes the tray, not the app.
            send(Input::Lost);
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Launcher {
    pub fn new(scene: &Scene, theme: &Theme, motion: &Motion) -> Result<Self> {
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
            // Unlike the dock, this window takes focus: it needs the keyboard for searching.
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(wide("Bến - Ứng dụng").as_ptr()),
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
            let glass = scene.glass_in(&stage.content()?, &theme.sheet)?;
            // Everything stays inside the sheet's shape: pages slide in and out under its edge.
            let inside = glass.inner()?;
            let hot = scene.bead_in(&inside)?;
            let search_box = scene.pill_in(&inside)?;
            let search_icon = Line::new_in(scene, &inside)?;
            let search_text = Line::new_in(scene, &inside)?;
            let empty = Line::new_in(scene, &inside)?;
            Ok(Launcher {
                hwnd,
                _stage: stage,
                glass,
                hot,
                hot_at: Glide::new(motion),
                search_box,
                search_icon,
                search_text,
                empty,
                dots: Vec::new(),
                tiles: Vec::new(),
                matches: Vec::new(),
                query: String::new(),
                usage: Usage::default(),
                just_closed: false,
                blank: None,
                open: false,
                visible: false,
                show: Spring::with(0.0, motion.large()),
                scroll: Spring::with(0.0, motion.medium()),
                page: 0,
                grid: Grid { cols: 1, rows: 1, cell_w: 1.0, cell_h: 1.0, x: 0.0, y: 0.0 },
                icon: 0.0,
                scale: 1.0,
                size: (1.0, 1.0),
                margin: 0.0,
                root: (0.0, 0.0),
                cursor: None,
                held: None,
                carrying: None,
                label_font: None,
                search_font: None,
                icon_font: None,
                closed: None,
            })
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn set_theme(&mut self, theme: &Theme) {
        self.glass.set_material(&theme.sheet);
        self.forget_text();
    }

    /// Forgets painted names (they are painted for one size and one theme).
    fn forget_text(&mut self) {
        self.label_font = None;
        self.search_font = None;
        self.icon_font = None;
        for tile in &mut self.tiles {
            tile.label = None;
        }
        self.search_text.forget();
        self.search_icon.forget();
        self.empty.forget();
    }

    /// Lets go of every icon and name that was painted; they are read again when next shown.
    /// Returns true if there was anything to let go.
    pub fn release(&mut self) -> bool {
        let had = self.blank.is_some() || self.tiles.iter().any(|t| t.icon.is_some() || t.pixels.is_some());
        for tile in &mut self.tiles {
            tile.icon = None;
            tile.surface = None;
            tile.label = None;
            tile.pixels = None;
            tile.asked = false;
            tile.arrive.snap(0.0);
        }
        self.blank = None;
        self.closed = None;
        had
    }

    /// Seconds since the tray was closed, if it has been open and still holds its icons.
    pub fn idle_for(&self) -> Option<f32> {
        if self.visible {
            return None;
        }
        self.closed.map(|t| t.elapsed().as_secs_f32())
    }

    /// Takes in what the background reader found: apps (the most popular first, then in
    /// alphabetical order), their scores when they change, and icons. Returns true if
    /// anything arrived.
    pub fn absorb(&mut self, found: impl Iterator<Item = Found>, motion: &Motion) -> bool {
        let (mut added, mut any) = (false, false);
        for item in found {
            any = true;
            match item {
                Found::App { name, target, score } => {
                    if self.tiles.iter().any(|t| t.target == target) {
                        continue;
                    }
                    let key = name.to_lowercase();
                    let hay = ranking::hay(&name, &target);
                    let at = self.tiles.partition_point(|t| t.score > score || (t.score == score && t.key <= key));
                    self.tiles.insert(at, Tile { name, key, hay, score, target, pixels: None, asked: false, icon: None, surface: None, label: None, shown: false, arrive: Spring::with(0.0, motion.small()) });
                    added = true;
                }
                Found::Scores(scores) => {
                    let scores: std::collections::HashMap<String, f32> = scores.into_iter().collect();
                    for tile in &mut self.tiles {
                        if let Some(&score) = scores.get(&tile.target) {
                            tile.score = score;
                        }
                    }
                    self.tiles.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal).then_with(|| a.key.cmp(&b.key)));
                    // The tiles have moved: what was held is another tile now.
                    self.held = None;
                    added = true;
                }
                Found::Icon { target, pixels } => {
                    if let Some(tile) = self.tiles.iter_mut().find(|t| t.target == target) {
                        tile.pixels = Some(pixels);
                    }
                }
            }
        }
        if added {
            self.search();
        }
        any
    }

    /// Recomputes which apps match the search text: all of them in order of popularity when
    /// nothing is typed, otherwise the apps that answer it, the best answers first (and,
    /// among equals, the most popular and those picked for this search before).
    fn search(&mut self) {
        let query = Query::new(&self.query);
        if query.is_empty() {
            self.matches = (0..self.tiles.len()).collect();
        } else {
            let key = query.key();
            let mut found: Vec<(usize, f32)> = (0..self.tiles.len())
                .filter_map(|i| {
                    let tile = &self.tiles[i];
                    query.score(&tile.hay).map(|score| (i, score + 0.6 * tile.score + 60.0 * self.usage.pick_count(&key, &tile.target).min(6) as f32))
                })
                .collect();
            found.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| self.tiles[a.0].key.cmp(&self.tiles[b.0].key)));
            self.matches = found.into_iter().map(|(i, _)| i).collect();
        }
        let pages = self.grid.pages(self.matches.len());
        self.page = self.page.min(pages - 1);
        self.scroll.set(self.page as f32);
    }

    /// Notes that an app was chosen from the tray (for this search, if one was typed), and
    /// hands its target back.
    fn pick(&mut self, target: String) -> String {
        let query = Query::new(&self.query);
        if !query.is_empty() {
            self.usage.picked(&query.key(), &target);
        }
        self.usage.launched(&target);
        self.usage.save();
        target
    }

    /// True once after the tray has been closed: the apps can be scored again.
    pub fn take_closed(&mut self) -> bool {
        std::mem::take(&mut self.just_closed)
    }

    /// Opens the tray on `monitor`, its bottom edge at screen row `bottom`, growing out of
    /// screen column `from_x`. `icon` is the icon size in logical pixels.
    pub fn open(&mut self, scene: &Scene, theme: &Theme, motion: &Motion, monitor: RECT, scale: f32, icon: f32, bottom: i32, from_x: i32) -> Result<()> {
        let (mw, mh) = ((monitor.right - monitor.left) as f32, (monitor.bottom - monitor.top) as f32);
        let px = |v: f32| (v * scale).round();
        if self.scale != scale {
            self.forget_text();
        }
        if self.icon != px(icon) {
            // The icons that were read are the wrong size.
            self.release();
        }
        (self.scale, self.icon) = (scale, px(icon));
        let (pad, head, foot) = (px(22.0), px(72.0), px(36.0));
        let (cell_w, cell_h) = (self.icon + px(46.0), self.icon + px(50.0));
        let room = ((bottom - monitor.top) as f32 - px(48.0) - head - foot).min(mh * 0.64);
        self.grid = Grid::fit(mw * 0.68 - 2.0 * pad, room, cell_w, cell_h, pad, head);
        let (w, h) = (self.grid.width() + 2.0 * pad, head + self.grid.height() + foot);
        self.size = (w, h);
        let left = monitor.left + ((mw - w) * 0.5).round() as i32;
        let top = bottom - h as i32;
        self.root = (((from_x - left) as f32).clamp(px(40.0), w - px(40.0)), h);
        let margin = theme.sheet.shadow_reach + px(10.0);
        self.margin = margin;
        // Blur and shadow are sized for the screen it opens on.
        self.glass.set_material(&theme.sheet);
        self.glass.set_rect(scene, margin, margin, w, h, px(26.0))?;

        self.query.clear();
        self.held = None;
        self.carrying = None;
        self.page = 0;
        self.scroll = Spring::with(0.0, motion.medium());
        self.usage = Usage::load();
        self.search();
        self.open = true;
        self.cursor = None;
        if !self.visible {
            self.show.snap(0.0);
        }
        self.show.set(1.0);
        self.apply()?;
        let m = margin as i32;
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), left - m, top - m, w as i32 + 2 * m, h as i32 + 2 * m, SWP_SHOWWINDOW);
        }
        shell::activate(self.hwnd);
        self.visible = true;
        Ok(())
    }

    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.just_closed = true;
            self.show.set(0.0);
        }
        self.held = None;
        self.carrying = None;
    }

    /// The app whose tile is being dragged out of the tray.
    pub fn carried(&self) -> Option<String> {
        self.carrying.clone()
    }

    /// The same, as the mouse button comes up: the drag is over.
    pub fn take_carried(&mut self) -> Option<String> {
        self.carrying.take()
    }

    /// The picture of the app being dragged (its stand-in if its icon has not come yet) and
    /// its size in pixels, for the bubble that carries it.
    pub fn carried_icon(&self) -> Option<(CompositionDrawingSurface, f32)> {
        let target = self.carrying.as_ref()?;
        let tile = self.tiles.iter().find(|t| &t.target == target)?;
        Some((tile.surface.clone().or_else(|| self.blank.clone())?, self.icon))
    }

    fn turn_page(&mut self, by: i32) {
        let pages = self.grid.pages(self.matches.len()) as i32;
        self.page = (self.page as i32 + by).clamp(0, pages - 1) as usize;
        self.scroll.set(self.page as f32);
    }

    /// Reacts to mouse and keyboard. Returns the app to start, if one was chosen.
    pub fn handle(&mut self, input: Input) -> Option<String> {
        // Once an app is being dragged the tray has gone, but its window goes on hearing the
        // mouse until the button comes up.
        if !self.open && (self.carrying.is_none() || !matches!(input, Input::Move(..) | Input::Click(..) | Input::Leave)) {
            return None;
        }
        match input {
            Input::Move(x, y) => {
                let (x, y) = (x as f32 - self.margin, y as f32 - self.margin);
                self.cursor = Some((x, y));
                // A press on a tile turns into a drag once the cursor has clearly moved: the
                // tray fades away, and the app goes on under the cursor.
                if let (Some((tile, px, py)), None) = (self.held, &self.carrying) {
                    if (x - px).abs().max((y - py).abs()) >= 6.0 * self.scale {
                        self.carrying = self.tiles.get(tile).map(|t| t.target.clone());
                        if self.carrying.is_some() {
                            self.open = false;
                            self.just_closed = true;
                            self.show.set(0.0);
                        }
                    }
                }
            }
            Input::Leave => self.cursor = None,
            Input::Press(x, y) => {
                let (x, y) = (x as f32 - self.margin, y as f32 - self.margin);
                self.held = self.tile_at(x, y).map(|tile| (tile, x, y));
                self.carrying = None;
            }
            Input::Click(x, y) => {
                self.held = None;
                if self.carrying.is_some() {
                    // The end of a drag, not a click: the app sees where it was let go.
                    return None;
                }
                let (x, y) = (x as f32 - self.margin, y as f32 - self.margin);
                if let Some(target) = self.tile_at(x, y).map(|i| self.tiles[i].target.clone()) {
                    let target = self.pick(target);
                    self.close();
                    return Some(target);
                }
                if let Some(page) = self.dot_at(x, y) {
                    self.page = page;
                    self.scroll.set(page as f32);
                } else if x < 0.0 || y < 0.0 || x >= self.size.0 || y >= self.size.1 {
                    // On the shadow around the sheet: the same as clicking outside.
                    self.close();
                }
            }
            Input::Wheel(delta) => self.turn_page(if delta > 0 { -1 } else { 1 }),
            Input::Key(key) => match key {
                0x25 | 0x21 => self.turn_page(-1), // left arrow, Page Up
                0x27 | 0x22 => self.turn_page(1),  // right arrow, Page Down
                _ => {}
            },
            Input::Char(unit) => match unit {
                27 => {
                    // Esc clears the search first, then closes.
                    if self.query.is_empty() {
                        self.close();
                    } else {
                        self.query.clear();
                        self.search();
                    }
                }
                13 => {
                    // Enter opens the first match (of a search: with nothing typed it opens nothing).
                    if !self.query.trim().is_empty() {
                        if let Some(target) = self.matches.first().map(|&i| self.tiles[i].target.clone()) {
                            let target = self.pick(target);
                            self.close();
                            return Some(target);
                        }
                    }
                }
                8 => {
                    self.query.pop();
                    self.page = 0;
                    self.search();
                }
                unit if unit >= 32 => {
                    if let Some(ch) = char::from_u32(unit as u32) {
                        if self.query.chars().count() < 40 {
                            self.query.push(ch);
                            self.page = 0;
                            self.search();
                        }
                    }
                }
                _ => {}
            },
            Input::Lost => self.close(),
        }
        None
    }

    /// The app tile under a point (sheet pixels), on the current page.
    fn tile_at(&self, x: f32, y: f32) -> Option<usize> {
        let cell = self.grid.cell_at(x, y)?;
        self.matches.get(self.page * self.grid.per_page() + cell).copied()
    }

    fn search_rect(&self) -> (f32, f32, f32, f32) {
        let s = self.scale;
        let (w, h) = ((300.0 * s).round().min(self.size.0 - 40.0 * s), (36.0 * s).round());
        (((self.size.0 - w) * 0.5).round(), (20.0 * s).round(), w, h)
    }

    fn dot_rect(&self, index: usize, count: usize) -> (f32, f32, f32) {
        let s = self.scale;
        let spacing = 16.0 * s;
        let x = self.size.0 * 0.5 + (index as f32 - (count as f32 - 1.0) * 0.5) * spacing;
        (x, self.grid.y + self.grid.height() + 16.0 * s, 3.5 * s)
    }

    fn dot_at(&self, x: f32, y: f32) -> Option<usize> {
        let count = self.grid.pages(self.matches.len());
        (0..count).find(|&i| {
            let (cx, cy, r) = self.dot_rect(i, count);
            (x - cx).abs() <= r * 2.2 && (y - cy).abs() <= r * 3.0
        })
    }

    fn apply(&mut self) -> Result<()> {
        let shown = self.show.pos;
        // It grows out of the button it belongs to.
        let grow = 0.55 + 0.45 * shown.clamp(0.0, 1.1);
        self.glass.set_scale(grow, grow, self.root.0, self.root.1)?;
        self.glass.set_opacity((shown * 1.6).clamp(0.0, 1.0))
    }

    /// Advances the open / close and page-turn motion and lays everything out. `scan` is
    /// where missing icons are asked for. Returns true while something is still moving.
    pub fn update(&mut self, scene: &Scene, gfx: &Gfx, theme: &Theme, motion: &Motion, scan: Option<&AppScan>, dt: f32) -> Result<bool> {
        if !self.visible {
            return Ok(false);
        }
        self.show.tune(if self.open { motion.large() } else { motion.calm() });
        self.scroll.tune(motion.medium());
        self.show.step(dt);
        self.scroll.step(dt);
        if !self.open && self.show.pos < 0.02 && self.carrying.is_none() {
            // Shrunk away (and nothing is being dragged out of it): take the window off the screen.
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.visible = false;
            self.show.snap(0.0);
            self.closed = Some(Instant::now());
            return Ok(false);
        }
        self.apply()?;
        let (s, (w, _)) = (self.scale, self.size);
        let content = self.glass.inner()?;

        if self.label_font.is_none() {
            self.label_font = Some(text::format(gfx, UI_FONT, (12.0 * s).round(), DWRITE_FONT_WEIGHT_NORMAL)?);
            self.search_font = Some(text::format(gfx, UI_FONT, (14.0 * s).round(), DWRITE_FONT_WEIGHT_NORMAL)?);
            self.icon_font = Some(text::format(gfx, ICON_FONT, (13.0 * s).round(), DWRITE_FONT_WEIGHT_NORMAL)?);
        }
        let label_font = self.label_font.clone().unwrap();
        let (fg, dim) = (theme.text, theme.text_dim);

        // Search field: a magnifying glass, then what was typed.
        let (bx, by, bw, bh) = self.search_rect();
        self.search_box.place(bx, by, bw, bh, bh * 0.5)?;
        self.search_box.set_color(theme.track)?;
        self.search_icon.set(scene, gfx, self.icon_font.as_ref().unwrap(), &glyph(0xE721), dim, None)?;
        let iy = by + ((bh - self.search_icon.h) * 0.5).round();
        self.search_icon.place(bx + (13.0 * s).round(), iy)?;
        let placeholder = self.query.is_empty();
        self.search_text.set(scene, gfx, self.search_font.as_ref().unwrap(), if placeholder { "Tìm ứng dụng" } else { &self.query }, if placeholder { dim } else { fg }, Some(bw - 60.0 * s))?;
        let ty = by + ((bh - self.search_text.h) * 0.5).round();
        self.search_text.place(bx + (36.0 * s).round(), ty)?;

        // Where there are no apps to show, say why.
        let words = if self.tiles.is_empty() {
            "Đang đọc danh sách ứng dụng…".to_string()
        } else if self.matches.is_empty() {
            format!("Không tìm thấy “{}”", self.query.trim())
        } else {
            String::new()
        };
        self.empty.set(scene, gfx, self.search_font.as_ref().unwrap(), &words, dim, Some(w - 60.0 * s))?;
        let ey = self.grid.y + (self.grid.height() * 0.35).round();
        self.empty.place(((w - self.empty.w) * 0.5).round(), ey)?;

        // Tiles: the current page and, while a page turn is in motion, its neighbours.
        let per_page = self.grid.per_page();
        let icon = self.icon;
        let cell_w = self.grid.cell_w;
        let mut paints = 0;
        let mut arriving = false;
        for tile in &mut self.tiles {
            tile.shown = false;
        }
        for (position, &index) in self.matches.iter().enumerate() {
            let page = position / per_page;
            let away = page as f32 - self.scroll.pos;
            if away.abs() >= 1.0 {
                continue;
            }
            let tile = &mut self.tiles[index];
            // A stand-in at first; the app's own icon swells in when it has been read.
            if tile.icon.is_none() {
                if self.blank.is_none() {
                    self.blank = Some(scene.surface_from_pixels(&shell::placeholder_icon(icon as u32))?);
                }
                let sprite = scene.sprite_in(&content)?;
                sprite.set_surface(self.blank.as_ref().unwrap())?;
                tile.icon = Some(sprite);
            }
            if tile.surface.is_none() {
                match tile.pixels.take() {
                    Some(pixels) if paints < PAINTS_PER_FRAME => {
                        paints += 1;
                        let surface = scene.surface_from_pixels(&pixels)?;
                        tile.icon.as_ref().unwrap().set_surface(&surface)?;
                        tile.surface = Some(surface);
                        tile.arrive.snap(0.0);
                        tile.arrive.set(1.0);
                    }
                    Some(pixels) => tile.pixels = Some(pixels),
                    None if !tile.asked => {
                        if let Some(scan) = scan {
                            scan.want_icon(&tile.target, icon as u32);
                            tile.asked = true;
                        }
                    }
                    None => {}
                }
            }
            if tile.label.is_none() && paints < PAINTS_PER_FRAME {
                paints += 1;
                let mut line = Line::new_in(scene, &content)?;
                line.set(scene, gfx, &label_font, &tile.name, fg, Some(cell_w - 12.0 * s))?;
                tile.label = Some(line);
            }
            tile.arrive.tune(motion.small());
            tile.arrive.step(dt);
            arriving |= !tile.arrive.settled(1e-3);
            let (cx, cy) = self.grid.cell_center(position % per_page);
            let x = cx + away * w;
            let top = cy - (icon + 26.0 * s) * 0.5;
            let size = if tile.surface.is_some() { icon * (0.7 + 0.3 * tile.arrive.pos.clamp(0.0, 1.15)) } else { icon };
            let sprite = tile.icon.as_mut().unwrap();
            sprite.place((x - size * 0.5).round(), (top + (icon - size) * 0.5).round(), size, size)?;
            // The app being dragged out of the tray is dimmed where it was taken from.
            let lifted = self.carrying.as_deref() == Some(tile.target.as_str());
            sprite.set_opacity(if lifted { 0.3 } else if tile.surface.is_some() { 1.0 } else { 0.55 })?;
            if let Some(label) = tile.label.as_mut() {
                label.place(x - label.w * 0.5, top + icon + 8.0 * s)?;
                label.set_opacity(if lifted { 0.3 } else { 1.0 })?;
            }
            tile.shown = true;
        }
        for tile in &mut self.tiles {
            if !tile.shown {
                if let Some(sprite) = tile.icon.as_mut() {
                    sprite.set_opacity(0.0)?;
                }
                if let Some(label) = tile.label.as_mut() {
                    label.set_opacity(0.0)?;
                }
            }
        }

        // The drop of light under the cursor.
        // Typing a search, the first answer is lit when the cursor is not on one: it is what Enter opens.
        let hot = self
            .cursor
            .filter(|_| self.open)
            .and_then(|(mx, my)| self.grid.cell_at(mx, my))
            .filter(|&cell| self.matches.get(self.page * per_page + cell).is_some())
            .or_else(|| (self.open && !self.query.trim().is_empty() && self.page == 0 && !self.matches.is_empty()).then_some(0));
        self.hot_at.tune(motion);
        self.hot_at.aim(hot.map(|cell| {
            let (cx, cy) = self.grid.cell_center(cell);
            let (hw, hh) = (self.grid.cell_w - 8.0 * s, self.grid.cell_h - 6.0 * s);
            (cx - hw * 0.5, cy - hh * 0.5, hw, hh)
        }));
        self.hot_at.step(dt);
        let shape = self.hot_at.shape(motion.squash());
        self.hot.place(shape.x, shape.y, shape.w, shape.h, (16.0 * s).round())?;
        let (bead_top, bead_bottom, bead_edge) = theme.bead;
        self.hot.set_colors(bead_top, bead_bottom, bead_edge)?;
        self.hot.set_opacity(shape.alpha)?;

        // Page dots.
        let pages = self.grid.pages(self.matches.len());
        while self.dots.len() < pages.min(24) {
            self.dots.push(scene.pill_in(&content)?);
        }
        for i in 0..self.dots.len() {
            if i < pages && pages > 1 {
                let (cx, cy, r) = self.dot_rect(i, pages);
                // The dot of the page on show is a short dash.
                let near = (1.0 - (i as f32 - self.scroll.pos).abs()).clamp(0.0, 1.0);
                let half = r + near * r * 1.6;
                let dot = &mut self.dots[i];
                dot.place(cx - half, cy - r, half * 2.0, r * 2.0, r)?;
                dot.set_color(theme.tone(0.30 + 0.6 * near))?;
                dot.set_opacity(1.0)?;
            } else {
                self.dots[i].set_opacity(0.0)?;
            }
        }
        Ok(arriving || paints > 0 || !self.show.settled(1e-3) || !self.scroll.settled(1e-3) || !self.hot_at.settled())
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
