//! The dock: a capsule of liquid glass along the bottom of the screen, holding
//! the apps. This file is the dock's surface only: its items, how they move
//! and how they are drawn. What a click *does* is decided in `app.rs`.
//!
//! What makes it feel liquid:
//!
//! - icons swell under the cursor (see `layout.rs`), and the glass stretches
//!   with them, overshooting a little as it goes, like the skin of a drop;
//! - a drop of light glides from icon to icon after the cursor, stretching
//!   along its path;
//! - an icon sinks when pressed and springs back, pops in when its app
//!   opens, and flattens against the glass each time it lands from a hop;
//! - an icon swallows the window that is minimised into it: it bulges as the
//!   window lands, and wobbles back.

use crate::config::{self, Config};
use crate::gfx::Gfx;
use crate::layout::{layout, Magnifier, Slot};
use crate::motion::{hop_shape, Glide, Motion};
use crate::scene::{rgba, Bead, Glass, Layer, Pill, Scene, Sprite, Stage};
use crate::shade::Shade;
use crate::shell::{self, AppWindow, IconPixels};
use crate::spring::Spring;
use crate::system::Monitor;
use crate::text;
use crate::theme::Theme;
use std::collections::HashMap;
use windows::core::Result;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::CompositionDrawingSurface;

/// Identity of the dock's own tray button (not an app, never written to pins.txt).
pub const LAUNCHER_KEY: &str = "::launcher";

/// Every size the dock uses, in physical pixels for the current display scale.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    pub scale: f32,
    pub icon: f32,
    /// Width of one icon's slot at rest (icon + gap).
    pub slot: f32,
    pub separator: f32,
    pub pad_x: f32,
    pub pad_bottom: f32,
    pub panel_h: f32,
    pub radius: f32,
    pub margin: f32,
    pub win_w: i32,
    pub win_h: i32,
    /// Y of the icons' bottom edge, in window coordinates.
    pub baseline: f32,
    pub panel_top: f32,
    /// Extra scale at the cursor.
    pub amp: f32,
    pub mag_radius: f32,
    pub bounce_h: f32,
    pub label_h: f32,
    pub label_gap: f32,
    pub font: f32,
    /// Space between the drop of light and the edge of the glass.
    pub lens_inset: f32,
}

pub fn metrics(cfg: &Config, scale: f32, monitor_w: i32) -> Metrics {
    let px = |v: f32| (v * scale).round();
    let icon = px(cfg.icon_size);
    let gap = px(cfg.gap);
    let pad_top = px(cfg.padding);
    // The running mark lives in the bottom padding, so it gets a little more room.
    let pad_bottom = px(cfg.padding + if cfg.indicator { 3.0 } else { 0.0 });
    let panel_h = icon + pad_top + pad_bottom;
    let margin = px(cfg.margin_bottom);
    let amp = cfg.magnification - 1.0;
    let overflow = icon * amp;
    let bounce_h = (icon * cfg.bounce_height).round();
    let label_h = px(cfg.label_size * 2.0);
    let label_gap = px(10.0);
    let headroom = overflow + bounce_h.max(label_gap + label_h) + px(14.0);
    let win_h = (margin + panel_h + headroom).ceil();
    Metrics {
        scale,
        icon,
        slot: icon + gap,
        separator: px(13.0),
        pad_x: (px(cfg.padding + 1.0) - gap * 0.5).max(0.0),
        pad_bottom,
        panel_h,
        radius: (panel_h * cfg.corner_radius / 100.0).round(),
        margin,
        win_w: monitor_w,
        win_h: win_h as i32,
        baseline: win_h - margin - pad_bottom,
        panel_top: win_h - margin - panel_h,
        amp,
        mag_radius: cfg.spread * (icon + gap),
        bounce_h,
        label_h,
        label_gap,
        font: cfg.label_size * scale,
        lens_inset: (px(cfg.padding) * 0.42).round().max(px(2.0)),
    }
}

/// Pixel sizes each icon is pre-scaled to: at rest, fully magnified, and steps in between so
/// that no size is ever drawn from a bitmap more than about 1.4x larger than it.
pub fn icon_levels(m: &Metrics) -> Vec<u32> {
    let rest = m.icon.round().max(1.0);
    let max = (m.icon * (1.0 + m.amp)).ceil().max(rest);
    let mut levels = vec![rest as u32];
    let steps = ((max / rest).ln() / 1.4f32.ln()).ceil().max(1.0) as i32;
    for i in 1..=steps {
        let size = (rest * (max / rest).powf(i as f32 / steps as f32)).ceil() as u32;
        if size > *levels.last().unwrap() {
            levels.push(size);
        }
    }
    levels
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    App,
    Separator,
}

pub struct Bounce {
    t: f32,
    /// `Some(n)`: stop after n hops. `None`: keep going until the app's window shows up.
    hops: Option<u32>,
}

/// An app's name, painted once.
struct Label {
    surface: CompositionDrawingSurface,
    width: f32,
    height: f32,
}

pub struct Item {
    pub kind: Kind,
    pub key: String,
    pub target: String,
    pub name: String,
    pub pinned: bool,
    /// Open windows, front-most first.
    pub windows: Vec<HWND>,
    /// The icon pre-scaled to a few sizes between "at rest" and "fully magnified", smallest first.
    /// Showing the nearest size up keeps the icon sharp at every size, and pixel-exact at rest.
    /// The full-size picture they are made from is read when needed and not kept.
    surfaces: Vec<(f32, CompositionDrawingSurface)>,
    /// Which of `surfaces` the sprite currently shows.
    level: Option<usize>,
    sprite: Option<Sprite>,
    /// Running mark for apps, the divider line for the separator.
    mark: Option<Pill>,
    label: Option<Label>,
    /// 0 = absent, 1 = fully in the dock. Drives the pop-in and the slide-out.
    presence: Spring,
    /// Running mark visibility.
    running: Spring,
    /// 1 while this app owns the foreground window.
    active: Spring,
    /// 1 while the mouse button is held on the icon.
    press: Spring,
    /// 1 while the icon is being dragged to a new place.
    lift: Spring,
    /// Sideways offset from the icon's slot, in pixels. Non-zero while the icon is being dragged
    /// to a new place, and while a neighbour glides over to make room.
    shift: Spring,
    pub bounce: Option<Bounce>,
    bounce_y: f32,
    /// Width and height factors from hopping (flattened on landing, long in flight).
    hop: (f32, f32),
    /// A window landing in the icon makes it bulge like a drop that swallows something: wider
    /// and flatter for a moment (above zero), then back with a wobble. Below zero it is
    /// taller and thinner: the icon letting a window go.
    gulp: Spring,
    /// A gulp still to come: seconds to go, and how strong (below zero: letting go).
    gulp_in: Option<(f32, f32)>,
}

impl Item {
    pub fn app(key: String, target: String, pinned: bool, present: bool, motion: &Motion) -> Self {
        let name = shell::display_name(&target);
        let mut item = Item::blank(Kind::App, motion);
        item.presence.snap(if present { 1.0 } else { 0.0 });
        item.presence.set(1.0);
        Item { key, target, name, pinned, ..item }
    }

    /// The dock's own button that opens the app tray.
    fn launcher_button(motion: &Motion) -> Self {
        let mut item = Item::blank(Kind::App, motion);
        item.presence.snap(1.0);
        Item { key: LAUNCHER_KEY.to_string(), target: LAUNCHER_KEY.to_string(), name: "Ứng dụng".to_string(), pinned: true, ..item }
    }

    fn separator(motion: &Motion) -> Self {
        Item::blank(Kind::Separator, motion)
    }

    fn blank(kind: Kind, motion: &Motion) -> Self {
        Item {
            kind,
            key: String::new(),
            target: String::new(),
            name: String::new(),
            pinned: false,
            windows: Vec::new(),
            surfaces: Vec::new(),
            level: None,
            sprite: None,
            mark: None,
            label: None,
            presence: Spring::with(0.0, motion.medium()),
            running: Spring::with(0.0, motion.small()),
            active: Spring::with(0.0, motion.small()),
            press: Spring::with(0.0, motion.tiny()),
            lift: Spring::with(0.0, motion.small()),
            shift: Spring::with(0.0, motion.follow()),
            bounce: None,
            bounce_y: 0.0,
            hop: (1.0, 1.0),
            gulp: Spring::with(0.0, motion.jelly()),
            gulp_in: None,
        }
    }

    pub fn is_special(&self) -> bool {
        self.key.starts_with("::")
    }

    pub fn is_app(&self) -> bool {
        self.kind == Kind::App
    }

    /// Makes the icon hop: `Some(n)` times, or until its app has a window.
    pub fn hop(&mut self, hops: Option<u32>) {
        if self.bounce.is_none() {
            self.bounce = Some(Bounce { t: 0.0, hops });
        }
    }

    /// Makes the icon gulp, `delay` seconds from now: it takes a window in (`strength` above
    /// zero) or lets one go (below zero).
    pub fn gulp(&mut self, delay: f32, strength: f32) {
        self.gulp_in = Some((delay.max(0.0), strength));
    }

    fn settled(&self) -> bool {
        self.presence.settled(1e-3) && self.running.settled(1e-3) && self.active.settled(1e-3) && self.press.settled(1e-3) && self.lift.settled(1e-3) && self.shift.settled(0.05) && self.bounce.is_none() && self.gulp_in.is_none() && self.gulp.settled(1e-3)
    }

    /// The picture the icon's sizes are made from.
    fn pixels(&self, accent: (f32, f32, f32)) -> IconPixels {
        if self.key == LAUNCHER_KEY {
            return shell::launcher_icon(256, accent);
        }
        shell::load_icon(&self.target, 256).unwrap_or_else(|| {
            crate::log!("no icon for '{}'; using a plain tile", self.target);
            shell::placeholder_icon(96)
        })
    }
}

/// The name tag above the icon under the cursor.
struct Tag {
    glass: Glass,
    text: Sprite,
    /// Item whose name the sprite shows.
    shown: Option<String>,
}

/// What letting go of the mouse button over the dock meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Released {
    /// A click on this icon.
    Click(usize),
    /// The end of a drag that changed the order of the pinned apps (now saved).
    Reordered,
    Nothing,
}

pub struct Dock {
    hwnd: HWND,
    /// The window was made for this dock and goes when it goes (docks on other screens).
    owned: bool,
    stage: Stage,
    /// The window under this one that shows the glass's shadow.
    shade: Option<Shade>,
    glass: Glass,
    lens: Bead,
    lens_at: Glide,
    /// Icons, running marks and the divider, in window coordinates.
    layer: Layer,
    tag: Option<Tag>,
    text_format: Option<IDWriteTextFormat>,
    pub m: Metrics,
    pub items: Vec<Item>,
    widths: Vec<f32>,
    slots: Vec<Slot>,
    /// The glass as shown this frame: left, top, width, height.
    panel: (f32, f32, f32, f32),
    /// The ends of the glass: they chase the icons and overshoot.
    edge_left: Spring,
    edge_right: Spring,
    /// 1 while the cursor is over the dock: the glass thickens a touch.
    swell: Spring,
    /// 0 = flat, 1 = fully magnified.
    mag: Spring,
    cursor_x: f32,
    pub hovering: bool,
    pub hovered: Option<usize>,
    pub pressed: Option<usize>,
    label_alpha: Spring,
    label_index: Option<usize>,
    /// Set while an icon is being dragged to a new place: how far from the icon's centre it was grabbed.
    drag: Option<f32>,
    /// Where the mouse button went down, in window pixels.
    press_x: f32,
    pub win_x: i32,
    pub win_y: i32,
    region: Option<(i32, i32, i32, i32)>,
    accent: (f32, f32, f32),
}

impl Dock {
    pub fn new(hwnd: HWND, owned: bool, scene: &Scene, cfg: &Config, theme: &Theme, motion: &Motion, monitor: &Monitor) -> Result<Self> {
        let m = metrics(cfg, monitor.scale(), monitor.rect.right - monitor.rect.left);
        let stage = scene.stage(hwnd)?;
        let mut glass = scene.glass_in(&stage.content()?, &theme.dock)?;
        // The dock's window is cut down to the glass, so its shadow needs a window of its own.
        let shade = match Shade::new(scene).and_then(|shade| glass.cast_shadow_on(&shade.content()?).map(|_| shade)) {
            Ok(shade) => Some(shade),
            Err(e) => {
                crate::log!("no window for the dock's shadow ({e}); the dock goes without one");
                glass.without_shadow();
                None
            }
        };
        let lens = scene.bead_in(&glass.inner()?)?;
        let layer = scene.layer_in(&stage.content()?)?;
        let mut dock = Dock {
            hwnd,
            owned,
            stage,
            shade,
            glass,
            lens,
            lens_at: Glide::new(motion),
            layer,
            tag: None,
            text_format: None,
            m,
            items: Vec::new(),
            widths: Vec::new(),
            slots: Vec::new(),
            panel: (m.win_w as f32 * 0.5, m.panel_top, 0.0, m.panel_h),
            edge_left: Spring::with(m.win_w as f32 * 0.5, motion.medium()),
            edge_right: Spring::with(m.win_w as f32 * 0.5, motion.medium()),
            swell: Spring::with(0.0, motion.small()),
            mag: Spring::with(0.0, motion.small()),
            cursor_x: m.win_w as f32 * 0.5,
            hovering: false,
            hovered: None,
            pressed: None,
            label_alpha: Spring::with(0.0, motion.fade()),
            label_index: None,
            drag: None,
            press_x: 0.0,
            win_x: 0,
            win_y: 0,
            region: None,
            accent: theme.accent,
        };
        dock.load_pins(cfg, motion);
        Ok(dock)
    }

    fn load_pins(&mut self, cfg: &Config, motion: &Motion) {
        let pins = config::load_pins().unwrap_or_else(|| {
            // First run: start with File Explorer so the dock is not empty.
            let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
            let pins = vec![format!("{windir}\\explorer.exe")];
            config::save_pins(&pins);
            pins
        });
        if cfg.launcher {
            self.items.push(Item::launcher_button(motion));
        }
        for target in pins {
            let key = shell::key_for_target(&target);
            if self.items.iter().any(|i| i.key == key) {
                continue;
            }
            self.items.push(Item::app(key, target, true, true, motion));
        }
        self.items.push(Item::separator(motion));
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Makes the pinned apps, and their order, match `pins` (as just saved by the dock of
    /// another screen). Apps that are no longer pinned stay only while they have a window.
    pub fn adopt_pins(&mut self, pins: &[String], motion: &Motion) {
        let keys: Vec<String> = pins.iter().map(|target| shell::key_for_target(target)).collect();
        for item in self.items.iter_mut().filter(|i| i.is_app() && !i.is_special()) {
            if item.pinned && !keys.contains(&item.key) {
                item.pinned = false;
            }
        }
        let mut pinned = Vec::new();
        for (key, target) in keys.iter().zip(pins) {
            match self.items.iter().position(|i| i.is_app() && &i.key == key) {
                Some(at) => {
                    let mut item = self.items.remove(at);
                    item.pinned = true;
                    pinned.push(item);
                }
                None => pinned.push(Item::app(key.clone(), target.clone(), true, false, motion)),
            }
        }
        // The tray button, the pinned apps in their order, the divider, then whatever else is open.
        let mut rest = std::mem::take(&mut self.items);
        let button = rest.iter().position(|i| i.key == LAUNCHER_KEY).map(|at| rest.remove(at));
        let divider = rest.iter().position(|i| i.kind == Kind::Separator).map(|at| rest.remove(at)).unwrap_or_else(|| Item::separator(motion));
        self.items.extend(button);
        self.items.extend(pinned);
        self.items.push(divider);
        self.items.extend(rest);
        self.hovered = None;
        self.pressed = None;
        self.label_index = None;
        self.drag = None;
    }

    pub fn save_pins(&self) {
        let pins: Vec<String> = self.items.iter().filter(|i| i.is_app() && i.pinned && !i.is_special()).map(|i| i.target.clone()).collect();
        config::save_pins(&pins);
    }

    /// Adds or removes the tray button to match the settings. Returns true if it changed.
    pub fn set_launcher_button(&mut self, wanted: bool, motion: &Motion) -> bool {
        let has = self.items.iter().any(|i| i.key == LAUNCHER_KEY);
        if wanted == has {
            return false;
        }
        if wanted {
            self.items.insert(0, Item::launcher_button(motion));
        } else {
            self.items.retain(|i| i.key != LAUNCHER_KEY);
        }
        self.pressed = None;
        self.hovered = None;
        self.label_index = None;
        self.drag = None;
        true
    }

    // ------------------------------------------------------------- placement

    /// Sizes and positions the window on `monitor`, its bottom edge at screen row `bottom`.
    pub fn place(&mut self, cfg: &Config, monitor: &Monitor, bottom: i32) {
        let old = self.m;
        self.m = metrics(cfg, monitor.scale(), monitor.rect.right - monitor.rect.left);
        let m = self.m;
        self.win_x = monitor.rect.left;
        self.win_y = bottom - m.win_h;
        unsafe {
            if let Err(e) = SetWindowPos(self.hwnd, Some(HWND_TOPMOST), self.win_x, self.win_y, m.win_w, m.win_h, SWP_NOACTIVATE) {
                crate::log!("SetWindowPos failed: {e}");
            }
        }
        if let Some(shade) = self.shade.as_mut() {
            shade.place(self.hwnd, self.win_x, self.win_y, m.win_w, m.win_h);
        }
        // Text and pre-scaled icons are made for specific sizes: redo them only if those changed.
        if old.font != m.font {
            self.forget_labels();
        }
        if icon_levels(&old) != icon_levels(&m) {
            self.forget_icons();
        }
        self.region = None;
        if old.win_w != m.win_w {
            self.cursor_x = m.win_w as f32 * 0.5;
            self.edge_left.snap(m.win_w as f32 * 0.5);
            self.edge_right.snap(m.win_w as f32 * 0.5);
        }
    }

    pub fn set_theme(&mut self, theme: &Theme) {
        self.glass.set_material(&theme.dock);
        if let Some(tag) = self.tag.as_mut() {
            tag.glass.set_material(&theme.bubble);
        }
        if self.accent != theme.accent {
            self.accent = theme.accent;
            // The tray button is drawn in the accent colour.
            for item in self.items.iter_mut().filter(|i| i.key == LAUNCHER_KEY) {
                item.surfaces.clear();
                item.level = None;
            }
        }
        self.forget_labels();
    }

    /// Throws away painted app names (they are painted for one font size and one theme).
    pub fn forget_labels(&mut self) {
        self.text_format = None;
        if let Some(tag) = self.tag.as_mut() {
            tag.shown = None;
        }
        for item in &mut self.items {
            item.label = None;
        }
    }

    /// Throws away painted icons (after a size change or a GPU reset); they are painted again when shown.
    pub fn forget_icons(&mut self) {
        for item in &mut self.items {
            item.surfaces.clear();
            item.level = None;
        }
    }

    // ----------------------------------------------------------------- items

    /// Brings the items up to date with the open windows. `front` is the window in front.
    pub fn sync(&mut self, windows: Vec<AppWindow>, front: HWND, motion: &Motion) {
        for item in &mut self.items {
            item.windows.clear();
        }
        for w in windows {
            match self.items.iter_mut().find(|i| i.is_app() && i.key == w.key) {
                Some(item) => item.windows.push(w.hwnd),
                None => {
                    let mut item = Item::app(w.key, w.target, false, false, motion);
                    item.windows.push(w.hwnd);
                    self.items.push(item);
                }
            }
        }
        let mut pinned = 0;
        let mut unpinned = 0;
        for item in self.items.iter_mut().filter(|i| i.is_app()) {
            let running = !item.windows.is_empty();
            item.running.set(if running { 1.0 } else { 0.0 });
            item.active.set(if item.windows.contains(&front) { 1.0 } else { 0.0 });
            let stays = item.pinned || running || item.bounce.is_some();
            item.presence.set(if stays { 1.0 } else { 0.0 });
            if item.pinned {
                pinned += 1;
            } else if stays {
                unpinned += 1;
            }
        }
        if let Some(sep) = self.items.iter_mut().find(|i| i.kind == Kind::Separator) {
            sep.presence.set(if pinned > 0 && unpinned > 0 { 1.0 } else { 0.0 });
        }
    }

    pub fn toggle_pin(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        let mut item = self.items.remove(index);
        item.pinned = !item.pinned;
        if item.pinned {
            // Pinned apps live to the left of the separator.
            let sep = self.items.iter().position(|i| i.kind == Kind::Separator).unwrap_or(self.items.len());
            self.items.insert(sep, item);
        } else {
            self.items.push(item);
        }
        self.hovered = None;
        self.pressed = None;
        self.label_index = None;
        self.save_pins();
    }

    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.items.iter().position(|i| i.key == key)
    }

    /// The item that owns a window.
    pub fn owner_of(&self, window: HWND) -> Option<usize> {
        self.items.iter().position(|i| i.windows.contains(&window))
    }

    // ----------------------------------------------------------------- input

    /// Index of the app icon at a point in window coordinates.
    pub fn app_at(&self, x: f32, y: f32) -> Option<usize> {
        let index = self.slots.iter().position(|s| x >= s.left && x < s.right)?;
        let item = self.items.get(index)?;
        if !item.is_app() || item.presence.target < 0.5 {
            return None;
        }
        let (_, top, _, _) = self.icon_rect(index);
        (y >= top.min(self.m.panel_top) - 2.0).then_some(index)
    }

    /// (left, top, right, bottom) of an item's icon as shown this frame, in window coordinates.
    pub fn icon_rect(&self, index: usize) -> (f32, f32, f32, f32) {
        match (self.slots.get(index), self.items.get(index)) {
            (Some(&slot), Some(item)) => icon_rect_of(&self.m, slot, item),
            _ => (0.0, 0.0, 0.0, 0.0),
        }
    }

    /// Where an icon's centre is on the screen, and the screen row of the icons' bottom edge.
    pub fn icon_anchor(&self, index: usize) -> (i32, i32) {
        let (l, _, r, _) = self.icon_rect(index);
        (self.win_x + ((l + r) * 0.5).round() as i32, self.win_y + self.m.baseline.round() as i32)
    }

    /// Adds where each open window's icon stands when the dock is at rest (left, top, right,
    /// bottom in screen pixels). This is what Windows is told when it asks where a window goes
    /// as it is minimised, so its own animation flies to the right icon.
    pub fn min_rects(&self, out: &mut HashMap<isize, [i16; 4]>) {
        let m = self.m;
        let widths: Vec<f32> = self
            .items
            .iter()
            .map(|item| match (item.presence.target > 0.5, item.is_app()) {
                (false, _) => 0.0,
                (true, true) => m.slot,
                (true, false) => m.separator,
            })
            .collect();
        let mut slots = Vec::new();
        layout(&widths, m.win_w as f32 * 0.5, &Magnifier { amp: 0.0, radius: m.mag_radius, center: 0.0 }, &mut slots);
        let short = |v: f32| v.round().clamp(-32000.0, 32000.0) as i16;
        let bottom = self.win_y as f32 + m.baseline;
        for (item, slot) in self.items.iter().zip(&slots) {
            if !item.is_app() || item.windows.is_empty() {
                continue;
            }
            let centre = self.win_x as f32 + slot.center();
            let rect = [short(centre - m.icon * 0.5), short(bottom - m.icon), short(centre + m.icon * 0.5), short(bottom)];
            for window in &item.windows {
                out.insert(window.0 as isize, rect);
            }
        }
    }

    /// The screen row just above the tallest an icon can get: where things that stand on the
    /// dock (menus, previews) put their feet.
    pub fn top_line(&self) -> i32 {
        let m = self.m;
        self.win_y + (m.baseline - m.icon * (1.0 + m.amp) - 10.0 * m.scale).round() as i32
    }

    /// The mouse button went down at a point in window coordinates.
    pub fn press(&mut self, x: f32, y: f32) -> Option<usize> {
        self.pressed = self.app_at(x, y);
        self.press_x = x;
        self.drag = None;
        self.pressed
    }

    /// The mouse button came up: a click, the end of a drag, or a release somewhere else.
    pub fn release(&mut self, x: f32, y: f32) -> Released {
        let Some(index) = self.pressed.take() else { return Released::Nothing };
        if self.drag.take().is_some() {
            // Dropped after dragging: keep the new order, do not treat it as a click.
            if self.items.get(index).map(|i| i.pinned).unwrap_or(false) {
                self.save_pins();
                return Released::Reordered;
            }
            return Released::Nothing;
        }
        if self.app_at(x, y) == Some(index) {
            Released::Click(index)
        } else {
            Released::Nothing
        }
    }

    /// Whether the cursor (window coordinates) is over the dock: the glass, the strip below it,
    /// or the part of a magnified icon that sticks out above the glass.
    fn cursor_in_dock(&self, cx: f32, cy: f32) -> bool {
        let (px, py, pw, _) = self.panel;
        if self.slots.is_empty() || cx < px - 2.0 || cx > px + pw + 2.0 || cy >= self.m.win_h as f32 {
            return false;
        }
        let mut top = py;
        if let Some(index) = self.slots.iter().position(|s| cx >= s.left && cx < s.right) {
            if self.items.get(index).map(|i| i.is_app()).unwrap_or(false) {
                top = top.min(self.icon_rect(index).1);
            }
        }
        cy >= top - 1.0
    }

    // ---------------------------------------------------------------- update

    /// Advances all motion by `dt` seconds and updates the visuals.
    ///
    /// `cursor`: where the cursor is, in window coordinates. `hide`: 0 = shown, 1 = tucked
    /// below the screen edge. `quiet`: the name tag stays away (a preview is open).
    /// Returns true while something is still moving (another frame is needed).
    pub fn update(&mut self, scene: &Scene, gfx: &Gfx, cfg: &Config, theme: &Theme, motion: &Motion, cursor: Option<(f32, f32)>, hide: &Spring, quiet: bool, dt: f32) -> Result<bool> {
        let m = self.m;
        let (cx, cy) = cursor.unwrap_or((f32::NAN, f32::NAN));
        let shown = hide.target == 0.0;
        self.hovering = cursor.is_some() && shown && self.cursor_in_dock(cx, cy);
        if self.hovering {
            self.cursor_x = cx;
        }
        self.mag.tune(motion.small());
        self.swell.tune(motion.small());
        self.edge_left.tune(motion.medium());
        self.edge_right.tune(motion.medium());
        self.label_alpha.tune(motion.fade());
        self.lens_at.tune(motion);
        self.mag.set(if self.hovering { 1.0 } else { 0.0 });
        self.swell.set(if self.hovering { 1.0 } else { 0.0 });
        self.mag.step(dt);
        self.swell.step(dt);

        // Per-item motion.
        let squash = motion.squash();
        let bounce_h = m.bounce_h;
        let hop = cfg.bounce_time.max(0.05);
        let dragging = self.drag.is_some();
        for (index, item) in self.items.iter_mut().enumerate() {
            // Arriving, an icon pops in with a little overshoot; leaving, it just goes.
            item.presence.tune(if item.presence.target > 0.5 { motion.medium() } else { motion.calm() });
            item.running.tune(motion.small());
            item.active.tune(motion.small());
            item.press.tune(motion.tiny());
            item.lift.tune(motion.small());
            item.shift.tune(motion.follow());
            item.presence.step(dt);
            item.running.step(dt);
            item.active.step(dt);
            let held = self.pressed == Some(index);
            item.press.set(if held && !dragging { 1.0 } else { 0.0 });
            item.press.step(dt);
            item.lift.set(if held && dragging { 1.0 } else { 0.0 });
            item.lift.step(dt);
            if !(held && dragging) {
                item.shift.step(dt);
            }
            let jelly = motion.jelly();
            item.gulp.tune(jelly);
            if let Some((left, strength)) = item.gulp_in {
                if left > dt {
                    item.gulp_in = Some((left - dt, strength));
                } else {
                    item.gulp_in = None;
                    if !motion.reduced() {
                        // A push, not a place to go to: the spring swings out about a third
                        // of the way to 1 and comes back by itself.
                        item.gulp.vel += 0.58 * jelly.omega * strength;
                    }
                }
            }
            item.gulp.step(dt);
            if let Some(b) = &mut item.bounce {
                let before = (b.t / hop).floor();
                b.t += dt;
                let after = (b.t / hop).floor();
                let mut done = false;
                if after > before {
                    // Only stop when the icon is back on the ground.
                    done = match b.hops {
                        Some(n) => after as u32 >= n,
                        None => !item.windows.is_empty() || b.t > 10.0,
                    };
                }
                if done {
                    item.bounce = None;
                    item.bounce_y = 0.0;
                    item.hop = (1.0, 1.0);
                } else {
                    let f = (b.t / hop).fract();
                    item.bounce_y = bounce_h * 4.0 * f * (1.0 - f);
                    item.hop = hop_shape(f, squash);
                }
            }
        }

        // Drop apps that have finished sliding out.
        let before = self.items.len();
        self.items.retain(|item| !(item.is_app() && !item.pinned && item.presence.target == 0.0 && item.presence.pos < 0.004));
        let removed = self.items.len() != before;
        if removed {
            self.pressed = None;
            self.label_index = None;
        }

        // Dragging an icon sideways moves it to a new place among its neighbours.
        let moved = self.drag_icon(cx);

        // Layout: rest widths, then the magnifier stretches them around the cursor.
        self.widths.clear();
        for item in &self.items {
            let base = if item.is_app() { m.slot } else { m.separator };
            self.widths.push(base * item.presence.pos.clamp(0.0, 1.0));
        }
        let magnifier = Magnifier { amp: m.amp * self.mag.pos.clamp(0.0, 1.05), radius: m.mag_radius, center: self.cursor_x };
        layout(&self.widths, m.win_w as f32 * 0.5, &magnifier, &mut self.slots);

        // An icon that just changed places starts from where it was and glides to its new slot.
        for (index, was_at) in moved {
            if let (Some(slot), Some(item)) = (self.slots.get(index), self.items.get_mut(index)) {
                item.shift.pos = was_at - slot.center();
                item.shift.vel = 0.0;
            }
        }
        // The dragged icon itself stays under the cursor.
        if let (Some(index), Some(grab)) = (self.pressed, self.drag) {
            if let (Some(slot), Some(item)) = (self.slots.get(index), self.items.get_mut(index)) {
                item.shift.pos = (cx - grab) - slot.center();
                item.shift.vel = 0.0;
            }
        }

        self.hovered = if self.hovering && self.drag.is_none() { self.app_at(cx, cy) } else { None };
        if let Some(h) = self.hovered {
            self.label_index = Some(h);
        }
        self.label_alpha.set(if self.hovered.is_some() && cfg.labels && !quiet { 1.0 } else { 0.0 });
        self.label_alpha.step(dt);

        // The glass hugs the icons. Its ends are on springs: growing, they keep up with the
        // icons and then swing a little past them; shrinking, they trail behind. They never
        // cut into the icons.
        let (left, right) = match (self.slots.first(), self.slots.last()) {
            (Some(a), Some(b)) => (a.left - m.pad_x, b.right + m.pad_x),
            _ => (m.win_w as f32 * 0.5, m.win_w as f32 * 0.5),
        };
        self.edge_left.set(left);
        self.edge_right.set(right);
        self.edge_left.step(dt);
        self.edge_right.step(dt);
        let (shown_left, shown_right) = (self.edge_left.pos.min(left), self.edge_right.pos.max(right));
        let thicken = self.swell.pos.clamp(0.0, 1.2) * 1.5 * m.scale * squash;
        self.panel = (shown_left, m.panel_top - thicken, shown_right - shown_left, m.panel_h + thicken * 2.0);
        let (px, py, pw, ph) = self.panel;
        let radius = (m.radius + thicken).min(ph * 0.5);

        // The drop of light sits under the icon the cursor is on.
        let lens_target = self.hovered.filter(|_| cfg.lens).and_then(|i| self.slots.get(i).copied()).map(|slot| {
            let inset = m.lens_inset;
            let trim = (m.slot - m.icon) * 0.5 * slot.scale - inset;
            (slot.left + trim - px, inset, (slot.right - slot.left) - 2.0 * trim, ph - 2.0 * inset)
        });
        self.lens_at.aim(lens_target);
        self.lens_at.step(dt);

        // Everything below goes to the compositor as one batch: glass, icons, marks and the
        // name tag always change in the same frame.
        let slide = (hide.pos.max(0.0) * (m.panel_h + m.margin + theme.dock.shadow_reach + 6.0 * m.scale)).round();
        self.stage.set_offset(0.0, slide)?;
        if let Some(shade) = self.shade.as_mut() {
            shade.set_offset(0.0, slide)?;
        }
        self.glass.set_rect(scene, px, py, pw, ph, radius)?;
        self.show_lens(theme, squash, radius)?;
        self.show_items(scene, cfg, theme)?;
        self.show_label(scene, gfx, theme, motion)?;
        self.update_region(hide);

        // The cursor resting on the dock is no reason for another frame: when it moves, the
        // window is told, and that wakes the loop.
        Ok(removed
            || self.pressed.is_some()
            || !self.mag.settled(1e-3)
            || !self.swell.settled(1e-3)
            || !self.edge_left.settled(0.05)
            || !self.edge_right.settled(0.05)
            || !self.label_alpha.settled(1e-3)
            || !self.lens_at.settled()
            || self.items.iter().any(|i| !i.settled()))
    }

    /// True right after an item was dropped from the dock, so the app re-reads the windows.
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// Handles dragging an icon sideways. Returns the icons that changed places this frame, as
    /// (new index, where its centre was on screen), so they can glide instead of jumping.
    fn drag_icon(&mut self, cx: f32) -> Vec<(usize, f32)> {
        let mut moved = Vec::new();
        let Some(mut index) = self.pressed else { return moved };
        if index >= self.items.len() || index >= self.slots.len() || !cx.is_finite() {
            return moved;
        }
        if self.drag.is_none() {
            // A press turns into a drag once the cursor has clearly moved.
            if (cx - self.press_x).abs() < 6.0 * self.m.scale {
                return moved;
            }
            if self.items[index].is_special() {
                return moved;
            }
            self.drag = Some(self.press_x - self.slots[index].center());
        }
        let grab = self.drag.unwrap_or(0.0);
        let centre = cx - grab;
        // Swap with a neighbour each time the dragged icon passes its middle. Pinned apps stay
        // among pinned apps, open ones among open ones.
        loop {
            let pinned = self.items[index].pinned;
            let same_group = |item: &Item| item.is_app() && !item.is_special() && item.pinned == pinned && item.presence.target > 0.5;
            let right = index + 1 < self.items.len() && index + 1 < self.slots.len() && same_group(&self.items[index + 1]) && centre > self.slots[index + 1].center();
            let left = index > 0 && same_group(&self.items[index - 1]) && centre < self.slots[index - 1].center();
            let other = if right {
                index + 1
            } else if left {
                index - 1
            } else {
                break;
            };
            // The neighbour takes the dragged icon's slot; remember where it was drawn.
            let was_at = self.slots[other].center() + self.items[other].shift.pos;
            self.items.swap(index, other);
            self.slots.swap(index, other);
            moved.push((index, was_at));
            index = other;
            self.pressed = Some(index);
            if moved.len() > self.items.len() {
                break;
            }
        }
        if !moved.is_empty() {
            self.label_index = None;
        }
        moved
    }

    /// Limits which part of the window takes mouse input. At rest only the glass does, so clicks
    /// just above the dock reach the windows behind it. While icons are magnified or hopping the
    /// area grows to cover them.
    fn update_region(&mut self, hide: &Spring) {
        let m = self.m;
        let hidden = hide.target == 1.0 && hide.pos > 0.97;
        let lively = self.mag.pos > 0.001 || self.label_alpha.pos > 0.001 || self.items.iter().any(|i| i.bounce.is_some());
        let (px, _, pw, _) = self.panel;
        let wanted = if hidden {
            (0, 0, 0, 0)
        } else if lively {
            let grow = m.amp * m.mag_radius * 0.5 + 24.0 * m.scale;
            let rest_left = m.win_w as f32 * 0.5 - self.widths.iter().sum::<f32>() * 0.5 - m.pad_x;
            let rest_right = m.win_w as f32 - rest_left;
            ((rest_left - grow).floor() as i32, 0, (rest_right + grow).ceil() as i32, m.win_h)
        } else {
            ((px - 2.0).floor() as i32, (m.panel_top - 2.0).floor() as i32, (px + pw + 2.0).ceil() as i32, m.win_h)
        };
        if self.region == Some(wanted) {
            return;
        }
        self.region = Some(wanted);
        unsafe {
            // The system takes ownership of the region handle.
            let rgn = CreateRectRgn(wanted.0, wanted.1, wanted.2, wanted.3);
            SetWindowRgn(self.hwnd, Some(rgn), false);
        }
    }

    /// Where the glass is on the screen: left, top, right, bottom.
    pub fn panel_on_screen(&self) -> RECT {
        let (x, y, w, h) = self.panel;
        RECT { left: self.win_x + x.floor() as i32, top: self.win_y + y.floor() as i32, right: self.win_x + (x + w).ceil() as i32, bottom: self.win_y + (y + h).ceil() as i32 }
    }

    /// For the self-test: a point that sweeps left and right across the icons.
    pub fn sweep_point(&self, t: f32) -> (f32, f32) {
        let (px, _, pw, _) = self.panel;
        let centre = px + pw * 0.5;
        let reach = (pw * 0.5 - self.m.icon * 0.5).max(1.0);
        (centre + reach * (t * 2.2).sin(), self.m.panel_top + self.m.panel_h * 0.5)
    }

    // --------------------------------------------------------------- visuals

    fn show_lens(&mut self, theme: &Theme, squash: f32, radius: f32) -> Result<()> {
        if !self.lens_at.visible() {
            return self.lens.set_opacity(0.0);
        }
        let s = self.lens_at.shape(squash);
        // Pressing the icon squeezes the drop under it.
        let press = self.pressed.and_then(|i| self.items.get(i)).map(|i| i.press.pos.clamp(0.0, 1.0)).unwrap_or(0.0);
        let squeeze = 1.0 - 0.07 * press;
        let (w, h) = (s.w * squeeze, s.h * squeeze);
        let (x, y) = (s.x + (s.w - w) * 0.5, s.y + (s.h - h) * 0.5);
        // Concentric with the glass it lies in.
        let r = (radius - self.m.lens_inset).max(4.0 * self.m.scale).min(h * 0.5);
        self.lens.place(x, y, w, h, r)?;
        let (top, bottom, edge) = theme.bead;
        self.lens.set_colors(top, bottom, edge)?;
        self.lens.set_opacity(s.alpha)
    }

    /// Positions every icon, running mark and the divider for this frame.
    fn show_items(&mut self, scene: &Scene, cfg: &Config, theme: &Theme) -> Result<()> {
        let m = self.m;
        let (ar, ag, ab) = theme.accent;
        let neutral = if theme.dark { (1.0, 1.0, 1.0, 0.74) } else { (0.0, 0.0, 0.0, 0.55) };
        let levels = icon_levels(&m);
        let show_indicator = cfg.indicator;
        let accent = self.accent;
        let content = self.layer.content()?;
        let Dock { items, slots, .. } = self;

        for (index, item) in items.iter_mut().enumerate() {
            let Some(&slot) = slots.get(index) else { break };
            let presence = item.presence.pos.clamp(0.0, 1.0);
            let cx = slot.center();

            if item.kind == Kind::Separator {
                if item.mark.is_none() {
                    item.mark = Some(scene.pill_in(&content)?);
                }
                let line = item.mark.as_mut().unwrap();
                let inset = m.panel_h * 0.26;
                let w = (2.0 * m.scale).round().max(1.0);
                line.place((cx - w * 0.5).round(), m.panel_top + inset, w, m.panel_h - inset * 2.0, w * 0.5)?;
                line.set_color(theme.divider)?;
                line.set_opacity(presence)?;
                continue;
            }

            // Paint the icon at each size it will be shown at, once.
            if item.surfaces.is_empty() {
                let pixels = item.pixels(accent);
                for &size in &levels {
                    let surface = scene.surface_from_pixels(&shell::resample(&pixels, size))?;
                    item.surfaces.push((size as f32, surface));
                }
                item.level = None;
            }
            if item.sprite.is_none() {
                item.sprite = Some(scene.sprite_in(&content)?);
                item.level = None;
            }
            if item.mark.is_none() {
                item.mark = Some(scene.pill_in(&content)?);
            }

            let (mut l, mut t, r, b) = icon_rect_of(&m, slot, item);
            let (mut w, mut h) = (r - l, b - t);
            if (w - m.icon).abs() < 0.02 && (h - m.icon).abs() < 0.02 && item.bounce_y == 0.0 {
                // Standing still: sit on whole pixels so the icon is shown 1:1, perfectly sharp.
                l = l.round();
                t = t.round();
                (w, h) = (m.icon, m.icon);
            }
            let size = w.max(h);
            let level = item.surfaces.iter().position(|(s, _)| *s >= size - 0.5).unwrap_or(item.surfaces.len().saturating_sub(1));
            let sprite = item.sprite.as_mut().unwrap();
            if item.level != Some(level) {
                if let Some((_, surface)) = item.surfaces.get(level) {
                    sprite.set_surface(surface)?;
                    item.level = Some(level);
                }
            }
            sprite.place(l, t, w, h)?;
            sprite.set_opacity(presence * (1.0 - 0.16 * item.press.pos.clamp(0.0, 1.0)))?;

            // Running mark: a small drop under the icon that lengthens and takes the accent
            // colour while the app is in front.
            let mark = item.mark.as_mut().unwrap();
            let running = if show_indicator { item.running.pos.clamp(0.0, 1.0) } else { 0.0 };
            if running > 0.01 {
                let active = item.active.pos.clamp(0.0, 1.2);
                let mix = active.min(1.0);
                let h = (3.5 * m.scale).round().max(2.0);
                let w = (h + 10.0 * m.scale * active) * (0.4 + 0.6 * running);
                let y = m.baseline + (m.pad_bottom - h) * 0.5 + 0.5 * m.scale;
                mark.place(cx + item.shift.pos - w * 0.5, y, w, h, h * 0.5)?;
                mark.set_color(rgba(
                    neutral.0 + (ar - neutral.0) * mix,
                    neutral.1 + (ag - neutral.1) * mix,
                    neutral.2 + (ab - neutral.2) * mix,
                    neutral.3 + (1.0 - neutral.3) * mix,
                ))?;
                mark.set_opacity(running * presence)?;
            } else {
                mark.set_opacity(0.0)?;
            }
        }
        Ok(())
    }

    /// The app's name on a small piece of glass above the icon under the cursor.
    fn show_label(&mut self, scene: &Scene, gfx: &Gfx, theme: &Theme, motion: &Motion) -> Result<()> {
        let alpha = self.label_alpha.pos.clamp(0.0, 1.0);
        let index = self.label_index.filter(|&i| alpha > 0.01 && i < self.items.len() && i < self.slots.len() && self.items[i].is_app());
        let Some(index) = index else {
            if let Some(tag) = self.tag.as_mut() {
                tag.glass.set_opacity(0.0)?;
            }
            return Ok(());
        };
        let m = self.m;

        if self.items[index].label.is_none() {
            if self.text_format.is_none() {
                self.text_format = Some(text::format(gfx, text::UI_FONT, m.font, DWRITE_FONT_WEIGHT_MEDIUM)?);
            }
            let (layout, width, height) = text::fit(gfx, self.text_format.as_ref().unwrap(), &self.items[index].name, Some(m.win_w as f32 * 0.4))?;
            let surface = scene.surface_from_text(&layout, width, height, theme.text)?;
            self.items[index].label = Some(Label { surface, width, height });
        }
        if self.tag.is_none() {
            let glass = scene.glass_in(&self.stage.content()?, &theme.bubble)?;
            let text = scene.sprite_in(&glass.outer()?)?;
            self.tag = Some(Tag { glass, text, shown: None });
        }

        let (l, t, r, _) = self.icon_rect(index);
        let item = &self.items[index];
        let label = item.label.as_ref().unwrap();
        let pad = (0.85 * m.font).round();
        let w = label.width + pad * 2.0;
        let h = m.label_h;
        let x = ((l + r) * 0.5 - w * 0.5).clamp(4.0, (m.win_w as f32 - w - 4.0).max(4.0)).round();
        let y = (t - m.label_gap - h).max(2.0).round();

        let tag = self.tag.as_mut().unwrap();
        tag.glass.set_rect(scene, x, y, w, h, h * 0.5)?;
        // It swells up out of the icon it names.
        let grow = if motion.reduced() { 1.0 } else { 0.82 + 0.18 * alpha };
        tag.glass.set_scale(grow, grow, w * 0.5, h + m.label_gap * 0.5)?;
        tag.glass.set_opacity(alpha)?;
        if tag.shown.as_deref() != Some(item.key.as_str()) {
            tag.text.set_surface(&label.surface)?;
            tag.shown = Some(item.key.clone());
        }
        tag.text.place(pad, ((h - label.height) * 0.5).round(), label.width, label.height)
    }
}

impl Drop for Dock {
    fn drop(&mut self) {
        if self.owned {
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

fn icon_rect_of(m: &Metrics, slot: Slot, item: &Item) -> (f32, f32, f32, f32) {
    // Presence may run a little past 1 as the icon pops in; pressing sinks it, dragging lifts it.
    let presence = item.presence.pos.clamp(0.0, 1.12);
    let touch = (1.0 - 0.14 * item.press.pos.clamp(-0.4, 1.0)) * (1.0 + 0.08 * item.lift.pos.clamp(0.0, 1.2));
    let size = m.icon * slot.scale * presence * touch;
    // Swallowing a window the icon spreads sideways and flattens; letting one go it stretches up.
    let gulp = item.gulp.pos.clamp(-0.6, 0.8);
    let (w, h) = (size * item.hop.0 * (1.0 + 0.5 * gulp), size * item.hop.1 * (1.0 - 0.34 * gulp));
    let cx = slot.center() + item.shift.pos;
    let bottom = m.baseline - item.bounce_y;
    (cx - w * 0.5, bottom - h, cx + w * 0.5, bottom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metrics_scale_with_the_display() {
        let cfg = Config::default();
        let a = metrics(&cfg, 1.0, 1920);
        let b = metrics(&cfg, 2.0, 3840);
        assert_eq!(b.icon, a.icon * 2.0);
        assert_eq!(b.panel_h, a.panel_h * 2.0);
        // The glass sits `margin` above the window's bottom edge, icons inside it.
        assert_eq!(a.panel_top + a.panel_h + a.margin, a.win_h as f32);
        assert!(a.baseline > a.panel_top && a.baseline < a.panel_top + a.panel_h);
        // A capsule by default.
        assert_eq!(a.radius, (a.panel_h * 0.5).round());
        // Room above for a fully magnified icon.
        assert!(a.panel_top > a.icon * a.amp);
    }

    #[test]
    fn icon_levels_cover_rest_to_magnified() {
        let mut cfg = Config::default();
        cfg.icon_size = 48.0;
        cfg.magnification = 2.0;
        let levels = icon_levels(&metrics(&cfg, 1.0, 1920));
        assert_eq!(levels[0], 48);
        assert_eq!(*levels.last().unwrap(), 96);
        for pair in levels.windows(2) {
            assert!(pair[1] > pair[0]);
            assert!(pair[1] as f32 / pair[0] as f32 <= 1.45, "{pair:?}");
        }
        cfg.magnification = 1.0;
        assert_eq!(icon_levels(&metrics(&cfg, 1.0, 1920)), vec![48]);
    }
}
