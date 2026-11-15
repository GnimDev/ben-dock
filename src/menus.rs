//! What is in each menu: the top bar's menus and the dock's own.
//!
//! Windows has no way to lift another app's menus into the bar, so "Tệp",
//! "Sửa", "Xem"... hold the commands nearly every app shares and send the
//! app in front the usual keyboard shortcut for each.

use crate::menu::Entry;
use crate::system::{Battery, ALT, CTRL, SHIFT, WIN};
use crate::wifi::{Network, Scan};

pub const OPEN: usize = 1;
pub const PIN: usize = 2;
pub const CLOSE: usize = 3;
pub const TASKBAR: usize = 10;
pub const AUTOSTART: usize = 11;
pub const SETTINGS: usize = 12;
pub const QUIT: usize = 20;
pub const WINDOWS_SETTINGS: usize = 30;
pub const LOCK: usize = 31;
pub const SLEEP: usize = 32;
pub const RESTART: usize = 33;
pub const SHUT_DOWN: usize = 34;
pub const SIGN_OUT: usize = 35;
pub const LAUNCHER: usize = 36;
pub const BEN_FOLDER: usize = 37;
pub const WINDOW_MINIMIZE: usize = 40;
pub const WINDOW_MAXIMIZE: usize = 41;
pub const WINDOW_CLOSE: usize = 42;
pub const APP_QUIT: usize = 43;
pub const VOLUME: usize = 50;
pub const MUTE: usize = 51;
pub const SOUND_SETTINGS: usize = 52;
pub const BATTERY_SETTINGS: usize = 53;
pub const NETWORK_SETTINGS: usize = 55;
pub const WIFI_PASSWORD: usize = 60;
pub const WIFI_JOIN: usize = 61;
pub const WIFI_CANCEL: usize = 62;
pub const WIFI_REFRESH: usize = 63;
pub const LOCATION_SETTINGS: usize = 64;
/// Row of a Wi-Fi network: `WIFI_NETWORK | index into the list that was shown`.
pub const WIFI_NETWORK: usize = 0x20000;
/// Commands from here up are keyboard shortcuts: `KEYS | modifiers << 8 | key`.
pub const KEYS: usize = 0x10000;

/// The menus of the top bar, left to right after the logo and the app's name.
pub const TITLES: [&str; 5] = ["Tệp", "Sửa", "Xem", "Cửa sổ", "Trợ giúp"];

const F1: u16 = 0x70;
const F5: u16 = 0x74;
const F11: u16 = 0x7A;
const LEFT: u16 = 0x25;
const RIGHT: u16 = 0x27;
const TAB: u16 = 0x09;
const PLUS: u16 = 0xBB;
const MINUS: u16 = 0xBD;

/// A row that presses a keyboard shortcut in the app in front.
fn keys(text: &str, hint: &str, mods: u8, key: u16) -> Entry {
    Entry::item(KEYS | ((mods as usize) << 8) | key as usize, text).hint(hint)
}

fn letter(text: &str, hint: &str, mods: u8, key: char) -> Entry {
    keys(text, hint, mods, key as u16)
}

/// The shortcut held in a command id, if it is one: (modifiers, key).
pub fn shortcut(id: usize) -> Option<(u8, u16)> {
    (id & KEYS != 0).then_some((((id >> 8) & 0xff) as u8, (id & 0xff) as u16))
}

/// The logo's menu: the dock itself and the computer.
pub fn logo(taskbar_shown: bool, autostart: bool) -> Vec<Entry> {
    vec![
        Entry::item(SETTINGS, "Cài đặt Bến…"),
        Entry::item(WINDOWS_SETTINGS, "Cài đặt Windows…"),
        Entry::separator(),
        Entry::item(TASKBAR, "Hiện taskbar của Windows").checked(taskbar_shown),
        Entry::item(AUTOSTART, "Khởi động cùng Windows").checked(autostart),
        Entry::separator(),
        Entry::item(SLEEP, "Ngủ"),
        Entry::item(RESTART, "Khởi động lại…"),
        Entry::item(SHUT_DOWN, "Tắt máy…"),
        Entry::separator(),
        Entry::item(LOCK, "Khoá màn hình").hint("Win+L"),
        Entry::item(SIGN_OUT, "Đăng xuất…"),
        Entry::separator(),
        Entry::item(QUIT, "Thoát Bến"),
    ]
}

/// The menu under the name of the app in front. `has_window`: there is a window to act on.
pub fn app(name: &str, has_window: bool) -> Vec<Entry> {
    vec![
        Entry::item(WINDOW_MINIMIZE, "Thu nhỏ cửa sổ").enabled(has_window),
        Entry::item(WINDOW_MAXIMIZE, "Phóng to / Thu về").enabled(has_window),
        Entry::separator(),
        Entry::item(WINDOW_CLOSE, "Đóng cửa sổ").hint("Alt+F4").enabled(has_window),
        Entry::item(APP_QUIT, &format!("Thoát {name}")).enabled(has_window),
    ]
}

/// One of the `TITLES` menus.
pub fn titled(index: usize, has_window: bool) -> Vec<Entry> {
    match index {
        0 => vec![
            letter("Mới", "Ctrl+N", CTRL, 'N'),
            letter("Mở…", "Ctrl+O", CTRL, 'O'),
            Entry::separator(),
            letter("Lưu", "Ctrl+S", CTRL, 'S'),
            letter("Lưu thành…", "Ctrl+Shift+S", CTRL | SHIFT, 'S'),
            Entry::separator(),
            letter("In…", "Ctrl+P", CTRL, 'P'),
            Entry::separator(),
            letter("Đóng thẻ", "Ctrl+W", CTRL, 'W'),
            Entry::item(WINDOW_CLOSE, "Đóng cửa sổ").hint("Alt+F4").enabled(has_window),
        ],
        1 => vec![
            letter("Hoàn tác", "Ctrl+Z", CTRL, 'Z'),
            letter("Làm lại", "Ctrl+Y", CTRL, 'Y'),
            Entry::separator(),
            letter("Cắt", "Ctrl+X", CTRL, 'X'),
            letter("Sao chép", "Ctrl+C", CTRL, 'C'),
            letter("Dán", "Ctrl+V", CTRL, 'V'),
            letter("Chọn tất cả", "Ctrl+A", CTRL, 'A'),
            Entry::separator(),
            letter("Tìm…", "Ctrl+F", CTRL, 'F'),
        ],
        2 => vec![
            keys("Phóng to", "Ctrl++", CTRL, PLUS),
            keys("Thu nhỏ", "Ctrl+-", CTRL, MINUS),
            letter("Cỡ thật", "Ctrl+0", CTRL, '0'),
            Entry::separator(),
            keys("Tải lại", "F5", 0, F5),
            keys("Toàn màn hình", "F11", 0, F11),
        ],
        3 => vec![
            Entry::item(WINDOW_MINIMIZE, "Thu nhỏ").enabled(has_window),
            Entry::item(WINDOW_MAXIMIZE, "Phóng to / Thu về").enabled(has_window),
            Entry::separator(),
            keys("Ghép nửa trái", "Win+←", WIN, LEFT).enabled(has_window),
            keys("Ghép nửa phải", "Win+→", WIN, RIGHT).enabled(has_window),
            Entry::separator(),
            keys("Tất cả cửa sổ", "Win+Tab", WIN, TAB),
            letter("Hiện màn hình nền", "Win+D", WIN, 'D'),
            keys("Chuyển ứng dụng", "Alt+Tab", ALT, TAB),
        ],
        _ => vec![
            keys("Trợ giúp của ứng dụng", "F1", 0, F1),
            Entry::separator(),
            Entry::item(LAUNCHER, "Tất cả ứng dụng"),
            Entry::item(SETTINGS, "Cài đặt Bến…"),
            Entry::item(BEN_FOLDER, "Mở thư mục của Bến"),
        ],
    }
}

/// The menu of a dock icon (`item` = name, has windows, how many, pinned), or of the dock itself.
pub fn dock(item: Option<(&str, usize, bool)>, taskbar_shown: bool, autostart: bool) -> Vec<Entry> {
    let mut rows = Vec::new();
    if let Some((name, windows, pinned)) = item {
        rows.push(Entry::header(name));
        rows.push(Entry::item(OPEN, if windows == 0 { "Mở" } else { "Mở cửa sổ mới" }));
        rows.push(Entry::item(PIN, if pinned { "Gỡ khỏi Bến" } else { "Giữ trong Bến" }));
        if windows > 0 {
            rows.push(Entry::item(CLOSE, if windows > 1 { "Đóng tất cả cửa sổ" } else { "Đóng" }));
        }
        rows.push(Entry::separator());
    }
    rows.push(Entry::item(SETTINGS, "Cài đặt…"));
    rows.push(Entry::item(TASKBAR, "Hiện taskbar của Windows").checked(taskbar_shown));
    rows.push(Entry::item(AUTOSTART, "Khởi động cùng Windows").checked(autostart));
    rows.push(Entry::separator());
    rows.push(Entry::item(QUIT, "Thoát Bến"));
    rows
}

/// The speaker's menu: a volume slider and mute.
pub fn sound(level: f32, muted: bool) -> Vec<Entry> {
    vec![
        Entry::title("Âm thanh"),
        Entry::slider(VOLUME, level),
        Entry::separator(),
        Entry::item(MUTE, "Tắt tiếng").checked(muted),
        Entry::separator(),
        Entry::item(SOUND_SETTINGS, "Mở Cài đặt âm thanh của Windows…"),
    ]
}

/// The battery's menu: how full it is and what the computer is running on.
pub fn battery(state: Option<Battery>) -> Vec<Entry> {
    let mut rows = Vec::new();
    match state {
        Some(b) => {
            rows.push(Entry::title(&format!("Pin: {}%", b.percent)));
            rows.push(Entry::header(if b.plugged { "Nguồn điện: bộ sạc" } else { "Nguồn điện: pin" }));
            if b.charging {
                rows.push(Entry::header("Đang sạc"));
            } else if let Some(minutes) = b.minutes_left.filter(|_| !b.plugged) {
                let (h, m) = (minutes / 60, minutes % 60);
                rows.push(Entry::header(&if h > 0 { format!("Còn khoảng {h} giờ {m} phút") } else { format!("Còn khoảng {m} phút") }));
            }
        }
        None => rows.push(Entry::title("Máy này không có pin")),
    }
    rows.push(Entry::separator());
    rows.push(Entry::item(BATTERY_SETTINGS, "Mở Cài đặt pin của Windows…"));
    rows
}

/// Whether a command belongs to the Wi-Fi list (the menu stays open for these).
pub fn is_wifi(id: usize) -> bool {
    id & WIFI_NETWORK != 0 || matches!(id, WIFI_PASSWORD | WIFI_JOIN | WIFI_CANCEL | WIFI_REFRESH)
}

/// The index held in a Wi-Fi network row's id.
pub fn wifi_index(id: usize) -> Option<usize> {
    (id & WIFI_NETWORK != 0).then_some(id & 0xffff)
}

/// What the Wi-Fi menu shows besides the list of networks.
pub struct WifiView<'a> {
    pub scan: &'a Scan,
    pub online: bool,
    /// The network a password is being asked for.
    pub asking: Option<&'a str>,
    /// The network being joined right now.
    pub joining: Option<&'a str>,
    /// A line about what just happened (a failed attempt).
    pub note: Option<&'a str>,
}

/// A lock (if the network needs a password) and its signal strength, as symbols.
fn wifi_symbols(network: &Network) -> String {
    let glyph = |code: u32| char::from_u32(code).unwrap_or(' ');
    let bars = match network.signal {
        0..=29 => 0xE872,
        30..=54 => 0xE873,
        55..=79 => 0xE874,
        _ => 0xE701,
    };
    if network.secure {
        format!("{}  {}", glyph(0xE72E), glyph(bars))
    } else {
        glyph(bars).to_string()
    }
}

/// The network symbol's menu: the Wi-Fi networks in range, the connected one ticked.
pub fn wifi(view: &WifiView) -> Vec<Entry> {
    let mut rows = vec![Entry::title("Wi-Fi")];
    // While a password is being asked for, its note is shown in the panel beside the list.
    if let (Some(note), None) = (view.note, view.asking) {
        rows.push(Entry::header(note));
    }
    if let Some(name) = view.joining {
        rows.push(Entry::header(&format!("Đang kết nối “{name}”…")));
    }
    match view.scan {
        Scan::Found(list) => {
            if list.is_empty() {
                rows.push(Entry::header("Không thấy mạng nào"));
            }
            let mut others = false;
            for (index, network) in list.iter().enumerate().take(12) {
                if !network.connected && !others {
                    others = true;
                    if index > 0 {
                        rows.push(Entry::separator());
                    }
                    rows.push(Entry::header("Mạng khác"));
                }
                rows.push(Entry::item(WIFI_NETWORK | index, &network.name).checked(network.connected).symbols(&wifi_symbols(network)));
            }
        }
        Scan::NoAdapter => {
            rows.push(Entry::header("Máy này không có Wi-Fi"));
            rows.push(Entry::header(if view.online { "Đang có Internet" } else { "Không có Internet" }));
        }
        Scan::Denied => {
            rows.push(Entry::header("Windows không cho xem danh sách Wi-Fi"));
            rows.push(Entry::header("vì Vị trí đang tắt"));
            rows.push(Entry::item(LOCATION_SETTINGS, "Mở Cài đặt Vị trí của Windows…"));
        }
        Scan::Failed(code) => rows.push(Entry::header(&format!("Không đọc được danh sách Wi-Fi (lỗi {code})"))),
    }
    rows.push(Entry::separator());
    if matches!(view.scan, Scan::Found(_)) {
        rows.push(Entry::item(WIFI_REFRESH, "Tìm lại mạng"));
    }
    rows.push(Entry::item(NETWORK_SETTINGS, "Mở Cài đặt mạng của Windows…"));
    rows
}

/// The panel that opens beside a network of the list to ask for its password.
pub fn wifi_password(name: &str, note: Option<&str>) -> Vec<Entry> {
    vec![
        Entry::title(name),
        Entry::header(note.unwrap_or("Nhập mật khẩu Wi-Fi")),
        Entry::input(WIFI_PASSWORD, "Mật khẩu"),
        Entry::item(WIFI_JOIN, "Kết nối").hint("Enter"),
        Entry::item(WIFI_CANCEL, "Huỷ"),
    ]
}
