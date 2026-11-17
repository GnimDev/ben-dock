//! Keeping ExplorerBlurMica's colours in step with the theme.
//!
//! File Explorer cannot be made glass from outside its own process, and doing
//! it from inside means putting our code into Explorer — fragile, and a crash
//! there is a crash of the desktop. ExplorerBlurMica is an open program that
//! already does it well (<https://github.com/Maplespe/ExplorerBlurMica>), so
//! Bến leaves the glass to it and does the one part it is placed to do: give
//! it the theme's colours.
//!
//! The program keeps its settings in a file `config.ini` beside its own
//! library; the library's place is written in the registry under the program's
//! class. When the theme has a glass of its own, Bến writes that file with the
//! theme's colour for light and for dark, so Explorer's glass matches the dock.
//! Bến does not install the program, and changes it made by hand are the ones
//! Bến overwrites while this is on. The program reads the file when a folder
//! window opens, so a window has to be reopened for a change to show.

use crate::config::Config;
use crate::theme::Scheme;
use crate::util::wide;
use std::path::PathBuf;
use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CLASSES_ROOT, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ};

/// The class ExplorerBlurMica registers its library under (its own source names it).
const CLSID: &str = "{887D3A6A-502E-4AF5-9CE6-D515E12AFE89}";

/// What came of writing the colours.
#[derive(Debug)]
pub enum Done {
    /// Written anew.
    Written(PathBuf),
    /// Already said what it was to say.
    Already,
    /// The program is not installed (its class is not registered).
    Missing,
    /// Its folder cannot be written to (it is somewhere only an administrator may change).
    Locked(PathBuf),
    /// Something else stopped the writing.
    Failed(String),
}

/// A string value of the registry, read from the machine's classes.
fn class_string(subkey: &str, value: &str) -> Option<String> {
    let (sub, val) = (wide(subkey), wide(value));
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    let mut size = 0u32;
    unsafe {
        if RegGetValueW(HKEY_CLASSES_ROOT, PCWSTR(sub.as_ptr()), PCWSTR(val.as_ptr()), flags, None, None, Some(&mut size)) != ERROR_SUCCESS || size < 4 {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2];
        if RegGetValueW(HKEY_CLASSES_ROOT, PCWSTR(sub.as_ptr()), PCWSTR(val.as_ptr()), flags, None, Some(buf.as_mut_ptr() as *mut _), Some(&mut size)) != ERROR_SUCCESS {
            return None;
        }
        let text = String::from_utf16_lossy(&buf);
        let text = text.trim_end_matches('\0');
        (!text.is_empty()).then(|| text.to_string())
    }
}

/// Where ExplorerBlurMica's library is, if it is installed.
fn library() -> Option<PathBuf> {
    let path = PathBuf::from(class_string(&format!("CLSID\\{CLSID}\\InprocServer32"), "")?);
    path.exists().then_some(path)
}

/// Where the program keeps its settings, if it is installed: `config.ini` beside its library.
pub fn config_file() -> Option<PathBuf> {
    Some(library()?.parent()?.join("config.ini"))
}

/// Whether ExplorerBlurMica is installed.
pub fn installed() -> bool {
    library().is_some()
}

/// The settings file's text for a theme: the effect, and the glass colour for light and for
/// dark. A colour is the theme's own glass (`Scheme`) where it has one; without a theme of
/// the app's own, white on light and black on dark, as the program itself starts out.
pub fn config_text(cfg: &Config) -> String {
    let scheme = Scheme::of(cfg);
    let (dark, light) = scheme.glass.unwrap_or(((0.0, 0.0, 0.0), (1.0, 1.0, 1.0)));
    // How much of the colour covers what is behind: from the "opacity" setting, kept within
    // a band that still lets the blur read.
    let alpha = (96.0 + cfg.tint_opacity.clamp(0.0, 1.0) * 104.0).round() as u8;
    let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    let colour = |rgb: (f32, f32, f32)| format!("r={}\r\ng={}\r\nb={}\r\na={alpha}", byte(rgb.0), byte(rgb.1), byte(rgb.2));
    // effect 3 is "Blur (Clear)", the one that works on both Windows 10 and 11. The address
    // bar and the toolbar (a WinUI part) are cleared so the glass reaches them; the line
    // between the tree and the view is kept. The scrollbar is left alone (clearing it has
    // the program draw its own, which looks unlike the system's).
    format!("[config]\r\neffect=3\r\nclearAddress=true\r\nclearBarBg=false\r\nclearWinUIBg=true\r\nshowLine=true\r\n[light]\r\n{}\r\n[dark]\r\n{}\r\n", colour(light), colour(dark))
}

/// Writes the colours of the theme into ExplorerBlurMica's settings, if it is installed and
/// the file does not already say just that.
pub fn apply(cfg: &Config) -> Done {
    let Some(path) = config_file() else { return Done::Missing };
    let text = config_text(cfg);
    if std::fs::read_to_string(&path).map(|old| old == text).unwrap_or(false) {
        return Done::Already;
    }
    match std::fs::write(&path, &text) {
        Ok(()) => Done::Written(path),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Done::Locked(path),
        Err(e) => Done::Failed(e.to_string()),
    }
}

/// Writes the colours and says in the log what came of it (nothing, when it is not installed:
/// that is the usual case and not worth a line every time the theme changes).
pub fn sync(cfg: &Config) {
    match apply(cfg) {
        Done::Written(path) => crate::log!("explorer glass: ExplorerBlurMica's colours set from the theme ({}); reopen a folder window to see it", path.display()),
        Done::Locked(path) => crate::log!("explorer glass: ExplorerBlurMica is in a folder Bến may not write to ({}); put it in a folder of your own, or set its colours by hand", path.display()),
        Done::Failed(e) => crate::log!("explorer glass: ExplorerBlurMica's colours could not be written: {e}"),
        Done::Missing | Done::Already => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme;

    #[test]
    fn the_settings_have_the_keys_the_program_reads() {
        let text = config_text(&Config::default());
        // The sections and keys, with the program's own spelling.
        for line in ["[config]", "effect=3", "clearAddress=true", "clearBarBg=false", "clearWinUIBg=true", "showLine=true", "[light]", "[dark]"] {
            assert!(text.contains(&format!("{line}\r\n")), "missing {line}");
        }
        assert_eq!(text.matches("[light]").count(), 1);
        assert_eq!(text.matches("[dark]").count(), 1);
        // Every line ends the same way (the program reads an ordinary ini), none doubled.
        assert!(text.ends_with("\r\n") && !text.contains("\r\r") && !text.contains("\n\n"));
        // (At a line's start, so the "g=" inside clearBarBg / clearWinUIBg does not count.)
        for key in ["\r\nr=", "\r\ng=", "\r\nb=", "\r\na="] {
            assert_eq!(text.matches(key).count(), 2, "{key}");
        }
    }

    #[test]
    fn the_colours_are_the_theme_s_glass_light_and_dark() {
        // A set with a glass of its own: dawn is cream on light, a warm near-black on dark.
        let mut cfg = Config::default();
        theme::choose(2, &mut cfg);
        let text = config_text(&cfg);
        let section = |name: &str| text.split(name).nth(1).unwrap().lines().filter(|l| l.contains('=')).take(4).map(|l| l.split('=').nth(1).unwrap().parse::<u8>().unwrap()).collect::<Vec<_>>();
        let (light, dark) = (section("[light]"), section("[dark]"));
        // Dawn's day glass is bright, its night glass dark.
        assert!(light[0] > 230 && light[1] > 230 && light[2] > 220, "light {light:?}");
        assert!(dark[0] < 60 && dark[1] < 40 && dark[2] < 40, "dark {dark:?}");
        // The warm colour: more red than blue at both ends.
        assert!(light[0] >= light[2] && dark[0] >= dark[2]);
        // Without a set of the app's own: white on light, black on dark (as the program starts).
        let plain = config_text(&Config::default());
        assert!(plain.contains("[light]\r\nr=255\r\ng=255\r\nb=255") && plain.contains("[dark]\r\nr=0\r\ng=0\r\nb=0"));
    }

    /// Prints the settings Bến would write for a theme, to look over against the program's
    /// own format, and says whether ExplorerBlurMica is installed on this computer.
    /// Run by hand: BEN_SCHEME=1 cargo test blurmica_sample -- --ignored --nocapture
    #[test]
    #[ignore]
    fn blurmica_sample() {
        let mut cfg = Config::default();
        theme::choose(std::env::var("BEN_SCHEME").ok().and_then(|v| v.parse().ok()).unwrap_or(1), &mut cfg);
        println!("installed: {}; config file: {:?}", installed(), config_file());
        println!("----\n{}----", config_text(&cfg));
    }

    #[test]
    fn opacity_sets_the_blend_and_stays_in_a_readable_band() {
        let alpha_at = |t: f32| {
            let mut cfg = Config::default();
            cfg.tint_opacity = t;
            config_text(&cfg).lines().find(|l| l.starts_with("a=")).unwrap().split('=').nth(1).unwrap().parse::<u8>().unwrap()
        };
        let (clear, dense) = (alpha_at(0.0), alpha_at(1.0));
        assert!(clear < dense && clear >= 90 && dense <= 200, "{clear}..{dense}");
    }
}
