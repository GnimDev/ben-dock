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
use crate::scene::{faded, rgba, Bead, Glass, Layer, Pill, Scene, Skin, Sprite, Stage};
use crate::shade::Shade;
use crate::shell::{self, AppWindow, IconPixels};
use crate::spring::Spring;
use crate::surface::{Surface, Weight};
use crate::system::Monitor;
use crate::text;
use crate::theme::Theme;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::core::Result;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Composition::CompositionDrawingSurface;

/// Identity of the dock's own tray button (not an app, never written to pins.txt).
pub const LAUNCHER_KEY: &str = "::launcher";

/// A divider the user put between pinned apps, as it is written in pins.txt.
pub const DIVIDER: &str = "---";
/// An icon pushed in from outside meets the glass as it would the skin of a drop of water
/// (see `Intake`). How far the skin lets it sink before it gives, as a share of the glass's
/// height; for how long it has to be strained, in seconds; and how far into the glass the
/// cursor has to be, as a share of its height.
const SINK: f32 = 0.44;
const HOLD: f32 = 0.4;
const GIVE: f32 = 0.3;
/// How far the icons beside it have moved apart meanwhile, from the first touch to just
/// before the skin gives (1 = room for the whole icon).
const GAP: (f32, f32) = (0.16, 0.5);
/// How much of a wave's height an icon standing in the water goes up and down with.
const RIDE: f32 = 0.55;
/// Drops thrown up when the skin gives: how many, and how fast they fall (pixels per second
/// squared, at a display scale of 1).
const DROPS: usize = 6;
const GRAVITY: f32 = 1300.0;
/// Counts the dividers made, so that each has a key of its own.
static DIVIDERS: AtomicU32 = AtomicU32::new(0);

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
    /// Being dragged in from outside (a file from Explorer, an app from the tray): it stands
    /// among the pinned apps, but is not one of them until it is dropped.
    ghost: bool,
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
    /// Where the skin of the glass holds the icon, while it is pushed in from outside.
    held: Held,
    /// How far the water lifts the icon as a wave passes under it, in pixels.
    ride: f32,
}

/// Where an icon is while it is pushed in from outside and the skin of the glass holds it
/// (see `Intake`). Worked out again every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Held {
    /// 1 = held outside the glass, 0 = in its place in the dock. On its way in it swings a
    /// little past its place before it rests.
    out: f32,
    /// Its bottom edge out there, in window coordinates.
    bottom: f32,
    /// Its size out there (1 = whole).
    size: f32,
    /// Its width and height out there: pressed a little flat against the skin.
    shape: (f32, f32),
}

impl Default for Held {
    fn default() -> Self {
        Held { out: 0.0, bottom: 0.0, size: 1.0, shape: (1.0, 1.0) }
    }
}

/// An icon being pushed into the glass from outside, as into a drop of water. The skin holds
/// it out and dents under it; the harder it is pushed, the further the skin stretches and the
/// more the whole drop swells. Strained long enough, the skin gives: the icon slips through
/// to its place, the dent springs back, drops fly up, and waves run along the surface.
struct Intake {
    /// The item this is about.
    key: String,
    /// How far the bottom edge of the icon is below the resting top edge of the glass, in
    /// pixels (above it: negative).
    sink: Spring,
    /// For how long the skin has been strained, in seconds.
    strained: f32,
    /// The skin has given.
    through: bool,
    /// 1 = held outside by the skin, 0 = in its place.
    out: Spring,
    /// The icon's size while it is held out: it swells into view under the cursor, and
    /// shrinks away there if it is taken back.
    size: Spring,
    /// Where the skin last held it.
    last: Held,
}

impl Intake {
    fn new(key: String, motion: &Motion) -> Self {
        // It arrives from above the edge.
        Intake { key, sink: Spring::with(-2.0, motion.calm()), strained: 0.0, through: false, out: Spring::with(1.0, motion.medium()), size: Spring::with(0.0, motion.small()), last: Held::default() }
    }
}

/// A drop of water in the air.
struct Droplet {
    bead: Bead,
    /// Its centre, in window coordinates, and its speed.
    at: (f32, f32),
    speed: (f32, f32),
    size: f32,
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

    /// A divider the user put between pinned apps. It is kept in pins.txt with them.
    fn divider(present: bool, motion: &Motion) -> Self {
        let mut item = Item::blank(Kind::Separator, motion);
        item.presence.snap(if present { 1.0 } else { 0.0 });
        item.presence.set(1.0);
        let key = format!("{DIVIDER}{}", DIVIDERS.fetch_add(1, Ordering::Relaxed));
        Item { key, target: DIVIDER.to_string(), pinned: true, ..item }
    }

    fn blank(kind: Kind, motion: &Motion) -> Self {
        Item {
            kind,
            key: String::new(),
            target: String::new(),
            name: String::new(),
            pinned: false,
            ghost: false,
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
            held: Held::default(),
            ride: 0.0,
        }
    }

    pub fn is_special(&self) -> bool {
        self.key.starts_with("::")
    }

    pub fn is_app(&self) -> bool {
        self.kind == Kind::App
    }

    /// A divider the user put there (not the one before the open apps, which is the dock's own).
    pub fn is_divider(&self) -> bool {
        self.kind == Kind::Separator && self.pinned
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

/// The bubble beside the dock that sends every window out of the way: a round piece of the
/// dock's glass, the drop of light the cursor gets on it, and the screen painted on it.
struct Desk {
    glass: Glass,
    lens: Bead,
    sign: Sprite,
    label: Option<Label>,
}

/// What letting go of the mouse button over the dock meant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Released {
    /// A click on this icon.
    Click(usize),
    /// A click on the bubble beside the dock: every window out of the way.
    Desktop,
    /// The end of a drag that changed the order of the pinned apps (now saved).
    Reordered,
    /// A pinned app was dragged out of the dock and let go there: it is off the dock (now saved).
    Removed,
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
    /// The glass straining as an icon is pushed in from outside: like a drop of water it
    /// swells (towards 1) the further its skin is stretched, and wobbles back to rest.
    strain: Spring,
    /// The icon that is being pushed in from outside, until it is in its place.
    intake: Option<Intake>,
    /// The top edge of the glass, which behaves like water while that goes on, and the
    /// outline the glass wears while its edge is not level.
    surface: Surface,
    skin: Option<Skin>,
    edge: Vec<(f32, f32)>,
    /// Where the skin has just given: drops fly up from here.
    splash: Option<f32>,
    spray: Vec<Droplet>,
    /// Counts the drops, so that no two splashes are alike.
    sprayed: u32,
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
    /// The dragged pinned app has been pulled up out of the dock: its room is closed, and the
    /// app is told (see `pull`) so that a bubble can carry the icon. Let go there, it comes off.
    pulled: bool,
    pull_told: bool,
    /// Tucked out of view, the dock leaves a small bar at the screen's bottom edge to come
    /// back by (see `set_bar`): white, with a faint dark rim that shows it on a white window.
    bar: bool,
    bar_rim: Pill,
    bar_pill: Pill,
    /// The bubble beside the dock that shows the desktop (see `show_desk`). It stands there
    /// while a window is showing (`set_desk`), and waits below the screen's edge otherwise:
    /// 0 = in its place, 1 = out of view.
    desk: Desk,
    desk_wanted: bool,
    desk_away: Spring,
    /// The cursor is on it; the mouse button went down on it.
    pub desk_hot: bool,
    desk_down: bool,
    desk_glow: Spring,
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
        // The bubble that shows the desktop is of the same glass, and its shadow lies where the
        // dock's does.
        let mut desk_glass = scene.glass_in(&stage.content()?, &theme.dock)?;
        match shade.as_ref() {
            Some(shade) => desk_glass.cast_shadow_on(&shade.content()?)?,
            None => desk_glass.without_shadow(),
        }
        let desk = Desk { lens: scene.bead_in(&desk_glass.inner()?)?, sign: scene.sprite_in(&desk_glass.outer()?)?, glass: desk_glass, label: None };
        let layer = scene.layer_in(&stage.content()?)?;
        let bar_rim = scene.pill_in(&stage.content()?)?;
        let bar_pill = scene.pill_in(&stage.content()?)?;
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
            strain: Spring::with(0.0, motion.jelly()),
            intake: None,
            surface: Surface::default(),
            skin: None,
            edge: Vec::new(),
            splash: None,
            spray: Vec::new(),
            sprayed: 0,
            mag: Spring::with(0.0, motion.small()),
            cursor_x: m.win_w as f32 * 0.5,
            hovering: false,
            hovered: None,
            pressed: None,
            label_alpha: Spring::with(0.0, motion.fade()),
            label_index: None,
            drag: None,
            press_x: 0.0,
            pulled: false,
            pull_told: false,
            bar: false,
            bar_rim,
            bar_pill,
            desk,
            desk_wanted: false,
            desk_away: Spring::with(1.0, motion.medium()),
            desk_hot: false,
            desk_down: false,
            desk_glow: Spring::with(0.0, motion.fade()),
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
            if target == DIVIDER {
                self.items.push(Item::divider(true, motion));
                continue;
            }
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
                item.ghost = false;
            }
        }
        let mut pinned = Vec::new();
        for (key, target) in keys.iter().zip(pins) {
            if target == DIVIDER {
                pinned.push(match self.items.iter().position(|i| i.is_divider()) {
                    Some(at) => self.items.remove(at),
                    None => Item::divider(false, motion),
                });
                continue;
            }
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
        rest.retain(|i| !i.is_divider());
        let button = rest.iter().position(|i| i.key == LAUNCHER_KEY).map(|at| rest.remove(at));
        let divider = rest.iter().position(|i| i.kind == Kind::Separator && !i.pinned).map(|at| rest.remove(at)).unwrap_or_else(|| Item::separator(motion));
        self.items.extend(button);
        self.items.extend(pinned);
        self.items.push(divider);
        self.items.extend(rest);
        self.hovered = None;
        self.pressed = None;
        self.label_index = None;
        self.drag = None;
        self.pulled = false;
    }

    pub fn save_pins(&self) {
        let pins: Vec<String> = self.items.iter().filter(|i| i.pinned && !i.ghost && !i.is_special()).map(|i| i.target.clone()).collect();
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
        self.pulled = false;
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
        self.desk.glass.set_material(&theme.dock);
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
        self.desk.label = None;
        if let Some(tag) = self.tag.as_mut() {
            tag.shown = None;
        }
        for item in &mut self.items {
            item.label = None;
        }
    }

    /// Throws away painted icons (after a size change or a GPU reset); they are painted again when shown.
    pub fn forget_icons(&mut self) {
        self.desk.label = None;
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
        let held_out = self.intake.as_ref().map(|i| !i.through).unwrap_or(false);
        // The app being pulled out of the dock keeps its room closed until it is let go.
        let pulled_key = if self.pulled { self.pressed.and_then(|p| self.items.get(p)).map(|i| i.key.clone()) } else { None };
        for item in self.items.iter_mut().filter(|i| i.is_app()) {
            let running = !item.windows.is_empty();
            item.running.set(if running { 1.0 } else { 0.0 });
            item.active.set(if item.windows.contains(&front) { 1.0 } else { 0.0 });
            let stays = item.pinned || running || item.bounce.is_some();
            // An icon the skin of the glass still holds out has only part of its room yet.
            if !(item.ghost && held_out) && pulled_key.as_deref() != Some(item.key.as_str()) {
                item.presence.set(if stays { 1.0 } else { 0.0 });
            }
            if item.pinned {
                pinned += 1;
            } else if stays {
                unpinned += 1;
            }
        }
        if let Some(sep) = self.items.iter_mut().find(|i| i.kind == Kind::Separator && !i.pinned) {
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
            let sep = self.items.iter().position(|i| i.kind == Kind::Separator && !i.pinned).unwrap_or(self.items.len());
            self.items.insert(sep, item);
        } else {
            self.items.push(item);
        }
        self.hovered = None;
        self.pressed = None;
        self.label_index = None;
        self.save_pins();
    }

    /// Puts a divider after the pinned app at `index`.
    pub fn add_divider(&mut self, index: usize, motion: &Motion) {
        if !self.items.get(index).map(|i| i.pinned).unwrap_or(false) {
            return;
        }
        self.items.insert(index + 1, Item::divider(false, motion));
        self.hovered = None;
        self.pressed = None;
        self.label_index = None;
        self.save_pins();
    }

    /// Takes away the divider at `index`.
    pub fn remove_divider(&mut self, index: usize) {
        if !self.items.get(index).map(|i| i.is_divider()).unwrap_or(false) {
            return;
        }
        self.items.remove(index);
        self.hovered = None;
        self.pressed = None;
        self.label_index = None;
        self.save_pins();
    }

    // ------------------------------------------------ dragged in from outside

    /// Something is being dragged over the dock from outside (files from Explorer, an app from
    /// the tray). The first of `targets` that is not pinned yet joins the pinned apps and
    /// follows the cursor, as if it had been picked up in the dock. `x`: the cursor, in window
    /// coordinates.
    pub fn drag_in(&mut self, targets: &[String], x: f32, motion: &Motion) {
        if self.items.iter().any(|i| i.ghost) {
            return;
        }
        let find = |items: &[Item], target: &str| {
            let key = shell::key_for_target(target);
            items.iter().position(|i| i.is_app() && i.key == key)
        };
        let Some(target) = targets.iter().find(|target| !find(&self.items, target).map(|at| self.items[at].pinned).unwrap_or(false)) else { return };
        let item = match find(&self.items, target) {
            // Open but not pinned: its own icon is the one that moves.
            Some(at) => {
                if at < self.slots.len() {
                    self.slots.remove(at);
                }
                self.items.remove(at)
            }
            None => Item::app(shell::key_for_target(target), target.clone(), true, false, motion),
        };
        // It comes in where the pinned apps end; from there it changes places like any dragged icon.
        let at = self.items.iter().position(|i| i.kind == Kind::Separator && !i.pinned).unwrap_or(self.items.len());
        let edge = self.slots.get(at).map(|s| s.left).or(self.slots.last().map(|s| s.right)).unwrap_or(x);
        self.items.insert(at, Item { pinned: true, ghost: true, ..item });
        self.slots.insert(at.min(self.slots.len()), Slot { left: edge, right: edge, scale: 1.0 });
        self.pressed = Some(at);
        self.press_x = x;
        self.drag = Some(0.0);
        self.hovered = None;
        self.label_index = None;
        if !motion.reduced() {
            // The glass does not let it in at once: its skin holds the icon out, with only a
            // little room made for it, until it gives (see `Intake`).
            let item = &mut self.items[at];
            item.presence.set(GAP.0);
            self.intake = Some(Intake::new(item.key.clone(), motion));
        }
    }

    /// The skin of the glass gives: the icon it held out slips through to its place, thin
    /// and tall for a moment, and the glass wobbles back.
    fn let_in(&mut self) {
        let Some(intake) = self.intake.as_mut().filter(|i| !i.through) else { return };
        intake.through = true;
        intake.out.set(0.0);
        self.strain.set(0.0);
        let key = intake.key.clone();
        if let Some(index) = self.items.iter().position(|i| i.is_app() && i.key == key) {
            let item = &mut self.items[index];
            item.presence.set(1.0);
            item.gulp(0.0, -1.3);
            // Where it went through, the water splashes (see `flow`).
            self.splash = self.slots.get(index).map(|slot| slot.center() + item.shift.pos);
        }
    }

    /// What was being dragged in left the dock again, or was given up.
    pub fn drag_out(&mut self) {
        let Some(at) = self.items.iter().position(|i| i.ghost) else { return };
        self.pressed = None;
        self.drag = None;
        self.label_index = None;
        self.strain.set(0.0);
        let item = &mut self.items[at];
        item.ghost = false;
        item.pinned = false;
        if item.windows.is_empty() {
            // It shrinks away where it stands: in the dock, or outside it if the skin of the
            // glass still held it (see `flow`).
            item.presence.set(0.0);
            return;
        }
        // An open app goes back among the open ones.
        self.intake = None;
        item.presence.set(1.0);
        let was_at = self.slots.get(at).map(|s| s.center() + item.shift.pos);
        let mut item = self.items.remove(at);
        if at < self.slots.len() {
            self.slots.remove(at);
        }
        if let (Some(was_at), Some(last)) = (was_at, self.slots.last().copied()) {
            item.shift.pos = was_at - last.right;
            item.shift.vel = 0.0;
            self.slots.push(Slot { left: last.right, right: last.right, scale: 1.0 });
        }
        self.items.push(item);
    }

    /// What was dragged in was dropped on the dock: it stays where it is, and the rest of
    /// `targets` are pinned after it (now saved). Returns false if nothing changed.
    pub fn drop_in(&mut self, targets: &[String], motion: &Motion) -> bool {
        // Dropped while the glass was still holding out: it gives now.
        self.let_in();
        self.pressed = None;
        self.drag = None;
        let ghost = self.items.iter().position(|i| i.ghost);
        let mut at = match ghost {
            Some(at) => {
                self.items[at].ghost = false;
                at + 1
            }
            None => self.items.iter().position(|i| i.kind == Kind::Separator && !i.pinned).unwrap_or(self.items.len()),
        };
        let mut changed = ghost.is_some();
        for target in targets {
            let key = shell::key_for_target(target);
            let item = match self.items.iter().position(|i| i.is_app() && i.key == key) {
                Some(i) if self.items[i].pinned => continue,
                Some(i) => {
                    if i < at {
                        at -= 1;
                    }
                    self.items.remove(i)
                }
                None => Item::app(key, target.clone(), true, false, motion),
            };
            self.items.insert(at, Item { pinned: true, ..item });
            at += 1;
            changed = true;
        }
        if changed {
            self.hovered = None;
            self.label_index = None;
            self.save_pins();
        }
        changed
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

    /// Index of the divider (one the user put there) at a point in window coordinates.
    pub fn divider_at(&self, x: f32, y: f32) -> Option<usize> {
        let index = self.slots.iter().position(|s| x >= s.left && x < s.right)?;
        (self.items.get(index)?.is_divider() && y >= self.m.panel_top - 2.0).then_some(index)
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
        // A divider can be picked up and moved like an icon.
        self.pressed = self.app_at(x, y).or(self.divider_at(x, y));
        self.press_x = x;
        self.drag = None;
        self.pulled = false;
        self.desk_down = self.desk_at(x, y);
        self.pressed
    }

    /// If a pinned app has just been pulled out of the dock (true) or put back (false) since the
    /// last time this was asked.
    pub fn pull(&mut self) -> Option<bool> {
        (self.pulled != self.pull_told).then(|| {
            self.pull_told = self.pulled;
            self.pulled
        })
    }

    /// The picture of the app being pulled out of the dock, and its size in pixels.
    pub fn pulled_icon(&self) -> Option<(CompositionDrawingSurface, f32)> {
        let item = self.items.get(self.pressed?)?;
        let size = self.m.icon;
        let (_, surface) = item.surfaces.iter().find(|(s, _)| *s >= size - 0.5).or(item.surfaces.last())?;
        Some((surface.clone(), size))
    }

    /// The mouse button came up: a click, the end of a drag, or a release somewhere else.
    pub fn release(&mut self, x: f32, y: f32) -> Released {
        if std::mem::take(&mut self.desk_down) {
            // Pressed on the bubble beside the dock: a click if it is let go there too.
            return if self.desk_at(x, y) { Released::Desktop } else { Released::Nothing };
        }
        let Some(index) = self.pressed.take() else { return Released::Nothing };
        if self.drag.take().is_some() {
            if std::mem::take(&mut self.pulled) {
                // Let go outside the dock: the app comes off it. (One that is open stays among the
                // open apps.)
                self.toggle_pin(index);
                if let Some(item) = self.items.last_mut().filter(|i| !i.windows.is_empty()) {
                    item.presence.set(1.0);
                }
                return Released::Removed;
            }
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
        self.desk_hot = cursor.is_some() && shown && self.desk_at(cx, cy);
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

        // While its skin holds an icon out, the glass swells steadily; let go, it wobbles
        // (see `flow`, which says how far it is strained).
        let held_out = self.intake.as_ref().map(|i| !i.through).unwrap_or(false);
        self.strain.tune(if held_out { motion.calm() } else { motion.jelly() });
        self.strain.step(dt);

        // Drop apps that have finished sliding out.
        let before = self.items.len();
        self.items.retain(|item| !(item.is_app() && !item.pinned && item.presence.target == 0.0 && item.presence.pos < 0.004));
        let removed = self.items.len() != before;
        if removed {
            // What is being dragged in from outside keeps following the cursor.
            self.pressed = self.items.iter().position(|i| i.ghost);
            self.label_index = None;
        }

        // Dragging an icon sideways moves it to a new place among its neighbours.
        let moved = self.drag_icon(cx, cy);

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
        // Strained, the whole drop swells: wider, rounder, and most of all upwards, towards
        // what is pushing in. Giving, it swings a little the other way.
        let bulge = self.strain.pos.clamp(-0.5, 1.3) * 9.0 * m.scale * squash;
        self.panel = (shown_left - bulge, m.panel_top - thicken - bulge * 1.2, shown_right - shown_left + bulge * 2.0, m.panel_h + thicken * 2.0 + bulge * 1.6);
        let (px, py, pw, ph) = self.panel;
        let radius = (m.radius + thicken + bulge.max(0.0)).min(ph * 0.5);

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
        // The bar the tucked-away dock leaves at the bottom edge. It does not slide with the
        // rest, and shows as the dock goes.
        let (bar_w, bar_h, rim) = ((120.0 * m.scale).round(), (4.0 * m.scale).round(), m.scale.round().max(1.0));
        let (bar_x, bar_y) = (((m.win_w as f32 - bar_w) * 0.5).round(), m.win_h as f32 - bar_h - (3.0 * m.scale).round() - slide);
        let bar_shown = if self.bar { hide.pos.clamp(0.0, 1.0) } else { 0.0 };
        self.bar_rim.place(bar_x - rim, bar_y - rim, bar_w + 2.0 * rim, bar_h + 2.0 * rim, bar_h * 0.5 + rim)?;
        self.bar_rim.set_color(rgba(0.0, 0.0, 0.0, 0.32))?;
        self.bar_rim.set_opacity(bar_shown)?;
        self.bar_pill.place(bar_x, bar_y, bar_w, bar_h, bar_h * 0.5)?;
        self.bar_pill.set_color(rgba(1.0, 1.0, 1.0, 0.92))?;
        self.bar_pill.set_opacity(bar_shown)?;
        // While an icon is pushed in from outside, and for a moment after, the glass is a
        // drop of water: its top edge dents and waves run along it. The glass gets room above
        // its resting edge for the highest of them, and is cut to the line they make.
        let raise = (self.flow(scene, theme, motion, cy, radius, dt)? - 0.05).max(0.0).ceil();
        self.glass.set_rect(scene, px, py - raise, pw, ph + raise, radius)?;
        self.wear_skin(scene, gfx, raise)?;
        self.show_lens(theme, squash, radius, raise)?;
        self.show_items(scene, cfg, theme)?;
        self.show_label(scene, gfx, theme, motion)?;
        self.show_desk(scene, gfx, theme, motion, dt)?;
        self.update_region(hide);

        // The cursor resting on the dock is no reason for another frame: when it moves, the
        // window is told, and that wakes the loop.
        Ok(removed
            || self.pressed.is_some()
            || !self.mag.settled(1e-3)
            || !self.swell.settled(1e-3)
            || self.intake.is_some()
            || self.splash.is_some()
            || !self.spray.is_empty()
            || !self.surface.calm()
            || !self.strain.settled(1e-3)
            || !self.edge_left.settled(0.05)
            || !self.edge_right.settled(0.05)
            || !self.label_alpha.settled(1e-3)
            || !self.lens_at.settled()
            || !self.desk_away.settled(1e-3)
            || !self.desk_glow.settled(1e-3)
            || self.items.iter().any(|i| !i.settled()))
    }

    /// True right after an item was dropped from the dock, so the app re-reads the windows.
    pub fn count(&self) -> usize {
        self.items.len()
    }

    /// The glass as a drop of water (see `surface.rs` and `Intake`). Moves the icon that is
    /// being pushed in from outside, the surface it presses on, the icons standing in the
    /// water and the drops in the air. `cy`: the cursor's row. `radius`: of the glass's
    /// corners. Returns how far the highest wave stands above the resting edge.
    fn flow(&mut self, scene: &Scene, theme: &Theme, motion: &Motion, cy: f32, radius: f32, dt: f32) -> Result<f32> {
        let m = self.m;
        let (px, py, pw, ph) = self.panel;
        for item in &mut self.items {
            item.held = Held::default();
            item.ride = 0.0;
        }
        if self.intake.is_none() && self.splash.is_none() && self.spray.is_empty() && self.surface.calm() {
            return Ok(0.0);
        }
        // The water follows the settings for motion: as quick as the springs are, and as
        // ready to come to rest.
        let jelly = motion.jelly();
        let (pace, damping) = (jelly.omega / 30.0, 8.2 * jelly.zeta);
        let squash = motion.squash();
        self.surface.span(px + radius, px + pw - radius, 3.0 * m.scale, m.win_w as f32);

        let mut weight = None;
        let mut gives = false;
        if let Some(mut intake) = self.intake.take() {
            let index = self.items.iter().position(|i| i.is_app() && i.key == intake.key);
            if let Some((index, slot)) = index.and_then(|i| self.slots.get(i).map(|slot| (i, *slot))) {
                let item = &mut self.items[index];
                intake.out.tune(motion.medium());
                intake.size.tune(motion.small());
                let mut keep = true;
                if intake.through {
                    // Through the skin: it settles into its place.
                    intake.out.step(dt);
                    item.held = Held { out: intake.out.pos, ..intake.last };
                    keep = !intake.out.settled(1e-3);
                } else {
                    // Taken back out of the dock, it shrinks away where the skin held it.
                    let taken_back = item.presence.target == 0.0;
                    if !taken_back {
                        let icon = m.icon * slot.scale;
                        let deepest = SINK * ph;
                        // Centred on the cursor, the icon would reach this far below the edge
                        // of the glass. The skin lets it sink less and less willingly, and
                        // never further than `deepest`; above the edge it is free.
                        let wants = if cy.is_finite() { cy + icon * 0.5 - py } else { 0.0 };
                        let pressing = wants > 0.0;
                        intake.sink.tune(if pressing { motion.calm() } else { motion.follow() });
                        intake.sink.set(if pressing { deepest * (1.0 - (-wants / deepest).exp()) } else { wants.max(-icon * 1.5) });
                        intake.sink.step(dt);
                        let sunk = intake.sink.pos;
                        let strain = (sunk / deepest).clamp(0.0, 1.0);
                        intake.strained = if sunk > 0.0 { intake.strained + dt * pace } else { 0.0 };
                        // The further the skin is stretched, the more the whole drop swells
                        // and the further the icons beside it move apart.
                        self.strain.set(strain);
                        item.presence.set(GAP.0 + (GAP.1 - GAP.0) * strain);
                        if sunk > 0.0 {
                            // What presses on the water is round underneath and a little wider
                            // than the icon. It stays off the round ends of the glass, where
                            // there is no edge to dent.
                            let round = icon * 0.62 + 4.0 * m.scale;
                            let (first, last) = (px + radius + round * 0.7, px + pw - radius - round * 0.7);
                            if first < last {
                                weight = Some(Weight { x: (slot.center() + item.shift.pos).clamp(first, last), depth: sunk, radius: round });
                            }
                        }
                        // Strained for long enough, with the cursor well inside, the skin gives.
                        gives = intake.strained >= HOLD && wants >= icon * 0.5 + GIVE * ph;
                        // The icon stands on the skin, pressed a little flat against it.
                        intake.last = Held { out: 1.0, bottom: py + sunk + m.scale, size: 1.0, shape: (1.0 + 0.10 * strain * squash, 1.0 - 0.13 * strain * squash) };
                    }
                    intake.size.set(if taken_back { 0.0 } else { 1.0 });
                    intake.size.step(dt);
                    item.held = Held { size: intake.size.pos.max(0.0), ..intake.last };
                }
                if keep {
                    self.intake = Some(intake);
                }
            }
        }
        if gives {
            self.let_in();
        }
        if let Some(x) = self.splash.take() {
            self.splash_at(scene, x, squash)?;
        }
        self.surface.advance(dt * pace, damping, weight);

        // The icons stand in the water: a wave passing under one lifts it.
        let Dock { items, slots, surface, .. } = self;
        for (item, slot) in items.iter_mut().zip(slots.iter()) {
            if item.is_app() {
                item.ride = surface.height(slot.center() + item.shift.pos) * RIDE;
            }
        }

        // The drops in the air rise, fall, and are water again where they land.
        let (top, bottom, edge) = (faded(theme.dock.rim, 0.6), faded(theme.dock.rim, 0.2), theme.dock.rim);
        let fall = dt * pace;
        let mut index = 0;
        while index < self.spray.len() {
            let drop = &mut self.spray[index];
            drop.speed.1 += GRAVITY * m.scale * fall;
            drop.at.0 += drop.speed.0 * fall;
            drop.at.1 += drop.speed.1 * fall;
            // Past the ends of the glass there is nothing to land on.
            let water = if drop.at.0 > px && drop.at.0 < px + pw { py - self.surface.height(drop.at.0) } else { m.win_h as f32 };
            if drop.speed.1 > 0.0 && drop.at.1 >= water {
                // A small ring where it lands.
                self.surface.push(drop.at.0, 7.0 * m.scale, -22.0 * drop.size);
                self.spray.remove(index);
                continue;
            }
            drop.bead.place(drop.at.0 - drop.size * 0.5, drop.at.1 - drop.size * 0.5, drop.size, drop.size, drop.size * 0.5)?;
            drop.bead.set_colors(top, bottom, edge)?;
            drop.bead.set_opacity(1.0)?;
            index += 1;
        }

        if self.surface.calm() {
            // Level again, to the last fraction of a pixel.
            self.surface.still();
        }
        Ok(self.surface.crest())
    }

    /// The skin of the glass has given at `x`: the dent it leaves springs up, the icons on
    /// either side are pushed apart by the water that is pushed aside, and drops fly up.
    fn splash_at(&mut self, scene: &Scene, x: f32, squash: f32) -> Result<()> {
        let m = self.m;
        let (_, py, _, _) = self.panel;
        self.surface.push(x, 26.0 * m.scale, 150.0 * m.scale * squash);
        for (item, slot) in self.items.iter_mut().zip(&self.slots) {
            let away = slot.center() - x;
            // Not the icon that came through itself.
            if away.abs() >= m.slot * 0.5 {
                item.shift.vel += away.signum() * 420.0 * m.scale * squash * (-away.abs() / (2.2 * m.slot)).exp();
            }
        }
        let content = self.layer.content()?;
        for _ in 0..DROPS {
            // Not left to chance, but no two drops alike, and no two splashes.
            let mut next = || {
                self.sprayed = self.sprayed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((self.sprayed >> 8) & 0xffff) as f32 / 65535.0
            };
            let (side, up, big) = (next() - 0.5, next(), next());
            let size = ((2.5 + 3.0 * big) * m.scale).round().max(2.0);
            let mut bead = scene.bead_in(&content)?;
            bead.set_opacity(0.0)?;
            self.spray.push(Droplet { bead, at: (x + side * 14.0 * m.scale, py), speed: (side * 220.0 * m.scale, -(150.0 + 140.0 * up) * m.scale), size });
        }
        Ok(())
    }

    /// Cuts the glass to its top edge while that is not level (`raise`: the room it has above
    /// its resting edge), and gives it its own outline back once the water is calm.
    fn wear_skin(&mut self, scene: &Scene, gfx: &Gfx, raise: f32) -> Result<()> {
        if self.surface.calm() {
            return match self.skin.as_mut() {
                Some(skin) => skin.take_off(&self.glass),
                None => Ok(()),
            };
        }
        let (px, ..) = self.panel;
        self.surface.points(&mut self.edge);
        for point in &mut self.edge {
            *point = (point.0 - px, raise - point.1);
        }
        if self.skin.is_none() {
            self.skin = Some(scene.skin(gfx)?);
        }
        match self.skin.as_mut() {
            Some(skin) => skin.wear(&self.glass, raise, &self.edge),
            None => Ok(()),
        }
    }

    /// Handles dragging an icon sideways. Returns the icons that changed places this frame, as
    /// (new index, where its centre was on screen), so they can glide instead of jumping.
    fn drag_icon(&mut self, cx: f32, cy: f32) -> Vec<(usize, f32)> {
        let mut moved = Vec::new();
        let Some(mut index) = self.pressed else { return moved };
        if index >= self.items.len() || index >= self.slots.len() || !cx.is_finite() {
            return moved;
        }
        if self.drag.is_none() {
            // A press turns into a drag once the cursor has clearly moved sideways, or has left
            // the dock upwards, past where a magnified icon reaches. (A hand that only trembles
            // on a click must not start one: the click would be lost.)
            let lifted = cy.is_finite() && cy < self.m.panel_top - self.m.icon * (self.m.amp + 0.5);
            if (cx - self.press_x).abs() < 6.0 * self.m.scale && !lifted {
                return moved;
            }
            if self.items[index].is_special() {
                return moved;
            }
            self.drag = Some(self.press_x - self.slots[index].center());
        }
        // Pulled up out of the dock, a pinned app is on its way off it: its room closes. Taken
        // back over the dock, it is let in again.
        if cy.is_finite() && self.items[index].is_app() && self.items[index].pinned && !self.items[index].ghost {
            // (Higher than a magnified icon stands; and once out, it takes coming well back to be in again.)
            let line = self.m.panel_top - self.m.icon * (self.m.amp + if self.pulled { 0.1 } else { 0.5 });
            let out = cy < line;
            if out != self.pulled {
                self.pulled = out;
                self.items[index].presence.set(if out { 0.0 } else { 1.0 });
                self.label_index = None;
            }
        }
        if self.pulled {
            return moved;
        }
        let grab = self.drag.unwrap_or(0.0);
        let centre = cx - grab;
        // Swap with a neighbour each time the dragged icon passes its middle. Pinned apps stay
        // among pinned apps, open ones among open ones.
        loop {
            let pinned = self.items[index].pinned;
            let same_group = |item: &Item| (item.is_app() || item.is_divider()) && !item.is_special() && item.pinned == pinned && item.presence.target > 0.5;
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
        // (The same while an icon is pushed in from outside: it stands above the glass, and so
        // do the waves and the drops it makes.)
        let watery = self.intake.is_some() || !self.spray.is_empty() || !self.surface.calm();
        let lively = watery || self.mag.pos > 0.001 || self.label_alpha.pos > 0.001 || self.items.iter().any(|i| i.bounce.is_some());
        let (px, _, pw, _) = self.panel;
        // The bubble beside the dock is part of it while it is in view.
        let (desk_left, _, desk_size) = self.desk_rect();
        let desk_right = if self.desk_away.pos < 0.97 { (desk_left + desk_size + 2.0).ceil() as i32 } else { 0 };
        let wanted = if hidden && self.bar {
            // Out of view, but for the strip at the bottom edge that brings it back.
            let (l, t, r, b) = self.bar_zone();
            (l.floor() as i32, t.floor() as i32, r.ceil() as i32, b.ceil() as i32)
        } else if hidden {
            (0, 0, 0, 0)
        } else if lively {
            let grow = m.amp * m.mag_radius * 0.5 + 24.0 * m.scale;
            let rest_left = m.win_w as f32 * 0.5 - self.widths.iter().sum::<f32>() * 0.5 - m.pad_x;
            let rest_right = m.win_w as f32 - rest_left;
            ((rest_left - grow).floor() as i32, 0, ((rest_right + grow).ceil() as i32).max(desk_right), m.win_h)
        } else {
            ((px - 2.0).floor() as i32, (m.panel_top - 2.0).floor() as i32, ((px + pw + 2.0).ceil() as i32).max(desk_right), m.win_h)
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

    /// Whether the dock, when it is out of view, leaves a bar at the screen's bottom edge to
    /// come back by (it does when it is only tucked away, not for a full-screen app).
    pub fn set_bar(&mut self, on: bool) {
        self.bar = on;
    }

    /// Whether the bubble that shows the desktop stands beside the dock: it does while a
    /// window is showing, when there is something to send out of the way.
    pub fn set_desk(&mut self, wanted: bool) {
        self.desk_wanted = wanted;
    }

    /// That bubble's place beside the dock: left, top and size (it is round), in window
    /// coordinates.
    fn desk_rect(&self) -> (f32, f32, f32) {
        let m = self.m;
        let (px, _, pw, _) = self.panel;
        ((px + pw + 10.0 * m.scale).round(), m.panel_top, m.panel_h)
    }

    /// Whether a point in window coordinates is on that bubble, or on the strip below it.
    fn desk_at(&self, x: f32, y: f32) -> bool {
        let (left, top, size) = self.desk_rect();
        self.desk_wanted && self.desk_away.pos < 0.5 && x >= left - 2.0 && x <= left + size + 2.0 && y >= top - 2.0 && y < self.m.win_h as f32
    }

    /// The bubble beside the dock that shows the desktop. It comes up from below the screen's
    /// edge when a window is showing and goes back down when none is, as the dock does.
    fn show_desk(&mut self, scene: &Scene, gfx: &Gfx, theme: &Theme, motion: &Motion, dt: f32) -> Result<()> {
        let m = self.m;
        // Coming into view it bounces a little; leaving, it just goes.
        self.desk_away.tune(if self.desk_wanted { motion.medium() } else { motion.calm() });
        self.desk_away.set(if self.desk_wanted { 0.0 } else { 1.0 });
        self.desk_away.step(dt);
        self.desk_glow.tune(motion.fade());
        self.desk_glow.set(if self.desk_hot { 1.0 } else { 0.0 });
        self.desk_glow.step(dt);

        let (x, y, size) = self.desk_rect();
        let down = (self.desk_away.pos.max(0.0) * (m.panel_h + m.margin + theme.dock.shadow_reach + 6.0 * m.scale)).round();
        let desk = &mut self.desk;
        desk.glass.set_rect(scene, x, y + down, size, size, size * 0.5)?;
        if desk.label.is_none() {
            // A screen, from the symbol font.
            let format = text::format(gfx, text::ICON_FONT, (m.icon * 0.72).round(), DWRITE_FONT_WEIGHT_NORMAL)?;
            let (layout, width, height) = text::fit(gfx, &format, &text::glyph(0xE7F4), None)?;
            let surface = scene.surface_from_text(&layout, width, height, theme.text)?;
            desk.sign.set_surface(&surface)?;
            desk.label = Some(Label { surface, width, height });
        }
        let label = desk.label.as_ref().unwrap();
        desk.sign.place(((size - label.width) * 0.5).round(), ((size - label.height) * 0.5).round(), label.width, label.height)?;
        // The drop of light under the cursor; pressing squeezes it (as in `show_lens`).
        let inset = m.lens_inset + if self.desk_down { size * 0.035 } else { 0.0 };
        desk.lens.place(inset, inset, size - 2.0 * inset, size - 2.0 * inset, size * 0.5 - inset)?;
        let (top, bottom, edge) = theme.bead;
        desk.lens.set_colors(top, bottom, edge)?;
        desk.lens.set_opacity(self.desk_glow.pos.clamp(0.0, 1.0))
    }

    /// The strip at the screen's bottom edge that brings the tucked-away dock back: as wide
    /// as the dock at rest, in window coordinates (left, top, right, bottom).
    pub fn bar_zone(&self) -> (f32, f32, f32, f32) {
        let m = self.m;
        let half = (self.widths.iter().sum::<f32>() * 0.5 + m.pad_x).max(90.0 * m.scale);
        let mid = m.win_w as f32 * 0.5;
        (mid - half, m.win_h as f32 - (10.0 * m.scale).round(), mid + half, m.win_h as f32)
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

    /// `raise`: how far the glass reaches above its resting edge this frame (see `update`).
    fn show_lens(&mut self, theme: &Theme, squash: f32, radius: f32, raise: f32) -> Result<()> {
        if !self.lens_at.visible() {
            return self.lens.set_opacity(0.0);
        }
        let s = self.lens_at.shape(squash);
        // Pressing the icon squeezes the drop under it.
        let press = self.pressed.and_then(|i| self.items.get(i)).map(|i| i.press.pos.clamp(0.0, 1.0)).unwrap_or(0.0);
        let squeeze = 1.0 - 0.07 * press;
        let (w, h) = (s.w * squeeze, s.h * squeeze);
        let (x, y) = (s.x + (s.w - w) * 0.5, s.y + (s.h - h) * 0.5 + raise);
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
                line.place((cx + item.shift.pos - w * 0.5).round(), m.panel_top + inset, w, m.panel_h - inset * 2.0, w * 0.5)?;
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
            if (w - m.icon).abs() < 0.02 && (h - m.icon).abs() < 0.02 && item.bounce_y == 0.0 && item.ride == 0.0 && item.held.out == 0.0 {
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
            // (An icon the skin of the glass holds out is as solid as it is large out there.)
            let solid = presence + (item.held.size.clamp(0.0, 1.0) - presence) * item.held.out.clamp(0.0, 1.0);
            sprite.set_opacity(solid * (1.0 - 0.16 * item.press.pos.clamp(0.0, 1.0)))?;

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
            crate::dropzone::forget(self.hwnd);
            unsafe {
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }
}

fn icon_rect_of(m: &Metrics, slot: Slot, item: &Item) -> (f32, f32, f32, f32) {
    // Presence may run a little past 1 as the icon pops in; pressing sinks it, dragging lifts it.
    let presence = item.presence.pos.clamp(0.0, 1.12);
    // An icon pushed in from outside, which the skin of the glass still holds out, is out
    // there: at its own size, pressed a little flat. On its way to its place it is, for a
    // moment, as small as the room it has so far: it squeezes through.
    let held = item.held;
    let out = held.out.clamp(0.0, 1.0);
    let presence = presence + (held.size - presence) * out;
    let touch = (1.0 - 0.14 * item.press.pos.clamp(-0.4, 1.0)) * (1.0 + 0.08 * item.lift.pos.clamp(0.0, 1.2));
    let size = m.icon * slot.scale * presence * touch;
    // Swallowing a window the icon spreads sideways and flattens; letting one go it stretches up.
    let gulp = item.gulp.pos.clamp(-0.6, 0.8);
    let (w, h) = (size * item.hop.0 * (1.0 + 0.5 * gulp) * (1.0 + (held.shape.0 - 1.0) * out), size * item.hop.1 * (1.0 - 0.34 * gulp) * (1.0 + (held.shape.1 - 1.0) * out));
    let cx = slot.center() + item.shift.pos;
    // A wave under the icon lifts it. Landing in its place from outside, it swings a little
    // below it first.
    let rest = m.baseline - item.bounce_y - item.ride;
    let bottom = rest + (held.bottom - rest) * held.out;
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
