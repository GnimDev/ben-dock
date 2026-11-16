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
    // Look
    /// What the glass is made of: 0 = liquid, 1 = frosted, 2 = ink, 3 = solid.
    pub style: f32,
    /// 0 = follow Windows, 1 = dark, 2 = light.
    pub theme: f32,
    pub tint_opacity: f32,
    /// Blur of what is behind the glass. 0 = no blur.
    pub blur_radius: f32,
    /// How much of the Windows accent colour the glass takes on.
    pub accent_tint: f32,
    pub shadow: bool,

    // Dock (logical pixels, multiplied by the display scale)
    pub icon_size: f32,
    pub gap: f32,
    pub padding: f32,
    /// Corner radius as a percentage of the panel height (50 = a capsule).
    pub corner_radius: f32,
    pub margin_bottom: f32,
    pub magnification: f32,
    pub spread: f32,
    /// The drop of light that follows the cursor.
    pub lens: bool,
    pub indicator: bool,
    pub labels: bool,
    pub label_size: f32,
    pub bounce: bool,
    pub bounce_height: f32,
    pub bounce_time: f32,

    // Island
    pub island: bool,
    pub island_height: f32,
    pub island_hover: bool,
    pub island_alerts: bool,
    pub island_media: bool,

    // Corners
    pub corner_claude: bool,
    pub corner_notices: bool,
    pub corner_weather: bool,
    pub corner_system: bool,

    // Motion
    pub anim_speed: f32,
    pub elastic: f32,

    // Behaviour
    pub click_minimize: bool,
    pub hide_on_fullscreen: bool,
    pub show_on_desktop: bool,
    pub show_when_maximized: bool,
    pub hide_taskbar: bool,
    pub reserve_space: bool,
    pub launcher: bool,
    pub launcher_icon: f32,
    /// A tap of the Windows key opens the app tray instead of the Start menu.
    pub win_key: bool,

    // Windows
    /// A window that has just opened floats in the middle of its screen instead of filling it.
    pub float_windows: bool,
    /// How much of the screen's width and height a floating window takes at most.
    pub float_size: f32,
    /// 0 = Windows' own, 1 = shrink, 2 = genie, 3 = drop.
    pub minimize_effect: f32,
    pub minimize_time: f32,
    pub preview: bool,
    pub preview_delay: f32,
    pub preview_size: f32,

    // Widgets on the desktop
    pub widget_clock: bool,
    pub widget_calendar: bool,
    pub widget_monitor: bool,
    pub widget_todo: bool,
    pub widget_focus: bool,
    /// Minutes of one stretch of focus, and of the rest after it.
    pub focus_minutes: f32,
    pub break_minutes: f32,
    pub widget_note: bool,
    pub widget_progress: bool,

    // The live wallpaper
    /// 0 = off (the desktop's own picture), 1 = dark water, 2 = blue water.
    pub wallpaper: f32,
    pub wallpaper_cursor: bool,
    pub wallpaper_music: bool,
    pub wallpaper_strength: f32,

    // Diagnostics (ini only)
    pub stats: bool,
    pub hardware_gpu: bool,
    /// Most memory (MB) Windows lets the app keep in RAM. 0 = no limit.
    pub ram_limit: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            style: 0.0,
            theme: 0.0,
            tint_opacity: 0.5,
            blur_radius: 26.0,
            accent_tint: 0.10,
            shadow: true,
            icon_size: 46.0,
            gap: 8.0,
            padding: 9.0,
            corner_radius: 50.0,
            margin_bottom: 10.0,
            magnification: 1.6,
            spread: 2.4,
            lens: true,
            indicator: true,
            labels: true,
            label_size: 13.0,
            bounce: true,
            bounce_height: 0.5,
            bounce_time: 0.56,
            island: true,
            island_height: 30.0,
            island_hover: true,
            island_alerts: true,
            island_media: true,
            corner_claude: true,
            corner_notices: true,
            corner_weather: true,
            corner_system: true,
            anim_speed: 1.0,
            elastic: 0.5,
            click_minimize: true,
            hide_on_fullscreen: true,
            show_on_desktop: true,
            show_when_maximized: false,
            hide_taskbar: true,
            reserve_space: false,
            launcher: true,
            launcher_icon: 56.0,
            win_key: true,
            float_windows: true,
            float_size: 0.7,
            minimize_effect: 2.0,
            minimize_time: 0.36,
            preview: true,
            preview_delay: 0.4,
            preview_size: 220.0,
            widget_clock: true,
            widget_calendar: true,
            widget_monitor: true,
            widget_todo: false,
            widget_focus: false,
            focus_minutes: 25.0,
            break_minutes: 5.0,
            widget_note: false,
            widget_progress: false,
            wallpaper: 0.0,
            wallpaper_cursor: true,
            wallpaper_music: true,
            wallpaper_strength: 1.0,
            stats: false,
            hardware_gpu: false,
            ram_limit: 256.0,
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
    param!("style", "Giao diện", "Chất liệu", "Chất liệu của các bề mặt: 0 = kính lỏng (trong, viền sáng), 1 = sương mờ (đục hơn, dễ đọc hơn), 2 = mực (kính đen như đảo của iPhone), 3 = đặc (không làm nhoè phía sau, nhẹ máy nhất)", Kind::Choice(&["Kính lỏng", "Sương mờ", "Mực", "Đặc"]), true, num!(style)),
    param!("theme", "Giao diện", "Sáng / tối", "Sáng hay tối: 0 = theo Windows, 1 = tối, 2 = sáng", Kind::Choice(&["Theo Windows", "Tối", "Sáng"]), true, num!(theme)),
    param!("tint_opacity", "Giao diện", "Độ đục của kính", "Độ đục của kính: 0 = trong nhất, 1 = đục nhất", n(0.0, 1.0, 0.01), true, num!(tint_opacity)),
    param!("blur_radius", "Giao diện", "Độ nhoè phía sau", "Kính làm nhoè những gì nằm sau nó bao nhiêu (0 = không nhoè)", n(0.0, 60.0, 1.0), true, num!(blur_radius)),
    param!("accent_tint", "Giao diện", "Nhuộm màu nhấn", "Kính ngả sang màu nhấn của Windows bao nhiêu: 0 = không, 1 = nhiều nhất", n(0.0, 1.0, 0.01), true, num!(accent_tint)),
    param!("shadow", "Giao diện", "Đổ bóng", "Bóng mềm dưới các bề mặt", Kind::Toggle, true, flag!(shadow)),
    param!("anim_speed", "Chuyển động", "Tốc độ", "Tốc độ của mọi chuyển động. 1.0 = mặc định", n(0.5, 2.0, 0.05), true, num!(anim_speed)),
    param!("elastic", "Chuyển động", "Độ đàn hồi", "Mọi thứ nảy và co giãn nhiều hay ít: 0 = dừng êm không nảy, 1 = nảy nhiều nhất", n(0.0, 1.0, 0.05), true, num!(elastic)),
    param!("island", "Đảo động", "Bật đảo động", "Đảo động ở giữa mép trên màn hình: giờ, pin, mạng, nhạc đang phát và các thông báo nhanh", Kind::Toggle, true, flag!(island)),
    param!("island_height", "Đảo động", "Chiều cao khi thu gọn", "Chiều cao của đảo khi thu gọn (điểm ảnh)", n(24.0, 40.0, 1.0), true, num!(island_height)),
    param!("island_hover", "Đảo động", "Rê chuột để mở rộng", "Rê chuột lên đảo thì nó mở rộng thành bảng điều khiển (tắt: phải bấm)", Kind::Toggle, true, flag!(island_hover)),
    param!("island_alerts", "Đảo động", "Thông báo nhanh", "Đảo nở ra báo khi đổi âm lượng, cắm / rút sạc, pin yếu, có / mất mạng", Kind::Toggle, true, flag!(island_alerts)),
    param!("island_media", "Đảo động", "Nhạc đang phát", "Hiện bài đang phát, ảnh bìa, thanh tua và nút điều khiển trên đảo", Kind::Toggle, true, flag!(island_media)),
    param!("corner_claude", "Bốn góc", "Claude (trên trái)", "Viên kính ở góc trên bên trái: phần trăm giới hạn tuần của Claude đã dùng khi Claude Code có ghi lại, nếu không thì số token hôm nay. Đọc từ những gì Claude Code lưu trên máy này", Kind::Toggle, true, flag!(corner_claude)),
    param!("corner_notices", "Bốn góc", "Thông báo (trên phải)", "Viên kính ở góc trên bên phải: chuông và số thông báo; mỗi thông báo mới của ứng dụng nở ra từ đây", Kind::Toggle, true, flag!(corner_notices)),
    param!("corner_weather", "Bốn góc", "Thời tiết (dưới trái)", "Viên kính ở góc dưới bên trái: thời tiết nơi máy đang ở. Vị trí (làm tròn khoảng 10 km) được gửi tới Open-Meteo để hỏi thời tiết", Kind::Toggle, true, flag!(corner_weather)),
    param!("corner_system", "Bốn góc", "Máy tính (dưới phải)", "Viên kính ở góc dưới bên phải: bộ xử lý và bộ nhớ đang dùng. Bấm vào đúng góc màn hình để hiện màn hình nền", Kind::Toggle, true, flag!(corner_system)),
    param!("icon_size", "Dock", "Cỡ icon", "Cỡ icon khi đứng yên (điểm ảnh, trước khi nhân tỉ lệ màn hình)", n(24.0, 96.0, 1.0), true, num!(icon_size)),
    param!("gap", "Dock", "Khoảng cách giữa icon", "Khoảng cách giữa các icon", n(0.0, 30.0, 1.0), true, num!(gap)),
    param!("padding", "Dock", "Lề trong", "Khoảng cách từ icon tới mép dock", n(4.0, 24.0, 1.0), true, num!(padding)),
    param!("corner_radius", "Dock", "Độ bo góc", "Độ bo góc của dock, tính theo % chiều cao (50 = bo tròn thành viên thuốc)", n(0.0, 50.0, 1.0), true, num!(corner_radius)),
    param!("margin_bottom", "Dock", "Cách mép dưới màn hình", "Khoảng cách từ dock tới mép dưới màn hình", n(0.0, 40.0, 1.0), true, num!(margin_bottom)),
    param!("magnification", "Dock", "Độ phóng to", "Icon dưới con trỏ to gấp mấy lần. 1.0 = không phóng to", n(1.0, 2.5, 0.05), true, num!(magnification)),
    param!("spread", "Dock", "Độ lan", "Phóng to lan ra mấy icon ở mỗi bên con trỏ", n(1.0, 6.0, 0.1), true, num!(spread)),
    param!("lens", "Dock", "Giọt sáng theo chuột", "Giọt sáng trượt theo con trỏ giữa các icon", Kind::Toggle, true, flag!(lens)),
    param!("indicator", "Dock", "Chấm báo app đang mở", "Chấm nhỏ dưới icon của ứng dụng đang mở", Kind::Toggle, true, flag!(indicator)),
    param!("labels", "Dock", "Hiện tên khi rê chuột", "Hiện tên ứng dụng khi rê chuột", Kind::Toggle, true, flag!(labels)),
    param!("label_size", "Dock", "Cỡ chữ tên", "Cỡ chữ của tên ứng dụng", n(9.0, 20.0, 1.0), true, num!(label_size)),
    param!("bounce", "Dock", "Icon nảy khi mở app", "Icon nảy lên khi mở ứng dụng", Kind::Toggle, true, flag!(bounce)),
    param!("bounce_height", "Dock", "Độ cao khi nảy", "Icon nảy cao bao nhiêu, tính theo cỡ icon (0.5 = nửa icon)", n(0.1, 1.5, 0.05), true, num!(bounce_height)),
    param!("bounce_time", "Dock", "Thời gian một lần nảy", "Một lần nảy lên rồi rơi xuống mất mấy giây", n(0.25, 1.5, 0.01), true, num!(bounce_time)),
    param!("click_minimize", "Hành vi", "Bấm icon app đang ở trước để thu nhỏ", "Bấm vào icon của ứng dụng đang ở trước thì thu nhỏ nó", Kind::Toggle, true, flag!(click_minimize)),
    param!("hide_on_fullscreen", "Hành vi", "Ẩn khi có app toàn màn hình", "Dock và đảo trượt khỏi màn hình khi có ứng dụng chiếm toàn màn hình", Kind::Toggle, true, flag!(hide_on_fullscreen)),
    param!("show_on_desktop", "Hành vi", "Luôn hiện ở màn hình chính", "Dock, đảo và các góc luôn hiện khi không có cửa sổ nào phóng to. Tắt: chúng ẩn đi, chỉ để lại một thanh trắng ở mép dưới màn hình; rê chuột vào thanh đó thì hiện ra", Kind::Toggle, true, flag!(show_on_desktop)),
    param!("show_when_maximized", "Hành vi", "Luôn hiện khi có cửa sổ phóng to", "Dock và đảo vẫn hiện khi có cửa sổ phóng to (các góc thì luôn ẩn). Tắt: chúng ẩn đi, chỉ để lại một thanh trắng ở mép dưới màn hình; rê chuột vào thanh đó thì hiện ra, rời đi 2 giây thì ẩn lại, đảo còn lại giờ phút thêm 8 giây", Kind::Toggle, true, flag!(show_when_maximized)),
    param!("hide_taskbar", "Hành vi", "Ẩn taskbar của Windows", "Ẩn taskbar của Windows khi Bến chạy (thoát Bến thì taskbar trở lại)", Kind::Toggle, true, flag!(hide_taskbar)),
    param!("reserve_space", "Hành vi", "Cửa sổ phóng to không che dock và đảo", "Giữ chỗ ở mép trên và mép dưới để cửa sổ phóng to không che dock và đảo. Chỉ nên bật cùng với \"Luôn hiện khi có cửa sổ phóng to\"", Kind::Toggle, true, flag!(reserve_space)),
    param!("launcher", "Hành vi", "Nút khay ứng dụng trong dock", "Icon đầu tiên của dock mở khay tất cả ứng dụng", Kind::Toggle, true, flag!(launcher)),
    param!("launcher_icon", "Hành vi", "Cỡ icon trong khay ứng dụng", "Cỡ icon trong khay ứng dụng (điểm ảnh)", n(40.0, 96.0, 4.0), true, num!(launcher_icon)),
    param!("win_key", "Hành vi", "Phím Windows mở khay ứng dụng", "Bấm phím Windows một mình thì mở khay ứng dụng của Bến thay cho menu Start (gõ ngay để tìm ứng dụng). Phím Windows bấm cùng phím khác (Win+D, Win+E...) vẫn như cũ, và Ctrl+Esc vẫn mở menu Start. Cần bật nút khay ứng dụng", Kind::Toggle, true, flag!(win_key)),
    param!("float_windows", "Cửa sổ", "Cửa sổ mới mở dạng nổi", "Cửa sổ vừa mở không phóng to hết màn hình mà nổi ở giữa, chỉ chiếm một phần màn hình. Bến chỉ làm việc này một lần, lúc cửa sổ mở ra; sau đó phóng to hay kéo cỡ tuỳ ý. Cửa sổ vốn đã nhỏ, cửa sổ không đổi cỡ được và trò chơi, video toàn màn hình thì giữ nguyên", Kind::Toggle, true, flag!(float_windows)),
    param!("float_size", "Cửa sổ", "Cỡ cửa sổ nổi", "Cửa sổ nổi chiếm nhiều nhất bao nhiêu phần chiều rộng và chiều cao của màn hình (0.7 = 70%)", n(0.4, 0.95, 0.01), true, num!(float_size)),
    param!("minimize_effect", "Cửa sổ", "Hiệu ứng thu nhỏ", "Khi bấm icon trên dock để thu nhỏ hoặc mở lại cửa sổ: 0 = hoạt ảnh của Windows, 1 = co lại dần về icon, 2 = chảy vào icon qua một cái phễu cong, như hiệu ứng thần đèn trên Mac, 3 = trượt xuống icon. Thu nhỏ bằng nút trên cửa sổ hay bằng bàn phím cũng dùng hiệu ứng này (Bến tắt hoạt ảnh của Windows cho các cửa sổ đang mở). Mở lại bằng Alt+Tab hay cách khác vẫn là hoạt ảnh của Windows, và cách nào cửa sổ cũng bay về đúng icon của nó trên dock", Kind::Choice(&["Của Windows", "Co lại", "Chảy", "Trượt"]), true, num!(minimize_effect)),
    param!("minimize_time", "Cửa sổ", "Thời gian hiệu ứng thu nhỏ", "Hiệu ứng của Bến (co lại, chảy, trượt) kéo dài mấy giây. Kiểu chảy trên Mac dài khoảng 0.45", n(0.15, 1.2, 0.01), true, num!(minimize_time)),
    param!("preview", "Cửa sổ", "Xem trước cửa sổ khi rê chuột", "Rê chuột lên icon của ứng dụng đang mở thì hiện hình thu nhỏ các cửa sổ của nó", Kind::Toggle, true, flag!(preview)),
    param!("preview_delay", "Cửa sổ", "Chờ bao lâu thì hiện xem trước", "Rê chuột bao nhiêu giây thì hiện xem trước", n(0.1, 1.5, 0.05), true, num!(preview_delay)),
    param!("preview_size", "Cửa sổ", "Cỡ hình xem trước", "Chiều rộng mỗi hình xem trước (điểm ảnh)", n(120.0, 400.0, 10.0), true, num!(preview_size)),
    param!("widget_clock", "Widget", "Đồng hồ", "Widget đồng hồ trên màn hình nền: giờ lớn và ngày tháng. Mọi widget đều kéo được tới chỗ khác; vị trí được nhớ trong widgets.txt", Kind::Toggle, true, flag!(widget_clock)),
    param!("widget_calendar", "Widget", "Lịch tháng", "Widget lịch: tháng này, hôm nay được tô màu nhấn", Kind::Toggle, true, flag!(widget_calendar)),
    param!("widget_monitor", "Widget", "Theo dõi máy", "Widget theo dõi máy: bộ xử lý (kèm biểu đồ), bộ nhớ, ổ đĩa, tốc độ mạng và pin", Kind::Toggle, true, flag!(widget_monitor)),
    param!("widget_todo", "Widget", "Việc cần làm", "Widget việc cần làm: đọc từ todo.txt cạnh ben.exe, mỗi dòng một việc, dòng bắt đầu bằng 'x ' là việc đã xong. Bấm một việc để đánh dấu xong, bấm cây bút để sửa danh sách", Kind::Toggle, true, flag!(widget_todo)),
    param!("widget_focus", "Widget", "Hẹn giờ tập trung", "Widget hẹn giờ tập trung (Pomodoro): bấm để chạy hoặc tạm dừng, bấm mũi tên vòng để đặt lại. Hết giờ tập trung thì chuyển sang giờ nghỉ và ngược lại", Kind::Toggle, true, flag!(widget_focus)),
    param!("focus_minutes", "Widget", "Số phút tập trung", "Một lượt tập trung dài mấy phút", n(5.0, 90.0, 5.0), true, num!(focus_minutes)),
    param!("break_minutes", "Widget", "Số phút nghỉ", "Nghỉ mấy phút sau mỗi lượt tập trung", n(1.0, 30.0, 1.0), true, num!(break_minutes)),
    param!("widget_note", "Widget", "Ghi chú", "Widget ghi chú: hiện nội dung note.txt cạnh ben.exe. Bấm vào để mở file và sửa", Kind::Toggle, true, flag!(widget_note)),
    param!("widget_progress", "Widget", "Tiến độ thời gian", "Widget tiến độ: hôm nay, tuần này, tháng này và năm nay đã trôi qua bao nhiêu phần trăm", Kind::Toggle, true, flag!(widget_progress)),
    param!("wallpaper", "Hình nền", "Hình nền nước động", "Mặt nước động thay cho ảnh nền của Windows: 0 = tắt (giữ ảnh nền của Windows), 1 = nước tối, 2 = nước xanh. Ảnh nền của Windows không bị đổi: tắt đi là thấy lại", Kind::Choice(&["Tắt", "Nước tối", "Nước xanh"]), true, num!(wallpaper)),
    param!("wallpaper_cursor", "Hình nền", "Gợn sóng theo chuột", "Con trỏ lướt trên màn hình nền để lại gợn sóng, bấm chuột làm vòng sóng toả ra", Kind::Toggle, true, flag!(wallpaper_cursor)),
    param!("wallpaper_music", "Hình nền", "Gợn sóng theo nhạc", "Khi máy đang phát nhạc, mỗi nhịp mạnh làm một giọt rơi xuống mặt nước. Bến chỉ hỏi Windows mức âm lượng đang phát, không thu âm", Kind::Toggle, true, flag!(wallpaper_music)),
    param!("wallpaper_strength", "Hình nền", "Độ mạnh của sóng", "Sóng mạnh hay nhẹ. 1.0 = mặc định", n(0.3, 2.0, 0.05), true, num!(wallpaper_strength)),
    param!("stats", "Chẩn đoán", "", "Ghi tốc độ khung hình và lượng RAM vào ben.log", Kind::Toggle, false, flag!(stats)),
    param!("hardware_gpu", "Chẩn đoán", "", "Vẽ icon và chữ bằng card đồ hoạ thay vì bộ xử lý. Tốn thêm vài chục MB RAM, chỉ nên bật nếu hình bị lỗi. Đổi xong cần mở lại Bến", Kind::Toggle, false, flag!(hardware_gpu)),
    param!("ram_limit", "Chẩn đoán", "", "Giới hạn RAM (MB): Windows không cho Bến giữ nhiều hơn chừng này trong RAM. 0 = không giới hạn. Đổi xong cần mở lại Bến", n(0.0, 1024.0, 1.0), false, num!(ram_limit)),
];

impl Param {
    /// Clamps a value to what this setting accepts and rounds it to its step.
    pub fn clean(&self, value: f32) -> f32 {
        match self.kind {
            Kind::Number { min, max, step } => {
                let v = if step > 0.0 { (value / step).round() * step } else { value };
                // 30 steps of 0.01 come to 0.29999998, not 0.3: settle on the nearest thousandth,
                // so a value written to the file and read back is the same number.
                ((v * 1000.0).round() / 1000.0).clamp(min, max)
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

/// When ben.ini was last written, used to notice edits made while the app runs.
pub fn config_modified() -> Option<SystemTime> {
    std::fs::metadata(ini_path()).and_then(|m| m.modified()).ok()
}

fn parse_value(text: &str) -> Option<f32> {
    match text.to_ascii_lowercase().as_str() {
        "true" | "yes" | "on" => Some(1.0),
        "false" | "no" | "off" => Some(0.0),
        other => other.replace(',', ".").parse::<f32>().ok().filter(|v| v.is_finite()),
    }
}

/// Settings of the first design (0.1) whose meaning is the same in this one. A file written
/// by that version keeps these; everything about the look starts from the new defaults,
/// because the old numbers were tuned for a different material.
const CARRIED_OVER: &[&str] = &[
    "icon_size",
    "gap",
    "margin_bottom",
    "magnification",
    "spread",
    "theme",
    "indicator",
    "labels",
    "label_size",
    "bounce",
    "anim_speed",
    "click_minimize",
    "hide_on_fullscreen",
    "hide_taskbar",
    "reserve_space",
    "launcher",
    "minimize_effect",
    "minimize_time",
    "preview",
    "preview_delay",
    "preview_size",
    "stats",
];

/// Reads settings from the text of an ini file. Returns the settings and how many of the
/// known keys the file did not mention.
pub fn parse(text: &str) -> (Config, usize) {
    let lines: Vec<(String, &str)> = text
        .lines()
        .map(|l| l.trim().trim_start_matches('\u{feff}'))
        .filter(|l| !(l.is_empty() || l.starts_with('#') || l.starts_with(';') || l.starts_with('[')))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim()))
        .collect();
    // A file from before the liquid design has no "style".
    let old = !lines.is_empty() && !lines.iter().any(|(k, _)| k == "style");
    let mut c = Config::default();
    let mut missing = PARAMS.len();
    for (key, value) in &lines {
        if old && !CARRIED_OVER.contains(&key.as_str()) {
            continue;
        }
        match (PARAMS.iter().find(|p| p.key == key), parse_value(value)) {
            (Some(p), Some(v)) => {
                (p.set)(&mut c, p.clean(v));
                missing = missing.saturating_sub(1);
            }
            (Some(_), None) => crate::log!("ben.ini: '{value}' is not a valid value for {key}"),
            (None, _) => crate::log!("ben.ini: unknown key '{key}'"),
        }
    }
    (c, missing)
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
        let Ok(text) = std::fs::read_to_string(ini_path()) else {
            let c = Config::default();
            c.save();
            return c;
        };
        let (c, missing) = parse(&text);
        if missing > 0 {
            if !text.contains("style") {
                // Keep the file of the first design next to the new one.
                let kept = data_dir().join("ben-0.1.ini");
                if !kept.exists() {
                    let _ = std::fs::write(&kept, &text);
                    crate::log!("ben.ini was written by the first design; kept as ben-0.1.ini, look settings start from the new defaults");
                }
            }
            // The file predates some settings: rewrite it so every setting is listed.
            c.save();
        }
        c
    }

    /// Writes ben.ini with every setting and a comment explaining each one.
    pub fn save(&self) {
        if let Err(e) = std::fs::write(ini_path(), self.to_ini()) {
            crate::log!("could not save ben.ini: {e}");
        }
    }

    pub fn to_ini(&self) -> String {
        let mut text = String::from("# Bến - cấu hình.\n# Sửa rồi lưu file là giao diện đổi theo ngay, không cần mở lại.\n# Cũng có thể chỉnh bằng cửa sổ Cài đặt (chuột phải lên dock).\n");
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
        text
    }
}

/// Pinned apps, one per line: an exe path, or `shell:AppsFolder\<id>` for a
/// Store app. A line of `---` is a divider between them. Returns `None` when the file does not exist yet (first run).
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
    let mut text = String::from("# Ứng dụng ghim trong Bến, mỗi dòng một ứng dụng (đường dẫn .exe hoặc shell:AppsFolder\\...)\n# Một dòng --- là một vách ngăn giữa các ứng dụng\n");
    for p in pins {
        text.push_str(p);
        text.push('\n');
    }
    if let Err(e) = std::fs::write(pins_path(), text) {
        crate::log!("could not save pins.txt: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_file_reads_back_the_same() {
        let mut c = Config::default();
        c.icon_size = 33.0;
        c.style = 2.0;
        c.elastic = 0.85;
        c.island = false;
        let (back, missing) = parse(&c.to_ini());
        assert_eq!(missing, 0);
        assert_eq!(back, c);
    }

    #[test]
    fn file_of_the_first_design_keeps_what_still_means_the_same() {
        let old = "icon_size = 33\ncorner_radius = 30\ntint_opacity = 0.55\nblur = true\nblur_radius = 30\nminimize_time = 0.30\ntop_bar = true\nhide_taskbar = false\n";
        let (c, missing) = parse(old);
        assert!(missing > 0);
        assert_eq!(c.icon_size, 33.0);
        assert_eq!(c.minimize_time, 0.30);
        assert!(!c.hide_taskbar);
        // The look starts afresh.
        let fresh = Config::default();
        assert_eq!(c.corner_radius, fresh.corner_radius);
        assert_eq!(c.tint_opacity, fresh.tint_opacity);
        assert_eq!(c.blur_radius, fresh.blur_radius);
    }

    #[test]
    fn values_are_cleaned() {
        let (c, _) = parse("style = 0\nicon_size = 500\nelastic = 0,25\nmagnification = abc\n");
        assert_eq!(c.icon_size, 96.0);
        assert_eq!(c.elastic, 0.25);
        assert_eq!(c.magnification, Config::default().magnification);
    }

    #[test]
    fn every_key_is_unique() {
        for (i, p) in PARAMS.iter().enumerate() {
            assert!(!PARAMS[..i].iter().any(|q| q.key == p.key), "duplicate key {}", p.key);
        }
    }
}
