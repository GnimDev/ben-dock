//! The app launcher: a full-screen glass sheet with every installed app in a
//! grid of pages. Type to search, scroll or use the arrow keys to change
//! page, click an app to open it, click anywhere else or press Esc to leave.

use crate::dock::{self, Event};
use crate::gfx::{color, Gfx};
use crate::glass::{rgba, Glass, Palette, Pill, Popup, Sprite};
use crate::layout::Grid;
use crate::shell::{AppEntry, IconPixels};
use crate::spring::Spring;
use crate::text::{self, Line, UI_FONT};
use crate::util::wide;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::CompositionDrawingSurface;

const CLASS_NAME: &str = "BenLauncherWindow";
/// How many icons and names are painted per frame at most, so opening never stutters.
const PAINTS_PER_FRAME: usize = 14;

/// What the launcher window reports to the dock.
#[derive(Clone, Copy, Debug)]
pub enum Input {
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
    /// Lower-case name, for searching.
    key: String,
    target: String,
    pixels: IconPixels,
    icon: Option<Sprite>,
    _surface: Option<CompositionDrawingSurface>,
    label: Option<Line>,
    shown: bool,
}

pub struct Launcher {
    hwnd: HWND,
    popup: Popup,
    tiles: Vec<Tile>,
    /// Indices into `tiles` that match the search, in display order.
    matches: Vec<usize>,
    query: String,
    open: bool,
    window_shown: bool,
    /// 0 = closed, 1 = fully open.
    show: Spring,
    /// Current page, as a position that glides to `page`.
    scroll: Spring,
    page: usize,
    grid: Grid,
    icon: f32,
    scale: f32,
    size: (f32, f32),
    origin: (i32, i32),
    dark: bool,
    search_box: Pill,
    search_text: Line,
    highlight: Pill,
    dots: Vec<Pill>,
    label_font: Option<IDWriteTextFormat>,
    search_font: Option<IDWriteTextFormat>,
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let send = |input: Input| dock::post(Event::Launcher(input));
    match msg {
        WM_LBUTTONUP => {
            send(Input::Click((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32));
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
            // Alt+F4 on the launcher closes the launcher, not the dock.
            send(Input::Lost);
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Launcher {
    pub fn new(glass: &Glass, blur_sigma: Option<f32>, palette: &Palette) -> Result<Self> {
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
            let popup = glass.popup(hwnd, blur_sigma, palette)?;
            let highlight = glass.pill_in(popup.content()?)?;
            let search_box = glass.pill_in(popup.content()?)?;
            let search_text = Line::new(glass.sprite_in(popup.content()?)?);
            Ok(Launcher {
                hwnd,
                popup,
                tiles: Vec::new(),
                matches: Vec::new(),
                query: String::new(),
                open: false,
                window_shown: false,
                show: Spring::new(0.0, 20.0, 1.0),
                scroll: Spring::new(0.0, 16.0, 0.9),
                page: 0,
                grid: Grid { cols: 1, rows: 1, cell_w: 1.0, cell_h: 1.0, x: 0.0, y: 0.0 },
                icon: 72.0,
                scale: 1.0,
                size: (1.0, 1.0),
                origin: (0, 0),
                dark: true,
                search_box,
                search_text,
                highlight,
                dots: Vec::new(),
                label_font: None,
                search_font: None,
            })
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// True while the launcher is on screen (open, or still fading out).
    pub fn is_visible(&self) -> bool {
        self.window_shown
    }

    pub fn set_palette(&self, palette: &Palette) {
        let _ = self.popup.set_palette(palette);
    }

    /// Forgets everything painted (after the GPU device was reset); it is painted again when shown.
    pub fn forget(&mut self) {
        for tile in &mut self.tiles {
            tile.icon = None;
            tile._surface = None;
            tile.label = None;
        }
        self.search_text.forget();
    }

    /// Adds apps as the background scan finds them, keeping the list in alphabetical order.
    pub fn absorb(&mut self, entries: impl Iterator<Item = AppEntry>) {
        let mut added = false;
        for entry in entries {
            let key = entry.name.to_lowercase();
            if self.tiles.iter().any(|t| t.target == entry.target) {
                continue;
            }
            let at = self.tiles.partition_point(|t| t.key <= key);
            self.tiles.insert(at, Tile { name: entry.name, key, target: entry.target, pixels: entry.icon, icon: None, _surface: None, label: None, shown: false });
            added = true;
        }
        if added {
            self.search();
        }
    }

    /// Recomputes which apps match the search text.
    fn search(&mut self) {
        let query = self.query.trim().to_lowercase();
        self.matches = (0..self.tiles.len()).filter(|&i| query.is_empty() || self.tiles[i].key.contains(&query)).collect();
        let pages = self.grid.pages(self.matches.len());
        self.page = self.page.min(pages - 1);
        self.scroll.set(self.page as f32);
    }

    /// Opens the launcher over `monitor`. `icon` is the icon size in logical pixels.
    pub fn open(&mut self, monitor: RECT, scale: f32, icon: f32, dark: bool) {
        let (w, h) = ((monitor.right - monitor.left) as f32, (monitor.bottom - monitor.top) as f32);
        let px = |v: f32| (v * scale).round();
        if (self.scale, self.icon, self.dark) != (scale, px(icon), dark) {
            // Names are painted for one size and one theme.
            self.label_font = None;
            self.search_font = None;
            for tile in &mut self.tiles {
                tile.label = None;
            }
            self.search_text.forget();
        }
        (self.scale, self.icon, self.dark, self.size, self.origin) = (scale, px(icon), dark, (w, h), (monitor.left, monitor.top));
        // Room above for the search field, below for the page dots and the dock.
        self.grid = Grid::fit(w, h, self.icon + px(74.0), self.icon + px(62.0), px(150.0), px(170.0));
        self.query.clear();
        self.page = 0;
        self.scroll = Spring::new(0.0, 16.0, 0.9);
        self.search();
        self.open = true;
        self.show.set(1.0);
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), monitor.left, monitor.top, w as i32, h as i32, SWP_SHOWWINDOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
        self.window_shown = true;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.show.set(0.0);
    }

    fn turn_page(&mut self, by: i32) {
        let pages = self.grid.pages(self.matches.len()) as i32;
        self.page = (self.page as i32 + by).clamp(0, pages - 1) as usize;
        self.scroll.set(self.page as f32);
    }

    /// Reacts to mouse and keyboard. Returns the app to start, if one was chosen.
    pub fn handle(&mut self, input: Input) -> Option<String> {
        if !self.open {
            return None;
        }
        match input {
            Input::Click(x, y) => {
                let (x, y) = (x as f32, y as f32);
                if let Some(target) = self.tile_at(x, y).map(|i| self.tiles[i].target.clone()) {
                    self.close();
                    return Some(target);
                }
                if let Some(page) = self.dot_at(x, y) {
                    self.page = page;
                    self.scroll.set(page as f32);
                } else if !self.in_search_box(x, y) {
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
                    // Enter opens the first match.
                    if let Some(target) = self.matches.first().map(|&i| self.tiles[i].target.clone()) {
                        self.close();
                        return Some(target);
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

    /// The app tile under a point (window pixels), on the current page.
    fn tile_at(&self, x: f32, y: f32) -> Option<usize> {
        let cell = self.grid.cell_at(x, y)?;
        self.matches.get(self.page * self.grid.per_page() + cell).copied()
    }

    fn search_rect(&self) -> (f32, f32, f32, f32) {
        let s = self.scale;
        let (w, h) = (300.0 * s, 36.0 * s);
        (((self.size.0 - w) * 0.5).round(), (64.0 * s).round(), w, h)
    }

    fn in_search_box(&self, x: f32, y: f32) -> bool {
        let (bx, by, bw, bh) = self.search_rect();
        x >= bx && x < bx + bw && y >= by && y < by + bh
    }

    fn dot_rect(&self, index: usize, count: usize) -> (f32, f32, f32) {
        let s = self.scale;
        let spacing = 18.0 * s;
        let x = self.size.0 * 0.5 + (index as f32 - (count as f32 - 1.0) * 0.5) * spacing;
        (x, self.grid.y + self.grid.rows as f32 * self.grid.cell_h + 26.0 * s, 4.0 * s)
    }

    fn dot_at(&self, x: f32, y: f32) -> Option<usize> {
        let count = self.grid.pages(self.matches.len());
        (0..count).find(|&i| {
            let (cx, cy, r) = self.dot_rect(i, count);
            (x - cx).abs() <= r * 2.2 && (y - cy).abs() <= r * 3.0
        })
    }

    /// Advances the open / close and page-turn motion and lays everything out. Returns true
    /// while the launcher is on screen.
    pub fn update(&mut self, glass: &Glass, gfx: &Gfx, cursor: POINT, dt: f32) -> Result<bool> {
        if !self.window_shown {
            return Ok(false);
        }
        self.show.step(dt);
        self.scroll.step(dt);
        if !self.open && self.show.pos < 0.01 {
            // Faded out: take the window away, and drop the search.
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            self.window_shown = false;
            self.popup.set_opacity(0.0)?;
            return Ok(false);
        }
        let (s, (w, h)) = (self.scale, self.size);
        let dark = self.dark;
        let shown = self.show.pos.clamp(0.0, 1.0);
        self.popup.set_frame(-1.0, -1.0, w + 2.0, h + 2.0, 0.0)?;
        self.popup.set_opacity(shown)?;
        // The grid settles into place from slightly enlarged.
        self.popup.set_content_scale(1.0 + 0.1 * (1.0 - shown), w * 0.5 + 1.0, h * 0.5 + 1.0)?;

        if self.label_font.is_none() {
            self.label_font = Some(text::format(gfx, UI_FONT, (12.5 * s).round(), DWRITE_FONT_WEIGHT_NORMAL)?);
            self.search_font = Some(text::format(gfx, UI_FONT, (14.5 * s).round(), DWRITE_FONT_WEIGHT_NORMAL)?);
        }
        let label_font = self.label_font.clone().unwrap();
        let fg = if dark { color(1.0, 1.0, 1.0, 0.95) } else { color(0.0, 0.0, 0.0, 0.88) };
        let dim = if dark { color(1.0, 1.0, 1.0, 0.5) } else { color(0.0, 0.0, 0.0, 0.5) };
        let tone = if dark { 1.0 } else { 0.0 };

        // Search field.
        let (bx, by, bw, bh) = self.search_rect();
        self.search_box.place(bx, by, bw, bh, bh * 0.5)?;
        self.search_box.set_color(rgba(tone, tone, tone, 0.14))?;
        let placeholder = self.query.is_empty();
        self.search_text.set(glass, gfx, self.search_font.as_ref().unwrap(), if placeholder { "Tìm ứng dụng" } else { &self.query }, if placeholder { dim } else { fg }, Some(bw - 32.0 * s))?;
        let (tw, th) = (self.search_text.w, self.search_text.h);
        self.search_text.place(bx + (bw - tw) * 0.5, by + (bh - th) * 0.5)?;

        // Tiles: the current page and, while a page turn is in motion, its neighbours.
        let per_page = self.grid.per_page();
        let icon = self.icon;
        let cell_w = self.grid.cell_w;
        let mut paints = 0;
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
            // Paint the icon and the name the first time they are needed, a few per frame.
            if tile.icon.is_none() {
                if paints >= PAINTS_PER_FRAME {
                    continue;
                }
                let surface = glass.surface_from_pixels(&tile.pixels)?;
                let sprite = glass.sprite_in(self.popup.content()?)?;
                sprite.set_surface(&surface)?;
                tile.icon = Some(sprite);
                tile._surface = Some(surface);
                paints += 1;
            }
            if tile.label.is_none() {
                if paints >= PAINTS_PER_FRAME {
                    continue;
                }
                paints += 1;
                let mut line = Line::new(glass.sprite_in(self.popup.content()?)?);
                line.set(glass, gfx, &label_font, &tile.name, fg, Some(cell_w - 14.0 * s))?;
                tile.label = Some(line);
            }
            let (cx, cy) = self.grid.cell_center(position % per_page);
            let x = cx + away * w;
            let top = cy - (icon + 30.0 * s) * 0.5;
            let sprite = tile.icon.as_mut().unwrap();
            sprite.place((x - icon * 0.5).round(), top.round(), icon, icon)?;
            sprite.set_opacity(1.0)?;
            let label = tile.label.as_mut().unwrap();
            label.place(x - label.w * 0.5, top + icon + 9.0 * s)?;
            label.set_opacity(1.0)?;
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

        // Highlight under the cursor.
        let (mx, my) = ((cursor.x - self.origin.0) as f32, (cursor.y - self.origin.1) as f32);
        match self.grid.cell_at(mx, my).filter(|&c| self.matches.get(self.page * per_page + c).is_some()) {
            Some(cell) if self.open => {
                let (cx, cy) = self.grid.cell_center(cell);
                let (hw, hh) = (self.grid.cell_w - 10.0 * s, self.grid.cell_h - 8.0 * s);
                self.highlight.place(cx - hw * 0.5, cy - hh * 0.5, hw, hh, 14.0 * s)?;
                self.highlight.set_color(rgba(tone, tone, tone, 0.12))?;
                self.highlight.set_opacity(1.0)?;
            }
            _ => self.highlight.set_opacity(0.0)?,
        }

        // Page dots.
        let pages = self.grid.pages(self.matches.len());
        while self.dots.len() < pages.min(24) {
            self.dots.push(glass.pill_in(self.popup.content()?)?);
        }
        for i in 0..self.dots.len() {
            if i < pages && pages > 1 {
                let (cx, cy, r) = self.dot_rect(i, pages);
                let dot = &mut self.dots[i];
                dot.place(cx - r, cy - r, r * 2.0, r * 2.0, r)?;
                dot.set_color(rgba(tone, tone, tone, if i == self.page { 0.9 } else { 0.32 }))?;
                dot.set_opacity(1.0)?;
            } else {
                self.dots[i].set_opacity(0.0)?;
            }
        }
        Ok(true)
    }
}

impl Drop for Launcher {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
