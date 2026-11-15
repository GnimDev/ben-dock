//! Settings (`ben.ini`) and the pinned-app list (`pins.txt`).
//!
//! Every setting is described once, in `PARAMS`. The ini reader, the ini
//! writer and the settings window are all generated from that table, so a
//! new setting only has to be added in one place.

use crate::util::data_dir;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    // Size and spacing (logical pixels, multiplied by the display scale)
    pub icon_size: f32,
    pub gap: f32,
    pub margin_bottom: f32,
    pub padding: f32,
    /// Corner radius as a percentage of the panel height (50 = fully round ends).
    pub corner_radius: f32,

    // Magnification
    pub magnification: f32,
    pub spread: f32,
    pub mag_speed: f32,
    /// How much the panel rises with the magnified icons: 0 = stays flat, 1 = rises as much as the icons grow.
    pub panel_grow: f32,

    // Look
    /// 0 = follow Windows, 1 = dark, 2 = light.
    pub theme: f32,
    pub blur: bool,
    pub blur_radius: f32,
    pub tint_opacity: f32,
    pub indicator: bool,

    // Top bar and launcher
    pub top_bar: bool,
    pub bar_height: f32,
    pub launcher: bool,
    pub launcher_icon: f32,

    // Motion
    pub anim_speed: f32,
    pub labels: bool,
    pub label_size: f32,
    pub bounce: bool,
    pub bounce_height: f32,
    pub bounce_time: f32,

    // Behaviour
    pub click_minimize: bool,
    pub hide_on_fullscreen: bool,
    pub hide_taskbar: bool,
    pub reserve_space: bool,

    // Window effects
    /// 0 = off, 1 = shrink, 2 = funnel, 3 = drop.
    pub minimize_effect: f32,
    pub minimize_time: f32,
    pub preview: bool,
    pub preview_delay: f32,
    pub preview_size: f32,

    // Diagnostics (ini only)
    pub stats: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            icon_size: 52.0,
            gap: 8.0,
            margin_bottom: 8.0,
            padding: 8.0,
            corner_radius: 30.0,
            magnification: 1.9,
            spread: 2.6,
            mag_speed: 26.0,
            panel_grow: 0.0,
            theme: 0.0,
            blur: true,
            blur_radius: 30.0,
            tint_opacity: 0.55,
            indicator: true,
            top_bar: true,
            bar_height: 30.0,
            launcher: true,
            launcher_icon: 72.0,
            anim_speed: 1.0,
            labels: true,
            label_size: 13.0,
            bounce: true,
            bounce_height: 0.55,
            bounce_time: 0.62,
            click_minimize: true,
            hide_on_fullscreen: true,
            hide_taskbar: true,
            reserve_space: true,
            minimize_effect: 2.0,
            minimize_time: 0.42,
            preview: true,
            preview_delay: 0.45,
            preview_size: 220.0,
            stats: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    /// A number between `min` and `max`, moved in steps of `step`.
    Number { min: f32, max: f32, step: f32 },
    Toggle,
    /// One of a few named choices; the value is the index.
    Choice(&'static [&'static str]),
}

pub struct Param {
    pub key: &'static str,
    /// Heading shown above the first setting of a group.
    pub section: &'static str,
    /// Name in the settings window.
    pub label: &'static str,
    /// Comment written above the key in ben.ini.
    pub help: &'static str,
    pub kind: Kind,
    /// Shown in the settings window (diagnostic settings are ini-only).
    pub in_window: bool,
    pub get: fn(&Config) -> f32,
    pub set: fn(&mut Config, f32),
}

fn b(v: bool) -> f32 {
    if v {
        1.0
    } else {
        0.0
    }
}

macro_rules! num {
    ($field:ident) => {
        (|c: &Config| c.$field, |c: &mut Config, v: f32| c.$field = v)
    };
}
macro_rules! flag {
    ($field:ident) => {
        (|c: &Config| b(c.$field), |c: &mut Config, v: f32| c.$field = v >= 0.5)
    };
}
macro_rules! param {
    ($key:literal, $section:literal, $label:literal, $help:literal, $kind:expr, $win:expr, $acc:expr) => {
        Param { key: $key, section: $section, label: $label, help: $help, kind: $kind, in_window: $win, get: $acc.0, set: $acc.1 }
    };
}

const fn n(min: f32, max: f32, step: f32) -> Kind {
    Kind::Number { min, max, step }
}

pub const PARAMS: &[Param] = &[
    param!("icon_size", "Kích thước", "Cỡ icon", "Cỡ icon khi đứng yên (điểm ảnh, trước khi nhân tỉ lệ màn hình)", n(24.0, 128.0, 1.0), true, num!(icon_size)),
    param!("gap", "Kích thước", "Khoảng cách giữa icon", "Khoảng cách giữa các icon", n(0.0, 40.0, 1.0), true, num!(gap)),
    param!("padding", "Kích thước", "Lề trong của nền", "Khoảng cách từ icon tới mép nền dock", n(2.0, 30.0, 1.0), true, num!(padding)),
    param!("corner_radius", "Kích thước", "Độ bo góc", "Độ bo góc của nền, tính theo % chiều cao nền (50 = bo tròn hết cỡ)", n(0.0, 50.0, 1.0), true, num!(corner_radius)),
    param!("margin_bottom", "Kích thước", "Cách mép dưới màn hình", "Khoảng cách từ dock tới mép dưới màn hình", n(0.0, 60.0, 1.0), true, num!(margin_bottom)),
    param!("magnification", "Phóng to", "Độ phóng to", "Icon dưới con trỏ to gấp mấy lần. 1.0 = tắt phóng to", n(1.0, 3.0, 0.05), true, num!(magnification)),
    param!("spread", "Phóng to", "Độ lan", "Hiệu ứng phóng to lan ra mấy icon ở mỗi bên con trỏ", n(1.0, 6.0, 0.1), true, num!(spread)),
    param!("mag_speed", "Phóng to", "Tốc độ phóng to", "Icon phồng lên / xẹp xuống nhanh cỡ nào khi chuột vào / ra dock (số lớn = nhanh)", n(6.0, 60.0, 1.0), true, num!(mag_speed)),
    param!("panel_grow", "Phóng to", "Nền cao lên theo icon", "Khi icon phóng to, nền dock cao lên theo bao nhiêu: 0 = nền giữ nguyên chiều cao (icon nhô lên trên), 1 = nền cao lên bằng đúng phần icon to ra", n(0.0, 1.0, 0.05), true, num!(panel_grow)),
    param!("theme", "Giao diện", "Màu nền", "Màu nền: 0 = theo Windows, 1 = tối, 2 = sáng", Kind::Choice(&["Theo Windows", "Tối", "Sáng"]), true, num!(theme)),
    param!("tint_opacity", "Giao diện", "Độ đục của nền", "Độ đục của nền: 0 = trong suốt, 1 = đặc", n(0.0, 1.0, 0.01), true, num!(tint_opacity)),
    param!("blur", "Giao diện", "Nền mờ kính", "Nền mờ kính (làm nhòe những gì nằm sau dock)", Kind::Toggle, true, flag!(blur)),
    param!("blur_radius", "Giao diện", "Độ mờ", "Độ mờ của nền kính", n(0.0, 100.0, 1.0), true, num!(blur_radius)),
    param!("indicator", "Giao diện", "Vạch báo app đang mở", "Vạch nhỏ dưới icon của ứng dụng đang mở", Kind::Toggle, true, flag!(indicator)),
    param!("top_bar", "Thanh trên và lưới ứng dụng", "Thanh trên cùng", "Thanh trên cùng màn hình: tên ứng dụng đang dùng, tìm kiếm, mạng, âm lượng, pin, đồng hồ", Kind::Toggle, true, flag!(top_bar)),
    param!("bar_height", "Thanh trên và lưới ứng dụng", "Chiều cao thanh trên", "Chiều cao thanh trên cùng (điểm ảnh)", n(22.0, 44.0, 1.0), true, num!(bar_height)),
    param!("launcher", "Thanh trên và lưới ứng dụng", "Nút lưới ứng dụng trong dock", "Icon đầu tiên của dock mở lưới tất cả ứng dụng toàn màn hình", Kind::Toggle, true, flag!(launcher)),
    param!("launcher_icon", "Thanh trên và lưới ứng dụng", "Cỡ icon trong lưới", "Cỡ icon trong lưới ứng dụng (điểm ảnh). Đổi xong cần mở lại Bến", n(48.0, 128.0, 4.0), true, num!(launcher_icon)),
    param!("anim_speed", "Chuyển động", "Tốc độ chuyển động chung", "Tốc độ các chuyển động khác (icon trượt vào / ra, dock trượt lên / xuống). 1.0 = mặc định", n(0.3, 3.0, 0.05), true, num!(anim_speed)),
    param!("labels", "Chuyển động", "Hiện tên khi rê chuột", "Hiện tên ứng dụng khi rê chuột", Kind::Toggle, true, flag!(labels)),
    param!("label_size", "Chuyển động", "Cỡ chữ tên", "Cỡ chữ của tên ứng dụng", n(9.0, 24.0, 1.0), true, num!(label_size)),
    param!("bounce", "Chuyển động", "Icon nảy khi mở app", "Icon nảy lên khi mở ứng dụng", Kind::Toggle, true, flag!(bounce)),
    param!("bounce_height", "Chuyển động", "Độ cao khi nảy", "Icon nảy cao bao nhiêu, tính theo cỡ icon (0.55 = hơn nửa icon)", n(0.1, 1.5, 0.05), true, num!(bounce_height)),
    param!("bounce_time", "Chuyển động", "Thời gian một lần nảy", "Một lần nảy lên rồi rơi xuống mất mấy giây", n(0.25, 1.5, 0.01), true, num!(bounce_time)),
    param!("click_minimize", "Hành vi", "Bấm icon app đang ở trước để thu nhỏ", "Bấm vào icon của ứng dụng đang ở trước thì thu nhỏ nó", Kind::Toggle, true, flag!(click_minimize)),
    param!("hide_on_fullscreen", "Hành vi", "Ẩn dock khi có app toàn màn hình", "Dock trượt xuống khi có ứng dụng chiếm toàn màn hình", Kind::Toggle, true, flag!(hide_on_fullscreen)),
    param!("hide_taskbar", "Hành vi", "Ẩn taskbar của Windows", "Ẩn taskbar của Windows khi Bến chạy (thoát Bến thì taskbar trở lại)", Kind::Toggle, true, flag!(hide_taskbar)),
    param!("reserve_space", "Hành vi", "Cửa sổ phóng to không che dock", "Giữ chỗ để cửa sổ phóng to không che dock", Kind::Toggle, true, flag!(reserve_space)),
    param!("minimize_effect", "Cửa sổ", "Hiệu ứng thu nhỏ", "Hiệu ứng khi thu nhỏ cửa sổ và khi mở lại từ dock: 0 = tắt, 1 = co lại dần về icon, 2 = hút xuống icon, 3 = trượt xuống icon. Khi bật, Bến tạm tắt hiệu ứng thu nhỏ có sẵn của Windows để hai hiệu ứng không chồng lên nhau", Kind::Choice(&["Tắt", "Co lại", "Hút", "Trượt"]), true, num!(minimize_effect)),
    param!("minimize_time", "Cửa sổ", "Thời gian hiệu ứng thu nhỏ", "Hiệu ứng thu nhỏ / mở lại kéo dài mấy giây", n(0.15, 1.2, 0.01), true, num!(minimize_time)),
    param!("preview", "Cửa sổ", "Xem trước cửa sổ khi rê chuột", "Rê chuột lên icon của ứng dụng đang mở thì hiện hình thu nhỏ các cửa sổ của nó", Kind::Toggle, true, flag!(preview)),
    param!("preview_delay", "Cửa sổ", "Chờ bao lâu thì hiện xem trước", "Rê chuột bao nhiêu giây thì hiện xem trước", n(0.1, 1.5, 0.05), true, num!(preview_delay)),
    param!("preview_size", "Cửa sổ", "Cỡ hình xem trước", "Chiều rộng mỗi hình xem trước (điểm ảnh)", n(120.0, 400.0, 10.0), true, num!(preview_size)),
    param!("stats", "Chẩn đoán", "", "Ghi tốc độ khung hình vào ben.log (để tìm nguyên nhân giật)", Kind::Toggle, false, flag!(stats)),
];

impl Param {
    /// Clamps a value to what this setting accepts and rounds it to its step.
    pub fn clean(&self, value: f32) -> f32 {
        match self.kind {
            Kind::Number { min, max, step } => {
                let v = if step > 0.0 { (value / step).round() * step } else { value };
                v.clamp(min, max)
            }
            Kind::Toggle => b(value >= 0.5),
            Kind::Choice(names) => value.round().clamp(0.0, (names.len() - 1) as f32),
        }
    }

    /// The value as written in ben.ini and shown in the settings window.
    pub fn format(&self, value: f32) -> String {
        match self.kind {
            Kind::Number { step, .. } => {
                if step >= 1.0 {
                    format!("{}", value.round() as i64)
                } else if step >= 0.1 {
                    format!("{value:.1}")
                } else {
                    format!("{value:.2}")
                }
            }
            Kind::Toggle => (if value >= 0.5 { "true" } else { "false" }).to_string(),
            Kind::Choice(_) => format!("{}", value.round() as i64),
        }
    }
}

fn ini_path() -> PathBuf {
    data_dir().join("ben.ini")
}

fn pins_path() -> PathBuf {
    data_dir().join("pins.txt")
}

pub fn config_file() -> PathBuf {
    ini_path()
}

/// When ben.ini was last written, used to notice edits made while the dock runs.
pub fn config_modified() -> Option<SystemTime> {
    std::fs::metadata(ini_path()).and_then(|m| m.modified()).ok()
}

fn parse_value(text: &str) -> Option<f32> {
    match text.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" => Some(1.0),
        "false" | "no" | "off" => Some(0.0),
        // Older files spelled the theme out.
        "auto" => Some(0.0),
        "dark" => Some(1.0),
        "light" => Some(2.0),
        other => other.replace(',', ".").parse::<f32>().ok().filter(|v| v.is_finite()),
    }
}

impl Config {
    pub fn is_dark_forced(&self) -> Option<bool> {
        match self.theme.round() as i32 {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        }
    }

    pub fn load() -> Self {
        let mut c = Config::default();
        let Ok(text) = std::fs::read_to_string(ini_path()) else {
            c.save();
            return c;
        };
        let mut missing = PARAMS.len();
        for line in text.lines() {
            let line = line.trim().trim_start_matches('\u{feff}');
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') || line.starts_with('[') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let key = k.trim().to_ascii_lowercase();
            match (PARAMS.iter().find(|p| p.key == key), parse_value(v.trim())) {
                (Some(p), Some(value)) => {
                    (p.set)(&mut c, p.clean(value));
                    missing = missing.saturating_sub(1);
                }
                (Some(_), None) => crate::log!("ben.ini: '{}' is not a valid value for {key}", v.trim()),
                (None, _) => crate::log!("ben.ini: unknown key '{key}'"),
            }
        }
        if missing > 0 {
            // The file predates some settings: rewrite it so every setting is listed.
            c.save();
        }
        c
    }

    /// Writes ben.ini with every setting and a comment explaining each one.
    pub fn save(&self) {
        let mut text = String::from("# Bến - cấu hình.\n# Sửa rồi lưu file là dock đổi theo ngay, không cần mở lại.\n# Cũng có thể chỉnh bằng cửa sổ Cài đặt (chuột phải lên dock).\n");
        let mut section = "";
        for p in PARAMS {
            if p.section != section {
                section = p.section;
                text.push_str(&format!("\n# ===== {section} =====\n"));
            }
            text.push_str(&format!("\n# {}", p.help));
            if let Kind::Number { min, max, .. } = p.kind {
                text.push_str(&format!(" [{} .. {}]", p.format(min), p.format(max)));
            }
            text.push_str(&format!("\n{} = {}\n", p.key, p.format((p.get)(self))));
        }
        if let Err(e) = std::fs::write(ini_path(), text) {
            crate::log!("could not save ben.ini: {e}");
        }
    }
}

/// Pinned apps, one per line: an exe path, or `shell:AppsFolder\<id>` for a
/// Store app. Returns `None` when the file does not exist yet (first run).
pub fn load_pins() -> Option<Vec<String>> {
    let text = std::fs::read_to_string(pins_path()).ok()?;
    Some(
        text.lines()
            .map(|l| l.trim().trim_start_matches('\u{feff}'))
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(str::to_string)
            .collect(),
    )
}

pub fn save_pins(pins: &[String]) {
    let mut text = String::from("# Ứng dụng ghim trong Bến, mỗi dòng một ứng dụng (đường dẫn .exe hoặc shell:AppsFolder\\...)\n");
    for p in pins {
        text.push_str(p);
        text.push('\n');
    }
    if let Err(e) = std::fs::write(pins_path(), text) {
        crate::log!("could not save pins.txt: {e}");
    }
}
