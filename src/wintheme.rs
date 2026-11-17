//! Windows' own colours from a theme of the app's.
//!
//! Windows keeps its look in a theme file: an .ini text that says, among other
//! things, what its accent colour is and whether apps and the system are light
//! or dark. Most of what a program cannot be given colours for from outside
//! follows those two: the Settings app, File Explorer, the buttons, switches
//! and links of apps, the controls of web pages.
//!
//! So the app writes a theme file for Windows from the theme in use: the
//! theme Windows has now, with nothing changed but its name, the accent colour
//! and light or dark. Opening such a file is Windows' own way of taking a
//! theme, and Windows then shows it among its themes. The app writes the file
//! and opens it only when the user asks for it. The theme Windows had before
//! the first of ours is kept beside them, so that it can be had back the same
//! way.

use crate::config::Config;
use crate::ranking;
use crate::system;
use crate::theme;
use crate::util::data_dir;
use std::path::{Path, PathBuf};

/// What every theme file of ours is called first, in Windows' list of themes.
const OURS: &str = "Ben - ";
/// What the theme Windows had before is called there.
const BEFORE: &str = "Windows truoc Ben";

/// The folder the theme files are written to.
fn folder() -> PathBuf {
    data_dir().join("themes")
}

/// The file that keeps the theme Windows had before it was first given one of ours.
pub fn before() -> PathBuf {
    folder().join(format!("{BEFORE}.theme"))
}

/// What a theme is called in Windows' list: plain letters only, so that it reads the same
/// whatever Windows takes the file's letters for ("Bến nước": "Ben - Ben nuoc").
fn display_name(name: &str) -> String {
    let plain = ranking::normalize(name);
    let mut letters = plain.chars();
    let first: String = letters.next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
    format!("{OURS}{first}{}", letters.as_str())
}

/// A theme file's text, and whether it is written with two bytes to a letter (UTF-16).
/// One written with one byte to a letter is kept byte for byte, whatever letters those
/// bytes stand for on this computer: nothing is put into it but plain letters.
fn read(path: &Path) -> Option<(String, bool)> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
        return Some((String::from_utf16_lossy(&units), true));
    }
    Some((bytes.iter().map(|&byte| byte as char).collect(), false))
}

fn write(path: &Path, text: &str, wide: bool) -> std::io::Result<()> {
    let bytes: Vec<u8> = if wide { [0xFF, 0xFE].into_iter().chain(text.encode_utf16().flat_map(|unit| unit.to_le_bytes())).collect() } else { text.chars().map(|c| c as u32 as u8).collect() };
    std::fs::create_dir_all(folder())?;
    std::fs::write(path, bytes)
}

/// The text of a theme file made from another: `base` under another name and, with
/// `looks`, saying those things about the look of Windows (a key and what it is set to)
/// in place of what `base` says about them. Everything else stays as it is: the picture
/// of the desktop, the sounds, the cursors. `None` if `base` has no part about the look:
/// Windows would not take it for a theme.
fn rewritten(base: &str, name: &str, looks: &[(&str, String)]) -> Option<String> {
    let in_part = |section: &str, part: &str| section.eq_ignore_ascii_case(part);
    // Says what has not been said yet, at the end of the part about the look (before the
    // empty lines that close it).
    let finish = |out: &mut Vec<String>, said: &mut Vec<bool>| {
        let mut at = out.len();
        while at > 0 && out[at - 1].trim().is_empty() {
            at -= 1;
        }
        for (index, (key, value)) in looks.iter().enumerate() {
            if !std::mem::replace(&mut said[index], true) {
                out.insert(at, format!("{key}={value}"));
                at += 1;
            }
        }
    };
    let mut out: Vec<String> = Vec::new();
    let mut said = vec![false; looks.len()];
    let (mut section, mut has_look) = (String::new(), false);
    for line in base.lines() {
        let words = line.trim();
        if words.starts_with('[') && words.ends_with(']') {
            if in_part(&section, "[VisualStyles]") {
                finish(&mut out, &mut said);
            }
            section = words.to_string();
            has_look |= in_part(&section, "[VisualStyles]");
            out.push(line.to_string());
            continue;
        }
        let key = words.split_once('=').map(|(key, _)| key.trim());
        if in_part(&section, "[Theme]") {
            match key {
                Some(key) if key.eq_ignore_ascii_case("DisplayName") => {
                    out.push(format!("DisplayName={name}"));
                    continue;
                }
                // (The mark of the theme this was made from: this one is another.)
                Some(key) if key.eq_ignore_ascii_case("ThemeId") => continue,
                _ => {}
            }
        }
        if in_part(&section, "[VisualStyles]") {
            if let Some(index) = key.and_then(|key| looks.iter().position(|(name, _)| name.eq_ignore_ascii_case(key))) {
                if !std::mem::replace(&mut said[index], true) {
                    out.push(format!("{}={}", looks[index].0, looks[index].1));
                }
                continue;
            }
        }
        out.push(line.to_string());
    }
    if in_part(&section, "[VisualStyles]") {
        finish(&mut out, &mut said);
    }
    has_look.then(|| out.join("\r\n") + "\r\n")
}

/// The text of the theme file for one of our themes: `base`, the theme Windows has, under
/// the name `name`, with this accent colour, and light or dark.
pub fn text(base: &str, name: &str, accent: (f32, f32, f32), dark: bool) -> Option<String> {
    let part = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u32;
    let mode = if dark { "Dark" } else { "Light" };
    // (The colour as Windows' own themes write it: C4, then red, green and blue.)
    let colour = format!("0XC4{:02X}{:02X}{:02X}", part(accent.0), part(accent.1), part(accent.2));
    rewritten(base, name, &[("AutoColorization", "0".to_string()), ("ColorizationColor", colour), ("SystemMode", mode.to_string()), ("AppMode", mode.to_string())])
}

/// True if a theme file's text is one of ours, or the one kept from before them.
fn is_ours(text: &str) -> bool {
    text.lines().filter_map(|line| line.trim().strip_prefix("DisplayName=")).any(|name| name.starts_with(OURS) || name == BEFORE)
}

/// Writes the theme file for the theme in use and says where it is. `name`: what that
/// theme is called. The first time, the theme Windows has is kept too (see `before`).
/// `None`: no theme of the app's own is in use, or Windows' theme could not be read.
pub fn make(name: &str, cfg: &Config) -> Option<PathBuf> {
    let accent = theme::Scheme::of(cfg).accent?;
    // (Light or dark as the app's surfaces are told to be; left to Windows, as Windows is.)
    let dark = cfg.is_dark_forced().unwrap_or_else(system::system_is_dark);
    // The theme Windows has; failing that, the one it comes with.
    let usual = std::env::var_os("SystemRoot").map(|root| PathBuf::from(root).join("Resources\\Themes\\aero.theme"));
    let (base, wide) = system::windows_theme_file().and_then(|path| read(&path)).or_else(|| usual.and_then(|path| read(&path)))?;
    if !before().exists() && !is_ours(&base) {
        if let Some(kept) = rewritten(&base, BEFORE, &[]) {
            if let Err(e) = write(&before(), &kept, wide) {
                crate::log!("the theme Windows had could not be kept: {e}");
            }
        }
    }
    let name = display_name(name);
    let path = folder().join(format!("{name}.theme"));
    match write(&path, &text(&base, &name, accent, dark)?, wide) {
        Ok(()) => Some(path),
        Err(e) => {
            crate::log!("the theme file for Windows could not be written: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A theme file as Windows writes it for a look the user has changed by hand, cut short.
    const UNSAVED: &str = "; Copyright \u{a9} Microsoft Corp.\r\n\r\n[Theme]\r\n; Windows - IDS_THEME_DISPLAYNAME_THEMEI\r\nDisplayName=Unsaved Theme\r\nThemeId={D76DD2BD-28BF-4082-9462-062A1D506D72}\r\n\r\n[Control Panel\\Desktop]\r\nWallpaper=C:\\Pictures\\b\u{ea}n.jpg\r\nPattern=\r\n\r\n[VisualStyles]\r\nPath=%SystemRoot%\\resources\\themes\\Aero\\Aero.msstyles\r\nColorStyle=NormalColor\r\nSize=NormalSize\r\nAutoColorization=1\r\nColorizationColor=0XC40078D4\r\nSystemMode=Dark\r\nAppMode=Dark\r\nVisualStyleVersion=10\r\n\r\n[MasterThemeSelector]\r\nMTSM=RJSPBS\r\n\r\n[Sounds]\r\nSchemeName=@%SystemRoot%\\System32\\mmres.dll,-800\r\n";

    #[test]
    fn a_theme_for_windows_changes_the_name_the_accent_and_light_or_dark_only() {
        let made = text(UNSAVED, "Ben - Binh minh", (0.890, 0.440, 0.140), false).expect("a theme");
        let lines: Vec<&str> = made.lines().collect();
        assert!(lines.contains(&"DisplayName=Ben - Binh minh"));
        assert!(lines.contains(&"AutoColorization=0"));
        assert!(lines.contains(&"ColorizationColor=0XC4E37024"));
        assert!(lines.contains(&"SystemMode=Light") && lines.contains(&"AppMode=Light"));
        // The mark of the theme it was made from is gone; nothing is said twice.
        assert!(!made.contains("ThemeId") && !made.contains("Unsaved Theme") && !made.contains("0078D4"));
        for key in ["DisplayName=", "AutoColorization=", "ColorizationColor=", "SystemMode=", "AppMode="] {
            assert_eq!(lines.iter().filter(|line| line.starts_with(key)).count(), 1, "{key}");
        }
        // Every other line is there as it was, in its place.
        let untouched = |text: &str| -> Vec<String> { text.lines().filter(|line| !["DisplayName=", "ThemeId=", "AutoColorization=", "ColorizationColor=", "SystemMode=", "AppMode="].iter().any(|key| line.starts_with(key))).map(String::from).collect() };
        assert_eq!(untouched(&made), untouched(UNSAVED));
        assert!(made.ends_with("mmres.dll,-800\r\n") && !made.contains("\r\r"));
        // Dark, and the theme kept from before: its name alone is changed.
        assert!(text(UNSAVED, "x", (0.0, 0.0, 0.0), true).unwrap().contains("SystemMode=Dark\r\nAppMode=Dark"));
        let kept = rewritten(UNSAVED, BEFORE, &[]).unwrap();
        assert!(kept.contains("DisplayName=Windows truoc Ben\r\n") && kept.contains("ColorizationColor=0XC40078D4") && kept.contains("AutoColorization=1"));
    }

    #[test]
    fn what_a_theme_does_not_say_about_its_look_is_added_and_no_look_is_no_theme() {
        // An older file: nothing about light or dark, and the look is the last part.
        let old = "[Theme]\nDisplayName=Old\n\n[VisualStyles]\nPath=%ResourceDir%\\Themes\\Aero\\Aero.msstyles\nColorizationColor=0X6B74B8FC\n\n";
        let made = text(old, "Ben - Dem", (0.0, 0.5, 1.0), true).unwrap();
        assert_eq!(made, "[Theme]\r\nDisplayName=Ben - Dem\r\n\r\n[VisualStyles]\r\nPath=%ResourceDir%\\Themes\\Aero\\Aero.msstyles\r\nColorizationColor=0XC40080FF\r\nAutoColorization=0\r\nSystemMode=Dark\r\nAppMode=Dark\r\n\r\n");
        assert_eq!(text("[Theme]\nDisplayName=No look\n", "x", (0.0, 0.0, 0.0), true), None);
        // A remark that looks like a key is left alone.
        assert!(text("[Theme]\n; DisplayName=old\nDisplayName=a\n[VisualStyles]\n", "b", (0.0, 0.0, 0.0), true).unwrap().contains("; DisplayName=old\r\nDisplayName=b"));
    }

    #[test]
    fn names_are_plain_and_a_file_is_kept_byte_for_byte() {
        assert_eq!(display_name("Bến nước"), "Ben - Ben nuoc");
        assert_eq!(display_name("Đầm sen"), "Ben - Dam sen");
        assert!(is_ours("[Theme]\r\nDisplayName=Ben - Dam sen\r\n") && is_ours("DisplayName=Windows truoc Ben") && !is_ours(UNSAVED));
        // One byte to a letter: every byte comes back as it went in, whatever it stands for.
        let folder = std::env::temp_dir().join(format!("ben-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let narrow = folder.join("narrow.theme");
        let bytes: Vec<u8> = (1..=255u8).collect();
        std::fs::write(&narrow, &bytes).unwrap();
        let (text, wide) = read(&narrow).unwrap();
        assert!(!wide && text.chars().map(|c| c as u32 as u8).collect::<Vec<u8>>() == bytes);
        // Two bytes to a letter: read as such, and to be written as such.
        let broad = folder.join("broad.theme");
        std::fs::write(&broad, [0xFF, 0xFE, b'[', 0, 0xBF, 0x1E, b']', 0]).unwrap();
        assert_eq!(read(&broad), Some(("[\u{1ebf}]".to_string(), true)));
        let _ = std::fs::remove_file(narrow);
        let _ = std::fs::remove_file(broad);
        let _ = std::fs::remove_dir(folder);
    }

    /// Reads the theme Windows has on this computer and says what a theme of ours made from
    /// it would change, without writing or opening anything.
    /// Run by hand: cargo test the_theme_windows_has_can_be_read -- --ignored --nocapture
    #[test]
    #[ignore]
    fn the_theme_windows_has_can_be_read() {
        let path = system::windows_theme_file().expect("Windows names its theme file");
        let (base, wide) = read(&path).expect("the file can be read");
        let made = text(&base, "Ben - Thu", (0.100, 0.500, 0.940), true).expect("a theme can be made from it");
        let changed: Vec<&str> = made.lines().filter(|line| !base.lines().any(|old| old == *line)).collect();
        println!("{} lines, two bytes to a letter: {wide}; a theme of ours would say instead:", base.lines().count());
        for line in &changed {
            println!("  {line}");
        }
        assert!(changed.len() <= 5 && made.lines().count() + 1 >= base.lines().count());
        // The files themselves, written beside the test (nothing is opened): the theme
        // for Windows, and the one Windows had, kept the first time.
        let mut cfg = Config::default();
        assert_eq!(make("Theo Windows", &cfg), None);
        theme::choose(2, &mut cfg);
        let path = make("Bình minh", &cfg).expect("the file is written");
        let (written, _) = read(&path).expect("and can be read back");
        println!("written: {:?}, {} lines; kept from before: {}", path.file_name().unwrap(), written.lines().count(), before().exists());
        assert!(path.ends_with("themes/Ben - Binh minh.theme") || path.ends_with("themes\\Ben - Binh minh.theme"));
        assert!(written.contains("DisplayName=Ben - Binh minh\r\n") && written.contains("ColorizationColor=0XC4E37024\r\n") && written.contains("SystemMode=Light\r\nAppMode=Light\r\n"));
        let (kept, _) = read(&before()).expect("the theme Windows had is kept");
        assert!(kept.contains("DisplayName=Windows truoc Ben\r\n") && !kept.contains("ThemeId="));
        assert_eq!(kept.lines().count() + 1, base.lines().count());
    }
}
