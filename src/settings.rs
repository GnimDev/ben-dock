//! The settings window: one row per entry of `config::PARAMS`, with a slider,
//! a switch or a set of choices. Every change is applied to the dock while
//! the control is being moved and written to ben.ini when it is released.
//!
//! The rows are sorted into tabs, listed down the left side (see `TABS`): a
//! tab shows the groups of settings that belong to one part of the app.
//!
//! The widgets are not rows. Their tab is a library: a tile for each, with a
//! small picture of it (painted by `widget::emblem`), its name and its switch.
//!
//! Nor are the themes: the first tab shows a picture of each of the app's own
//! sets of colours (painted by `theme::swatch`), and a click on one puts it to
//! use, with the glass it was made for (see `theme::choose`). Under them are the ways
//! the rest of Windows is made to match: two switches for the frames of other programs'
//! windows (see `frames.rs`), and two buttons that hand Windows a theme file of its own
//! kind (see `wintheme.rs`).
//!
//! One setting is not a number: which video the wallpaper plays. It has a row
//! of its own with a button that opens Windows' dialog for choosing a file.

use crate::config::{self, Config, Kind, PARAMS};
use crate::app::{self, Event};
use crate::gfx::{color, rect};
use crate::system;
use crate::theme;
use crate::util::wide;
use crate::wallpaper;
use crate::widget;
use crate::wintheme;
use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_numerics::Vector2;

const CLASS_NAME: &str = "BenSettingsWindow";
/// Told to the window when a video has been chosen in the dialog (it waits in `PICKED`).
const WM_VIDEO_PICKED: u32 = WM_APP + 1;
/// The kinds of file the dialog offers.
const VIDEO_FILES: &str = "*.mp4;*.m4v;*.mov;*.mkv;*.webm;*.wmv;*.avi";
const STYLE: WINDOW_STYLE = WINDOW_STYLE(WS_OVERLAPPED.0 | WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

// Layout, in logical pixels.
const COLUMN_W: f32 = 470.0;
const MARGIN: f32 = 22.0;
const ROW_H: f32 = 34.0;
const HEADER_H: f32 = 38.0;
const CONTROL_W: f32 = 150.0;
const VALUE_W: f32 = 46.0;
const BUTTON_H: f32 = 30.0;
/// The list of tabs down the left side: its width, the height of one tab, and the room
/// between its edge and what is in it.
const SIDE_W: f32 = 196.0;
const TAB_H: f32 = 38.0;
const SIDE_PAD: f32 = 10.0;
/// A widget's tile in the library: how wide and how tall it is, the room between two, how
/// many stand side by side, and the size of the picture on it.
const TILE_W: f32 = 134.0;
const TILE_H: f32 = 126.0;
const TILE_GAP: f32 = 12.0;
const TILES_ACROSS: usize = 3;
const EMBLEM: f32 = 60.0;
/// A theme in the gallery: as wide as a widget's tile, its picture (`theme::SWATCH`) with
/// its name under it. How tall the two are together.
const THEME_H: f32 = theme::SWATCH.1 + 28.0;

/// One tab: its name, its symbol in Windows' icon font, the groups of settings
/// (`Param::section`) it shows, in this order, and a few words that stand at the end of its
/// first heading (none: "").
struct Tab {
    name: &'static str,
    glyph: u32,
    sections: &'static [&'static str],
    hint: &'static str,
}

const TABS: [Tab; 8] = [
    Tab { name: "Theme", glyph: 0xE790, sections: &["Theme", "Đồng bộ Windows"], hint: "Chọn rồi tinh chỉnh ở Giao diện" },
    Tab { name: "Giao diện", glyph: 0xE771, sections: &["Giao diện", "Chuyển động"], hint: "" },
    Tab { name: "Dock", glyph: 0xE90E, sections: &["Dock"], hint: "" },
    Tab { name: "Đảo và bốn góc", glyph: 0xECA5, sections: &["Đảo động", "Bốn góc"], hint: "" },
    Tab { name: "Thư viện widget", glyph: 0xE8F1, sections: &["Widget", "Hẹn giờ tập trung"], hint: "Kéo widget trên màn hình nền để xếp chỗ" },
    Tab { name: "Hình nền", glyph: 0xE91B, sections: &["Hình nền"], hint: "" },
    Tab { name: "Cửa sổ", glyph: 0xE7C4, sections: &["Cửa sổ"], hint: "" },
    Tab { name: "Hành vi", glyph: 0xE713, sections: &["Hành vi"], hint: "" },
];

/// The widget a setting puts on the desktop, if it is one of those ("widget_clock": the
/// clock). Such a setting is shown as a tile of the library, not as a row.
fn widget_of(key: &str) -> Option<widget::Kind> {
    let name = key.strip_prefix("widget_")?;
    widget::Kind::ALL.into_iter().find(|kind| kind.name() == name)
}

/// The names of the app's own sets of colours, in their order: the choices of the setting
/// "scheme". They are shown as pictures in a gallery, not as a row.
fn themes() -> &'static [&'static str] {
    match PARAMS.iter().find(|p| p.key == "scheme").map(|p| p.kind) {
        Some(Kind::Choice(names)) => names,
        _ => &[],
    }
}

/// A few words about each of them, in the same order: they stand under the gallery for the
/// one in use.
const STORIES: [&str; 9] = [
    "Kính trơn, màu nhấn lấy từ Windows.",
    "Xanh của logo trên kính sẫm màu nước sâu.",
    "Nắng cam đầu ngày trên kính màu kem.",
    "Tím trên sông lúc chiều tà, nước còn giữ chút nắng ấm.",
    "Xanh ngọc dưới tán rừng ngập nước, kính sương mờ.",
    "Xám lục trên kính trắng lạnh, mọi thứ phía sau dịu đi.",
    "Đỏ đèn lồng trên kính đen viền vàng, sông đêm phố cổ.",
    "Mực chàm trên giấy: không nhoè, góc vuông vức.",
    "Hồng cánh sen trên màu xanh thẫm của nước đầm.",
];

/// The tab a group of settings is shown on. A group no tab names goes on the last one, so
/// that a new setting can never be left out of the window.
fn tab_of(section: &str) -> usize {
    TABS.iter().position(|tab| tab.sections.contains(&section)).unwrap_or(TABS.len() - 1)
}

#[derive(Clone, Copy, PartialEq)]
enum Control {
    /// Index into `PARAMS`.
    Param(usize),
    /// A widget's tile in the library; the index of its switch in `PARAMS`.
    Tile(usize),
    /// A theme in the gallery: its place in `theme::SCHEMES`.
    Scheme(usize),
    /// A few words about the theme in use, under the gallery.
    Story,
    /// Windows is handed a theme file made from the theme in use: its accent colour, and
    /// light or dark.
    WinTheme,
    /// Windows is handed back the theme it had before the first of ours.
    WinBack,
    /// Which video the wallpaper plays, and a button to choose another.
    Video,
    Header(&'static str),
    Autostart,
    Reset,
    OpenFile,
}

#[derive(Clone, Copy)]
struct Row {
    control: Control,
    /// Left edge of the row's column and top of the row, in logical pixels.
    x: f32,
    y: f32,
}

struct State {
    hwnd: HWND,
    target: ID2D1HwndRenderTarget,
    brush: ID2D1SolidColorBrush,
    text: IDWriteTextFormat,
    text_right: IDWriteTextFormat,
    text_center: IDWriteTextFormat,
    header: IDWriteTextFormat,
    icons: IDWriteTextFormat,
    /// The digits on the picture of the large clock.
    digits: IDWriteTextFormat,
    /// The pictures of the widgets, in the order of `widget::Kind::ALL`; painted when first
    /// shown, in this window's own colours.
    emblems: Vec<ID2D1Bitmap>,
    /// The accent colour they were painted in.
    emblems_in: (f32, f32, f32),
    /// The pictures of the themes, in the order of `theme::SCHEMES`; painted when first shown.
    swatches: Vec<ID2D1Bitmap>,
    /// The video the wallpaper plays, if one has been chosen.
    video: Option<String>,
    cfg: Config,
    /// The tab on show, and its rows.
    tab: usize,
    rows: Vec<Row>,
    scale: f32,
    dark: bool,
    /// The accent colour Windows has (the theme in use may have one of its own).
    accent: (f32, f32, f32),
    /// Slider being dragged.
    drag: Option<usize>,
    autostart: bool,
    /// ExplorerBlurMica is installed (the glass of File Explorer can be had).
    blur_installed: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// The tab that was on show when the window was last closed: it opens there again.
    static LAST_TAB: Cell<usize> = const { Cell::new(0) };
}

/// The dialog for choosing a video is open (one at a time), and the file chosen in it,
/// until the window has taken it.
static PICKING: AtomicBool = AtomicBool::new(false);
static PICKED: Mutex<Option<String>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    /// For looking at the window in a test: it opens behind every other window and does
    /// not take the keyboard.
    static QUIET: Cell<bool> = const { Cell::new(false) };
}

/// The rows of one tab, in one column to the right of the list of tabs, and how tall
/// they are together.
fn tab_rows(tab: usize) -> (Vec<Row>, f32) {
    let mut rows = Vec::new();
    let (x, mut y) = (SIDE_W, MARGIN - 8.0);
    // The tab's groups in the tab's own order, each under its heading.
    let mut sections: Vec<&str> = TABS[tab].sections.to_vec();
    for p in PARAMS.iter().filter(|p| p.in_window && tab_of(p.section) == tab) {
        if !sections.contains(&p.section) {
            sections.push(p.section);
        }
    }
    for section in sections {
        let mut headed = false;
        // How many tiles stand in the line of tiles that is being filled (see `Control::Tile`).
        let mut tiles = 0;
        for (index, p) in PARAMS.iter().enumerate().filter(|(_, p)| p.in_window && p.section == section) {
            if !std::mem::replace(&mut headed, true) {
                rows.push(Row { control: Control::Header(p.section), x, y });
                y += HEADER_H;
            }
            if p.key == "scheme" {
                // The gallery: a picture of each theme, side by side like the tiles.
                let lines = themes().len().div_ceil(TILES_ACROSS);
                for theme in 0..themes().len() {
                    let (across, down) = (theme % TILES_ACROSS, theme / TILES_ACROSS);
                    rows.push(Row { control: Control::Scheme(theme), x: x + MARGIN + across as f32 * (TILE_W + TILE_GAP), y: y + 4.0 + down as f32 * (THEME_H + TILE_GAP) });
                }
                y += 4.0 + lines as f32 * (THEME_H + TILE_GAP);
                rows.push(Row { control: Control::Story, x, y });
                y += ROW_H;
                continue;
            }
            if widget_of(p.key).is_some() {
                if tiles == TILES_ACROSS {
                    // The line is full: the next one begins under it.
                    y += TILE_H + TILE_GAP;
                    tiles = 0;
                }
                rows.push(Row { control: Control::Tile(index), x: x + MARGIN + tiles as f32 * (TILE_W + TILE_GAP), y });
                tiles += 1;
                continue;
            }
            // A row goes under the tiles before it.
            if std::mem::take(&mut tiles) > 0 {
                y += TILE_H;
            }
            rows.push(Row { control: Control::Param(index), x, y });
            y += ROW_H;
            if p.key == "wallpaper" {
                // Under the choice of wallpaper: which video, for when it is to be one.
                rows.push(Row { control: Control::Video, x, y });
                y += ROW_H;
            }
            if p.key == "explorer_glass" {
                // Under the switches for the windows' frames: what Windows itself is given.
                for control in [Control::WinTheme, Control::WinBack] {
                    rows.push(Row { control, x, y });
                    y += ROW_H;
                }
            }
        }
        if tiles > 0 {
            y += TILE_H + 4.0;
        }
    }
    // Starting with Windows is not a line of ben.ini, but it is how the app behaves.
    if tab == TABS.len() - 1 {
        rows.push(Row { control: Control::Autostart, x, y });
        y += ROW_H;
    }
    (rows, y + MARGIN)
}

/// The size of the window's content: as tall as the tallest tab.
fn content_size() -> (f32, f32) {
    let tallest = (0..TABS.len()).map(|tab| tab_rows(tab).1).fold(0.0, f32::max);
    // The list of tabs needs its own height too: the tabs, and the two buttons under them.
    let side = SIDE_PAD + TABS.len() as f32 * TAB_H + 2.0 * (BUTTON_H + 8.0) + MARGIN + SIDE_PAD;
    (SIDE_W + COLUMN_W, tallest.max(side))
}

/// The rows on show with tab `tab`: its own, and the two buttons at the foot of the list of
/// tabs, which are there on every tab.
fn build_rows(tab: usize, height: f32) -> Vec<Row> {
    let (mut rows, _) = tab_rows(tab);
    let foot = height - MARGIN + 4.0;
    rows.push(Row { control: Control::Reset, x: 0.0, y: foot - 2.0 * BUTTON_H - 8.0 });
    rows.push(Row { control: Control::OpenFile, x: 0.0, y: foot - BUTTON_H });
    rows
}

/// Where a tab is in the list down the left side (logical pixels).
fn tab_rect(tab: usize) -> (f32, f32, f32, f32) {
    let top = SIDE_PAD + tab as f32 * TAB_H;
    (SIDE_PAD, top, SIDE_W - SIDE_PAD, top + TAB_H - 4.0)
}

/// Opens the settings window, or brings it forward if it is already open.
pub fn open(cfg: &Config) {
    let existing = STATE.with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().map(|s| s.hwnd)));
    if let Some(hwnd) = existing {
        unsafe {
            let _ = ShowWindow(hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(hwnd);
        }
        return;
    }
    if let Err(e) = create(cfg) {
        crate::log!("could not open the settings window: {e}");
    }
}

/// The settings as currently shown in the window, if it is open.
pub fn current() -> Option<Config> {
    STATE.with(|s| s.try_borrow().ok().and_then(|s| s.as_ref().map(|s| s.cfg.clone())))
}

/// Shows new values (after ben.ini was edited by hand, for example).
pub fn sync(cfg: &Config) {
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            if let Some(state) = s.as_mut() {
                state.cfg = cfg.clone();
                unsafe {
                    let _ = InvalidateRect(Some(state.hwnd), None, false);
                }
            }
        }
    });
}

fn create(cfg: &Config) -> Result<()> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = wide(CLASS_NAME);
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance,
            // The program's own icon (the first one in its file, see `build.rs`); none if
            // the program was built without it.
            hIcon: LoadIconW(Some(instance), PCWSTR(1 as *const u16)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // Fails harmlessly if the class is already registered from an earlier open.
        RegisterClassExW(&class);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("Cài đặt Bến").as_ptr()),
            STYLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            400,
            300,
            None,
            None,
            Some(instance),
            None,
        )?;

        let dark = system::system_is_dark();
        // (BEN_LIGHT: a test looks at the light window whatever Windows is set to.)
        #[cfg(test)]
        let dark = dark && std::env::var("BEN_LIGHT").is_err();
        let on: i32 = dark as i32;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &on as *const i32 as *const _, 4);

        // Size the window so the content fits, then centre it on the screen.
        let scale = GetDpiForWindow(hwnd).max(96) as f32 / 96.0;
        let (content_w, content_h) = content_size();
        let tab = LAST_TAB.get().min(TABS.len() - 1);
        let rows = build_rows(tab, content_h);
        let mut frame = RECT { left: 0, top: 0, right: (content_w * scale).round() as i32, bottom: (content_h * scale).round() as i32 };
        let _ = AdjustWindowRectEx(&mut frame, STYLE, false, WINDOW_EX_STYLE(0));
        let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let x = work.left + ((work.right - work.left) - w).max(0) / 2;
        let y = work.top + ((work.bottom - work.top) - h).max(0) / 2;
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE);

        let mut client = RECT::default();
        let _ = GetClientRect(hwnd, &mut client);
        let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
        let target = factory.CreateHwndRenderTarget(
            &D2D1_RENDER_TARGET_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
                ..Default::default()
            },
            &D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U { width: client.right.max(1) as u32, height: client.bottom.max(1) as u32 },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            },
        )?;
        let brush = target.CreateSolidColorBrush(&color(1.0, 1.0, 1.0, 1.0), None)?;

        let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let make = |size: f32, weight: DWRITE_FONT_WEIGHT, align: DWRITE_TEXT_ALIGNMENT| -> Result<IDWriteTextFormat> {
            let format = dwrite.CreateTextFormat(
                PCWSTR(wide("Segoe UI Variable Text").as_ptr()),
                None,
                weight,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size * scale,
                PCWSTR(wide("").as_ptr()),
            )?;
            format.SetTextAlignment(align)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(format)
        };

        let state = State {
            hwnd,
            target,
            brush,
            text: make(13.5, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            text_right: make(13.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            text_center: make(13.0, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_TEXT_ALIGNMENT_CENTER)?,
            header: make(12.5, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            icons: {
                let format = dwrite.CreateTextFormat(PCWSTR(wide("Segoe Fluent Icons").as_ptr()), None, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 15.0 * scale, PCWSTR(wide("").as_ptr()))?;
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                format
            },
            digits: {
                let format = dwrite.CreateTextFormat(PCWSTR(wide("Segoe UI Variable Display").as_ptr()), None, DWRITE_FONT_WEIGHT_LIGHT, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, 25.0 * scale, PCWSTR(wide("").as_ptr()))?;
                format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                format
            },
            emblems: Vec::new(),
            emblems_in: (0.0, 0.0, 0.0),
            swatches: Vec::new(),
            video: wallpaper::video_path(),
            cfg: cfg.clone(),
            tab,
            rows,
            scale,
            dark,
            accent: system::accent_color(),
            drag: None,
            autostart: system::autostart_enabled(),
            blur_installed: crate::blurmica::installed(),
        };
        STATE.with(|s| *s.borrow_mut() = Some(state));
        #[cfg(test)]
        if QUIET.get() {
            // (With BEN_OUT it is looked at through pictures, and need not be on any screen.)
            let stay = if std::env::var("BEN_OUT").is_ok() { SET_WINDOW_POS_FLAGS(0) } else { SWP_NOMOVE };
            let _ = SetWindowPos(hwnd, Some(HWND_BOTTOM), -20000, 100, 0, 0, stay | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
            return Ok(());
        }
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
        Ok(())
    }
}

// --------------------------------------------------------------- geometry

/// Where a row's control sits (logical pixels): the slider track, the switch, or the choice strip.
fn control_rect(row: &Row, control: Control) -> (f32, f32, f32, f32) {
    let right = row.x + COLUMN_W - MARGIN;
    let cy = row.y + ROW_H * 0.5;
    match control {
        Control::Param(index) => match PARAMS[index].kind {
            Kind::Number { .. } => (right - VALUE_W - CONTROL_W, cy - 10.0, right - VALUE_W, cy + 10.0),
            Kind::Toggle => (right - 40.0, cy - 11.0, right, cy + 11.0),
            Kind::Choice(names) => {
                // Narrower cells when there are many choices, so the label still fits beside them.
                let w = if names.len() > 3 { 70.0 } else { 92.0 } * names.len() as f32;
                (right - w, cy - 13.0, right, cy + 13.0)
            }
        },
        // A tile's switch is in the middle of its foot.
        Control::Tile(_) => (row.x + TILE_W * 0.5 - 20.0, row.y + TILE_H - 32.0, row.x + TILE_W * 0.5 + 20.0, row.y + TILE_H - 10.0),
        // All of a theme takes the click: its picture and its name.
        Control::Scheme(_) => (row.x, row.y, row.x + TILE_W, row.y + THEME_H),
        Control::Autostart => (right - 40.0, cy - 11.0, right, cy + 11.0),
        // The button that opens the dialog; those that hand Windows a theme.
        Control::Video | Control::WinTheme | Control::WinBack => (right - 108.0, cy - 13.0, right, cy + 13.0),
        // The two buttons are as wide as the list of tabs they stand under.
        Control::Reset | Control::OpenFile => (row.x + SIDE_PAD, row.y, row.x + SIDE_W - SIDE_PAD, row.y + BUTTON_H),
        Control::Header(_) | Control::Story => (0.0, 0.0, 0.0, 0.0),
    }
}

fn inside(r: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= r.0 && x < r.2 && y >= r.1 && y < r.3
}

/// Slider value for a horizontal position.
fn slider_value(index: usize, track: (f32, f32, f32, f32), x: f32) -> f32 {
    let p = &PARAMS[index];
    let Kind::Number { min, max, .. } = p.kind else { return 0.0 };
    // The thumb travels between the two ends of the track, inset by its radius.
    let (a, b) = (track.0 + 8.0, track.2 - 8.0);
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    p.clean(min + (max - min) * t)
}

// ------------------------------------------------------------------ input

impl State {
    fn changed(&mut self, done: bool) {
        if done {
            self.cfg.save();
        }
        app::post(Event::Config(done));
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    /// Shows another tab.
    fn show_tab(&mut self, tab: usize) {
        if tab == self.tab || tab >= TABS.len() {
            return;
        }
        self.tab = tab;
        LAST_TAB.set(tab);
        self.rows = build_rows(tab, content_size().1);
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    fn mouse_down(&mut self, x: f32, y: f32) {
        if let Some(tab) = (0..TABS.len()).find(|&tab| inside(tab_rect(tab), x, y)) {
            self.show_tab(tab);
            return;
        }
        for row in self.rows.clone() {
            let area = control_rect(&row, row.control);
            match row.control {
                Control::Param(index) => {
                    let p = &PARAMS[index];
                    match p.kind {
                        Kind::Number { .. } => {
                            // A little slack above and below makes the thin track easy to hit.
                            if inside((area.0 - 8.0, row.y, area.2 + 8.0, row.y + ROW_H), x, y) {
                                self.drag = Some(index);
                                unsafe {
                                    SetCapture(self.hwnd);
                                }
                                (p.set)(&mut self.cfg, slider_value(index, area, x));
                                self.changed(false);
                                return;
                            }
                        }
                        Kind::Toggle => {
                            if inside((area.0 - 6.0, row.y, area.2 + 6.0, row.y + ROW_H), x, y) {
                                let on = (p.get)(&self.cfg) >= 0.5;
                                (p.set)(&mut self.cfg, if on { 0.0 } else { 1.0 });
                                self.changed(true);
                                return;
                            }
                        }
                        Kind::Choice(names) => {
                            if inside(area, x, y) {
                                let pick = ((x - area.0) / ((area.2 - area.0) / names.len() as f32)).floor();
                                // A video has to be chosen before it can be the wallpaper.
                                if p.key == "wallpaper" && p.clean(pick) as u32 == wallpaper::VIDEO && self.video.is_none() {
                                    pick_video(self.hwnd);
                                    return;
                                }
                                (p.set)(&mut self.cfg, p.clean(pick));
                                self.changed(true);
                                return;
                            }
                        }
                    }
                }
                Control::Tile(index) => {
                    // Anywhere on the tile will do, not only on its switch.
                    if inside((row.x, row.y, row.x + TILE_W, row.y + TILE_H), x, y) {
                        let p = &PARAMS[index];
                        let on = (p.get)(&self.cfg) >= 0.5;
                        (p.set)(&mut self.cfg, if on { 0.0 } else { 1.0 });
                        self.changed(true);
                        return;
                    }
                }
                Control::Scheme(index) => {
                    if inside(area, x, y) {
                        theme::choose(index, &mut self.cfg);
                        self.changed(true);
                        return;
                    }
                }
                Control::Video => {
                    if inside(area, x, y) {
                        pick_video(self.hwnd);
                        return;
                    }
                }
                Control::WinTheme => {
                    if inside(area, x, y) {
                        // Windows takes a theme by having its file opened, and then shows it
                        // among its own themes.
                        let name = themes().get(self.cfg.scheme.round() as usize).copied().unwrap_or("");
                        if let Some(path) = wintheme::make(name, &self.cfg) {
                            crate::shell::launch_soon(&path.to_string_lossy());
                        }
                        // (The theme Windows had has been kept: the row under this one says so.)
                        unsafe {
                            let _ = InvalidateRect(Some(self.hwnd), None, false);
                        }
                        return;
                    }
                }
                Control::WinBack => {
                    if inside(area, x, y) {
                        if wintheme::before().exists() {
                            crate::shell::launch_soon(&wintheme::before().to_string_lossy());
                        }
                        return;
                    }
                }
                Control::Autostart => {
                    if inside((area.0 - 6.0, row.y, area.2 + 6.0, row.y + ROW_H), x, y) {
                        system::set_autostart(!self.autostart);
                        self.autostart = system::autostart_enabled();
                        unsafe {
                            let _ = InvalidateRect(Some(self.hwnd), None, false);
                        }
                        return;
                    }
                }
                Control::Reset => {
                    if inside(area, x, y) {
                        self.cfg = Config::default();
                        self.changed(true);
                        return;
                    }
                }
                Control::OpenFile => {
                    if inside(area, x, y) {
                        crate::shell::launch_soon(&config::config_file().to_string_lossy());
                        return;
                    }
                }
                Control::Header(_) | Control::Story => {}
            }
        }
    }

    /// A video was chosen in the dialog: it is kept, and becomes the wallpaper.
    fn video_picked(&mut self) {
        let Some(path) = PICKED.lock().ok().and_then(|mut picked| picked.take()) else { return };
        wallpaper::choose_video(&path);
        self.video = Some(path);
        self.cfg.wallpaper = wallpaper::VIDEO as f32;
        self.changed(true);
    }

    fn mouse_move(&mut self, x: f32) {
        let Some(index) = self.drag else { return };
        let Some(row) = self.rows.iter().find(|r| r.control == Control::Param(index)).copied() else { return };
        let value = slider_value(index, control_rect(&row, row.control), x);
        let p = &PARAMS[index];
        if (p.get)(&self.cfg) != value {
            (p.set)(&mut self.cfg, value);
            self.changed(false);
        }
    }

    fn mouse_up(&mut self) {
        if self.drag.take().is_some() {
            unsafe {
                let _ = ReleaseCapture();
            }
            self.changed(true);
        }
    }

    // --------------------------------------------------------------- paint

    fn paint(&mut self) {
        let s = self.scale;
        let dark = self.dark;
        // (The accent colour of the theme in use, as everywhere else in the app.)
        let (ar, ag, ab) = theme::accent(&self.cfg, self.accent);
        let bg = if dark { color(0.125, 0.125, 0.135, 1.0) } else { color(0.955, 0.955, 0.965, 1.0) };
        let fg = if dark { color(1.0, 1.0, 1.0, 0.92) } else { color(0.0, 0.0, 0.0, 0.88) };
        let dim = if dark { color(1.0, 1.0, 1.0, 0.56) } else { color(0.0, 0.0, 0.0, 0.56) };
        let track = if dark { color(1.0, 1.0, 1.0, 0.16) } else { color(0.0, 0.0, 0.0, 0.14) };
        let line = if dark { color(1.0, 1.0, 1.0, 0.08) } else { color(0.0, 0.0, 0.0, 0.08) };
        let side = if dark { color(0.0, 0.0, 0.0, 0.16) } else { color(0.0, 0.0, 0.0, 0.035) };
        let plate = if dark { color(1.0, 1.0, 1.0, 0.055) } else { color(1.0, 1.0, 1.0, 0.72) };
        let accent = color(ar, ag, ab, 1.0);
        let knob = color(1.0, 1.0, 1.0, 1.0);

        let props = D2D1_BITMAP_PROPERTIES { pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }, dpiX: 96.0, dpiY: 96.0 };
        if self.swatches.is_empty() {
            // The pictures of the themes, painted once.
            let made: Result<Vec<ID2D1Bitmap>> = (0..theme::SCHEMES.len())
                .map(|index| {
                    let canvas = theme::swatch(index, s, dark, self.accent);
                    unsafe { self.target.CreateBitmap(D2D_SIZE_U { width: canvas.w, height: canvas.h }, Some(canvas.bgra.as_ptr() as *const _), canvas.w * 4, &props) }
                })
                .collect();
            match made {
                Ok(swatches) => self.swatches = swatches,
                Err(e) => crate::log!("the themes' pictures could not be made: {e}"),
            }
        }
        if self.emblems.is_empty() || self.emblems_in != (ar, ag, ab) {
            // The pictures of the widgets, in this window's own colours; painted again when
            // another theme brings another accent colour.
            self.emblems_in = (ar, ag, ab);
            let palette = widget::Palette::plain((ar, ag, ab), [fg.r, fg.g, fg.b, fg.a], [track.r, track.g, track.b, track.a]);
            let size = (EMBLEM * s).round().max(1.0) as u32;
            let made: Result<Vec<ID2D1Bitmap>> = widget::Kind::ALL
                .into_iter()
                .map(|kind| {
                    let canvas = widget::emblem(kind, size, &palette);
                    unsafe { self.target.CreateBitmap(D2D_SIZE_U { width: size, height: size }, Some(canvas.bgra.as_ptr() as *const _), size * 4, &props) }
                })
                .collect();
            match made {
                Ok(emblems) => self.emblems = emblems,
                Err(e) => crate::log!("the widgets' pictures could not be made: {e}"),
            }
        }

        let t = &self.target;
        let brush = &self.brush;
        let px = |r: (f32, f32, f32, f32)| rect(r.0 * s, r.1 * s, r.2 * s, r.3 * s);
        let round = |r: (f32, f32, f32, f32), radius: f32| D2D1_ROUNDED_RECT { rect: px(r), radiusX: radius * s, radiusY: radius * s };
        let circle = |x: f32, y: f32, r: f32| D2D1_ELLIPSE { point: Vector2 { X: x * s, Y: y * s }, radiusX: r * s, radiusY: r * s };
        let write = |text: &str, format: &IDWriteTextFormat, r: (f32, f32, f32, f32), c: &D2D1_COLOR_F| unsafe {
            let utf16: Vec<u16> = text.encode_utf16().collect();
            brush.SetColor(c);
            t.DrawText(&utf16, format, &px(r), brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        };
        let switch = |area: (f32, f32, f32, f32), on: bool| unsafe {
            brush.SetColor(if on { &accent } else { &track });
            t.FillRoundedRectangle(&round(area, 11.0), brush);
            brush.SetColor(&knob);
            let cx = if on { area.2 - 11.0 } else { area.0 + 11.0 };
            t.FillEllipse(&circle(cx, (area.1 + area.3) * 0.5, 8.0), brush);
        };

        unsafe {
            t.BeginDraw();
            t.Clear(Some(&bg));

            // The list of tabs down the left side, a shade apart from the rest, and a line
            // between the two.
            let mut client = RECT::default();
            let _ = GetClientRect(self.hwnd, &mut client);
            brush.SetColor(&side);
            t.FillRectangle(&rect(0.0, 0.0, SIDE_W * s, client.bottom as f32), brush);
            brush.SetColor(&line);
            t.FillRectangle(&rect((SIDE_W * s).round(), 0.0, (SIDE_W * s).round() + 1.0, client.bottom as f32), brush);
            for (index, tab) in TABS.iter().enumerate() {
                let area = tab_rect(index);
                let picked = index == self.tab;
                if picked {
                    // The tab on show: a plate under it and a short bar of the accent colour.
                    brush.SetColor(&track);
                    t.FillRoundedRectangle(&round(area, 7.0), brush);
                    brush.SetColor(&accent);
                    let middle = (area.1 + area.3) * 0.5;
                    t.FillRoundedRectangle(&round((area.0, middle - 8.0, area.0 + 3.0, middle + 8.0), 1.5), brush);
                }
                let symbol = char::from_u32(tab.glyph).map(String::from).unwrap_or_default();
                write(&symbol, &self.icons, (area.0 + 8.0, area.1, area.0 + 40.0, area.3), if picked { &fg } else { &dim });
                write(tab.name, if picked { &self.header } else { &self.text }, (area.0 + 44.0, area.1, area.2 - 4.0, area.3), &fg);
            }

            for (index, row) in self.rows.iter().enumerate() {
                let label_area = (row.x + MARGIN, row.y, row.x + COLUMN_W - MARGIN, row.y + ROW_H);
                let area = control_rect(row, row.control);
                match row.control {
                    Control::Header(name) => {
                        let line = (row.x + MARGIN, row.y + 8.0, row.x + COLUMN_W - MARGIN, row.y + HEADER_H);
                        write(&name.to_uppercase(), &self.header, line, &dim);
                        if index == 0 {
                            write(TABS[self.tab].hint, &self.text_right, line, &dim);
                        }
                    }
                    Control::Tile(index) => {
                        let p = &PARAMS[index];
                        let on = (p.get)(&self.cfg) >= 0.5;
                        let tile = (row.x, row.y, row.x + TILE_W, row.y + TILE_H);
                        // The tile: a plate. One whose widget is on the desktop is lit with
                        // the accent colour and rimmed in it; the others stand back.
                        brush.SetColor(&plate);
                        t.FillRoundedRectangle(&round(tile, 12.0), brush);
                        if on {
                            brush.SetColor(&color(ar, ag, ab, 0.13));
                            t.FillRoundedRectangle(&round(tile, 12.0), brush);
                            brush.SetColor(&color(ar, ag, ab, 0.9));
                            t.DrawRoundedRectangle(&round((tile.0 + 0.75, tile.1 + 0.75, tile.2 - 0.75, tile.3 - 0.75), 11.25), brush, 1.5 * s, None);
                        }
                        let (left, top) = (row.x + (TILE_W - EMBLEM) * 0.5, row.y + 10.0);
                        let kind = widget_of(p.key);
                        if let Some(emblem) = kind.and_then(|kind| widget::Kind::ALL.iter().position(|other| *other == kind)).and_then(|at| self.emblems.get(at)) {
                            t.DrawBitmap(emblem, Some(&px((left, top, left + EMBLEM, top + EMBLEM))), if on { 1.0 } else { 0.42 }, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                        }
                        if kind == Some(widget::Kind::Big) {
                            // Its digits are text: the hour, and the minutes fainter beside it.
                            let (middle, foot) = (left + EMBLEM * 0.5, top + EMBLEM * 0.72);
                            let faint = |c: &D2D1_COLOR_F, by: f32| color(c.r, c.g, c.b, c.a * by);
                            write("14", &self.digits, (left - 4.0, top, middle - 1.0, foot), &faint(&fg, if on { 1.0 } else { 0.42 }));
                            write("05", &self.digits, (middle + 1.0, top, left + EMBLEM + 4.0, foot), &faint(&fg, if on { 0.55 } else { 0.26 }));
                        }
                        write(p.label, &self.text_center, (row.x + 4.0, row.y + 72.0, row.x + TILE_W - 4.0, row.y + 92.0), if on { &fg } else { &dim });
                        switch(area, on);
                    }
                    Control::Scheme(index) => {
                        let picked = self.cfg.scheme.round() as usize == index;
                        let picture = (row.x, row.y, row.x + theme::SWATCH.0, row.y + theme::SWATCH.1);
                        if let Some(swatch) = self.swatches.get(index) {
                            t.DrawBitmap(swatch, Some(&px(picture)), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                        }
                        if picked {
                            // The one in use: a ring of its accent colour, a little way off.
                            brush.SetColor(&accent);
                            t.DrawRoundedRectangle(&round((picture.0 - 3.0, picture.1 - 3.0, picture.2 + 3.0, picture.3 + 3.0), 13.0), brush, 2.0 * s, None);
                        }
                        write(themes().get(index).copied().unwrap_or(""), &self.text_center, (area.0, picture.3 + 5.0, area.2, area.3), if picked { &fg } else { &dim });
                    }
                    Control::Story => {
                        let about = STORIES.get(self.cfg.scheme.round() as usize).copied().unwrap_or("");
                        write(about, &self.text, label_area, &dim);
                    }
                    Control::Param(index) => {
                        let p = &PARAMS[index];
                        let value = (p.get)(&self.cfg);
                        write(p.label, &self.text, label_area, &fg);
                        match p.kind {
                            Kind::Number { min, max, .. } => {
                                let cy = (area.1 + area.3) * 0.5;
                                let f = if max > min { ((value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
                                let thumb_x = area.0 + 8.0 + (area.2 - area.0 - 16.0) * f;
                                brush.SetColor(&track);
                                t.FillRoundedRectangle(&round((area.0, cy - 2.0, area.2, cy + 2.0), 2.0), brush);
                                brush.SetColor(&accent);
                                t.FillRoundedRectangle(&round((area.0, cy - 2.0, thumb_x, cy + 2.0), 2.0), brush);
                                let grabbed = self.drag == Some(index);
                                brush.SetColor(&knob);
                                t.FillEllipse(&circle(thumb_x, cy, if grabbed { 9.0 } else { 8.0 }), brush);
                                brush.SetColor(&accent);
                                t.FillEllipse(&circle(thumb_x, cy, if grabbed { 4.0 } else { 5.0 }), brush);
                                write(&p.format(value), &self.text_right, (area.2, row.y, area.2 + VALUE_W, row.y + ROW_H), &dim);
                            }
                            Kind::Toggle => {
                                // (The glass of File Explorer needs ExplorerBlurMica; say so
                                // where it is not installed, so the switch is not a dead end.)
                                if p.key == "explorer_glass" && !self.blur_installed {
                                    write("Cần cài ExplorerBlurMica", &self.text_right, (row.x + MARGIN, row.y, area.0 - 8.0, row.y + ROW_H), &dim);
                                }
                                switch(area, value >= 0.5);
                            }
                            Kind::Choice(names) => {
                                brush.SetColor(&track);
                                t.FillRoundedRectangle(&round(area, 7.0), brush);
                                let w = (area.2 - area.0) / names.len() as f32;
                                for (i, name) in names.iter().enumerate() {
                                    let cell = (area.0 + w * i as f32, area.1, area.0 + w * (i + 1) as f32, area.3);
                                    let picked = value.round() as usize == i;
                                    if picked {
                                        brush.SetColor(&accent);
                                        t.FillRoundedRectangle(&round((cell.0 + 2.0, cell.1 + 2.0, cell.2 - 2.0, cell.3 - 2.0), 5.0), brush);
                                    }
                                    write(name, &self.text_center, cell, if picked { &knob } else { &fg });
                                }
                            }
                        }
                    }
                    Control::Video => {
                        write("Video", &self.text, label_area, &fg);
                        // The name of the file, between the word and the button (what does
                        // not fit is cut off at its beginning: the end tells more).
                        let name = self.video.as_deref().map(|path| path.rsplit(['\\', '/']).next().unwrap_or(path)).unwrap_or("Chưa chọn video");
                        write(name, &self.text_right, (row.x + MARGIN + 52.0, row.y, area.0 - 12.0, row.y + ROW_H), &dim);
                        brush.SetColor(&track);
                        t.FillRoundedRectangle(&round(area, 6.0), brush);
                        write("Chọn video…", &self.text_center, area, &fg);
                    }
                    Control::WinTheme | Control::WinBack => {
                        // A button that has nothing to hand over stands back: no theme of the
                        // app's own is in use, or nothing has been kept from before.
                        let (label, button, ready) = if row.control == Control::WinTheme {
                            ("Màu nhấn, sáng / tối của Windows", "Áp dụng…", theme::Scheme::of(&self.cfg).accent.is_some())
                        } else {
                            ("Windows như trước khi áp dụng", "Trả lại…", wintheme::before().exists())
                        };
                        write(label, &self.text, label_area, &fg);
                        brush.SetColor(&track);
                        t.FillRoundedRectangle(&round(area, 6.0), brush);
                        write(button, &self.text_center, area, if ready { &fg } else { &dim });
                    }
                    Control::Autostart => {
                        write("Khởi động cùng Windows", &self.text, label_area, &fg);
                        switch(area, self.autostart);
                    }
                    Control::Reset | Control::OpenFile => {
                        brush.SetColor(&track);
                        t.FillRoundedRectangle(&round(area, 6.0), brush);
                        let text = if row.control == Control::Reset { "Về mặc định" } else { "Mở file ben.ini" };
                        write(text, &self.text_center, area, &fg);
                    }
                }
            }

            if let Err(e) = t.EndDraw(None, None) {
                crate::log!("settings window draw failed: {e}");
            }
        }
    }
}

/// Opens Windows' dialog for choosing a video file, over `owner`. The dialog has a thread of
/// its own, so that the dock goes on moving while it is open; when a file has been chosen,
/// `owner` is told with `WM_VIDEO_PICKED`. One dialog at a time.
fn pick_video(owner: HWND) {
    if PICKING.swap(true, Ordering::AcqRel) {
        return;
    }
    let owner = owner.0 as isize;
    let run = move || {
        let owner = HWND(owner as *mut _);
        // SAFETY: ordinary COM calls on interfaces this thread owns, between starting COM
        // on it and stopping it.
        let chosen = unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let chosen = ask_for_video(owner);
            CoUninitialize();
            chosen
        };
        PICKING.store(false, Ordering::Release);
        if let Some(path) = chosen {
            if let Ok(mut picked) = PICKED.lock() {
                *picked = Some(path);
            }
            unsafe {
                let _ = PostMessageW(Some(owner), WM_VIDEO_PICKED, WPARAM(0), LPARAM(0));
            }
        }
    };
    if std::thread::Builder::new().name("video dialog".into()).spawn(run).is_err() {
        PICKING.store(false, Ordering::Release);
    }
}

/// Shows the dialog and waits for it. Returns the full path of the file that was chosen,
/// or `None` if the dialog was closed without one.
unsafe fn ask_for_video(owner: HWND) -> Option<String> {
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let (name, kinds, title) = (wide("Video"), wide(VIDEO_FILES), wide("Chọn video làm hình nền"));
        dialog.SetFileTypes(&[COMDLG_FILTERSPEC { pszName: PCWSTR(name.as_ptr()), pszSpec: PCWSTR(kinds.as_ptr()) }]).ok()?;
        dialog.SetTitle(PCWSTR(title.as_ptr())).ok()?;
        // (An error here is the dialog closed without a choice.)
        dialog.Show(Some(owner)).ok()?;
        let path = dialog.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let text = path.to_string().ok();
        CoTaskMemFree(Some(path.0 as *const _));
        text
    }
}

fn point(lparam: LPARAM) -> (f32, f32) {
    (((lparam.0 & 0xffff) as i16) as f32, (((lparam.0 >> 16) & 0xffff) as i16) as f32)
}

/// Runs `f` on the window state unless it is already in use further up the call stack.
fn with_state(f: impl FnOnce(&mut State)) {
    STATE.with(|s| {
        if let Ok(mut s) = s.try_borrow_mut() {
            if let Some(state) = s.as_mut() {
                f(state);
            }
        }
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            with_state(|s| s.paint());
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            return LRESULT(0);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_LBUTTONDOWN => {
            let (x, y) = point(lparam);
            with_state(|s| s.mouse_down(x / s.scale, y / s.scale));
            return LRESULT(0);
        }
        WM_MOUSEMOVE => {
            let (x, _) = point(lparam);
            with_state(|s| s.mouse_move(x / s.scale));
            return LRESULT(0);
        }
        WM_LBUTTONUP => {
            with_state(|s| s.mouse_up());
            return LRESULT(0);
        }
        WM_VIDEO_PICKED => {
            with_state(|s| s.video_picked());
            return LRESULT(0);
        }
        WM_DESTROY => {
            STATE.with(|s| {
                if let Ok(mut s) = s.try_borrow_mut() {
                    *s = None;
                }
            });
            return LRESULT(0);
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Not a check of anything: opens the window behind every other one and goes through
    /// its tabs, a second and a half each, so that they can be looked at.
    /// Run by hand: cargo test --release settings_tab_by_tab -- --ignored
    /// (With BEN_OUT naming a folder, the window is put on a screen that is not there
    /// instead, and a picture of each tab is saved in that folder: nothing shows on the real
    /// screens. BEN_LIGHT=1 for the light window.)
    #[test]
    #[ignore]
    fn settings_tab_by_tab() {
        QUIET.set(true);
        let out = std::env::var("BEN_OUT").ok().map(std::path::PathBuf::from);
        // (A few widgets off, to see a tile of each kind.)
        let mut cfg = Config::default();
        (cfg.widget_todo, cfg.widget_sound) = (true, false);
        open(&cfg);
        let hwnd = STATE.with(|s| s.borrow().as_ref().map(|s| s.hwnd)).expect("the window");
        let started = std::time::Instant::now();
        let mut msg = MSG::default();
        let mut saved = 0;
        loop {
            let hold = if out.is_some() { 0.4 } else { 1.5 };
            let tab = (started.elapsed().as_secs_f32() / hold) as usize;
            if tab >= TABS.len() {
                break;
            }
            with_state(|state| state.show_tab(tab));
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
            // A picture of the tab once it has been on show for most of its time.
            if let (Some(out), true) = (out.as_ref(), saved == tab && started.elapsed().as_secs_f32() > (tab as f32 + 0.7) * hold) {
                let mut frame = RECT::default();
                unsafe {
                    let _ = GetWindowRect(hwnd, &mut frame);
                }
                let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
                let pixels = unsafe { crate::camera::print(hwnd, w, h) }.expect("a picture of the window");
                crate::camera::save_picture(&out.join(format!("tab{tab}.bmp")), w, h, &pixels);
                println!("tab {tab}: {w}x{h}");
                saved += 1;
            }
        }
        // A click on a theme of the gallery takes it, with the glass it was made for; the
        // window then wears that theme's accent colour (picked.bmp).
        let Some(out) = out else { return };
        with_state(|state| {
            state.show_tab(0);
            let row = state.rows.iter().find(|row| row.control == Control::Scheme(4)).copied().expect("the theme");
            state.mouse_down(row.x + 20.0, row.y + 20.0);
            assert_eq!((state.cfg.scheme, state.cfg.style, state.cfg.corner_radius), (4.0, 1.0, 38.0));
        });
        let clicked = std::time::Instant::now();
        while clicked.elapsed().as_secs_f32() < 0.3 {
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut frame = RECT::default();
        unsafe {
            let _ = GetWindowRect(hwnd, &mut frame);
        }
        let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
        let pixels = unsafe { crate::camera::print(hwnd, w, h) }.expect("a picture of the window");
        crate::camera::save_picture(&out.join("picked.bmp"), w, h, &pixels);
    }

    #[test]
    fn every_setting_of_the_window_is_on_exactly_one_tab() {
        let mut shown = vec![0; PARAMS.len()];
        let mut autostart = 0;
        for tab in 0..TABS.len() {
            let (rows, height) = tab_rows(tab);
            assert!(height > 0.0 && height <= content_size().1);
            for row in rows {
                match row.control {
                    Control::Param(index) | Control::Tile(index) => shown[index] += 1,
                    // (The themes stand for one setting together.)
                    Control::Scheme(0) => shown[PARAMS.iter().position(|p| p.key == "scheme").unwrap()] += 1,
                    Control::Autostart => autostart += 1,
                    _ => {}
                }
                if !matches!(row.control, Control::Tile(_) | Control::Scheme(_)) {
                    assert_eq!(row.x, SIDE_W);
                }
            }
        }
        for (index, p) in PARAMS.iter().enumerate() {
            assert_eq!(shown[index], p.in_window as usize, "{}", p.key);
        }
        assert_eq!(autostart, 1);
    }

    #[test]
    fn every_group_has_a_tab_and_every_tab_has_rows() {
        for p in PARAMS.iter().filter(|p| p.in_window) {
            assert!(TABS.iter().any(|tab| tab.sections.contains(&p.section)), "no tab names the group '{}'", p.section);
        }
        for tab in 0..TABS.len() {
            assert!(tab_rows(tab).0.iter().any(|row| matches!(row.control, Control::Param(_) | Control::Tile(_) | Control::Scheme(_))), "tab '{}' is empty", TABS[tab].name);
            // A tab starts with the heading of its first group.
            assert!(matches!(tab_rows(tab).0[0].control, Control::Header(_)));
        }
        assert_eq!(tab_of("a group that is new"), TABS.len() - 1);
    }

    #[test]
    fn every_widget_has_a_tile_and_the_tiles_fit_side_by_side() {
        let rows: Vec<Row> = (0..TABS.len()).flat_map(|tab| tab_rows(tab).0).collect();
        let tiles: Vec<&Row> = rows.iter().filter(|row| matches!(row.control, Control::Tile(_))).collect();
        // One for each widget there is, and each for a widget of its own.
        assert_eq!(tiles.len(), widget::Kind::ALL.len());
        for kind in widget::Kind::ALL {
            assert_eq!(tiles.iter().filter(|row| matches!(row.control, Control::Tile(index) if widget_of(PARAMS[index].key) == Some(kind))).count(), 1, "{kind:?}");
        }
        assert_eq!(widget_of("widget_clock"), Some(widget::Kind::Clock));
        assert_eq!(widget_of("widget_sundial"), None);
        assert_eq!(widget_of("wallpaper"), None);
        for (index, a) in tiles.iter().enumerate() {
            // Inside the column, clear of its edges.
            assert!(a.x >= SIDE_W + MARGIN && a.x + TILE_W <= SIDE_W + COLUMN_W - MARGIN + 0.01, "a tile at {}", a.x);
            // Its switch is on it.
            let switch = control_rect(a, a.control);
            assert!(switch.0 >= a.x && switch.2 <= a.x + TILE_W && switch.1 >= a.y && switch.3 <= a.y + TILE_H);
            for b in &tiles[..index] {
                assert!(a.x + TILE_W <= b.x || b.x + TILE_W <= a.x || a.y + TILE_H <= b.y || b.y + TILE_H <= a.y, "two tiles on each other");
            }
        }
        // What follows the tiles on their tab begins under the last of them.
        let lowest = tiles.iter().map(|row| row.y + TILE_H).fold(0.0, f32::max);
        let library = TABS.iter().position(|tab| tab.sections.contains(&"Widget")).unwrap();
        for row in tab_rows(library).0.iter().filter(|row| matches!(row.control, Control::Param(_))) {
            assert!(row.y >= lowest, "a row at {} on the tiles", row.y);
        }
    }

    #[test]
    fn every_theme_has_a_picture_in_the_gallery_and_a_click_on_it_takes_it() {
        let gallery = TABS.iter().position(|tab| tab.sections.contains(&"Theme")).unwrap();
        // The window opens on it.
        assert_eq!(gallery, 0);
        let (rows, height) = tab_rows(gallery);
        let pictures: Vec<&Row> = rows.iter().filter(|row| matches!(row.control, Control::Scheme(_))).collect();
        assert_eq!(pictures.len(), theme::SCHEMES.len());
        assert_eq!(themes().len(), theme::SCHEMES.len());
        for (index, a) in pictures.iter().enumerate() {
            assert!(a.control == Control::Scheme(index));
            // Inside the column and the window, under the heading, clear of the others.
            assert!(a.x >= SIDE_W + MARGIN && a.x + theme::SWATCH.0 <= SIDE_W + COLUMN_W - MARGIN + 0.01, "a theme at {}", a.x);
            assert!(a.y >= rows[0].y + HEADER_H && a.y + THEME_H <= height);
            for b in &pictures[..index] {
                assert!(a.x + TILE_W <= b.x || b.x + TILE_W <= a.x || a.y + THEME_H <= b.y || b.y + THEME_H <= a.y, "two themes on each other");
            }
            // The whole of it takes the click.
            assert_eq!(control_rect(a, a.control), (a.x, a.y, a.x + TILE_W, a.y + THEME_H));
        }
        assert_eq!(theme::SWATCH.0, TILE_W);
        // A few words about each, and under the gallery a line for those of the one in use.
        assert_eq!(STORIES.len(), theme::SCHEMES.len());
        let story = rows.iter().find(|row| row.control == Control::Story).expect("the line");
        assert!(pictures.iter().all(|picture| picture.y + THEME_H <= story.y) && story.y + ROW_H <= height);
    }

    #[test]
    fn under_the_gallery_are_the_ways_windows_is_made_to_match() {
        let (rows, height) = tab_rows(0);
        let at = |wanted: &dyn Fn(&Row) -> bool| rows.iter().position(|row| wanted(row)).expect("the row");
        let key = |row: &Row, key: &str| matches!(row.control, Control::Param(index) if PARAMS[index].key == key);
        // In this order: the line about the theme, a heading, the three switches, the two buttons.
        let order = [at(&|row| row.control == Control::Story), at(&|row| matches!(row.control, Control::Header("Đồng bộ Windows"))), at(&|row| key(row, "frame_borders")), at(&|row| key(row, "frame_titles")), at(&|row| key(row, "explorer_glass")), at(&|row| row.control == Control::WinTheme), at(&|row| row.control == Control::WinBack)];
        assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");
        assert!(order.windows(2).all(|pair| rows[pair[0]].y < rows[pair[1]].y));
        assert!(rows[order[6]].y + ROW_H <= height);
        // Each button once, at the end of its row.
        for control in [Control::WinTheme, Control::WinBack] {
            assert_eq!((0..TABS.len()).flat_map(|tab| tab_rows(tab).0).filter(|row| row.control == control).count(), 1);
            let row = &rows[at(&|row| row.control == control)];
            let button = control_rect(row, control);
            assert!(button.0 > row.x + COLUMN_W * 0.5 && button.2 <= row.x + COLUMN_W - MARGIN + 0.01 && button.1 >= row.y && button.3 <= row.y + ROW_H);
        }
    }

    #[test]
    fn the_video_has_a_row_of_its_own_under_the_choice_of_wallpaper() {
        let rows: Vec<Row> = (0..TABS.len()).flat_map(|tab| tab_rows(tab).0).collect();
        let video: Vec<usize> = rows.iter().enumerate().filter(|(_, row)| row.control == Control::Video).map(|(index, _)| index).collect();
        assert_eq!(video.len(), 1);
        let (above, row) = (&rows[video[0] - 1], &rows[video[0]]);
        assert!(matches!(above.control, Control::Param(index) if PARAMS[index].key == "wallpaper"));
        assert_eq!(row.y, above.y + ROW_H);
        // The choice above it has "Video" where the wallpaper says a video is.
        let Control::Param(index) = above.control else { unreachable!() };
        let Kind::Choice(names) = PARAMS[index].kind else { panic!("the wallpaper is a choice") };
        assert_eq!(names.get(wallpaper::VIDEO as usize), Some(&"Video"));
        // Its button stands at the end of the row, clear of the name of the file.
        let button = control_rect(row, row.control);
        assert!(button.0 > row.x + COLUMN_W * 0.5 && button.2 <= row.x + COLUMN_W - MARGIN + 0.01);
        assert!(button.1 >= row.y && button.3 <= row.y + ROW_H);
    }

    #[test]
    fn the_tabs_and_the_buttons_do_not_overlap() {
        let (_, height) = content_size();
        let rows = build_rows(0, height);
        let reset = rows.iter().find(|row| row.control == Control::Reset).unwrap();
        let open = rows.iter().find(|row| row.control == Control::OpenFile).unwrap();
        let last_tab = tab_rect(TABS.len() - 1);
        assert!(control_rect(reset, reset.control).1 >= last_tab.3, "the buttons start under the last tab");
        assert!(control_rect(open, open.control).1 >= control_rect(reset, reset.control).3);
        assert!(control_rect(open, open.control).3 <= height);
        assert!(control_rect(open, open.control).2 <= SIDE_W);
    }
}
