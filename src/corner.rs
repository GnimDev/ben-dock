//! The corners: four small pieces of the same glass as the island, one in each corner of
//! a screen, in the strips the island and the dock already keep free.
//!
//!   top left      Claude: how much of the week's limit is used; a bar dances while it works
//!   top right     notifications: a bell and how many; each new one swells out as a card
//!   bottom left   the weather: a sign for the sky, the temperature, a word or two
//!   bottom right  the computer: how busy the processor is, how full the memory
//!
//! Each rests as a pill and opens under the cursor like the island does, into a list of
//! rows the app hands it. A corner's glass grows out of its own corner. As with the island,
//! the window never moves; only the glass inside it changes size, and only the glass (and
//! the strip between it and the screen's edges) takes the mouse.

use crate::app::{self, Event};
use crate::config::Config;
use crate::dock;
use crate::gfx::Gfx;
use crate::island::{Action, Input, Page, Row, RowKind};
use crate::media::Art;
use crate::motion::{Glide, Motion};
use crate::scene::{Bead, Glass, Layer, Pill, Scene, Sprite, Stage};
use crate::shade::Shade;
use crate::spring::Spring;
use crate::system::Monitor;
use crate::text::{self, glyph, Line, ICON_FONT, UI_FONT};
use crate::theme::Theme;
use crate::util::wide;
use std::cell::Cell;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::VisualCollection;

const CLASS_NAME: &str = "BenCornerWindow";
const WM_MOUSELEAVE: u32 = 0x02A3;

const TIMER_ALERT: usize = 1;
const TIMER_DWELL: usize = 2;
const TIMER_LEAVE: usize = 3;
/// How long a notification's card stays, how long the cursor must rest before the corner
/// opens, and how long after it left before the corner closes. Milliseconds.
const ALERT_MS: u32 = 5200;
const DWELL_MS: u32 = 140;
const LEAVE_MS: u32 = 320;

// The open corner, in logical pixels: its width and the tallest it gets.
const OPEN_W: f32 = 340.0;
const OPEN_H_MOST: f32 = 400.0;
const OPEN_RADIUS: f32 = 22.0;
/// Most rows a list shows.
const MOST_ROWS: usize = 11;
/// Gap between the glass and the side of the screen.
const EDGE: f32 = 10.0;

/// Which corner, and so what it is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Claude,
    Notices,
    Weather,
    System,
}

impl Kind {
    fn left(self) -> bool {
        matches!(self, Kind::Claude | Kind::Weather)
    }

    fn top(self) -> bool {
        matches!(self, Kind::Claude | Kind::Notices)
    }

    /// The page of rows it opens into.
    fn rows(self) -> Page {
        match self {
            Kind::Claude => Page::Claude,
            Kind::Notices => Page::Notices,
            Kind::Weather => Page::Weather,
            Kind::System => Page::System,
        }
    }
}

/// What a corner shows at rest; read by the app, handed over every frame.
pub struct Show<'a> {
    /// False: the corner has nothing to show and stays out of view.
    pub there: bool,
    /// A symbol in front (0 = none).
    pub glyph: u32,
    pub text: &'a str,
    /// Dimmed text after `text`.
    pub sub: &'a str,
    /// Something is going on: a little bar dances at the end.
    pub busy: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Rest,
    Alert,
    Open,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    Row(usize),
    /// The cross at the end of a row (the row's index).
    Close(usize),
}

type Zones = Vec<(Zone, f32, f32, f32, f32)>;

#[derive(Clone)]
struct Fonts {
    text: IDWriteTextFormat,
    bold: IDWriteTextFormat,
    small: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    icons_big: IDWriteTextFormat,
}

/// The visuals of one row of the list.
struct RowLook {
    icon: Line,
    text: Line,
    hint: Line,
    mark: Line,
}

pub struct Corner {
    hwnd: HWND,
    kind: Kind,
    stage: Stage,
    /// The window under this one that shows the glass's shadow.
    shade: Option<Shade>,
    glass: Glass,

    // At rest: a symbol, a few words, a bar that dances.
    rest: Layer,
    r_bar: Pill,
    r_icon: Line,
    r_text: Line,
    r_sub: Line,

    // A notification's card.
    alert: Layer,
    a_art: Sprite,
    a_art_of: Option<Art>,
    a_icon: Line,
    a_text: Line,
    a_body: Line,

    // Open: a title and a list of rows.
    list: Layer,
    list_content: VisualCollection,
    l_title: Line,
    looks: Vec<RowLook>,
    /// The drop of light under the cursor.
    hot: Bead,
    hot_at: Glide,

    fonts: Option<Fonts>,
    mode: Mode,
    title: String,
    rows: Vec<Row>,
    /// The notification being told of: its two lines and the app's own mark.
    notice: Option<(String, String, Option<Art>)>,
    alerting: bool,
    w: Spring,
    h: Spring,
    r: Spring,
    /// How much of each content is showing: at rest, the card, the list.
    show: [Spring; 3],
    /// 0 = in view, 1 = slid out past the screen's edge (nothing to show).
    away: Spring,
    hovering: bool,
    pressed: Option<Zone>,
    cursor: Option<(f32, f32)>,
    /// Clickable parts of the open list, in window pixels.
    zones: Zones,

    scale: f32,
    /// Gap between the glass and the screen's top or bottom edge, and the pill's height.
    gap: f32,
    pill_h: f32,
    win: (f32, f32),
    /// Size of the card as last laid out (it keeps its shape while it fades away).
    alert_size: (f32, f32),
    region: Option<(i32, i32, i32, i32)>,
    /// What `place` was last called with, so repeating it does nothing.
    placed: Option<(RECT, i32, i32, i32)>,
}

thread_local! {
    /// The corner window whose mouse-leave is being tracked (0: none).
    static TRACKING: Cell<isize> = const { Cell::new(0) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let me = hwnd.0 as isize;
    let send = |input: Input| app::post(Event::Corner(me, input));
    let (x, y) = ((lparam.0 & 0xffff) as i16 as i32, ((lparam.0 >> 16) & 0xffff) as i16 as i32);
    match msg {
        WM_MOUSEMOVE => {
            if TRACKING.get() != me {
                let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
                if unsafe { TrackMouseEvent(&mut track) }.is_ok() {
                    TRACKING.set(me);
                }
            }
            send(Input::Move(x, y));
            return LRESULT(0);
        }
        WM_MOUSELEAVE => {
            if TRACKING.get() == me {
                TRACKING.set(0);
            }
            send(Input::Leave);
            return LRESULT(0);
        }
        WM_LBUTTONDOWN => {
            send(Input::Down(x, y));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            send(Input::Up(x, y));
            return LRESULT(0);
        }
        WM_TIMER => {
            unsafe {
                let _ = KillTimer(Some(hwnd), wparam.0);
            }
            send(Input::Timer(wparam.0));
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl Corner {
    pub fn new(scene: &Scene, theme: &Theme, motion: &Motion, kind: Kind) -> Result<Self> {
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
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(wide("Bến - Góc").as_ptr()),
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
            let mut glass = scene.glass_in(&stage.content()?, &theme.island)?;
            // The window is cut down to the glass, so its shadow needs a window of its own.
            let shade = match Shade::new(scene).and_then(|shade| glass.cast_shadow_on(&shade.content()?).map(|_| shade)) {
                Ok(shade) => Some(shade),
                Err(e) => {
                    crate::log!("no window for a corner's shadow ({e}); it goes without one");
                    glass.without_shadow();
                    None
                }
            };
            // Everything lives inside the glass's shape, so the shape cuts it as it shrinks.
            let inside = glass.inner()?;
            let line = |parent: &VisualCollection| Line::new_in(scene, parent);

            let rest = scene.layer_in(&inside)?;
            let r = rest.content()?;
            let r_bar = scene.pill_in(&r)?;

            let alert = scene.layer_in(&inside)?;
            let a = alert.content()?;
            let a_art = scene.sprite_in(&a)?;

            let list = scene.layer_in(&inside)?;
            let l = list.content()?;
            let hot = scene.bead_in(&l)?;

            Ok(Corner {
                hwnd,
                kind,
                r_bar,
                r_icon: line(&r)?,
                r_text: line(&r)?,
                r_sub: line(&r)?,
                rest,
                a_art,
                a_art_of: None,
                a_icon: line(&a)?,
                a_text: line(&a)?,
                a_body: line(&a)?,
                alert,
                l_title: line(&l)?,
                looks: Vec::new(),
                list_content: l,
                list,
                hot,
                hot_at: Glide::new(motion),
                glass,
                stage,
                shade,
                fonts: None,
                mode: Mode::Rest,
                title: String::new(),
                rows: Vec::new(),
                notice: None,
                alerting: false,
                w: Spring::with(30.0, motion.medium()),
                h: Spring::with(8.0, motion.medium()),
                r: Spring::with(4.0, motion.medium()),
                show: [Spring::with(0.0, motion.fade()); 3],
                away: Spring::with(1.0, motion.calm()),
                hovering: false,
                pressed: None,
                cursor: None,
                zones: Vec::new(),
                scale: 1.0,
                gap: 6.0,
                pill_h: 30.0,
                win: (1.0, 1.0),
                alert_size: (120.0, 54.0),
                region: None,
                placed: None,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// True while the corner is in use: under the cursor or open.
    pub fn engaged(&self) -> bool {
        self.hovering || self.mode == Mode::Open
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Puts the corner in its corner of `monitor`: the top ones level with the island, the
    /// bottom ones on the dock's own line (`bottom_line` is the bottom of the dock's strip).
    pub fn place(&mut self, monitor: &Monitor, cfg: &Config, theme: &Theme, bottom_line: i32) {
        let s = monitor.scale();
        let px = |v: f32| (v * s).round();
        let margin = theme.island.shadow_reach + px(10.0);
        let pill_h = px(cfg.island_height);
        let gap = if self.kind.top() {
            px(6.0)
        } else {
            let m = dock::metrics(cfg, s, monitor.width());
            (m.margin + (m.panel_h - pill_h) * 0.5).round().max(px(4.0))
        };
        let (win_w, win_h) = (px(EDGE) + px(OPEN_W) + margin, gap + px(OPEN_H_MOST) + margin);
        let key = (monitor.rect, bottom_line, pill_h as i32 + gap as i32 * 1000, win_w as i32);
        if self.placed == Some(key) {
            return;
        }
        self.placed = Some(key);
        let x = if self.kind.left() { monitor.rect.left } else { monitor.rect.right - win_w as i32 };
        let y = if self.kind.top() { monitor.rect.top } else { bottom_line - win_h as i32 };
        unsafe {
            let _ = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), x, y, win_w as i32, win_h as i32, SWP_NOACTIVATE);
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
        if let Some(shade) = self.shade.as_mut() {
            shade.place(self.hwnd, x, y, win_w as i32, win_h as i32);
        }
        if self.scale != s {
            // Text is painted for one size.
            self.forget();
        }
        (self.scale, self.gap, self.pill_h, self.win) = (s, gap, pill_h, (win_w, win_h));
        self.region = None;
    }

    pub fn set_theme(&mut self, theme: &Theme) {
        self.glass.set_material(&theme.island);
        self.forget();
    }

    /// Forgets painted text, pictures and fonts, so they are made again (after a theme, size
    /// or GPU change).
    pub fn forget(&mut self) {
        self.fonts = None;
        for line in [&mut self.r_icon, &mut self.r_text, &mut self.r_sub, &mut self.a_icon, &mut self.a_text, &mut self.a_body, &mut self.l_title] {
            line.forget();
        }
        // Rows are made again from scratch, and so is the picture.
        self.looks.clear();
        self.a_art_of = None;
    }

    fn timer(&self, id: usize, ms: u32) {
        unsafe {
            SetTimer(Some(self.hwnd), id, ms, None);
        }
    }

    fn cancel(&self, id: usize) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), id);
        }
    }

    /// Makes the corner swell into a card that tells of another app's notification.
    pub fn alert(&mut self, app: &str, title: &str, body: &str, logo: Option<Art>) {
        // Two lines. With the app's own mark beside them the app need not be named; a
        // notification of one line is told under the app's name.
        let (first, second) = match (title.is_empty(), body.is_empty(), logo.is_some()) {
            (true, ..) => (app.to_string(), body.to_string()),
            (false, true, _) => (app.to_string(), title.to_string()),
            (false, false, true) => (title.to_string(), body.to_string()),
            (false, false, false) => (format!("{app} · {title}"), body.to_string()),
        };
        self.notice = Some((first, second, logo));
        self.alerting = true;
        if self.mode != Mode::Open {
            self.mode = Mode::Alert;
        }
        self.timer(TIMER_ALERT, ALERT_MS);
    }

    /// Closes the open corner (something else took the stage).
    pub fn collapse(&mut self) {
        if self.mode == Mode::Open {
            self.mode = if self.alerting { Mode::Alert } else { Mode::Rest };
        }
        self.pressed = None;
    }

    /// The page of rows on show, if the corner is open.
    pub fn page(&self) -> Option<Page> {
        (self.mode == Mode::Open).then_some(self.kind.rows())
    }

    /// Fills the open list. Called again with new rows whenever what it says has changed.
    pub fn show_rows(&mut self, title: &str, rows: Vec<Row>) {
        self.title = title.to_string();
        self.rows = rows;
        self.rows.truncate(MOST_ROWS);
    }

    fn zone_at(&self, x: f32, y: f32) -> Option<Zone> {
        self.zones.iter().find(|(_, l, t, r, b)| x >= *l && x < *r && y >= *t && y < *b).map(|(zone, ..)| *zone)
    }

    /// Reacts to the mouse and the corner's timers. Opening asks the app for the rows.
    pub fn handle(&mut self, input: Input) -> Action {
        match input {
            Input::Move(x, y) => {
                self.cursor = Some((x as f32, y as f32));
                if !self.hovering {
                    self.hovering = true;
                    self.cancel(TIMER_LEAVE);
                    if self.mode != Mode::Open {
                        self.timer(TIMER_DWELL, DWELL_MS);
                    }
                }
            }
            Input::Leave => {
                self.hovering = false;
                self.cursor = None;
                self.cancel(TIMER_DWELL);
                if self.mode == Mode::Open {
                    self.timer(TIMER_LEAVE, LEAVE_MS);
                }
            }
            Input::Down(x, y) => {
                if self.mode == Mode::Open {
                    self.pressed = self.zone_at(x as f32, y as f32);
                }
            }
            Input::Up(x, y) => {
                let pressed = self.pressed.take();
                let (fx, fy) = (x as f32, y as f32);
                // The very corner of the screen, bottom right: what the end of the taskbar did.
                if self.kind == Kind::System && fx >= self.win.0 - 3.0 * self.scale && fy >= self.win.1 - 3.0 * self.scale {
                    return Action::Desktop;
                }
                if self.mode != Mode::Open {
                    self.cancel(TIMER_DWELL);
                    self.mode = Mode::Open;
                    return Action::Open(self.kind.rows());
                }
                let page = self.kind.rows();
                return match self.zone_at(fx, fy).filter(|zone| Some(*zone) == pressed) {
                    Some(Zone::Row(index)) => match self.rows.get(index) {
                        Some(row) if row.kind == RowKind::Item => Action::Row(page, row.id),
                        _ => Action::None,
                    },
                    Some(Zone::Close(index)) => match self.rows.get(index) {
                        Some(row) => Action::Close(page, row.id),
                        None => Action::None,
                    },
                    None => Action::None,
                };
            }
            Input::Timer(TIMER_ALERT) => {
                self.alerting = false;
                if self.mode == Mode::Alert {
                    self.mode = Mode::Rest;
                }
            }
            Input::Timer(TIMER_DWELL) => {
                if self.hovering && self.mode != Mode::Open {
                    self.mode = Mode::Open;
                    return Action::Open(self.kind.rows());
                }
            }
            Input::Timer(TIMER_LEAVE) => {
                if !self.hovering && self.mode == Mode::Open {
                    self.collapse();
                }
            }
            _ => {}
        }
        Action::None
    }

    /// Brings the contents up to date, advances the shape and lays everything out.
    /// `hide`: 0 = shown, 1 = slid out of view (for full-screen apps).
    /// Returns true while something is still moving.
    pub fn update(&mut self, scene: &Scene, gfx: &Gfx, theme: &Theme, motion: &Motion, show: &Show, hide: &Spring, dt: f32) -> Result<bool> {
        let s = self.scale;
        let px = |v: f32| (v * s).round();
        if self.fonts.is_none() {
            self.fonts = Some(Fonts {
                text: text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                bold: text::format(gfx, UI_FONT, px(13.0), DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
                small: text::format(gfx, UI_FONT, px(12.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                icons: text::format(gfx, ICON_FONT, px(14.0), DWRITE_FONT_WEIGHT_NORMAL)?,
                icons_big: text::format(gfx, ICON_FONT, px(17.0), DWRITE_FONT_WEIGHT_NORMAL)?,
            });
        }
        let f = self.fonts.clone().unwrap();
        let (fg, dim) = (theme.text, theme.text_dim);
        let mid = |line: &Line, height: f32| ((height - line.h) * 0.5).round();

        // --------------------------------------------------------------- at rest
        let h0 = self.pill_h;
        let mut x = px(12.0);
        self.r_icon.set(scene, gfx, &f.icons, &if show.glyph != 0 { glyph(show.glyph) } else { String::new() }, theme.accent_text(), None)?;
        self.r_text.set(scene, gfx, &f.text, show.text, fg, Some(px(170.0)))?;
        self.r_sub.set(scene, gfx, &f.small, show.sub, dim, Some(px(170.0)))?;
        for line in [&mut self.r_icon, &mut self.r_text, &mut self.r_sub] {
            if line.w > 0.0 {
                if x > px(12.0) {
                    x += px(7.0);
                }
                let y = mid(line, h0);
                line.place(x, y)?;
                x += line.w;
            }
        }
        // A bar that dances while something is going on, like the island's while music plays.
        if show.busy {
            let (bar_w, bar_h) = (px(3.0), px(11.0));
            x += px(8.0);
            self.r_bar.place(x, ((h0 - bar_h) * 0.5).round(), bar_w, bar_h, bar_w * 0.5)?;
            self.r_bar.set_color(theme.accent_color(1.0))?;
            self.r_bar.set_opacity(1.0)?;
            if motion.reduced() {
                self.r_bar.rest()?;
            } else {
                self.r_bar.pulse(0.3, 0.9, 0.0)?;
            }
            x += bar_w;
        } else {
            self.r_bar.rest()?;
            self.r_bar.set_opacity(0.0)?;
        }
        let rest_size = ((x + px(12.0)).max(h0), h0);

        // ------------------------------------------------- a notification's card
        if let Some((first, second, logo)) = &self.notice {
            let ah = h0 + px(24.0);
            self.a_icon.set(scene, gfx, &f.icons_big, &glyph(0xEA8F), theme.accent_text(), None)?;
            self.a_text.set(scene, gfx, &f.bold, first, fg, Some(px(240.0)))?;
            self.a_body.set(scene, gfx, &f.small, second, dim, Some(px(240.0)))?;
            let mut x = px(13.0);
            // The app's own mark takes the bell's place.
            if let Some(art) = logo {
                if self.a_art_of.as_ref() != Some(art) {
                    self.a_art.set_surface(&scene.surface_from_pixels(&art.0)?)?;
                    self.a_art_of = Some(art.clone());
                }
                let side = px(30.0);
                self.a_art.place(x, ((ah - side) * 0.5).round(), side, side)?;
                x += side + px(10.0);
            } else {
                let y = mid(&self.a_icon, ah);
                self.a_icon.place(x, y)?;
                x += self.a_icon.w + px(10.0);
            }
            self.a_art.set_opacity(if logo.is_some() { 1.0 } else { 0.0 })?;
            self.a_icon.set_opacity(if logo.is_some() { 0.0 } else { 1.0 })?;
            if self.a_body.w > 0.0 {
                let low = ah - px(9.0) - self.a_body.h;
                self.a_text.place(x, px(8.0))?;
                self.a_body.place(x, low)?;
            } else {
                let y = mid(&self.a_text, ah);
                self.a_text.place(x, y)?;
            }
            x += self.a_text.w.max(self.a_body.w);
            self.alert_size = (x + px(18.0), ah);
        }
        let alert_size = self.alert_size;

        // ------------------------------------------------------ open: the list
        let (ow, pad, head) = (px(OPEN_W), px(16.0), px(40.0));
        self.l_title.set(scene, gfx, &f.bold, &self.title, fg, Some(ow - 2.0 * pad))?;
        let y = mid(&self.l_title, head) + px(2.0);
        self.l_title.place(pad, y)?;
        while self.looks.len() < self.rows.len() {
            let c = &self.list_content;
            self.looks.push(RowLook { icon: Line::new_in(scene, c)?, text: Line::new_in(scene, c)?, hint: Line::new_in(scene, c)?, mark: Line::new_in(scene, c)? });
        }
        let mut y = head;
        let mut list_zones: Zones = Vec::new();
        for (index, look) in self.looks.iter_mut().enumerate() {
            let Some(row) = self.rows.get(index) else {
                for line in [&mut look.icon, &mut look.text, &mut look.hint, &mut look.mark] {
                    line.set_opacity(0.0)?;
                }
                continue;
            };
            let note = row.kind == RowKind::Note;
            let h = px(if note { 24.0 } else { 34.0 });
            let text_x = if row.glyph != 0 { pad + px(28.0) } else { pad };
            look.icon.set(scene, gfx, &f.icons, &if row.glyph != 0 { glyph(row.glyph) } else { String::new() }, fg, None)?;
            look.mark.set(scene, gfx, &f.icons, &if row.mark != 0 { glyph(row.mark) } else { String::new() }, theme.accent_text(), None)?;
            let mut edge = ow - pad;
            if look.mark.w > 0.0 {
                edge -= look.mark.w;
                let my = y + mid(&look.mark, h);
                look.mark.place(edge, my)?;
                edge -= px(8.0);
            }
            look.hint.set(scene, gfx, &f.small, &row.hint, dim, Some(px(130.0)))?;
            if look.hint.w > 0.0 {
                edge -= look.hint.w;
                let hy = y + mid(&look.hint, h);
                look.hint.place(edge, hy)?;
                edge -= px(10.0);
            }
            look.text.set(scene, gfx, if note { &f.small } else { &f.text }, &row.text, if note { dim } else { fg }, Some((edge - text_x).max(px(40.0))))?;
            let ty = y + mid(&look.text, h);
            look.text.place(text_x, ty)?;
            if !note {
                // A cross at the end is clicked by itself; it comes first, so it is found first.
                if row.close && look.mark.w > 0.0 {
                    list_zones.push((Zone::Close(index), ow - pad - look.mark.w - px(8.0), y, ow - pad + px(6.0), y + h));
                }
                list_zones.push((Zone::Row(index), pad - px(6.0), y, ow - pad + px(6.0), y + h));
            }
            if look.icon.w > 0.0 {
                let iy = y + mid(&look.icon, h);
                look.icon.place(pad + (px(18.0) - look.icon.w) * 0.5, iy)?;
            }
            for line in [&mut look.icon, &mut look.text, &mut look.hint, &mut look.mark] {
                line.set_opacity(1.0)?;
            }
            y += h;
        }
        let list_size = (ow, (y + px(12.0)).min(px(OPEN_H_MOST)));

        // -------------------------------------------------------------- shape
        // A corner with nothing to show cannot be open.
        if !show.there && self.mode == Mode::Open {
            self.mode = Mode::Rest;
        }
        let open = self.mode == Mode::Open;
        let (left, top) = (self.kind.left(), self.kind.top());
        let (tw, th, tr) = match self.mode {
            Mode::Rest => (rest_size.0.round(), rest_size.1, rest_size.1 * 0.5),
            Mode::Alert => (alert_size.0.round(), alert_size.1, alert_size.1 * 0.5),
            Mode::Open => (list_size.0.round(), list_size.1.round(), px(OPEN_RADIUS)),
        };
        // Growing, it overshoots like a drop; shrinking, it just settles.
        if tw != self.w.target || th != self.h.target {
            let growing = tw * th >= self.w.target * self.h.target;
            let tone = if growing { motion.medium() } else { motion.calm() };
            for spring in [&mut self.w, &mut self.h, &mut self.r] {
                spring.tune(tone);
            }
        }
        self.w.set(tw);
        self.h.set(th);
        self.r.set(tr);
        self.w.step(dt);
        self.h.step(dt);
        self.r.step(dt);
        let (w, h) = (self.w.pos.max(8.0), self.h.pos.max(6.0));
        let edge = px(EDGE);
        // The glass grows out of its own corner.
        let (gx, gy) = (if left { edge } else { self.win.0 - edge - w }, if top { self.gap } else { self.win.1 - self.gap - h });

        // Only the open list answers the mouse; its rows are where the glass will come to rest.
        let (zx, zy) = (if left { edge } else { self.win.0 - edge - tw }, if top { self.gap } else { self.win.1 - self.gap - th });
        self.zones.clear();
        if open {
            self.zones.extend(list_zones.iter().map(|(zone, l, t, r, b)| (*zone, zx + l, zy + t, zx + r, zy + b)));
        }

        // The drop of light under the cursor.
        let hot = self.cursor.filter(|_| open).and_then(|(cx, cy)| self.zones.iter().find(|(_, l, t, r, b)| cx >= *l && cx < *r && cy >= *t && cy < *b).copied());
        self.hot_at.tune(motion);
        self.hot_at.aim(hot.map(|(_, l, t, r, b)| (l - zx, t - zy, r - l, b - t)));
        self.hot_at.step(dt);
        let shape = self.hot_at.shape(motion.squash());
        let (bead_top, bead_bottom, bead_edge) = theme.bead;
        self.hot.place(shape.x, shape.y, shape.w, shape.h, px(11.0))?;
        self.hot.set_colors(bead_top, bead_bottom, bead_edge)?;
        self.hot.set_opacity(shape.alpha)?;

        // Out of view: for a full-screen app, or with nothing to show. The top corners
        // leave upwards, the bottom ones downwards.
        self.away.tune(if show.there { motion.medium() } else { motion.calm() });
        self.away.set(if show.there { 0.0 } else { 1.0 });
        self.away.step(dt);
        let out = hide.pos.max(self.away.pos).max(0.0);
        let slide = (out * (h + self.gap + theme.island.shadow_reach + px(8.0))).round() * if top { -1.0 } else { 1.0 };
        self.stage.set_offset(0.0, slide)?;
        if let Some(shade) = self.shade.as_mut() {
            shade.set_offset(0.0, slide)?;
        }
        self.glass.set_rect(scene, gx, gy, w, h, self.r.pos.max(0.0))?;

        // Contents: each fades and swells in, from the corner the glass grows out of.
        let wanted = match self.mode {
            Mode::Rest => [1.0, 0.0, 0.0],
            Mode::Alert => [0.0, 1.0, 0.0],
            Mode::Open => [0.0, 0.0, 1.0],
        };
        for (spring, target) in self.show.iter_mut().zip(wanted) {
            spring.tune(motion.fade());
            spring.set(target);
            spring.step(dt);
        }
        let grow = |shown: f32| if motion.reduced() { 1.0 } else { 0.88 + 0.12 * shown };
        let shown: Vec<f32> = self.show.iter().map(|spring| spring.pos.clamp(0.0, 1.0)).collect();
        let layers: [(&mut Layer, (f32, f32)); 3] = [(&mut self.rest, rest_size), (&mut self.alert, alert_size), (&mut self.list, list_size)];
        for ((layer, size), amount) in layers.into_iter().zip(shown) {
            layer.set_offset(if left { 0.0 } else { (w - size.0).round() }, if top { 0.0 } else { (h - size.1).round() })?;
            layer.set_scale(grow(amount), grow(amount), if left { 0.0 } else { size.0 }, if top { 0.0 } else { size.1 })?;
            layer.set_opacity(amount)?;
        }

        // The glass takes the mouse, and so does the strip between it and the screen's edges:
        // a cursor thrown into the corner lands on it.
        let hidden = out > 0.97;
        let wanted = if hidden {
            (0, 0, 0, 0)
        } else {
            (
                if left { 0 } else { (gx - 2.0).floor() as i32 },
                if top { 0 } else { (gy - 2.0).floor() as i32 },
                if left { (gx + w + 2.0).ceil() as i32 } else { self.win.0 as i32 },
                if top { (gy + h + 2.0).ceil() as i32 } else { self.win.1 as i32 },
            )
        };
        if self.region != Some(wanted) {
            self.region = Some(wanted);
            unsafe {
                // The system takes ownership of the region handle.
                let rgn = CreateRectRgn(wanted.0, wanted.1, wanted.2, wanted.3);
                SetWindowRgn(self.hwnd, Some(rgn), false);
            }
        }

        Ok(!self.w.settled(0.05)
            || !self.h.settled(0.05)
            || !self.r.settled(0.05)
            || !self.away.settled(1e-3)
            || self.show.iter().any(|spring| !spring.settled(1e-3))
            || !self.hot_at.settled())
    }
}

impl Drop for Corner {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
