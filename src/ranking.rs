//! What makes an app popular, and what a search for it means.
//!
//! Popularity comes from three places: how often Windows itself has seen the app started
//! (the counters it keeps for the Start menu, read from the registry), how often it was
//! started through Bến, and a short list of apps nearly everyone uses. Apps that are never
//! started, and installers, updaters and administrative tools, sink to the last pages.
//!
//! A search ignores case and accents, takes several words, and also finds an app by its
//! initials ("vsc"), by a short name it is known by ("cmd", "calc"), by the name of its
//! program ("winword"), and despite a slip of the fingers ("chorme"). What was picked for
//! a search before is put first the next time.

use crate::util::{data_dir, from_wide, wide};
use std::collections::HashMap;
use std::path::PathBuf;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::System::Registry::*;

/// Apps nearly everyone uses (parts of their names, lower-case, no accents): they are
/// popular before anything has been counted.
const KNOWN: &[&str] = &[
    "chrome", "edge", "firefox", "brave", "opera", "coc coc", "word", "excel", "powerpoint", "outlook", "onenote", "teams", "onedrive", "visual studio code",
    "visual studio", "android studio", "terminal", "powershell", "command prompt", "notepad", "calculator", "may tinh", "paint", "snipping tool", "task manager",
    "settings", "cai dat", "file explorer", "microsoft store", "photos", "camera", "clock", "spotify", "vlc media player", "zalo", "telegram", "discord", "slack",
    "zoom", "skype", "messenger", "whatsapp", "steam", "epic games launcher", "photoshop", "illustrator", "premiere pro", "figma", "canva", "blender", "obs studio",
    "docker desktop", "postman", "git bash", "unikey", "evkey", "claude", "chatgpt", "notion", "obsidian", "anydesk", "teamviewer", "winrar", "7 zip",
];

/// Parts of names (lower-case, no accents) of what is rarely wanted: installers, updaters,
/// administrative tools, help pages.
const JUNK: &[&str] = &[
    "installer", "updater", "update ", "uninstall", "setup", "repair", "diagnostic", "redistributable", "runtime", "sdk", "driver", "licen", "eula", "release notes",
    "readme", "character map", "component services", "odbc", "event viewer", "disk cleanup", "defragment", "resource monitor", "system information", "steps recorder",
    "fax", "xps", "math input", "iscsi", "print management", "windows tools", "administrative tools", "quick assist", "wordpad", "internet explorer", "get started",
    "tips", "feedback hub", "web help", "native tools", "cross tools", "verifier", "add a new",
];

/// Names an app is also known by (what the name has to contain as a phrase, and what to
/// call it): short forms, the name of its program, what it is for.
const ALIASES: &[(&str, &[&str])] = &[
    ("word", &["doc", "docx", "winword"]),
    ("excel", &["xls", "xlsx", "sheet"]),
    ("powerpoint", &["ppt", "pptx", "slide"]),
    ("outlook", &["mail", "email"]),
    ("onenote", &["note"]),
    ("visual studio code", &["vsc", "vscode", "code"]),
    ("visual studio", &["vs"]),
    ("command prompt", &["cmd", "dos", "console"]),
    ("terminal", &["wt", "cli", "console"]),
    ("powershell", &["ps", "pwsh", "shell"]),
    ("calculator", &["calc", "maytinh"]),
    ("may tinh", &["calc", "calculator"]),
    ("notepad", &["note", "text", "txt"]),
    ("paint", &["mspaint", "draw"]),
    ("snipping tool", &["snip", "screenshot", "capture"]),
    ("task manager", &["taskmgr", "tm", "task"]),
    ("settings", &["setting", "config", "control"]),
    ("cai dat", &["settings", "setting", "config"]),
    ("control panel", &["control", "cp"]),
    ("file explorer", &["explorer", "files", "folder"]),
    ("edge", &["msedge", "browser", "web"]),
    ("chrome", &["gc", "browser", "web"]),
    ("firefox", &["ff", "browser", "web"]),
    ("brave", &["browser", "web"]),
    ("opera", &["browser", "web"]),
    ("spotify", &["music", "nhac", "song"]),
    ("microsoft store", &["store", "shop"]),
    ("photos", &["pictures", "images", "anh"]),
    ("camera", &["cam", "webcam"]),
    ("clock", &["alarm", "timer"]),
    ("zalo", &["chat"]),
    ("telegram", &["tg", "chat"]),
    ("discord", &["dc", "chat"]),
    ("messenger", &["fb", "chat"]),
    ("zoom", &["meeting"]),
    ("photoshop", &["ps", "psd"]),
    ("premiere pro", &["pr", "video"]),
    ("after effects", &["ae"]),
    ("illustrator", &["ai"]),
    ("lightroom", &["lr"]),
    ("docker desktop", &["docker"]),
    ("git bash", &["git", "bash"]),
    ("sql server management studio", &["ssms", "sql"]),
    ("android studio", &["android"]),
    ("remote desktop connection", &["rdp", "mstsc", "remote"]),
    ("anydesk", &["remote"]),
    ("teamviewer", &["remote"]),
    ("vlc media player", &["vlc", "video", "movie", "phim"]),
    ("media player", &["music", "video", "phim", "nhac"]),
    ("obs studio", &["obs", "stream", "record"]),
    ("steam", &["game"]),
    ("epic games launcher", &["epic", "game"]),
    ("onedrive", &["cloud", "drive"]),
    ("unikey", &["uk"]),
];

/// Words in front of a name that most apps share; an abbreviation can skip them.
const BRANDS: &[&str] = &["microsoft", "windows", "google", "adobe", "ms", "apple", "autodesk", "jetbrains"];
/// Words of a search that say little: dropped when others are given ("ms word").
const WEAK: &[&str] = &["ms", "microsoft", "windows", "win", "app", "apps"];

// ------------------------------------------------------------------ the text

/// A letter without its accent (Vietnamese and the Latin accents), `đ` as `d`.
fn plain_char(c: char) -> char {
    match c {
        'à' | 'á' | 'ạ' | 'ả' | 'ã' | 'â' | 'ầ' | 'ấ' | 'ậ' | 'ẩ' | 'ẫ' | 'ă' | 'ằ' | 'ắ' | 'ặ' | 'ẳ' | 'ẵ' | 'ä' | 'å' => 'a',
        'è' | 'é' | 'ẹ' | 'ẻ' | 'ẽ' | 'ê' | 'ề' | 'ế' | 'ệ' | 'ể' | 'ễ' | 'ë' => 'e',
        'ì' | 'í' | 'ị' | 'ỉ' | 'ĩ' | 'î' | 'ï' => 'i',
        'ò' | 'ó' | 'ọ' | 'ỏ' | 'õ' | 'ô' | 'ồ' | 'ố' | 'ộ' | 'ổ' | 'ỗ' | 'ơ' | 'ờ' | 'ớ' | 'ợ' | 'ở' | 'ỡ' | 'ö' => 'o',
        'ù' | 'ú' | 'ụ' | 'ủ' | 'ũ' | 'ư' | 'ừ' | 'ứ' | 'ự' | 'ử' | 'ữ' | 'û' | 'ü' => 'u',
        'ỳ' | 'ý' | 'ỵ' | 'ỷ' | 'ỹ' | 'ÿ' => 'y',
        'đ' => 'd',
        'ç' => 'c',
        'ñ' => 'n',
        _ => c,
    }
}

/// Lower-case, no accents, anything that is not a letter, digit, `+` or `#` made a single
/// space: "Cài đặt  (x64)" becomes "cai dat x64".
pub fn normalize(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap = true;
    for c in text.chars().flat_map(|c| c.to_lowercase()) {
        let c = plain_char(c);
        if c.is_alphanumeric() || c == '+' || c == '#' {
            out.push(c);
            gap = false;
        } else if !gap {
            out.push(' ');
            gap = true;
        }
    }
    out.truncate(out.trim_end().len());
    out
}

/// Only the letters and digits of a normalised text.
fn alnum(text: &str) -> String {
    normalize(text).chars().filter(|c| c.is_alphanumeric()).collect()
}

/// The words of a name, also where capitals and digits begin one ("PowerShell" is "power
/// shell", "Office365" is "office 365"), lower-case, no accents.
fn parts_of(name: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let (mut word, mut before) = (String::new(), ' ');
    for c in name.chars() {
        let lower = c.to_lowercase().next().unwrap_or(c);
        let shown = plain_char(lower);
        if !shown.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
        } else {
            let capital = c != lower;
            if !word.is_empty() && ((capital && before.is_lowercase()) || (c.is_ascii_digit() != before.is_ascii_digit() && before.is_alphanumeric())) {
                words.push(std::mem::take(&mut word));
            }
            word.push(shown);
        }
        before = c;
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// True when `fragment` is the whole of `plain` or a whole run of its words.
fn has_phrase(plain: &str, fragment: &str) -> bool {
    plain == fragment || plain.starts_with(&format!("{fragment} ")) || plain.ends_with(&format!(" {fragment}")) || plain.contains(&format!(" {fragment} "))
}

/// Is this app the well-known one called `fragment`: its name, bar a brand word in front, a
/// version number or "x64" at the end, and at most one more word ("Microsoft Word", "Word
/// 2019", "Firefox Developer"), but not "Developer Command Prompt for VS" for "command prompt".
fn is_known(plain: &str, fragment: &str) -> bool {
    let words: Vec<&str> = plain.split(' ').collect();
    let brands = words.iter().take_while(|w| BRANDS.contains(w)).count().min(words.len() - 1);
    let mut core = &words[brands..];
    while core.len() > 1 && core.last().is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()) || matches!(*w, "x64" | "x86" | "arm64")) {
        core = &core[..core.len() - 1];
    }
    let known: Vec<&str> = fragment.split(' ').collect();
    core == known.as_slice() || (core.len() <= known.len() + 1 && (core.starts_with(&known) || core.ends_with(&known)))
}

/// What names the program of an app: the end of its identity, without extension or package
/// number ("chrome" for `{...}\Google\Chrome\Application\chrome.exe`).
fn ident_of(target: &str) -> String {
    let id = target.strip_prefix(crate::shell::APPS_FOLDER).unwrap_or(target);
    let last = if id.contains('\\') {
        id.rsplit('\\').next().unwrap_or(id).to_string()
    } else {
        let before = id.split('!').next().unwrap_or(id);
        let before = before.split('_').next().unwrap_or(before);
        before.rsplit('.').next().unwrap_or(before).to_string()
    };
    let lower = last.to_lowercase();
    alnum(lower.strip_suffix(".exe").or_else(|| lower.strip_suffix(".lnk")).unwrap_or(&lower))
}

// ---------------------------------------------------------------- the search

/// What a search looks at in one app, worked out once.
pub struct Hay {
    plain: String,
    words: Vec<String>,
    /// The words of the name, without the brand words in front, and with them.
    parts: Vec<String>,
    all_parts: Vec<String>,
    ident: String,
    aliases: Vec<&'static str>,
}

pub fn hay(name: &str, target: &str) -> Hay {
    let plain = normalize(name);
    let all_parts = parts_of(name);
    let skip = all_parts.iter().take_while(|w| BRANDS.contains(&w.as_str())).count().min(all_parts.len().saturating_sub(1));
    let aliases = ALIASES.iter().filter(|(fragment, _)| has_phrase(&plain, fragment)).flat_map(|(_, names)| names.iter().copied()).collect();
    Hay { words: plain.split(' ').map(str::to_string).collect(), parts: all_parts[skip..].to_vec(), all_parts, ident: ident_of(target), aliases, plain }
}

/// A search, as words.
pub struct Query {
    tokens: Vec<String>,
}

impl Query {
    pub fn new(text: &str) -> Query {
        let mut tokens: Vec<String> = normalize(text).split(' ').filter(|t| !t.is_empty()).map(str::to_string).collect();
        if tokens.len() > 1 && tokens.iter().any(|t| !WEAK.contains(&t.as_str())) {
            tokens.retain(|t| !WEAK.contains(&t.as_str()));
        }
        Query { tokens }
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// The search as it is remembered.
    pub fn key(&self) -> String {
        self.tokens.join(" ")
    }

    /// How well an app answers the search (more is better), or None if it does not.
    pub fn score(&self, hay: &Hay) -> Option<f32> {
        let mut sum = 0.0;
        for token in &self.tokens {
            let one = token_score(token, hay);
            if one <= 0.0 {
                return None;
            }
            sum += one;
        }
        (!self.tokens.is_empty()).then(|| sum / self.tokens.len() as f32)
    }
}

fn token_score(t: &str, h: &Hay) -> f32 {
    let n = t.len();
    if h.plain == t {
        return 1000.0;
    }
    let mut best = 0.0f32;
    let mut take = |score: f32| best = best.max(score);
    if h.aliases.contains(&t) {
        take(900.0);
    }
    if h.plain.starts_with(t) {
        take(800.0);
    }
    if let Some(i) = h.words.iter().position(|w| w.starts_with(t)) {
        take(700.0 - 15.0 * i as f32);
    }
    if n >= 2 && (abbreviation(t, &h.parts, true) || abbreviation(t, &h.all_parts, true)) {
        take(650.0);
    }
    if n >= 2 && h.aliases.iter().any(|a| a.starts_with(t)) {
        take(600.0);
    }
    if h.ident.starts_with(t) {
        take(550.0);
    }
    if n >= 2 {
        if let Some(at) = h.plain.find(t) {
            take(450.0 - 2.0 * at as f32);
        }
    }
    if n >= 3 && h.ident.contains(t) {
        take(380.0);
    }
    if n >= 4 && (h.words.iter().filter(|w| w.len() >= 4).any(|w| near(t, w)) || near(t, &h.ident)) {
        take(300.0);
    }
    if n >= 3 {
        if let Some(span) = subsequence(t, &h.plain) {
            take(100.0 + 100.0 * n as f32 / span as f32);
        }
    }
    best
}

/// Can `t` be cut into pieces that begin, in turn, the words of a name, the first piece
/// the first word? ("vsc" for "visual studio code", "vstudio", "vcode".) Words may be skipped
/// after the first.
fn abbreviation(t: &str, parts: &[String], first: bool) -> bool {
    if t.is_empty() {
        return true;
    }
    let candidates: &[String] = if first { &parts[..parts.len().min(1)] } else { parts };
    for (i, part) in candidates.iter().enumerate() {
        for k in (1..=part.len().min(t.len())).rev() {
            if t.is_char_boundary(k) && part.starts_with(&t[..k]) && abbreviation(&t[k..], &parts[i + 1..], false) {
                return true;
            }
        }
    }
    false
}

/// Is `t` the start of `word`, give or take one slip: a letter wrong, missing, extra, or two
/// next to each other swapped? ("chorme" for "chrome", "firefx" for "firefox".)
fn near(t: &str, word: &str) -> bool {
    let (a, b) = (t.as_bytes(), word.as_bytes());
    // Compare with the start of the word as long as the search, give or take a letter.
    [a.len() - 1, a.len(), a.len() + 1].into_iter().any(|len| len <= b.len() && within_one(a, &b[..len]))
}

fn within_one(a: &[u8], b: &[u8]) -> bool {
    if a == b {
        return true;
    }
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let p = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let s = a[p..].iter().rev().zip(b[p..].iter().rev()).take_while(|(x, y)| x == y).count();
    match (a.len() - p - s, b.len() - p - s) {
        (0, 1) | (1, 0) | (1, 1) => true,
        (2, 2) => a[p] == b[p + 1] && a[p + 1] == b[p],
        _ => false,
    }
}

/// How long a stretch of `text` holds the letters of `t` in order, if it holds them within
/// three times their number.
fn subsequence(t: &str, text: &str) -> Option<usize> {
    let mut letters = t.chars();
    let mut want = letters.next()?;
    let mut first = None;
    for (i, c) in text.chars().enumerate() {
        if c == want {
            let start = *first.get_or_insert(i);
            match letters.next() {
                Some(next) => want = next,
                None => {
                    let span = i - start + 1;
                    return (span <= t.chars().count() * 3).then_some(span);
                }
            }
        }
    }
    None
}

// -------------------------------------------------------------- popularity

const USER_ASSIST: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\UserAssist\\";
/// The two lists Windows keeps: programs run, and shortcuts run.
const USER_ASSIST_LISTS: [&str; 2] = ["{CEBFF5CD-ACE2-4F4F-9178-9926F41749EA}", "{F4E57C4B-2036-45F0-A9AB-443BCFE33D9F}"];

/// The names Windows counts starts under are written in ROT13.
fn rot13(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a') + 13) % 26 + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A') + 13) % 26 + b'A') as char,
            _ => c,
        })
        .collect()
}

/// The name of a program or shortcut without folders, extension or punctuation.
fn stem_of(text: &str) -> String {
    let last = text.rsplit(['\\', '/']).next().unwrap_or(text).to_lowercase();
    alnum(last.strip_suffix(".exe").or_else(|| last.strip_suffix(".lnk")).unwrap_or(&last))
}

/// How much each program or shortcut has been started, as Windows counted it: by its whole
/// name, and by its name without folders.
fn user_assist() -> (HashMap<String, f32>, HashMap<String, f32>) {
    let (mut full, mut stems) = (HashMap::new(), HashMap::new());
    for list in USER_ASSIST_LISTS {
        let path = wide(&format!("{USER_ASSIST}{list}\\Count"));
        let mut key = HKEY::default();
        if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, KEY_READ, &mut key) } != ERROR_SUCCESS {
            continue;
        }
        for index in 0.. {
            let mut name = [0u16; 1024];
            let mut name_len = name.len() as u32;
            let mut data = [0u8; 128];
            let mut data_len = data.len() as u32;
            let found = unsafe { RegEnumValueW(key, index, Some(PWSTR(name.as_mut_ptr())), &mut name_len, None, None, Some(data.as_mut_ptr()), Some(&mut data_len)) };
            if found == ERROR_NO_MORE_ITEMS {
                break;
            }
            if found != ERROR_SUCCESS || data_len < 16 {
                continue;
            }
            let decoded = rot13(&from_wide(&name));
            if decoded.starts_with("UEME_") {
                continue;
            }
            let word = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]) as f32;
            // Runs, and the time it was in front (ten minutes count as a run).
            let used = word(4) + word(12) / 600_000.0;
            *full.entry(decoded.to_lowercase()).or_insert(0.0) += used;
            *stems.entry(stem_of(&decoded)).or_insert(0.0) += used;
        }
        unsafe {
            let _ = RegCloseKey(key);
        }
    }
    (full, stems)
}

/// How popular each app is, in the order given: (name, launch target) for each.
pub fn popularity(apps: &[(String, String)]) -> Vec<f32> {
    let (full, stems) = user_assist();
    let usage = Usage::load();
    apps.iter()
        .map(|(name, target)| {
            let id = target.strip_prefix(crate::shell::APPS_FOLDER).unwrap_or(target).to_lowercase();
            let plain = normalize(name);
            // The same start is counted under the shortcut and under the program: take the larger.
            let seen = [full.get(&id), stems.get(&stem_of(&id)), stems.get(&alnum(&plain))].into_iter().flatten().fold(0.0f32, |a, b| a.max(*b));
            let mut score = 10.0 * (1.0 + seen + 3.0 * usage.launch_count(target) as f32).ln();
            if KNOWN.iter().any(|known| is_known(&plain, known)) {
                score += 12.0;
            }
            if JUNK.iter().any(|junk| plain.contains(junk)) {
                score -= 30.0;
            }
            score
        })
        .collect()
}

// ----------------------------------------------------------------- memory

/// What Bến remembers of what was started and what was picked for which search. Kept in
/// `usage.txt` next to `ben.ini`, a line each: `L`, count, target; `Q`, count, search, target.
#[derive(Default)]
pub struct Usage {
    launches: HashMap<String, u32>,
    picks: HashMap<(String, String), u32>,
}

fn usage_file() -> PathBuf {
    data_dir().join("usage.txt")
}

/// Most lines of each kind that are kept.
const MOST_KEPT: usize = 500;

impl Usage {
    pub fn load() -> Usage {
        let mut usage = Usage::default();
        let Ok(text) = std::fs::read_to_string(usage_file()) else { return usage };
        for line in text.lines() {
            let fields: Vec<&str> = line.split('\t').collect();
            match fields.as_slice() {
                ["L", count, target] => {
                    usage.launches.insert(target.to_string(), count.parse().unwrap_or(0));
                }
                ["Q", count, query, target] => {
                    usage.picks.insert((query.to_string(), target.to_string()), count.parse().unwrap_or(0));
                }
                _ => {}
            }
        }
        usage
    }

    pub fn save(&self) {
        let mut launches: Vec<_> = self.launches.iter().collect();
        launches.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let mut picks: Vec<_> = self.picks.iter().collect();
        picks.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let mut text = String::new();
        for (target, count) in launches.into_iter().take(MOST_KEPT) {
            text.push_str(&format!("L\t{count}\t{target}\n"));
        }
        for ((query, target), count) in picks.into_iter().take(MOST_KEPT) {
            text.push_str(&format!("Q\t{count}\t{query}\t{target}\n"));
        }
        // Written beside, then put in place, so that a reader never meets half of it.
        let (path, temp) = (usage_file(), data_dir().join("usage.txt.tmp"));
        if std::fs::write(&temp, text).and_then(|_| std::fs::rename(&temp, &path)).is_err() {
            crate::log!("could not save what was started");
        }
    }

    /// How many times an app was started through Bến.
    pub fn launch_count(&self, target: &str) -> u32 {
        self.launches.get(&target.to_lowercase()).copied().unwrap_or(0)
    }

    /// How many times this app was picked for this search (see `Query::key`).
    pub fn pick_count(&self, query: &str, target: &str) -> u32 {
        self.picks.get(&(query.to_string(), target.to_lowercase())).copied().unwrap_or(0)
    }

    pub fn launched(&mut self, target: &str) {
        *self.launches.entry(target.to_lowercase()).or_insert(0) += 1;
    }

    pub fn picked(&mut self, query: &str, target: &str) {
        *self.picks.entry((query.to_string(), target.to_lowercase())).or_insert(0) += 1;
    }
}

/// Notes that an app was started (from the dock).
pub fn record_launch(target: &str) {
    let mut usage = Usage::load();
    usage.launched(target);
    usage.save();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(query: &str, name: &str, target: &str) -> Option<f32> {
        Query::new(query).score(&hay(name, target))
    }

    #[test]
    fn accents_and_case_do_not_matter() {
        assert_eq!(normalize("Cài đặt  (x64)"), "cai dat x64");
        assert!(score("cai dat", "Cài đặt", "shell:AppsFolder\\windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel").is_some());
        assert!(score("CÀI", "Cài đặt", "x").is_some());
    }

    #[test]
    fn finds_by_initials_and_short_names() {
        let code = "shell:AppsFolder\\Microsoft.VisualStudioCode";
        assert!(score("vsc", "Visual Studio Code", code).is_some());
        assert!(score("vscode", "Visual Studio Code", code).is_some());
        assert!(score("code", "Visual Studio Code", code).is_some());
        assert!(score("cmd", "Command Prompt", "shell:AppsFolder\\{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\cmd.exe").is_some());
        assert!(score("ps", "Windows PowerShell", "x").is_some());
        assert!(score("winword", "Word", "shell:AppsFolder\\{6D809377-6AF0-444B-8957-A3773F02200E}\\Microsoft Office\\root\\Office16\\WINWORD.EXE").is_some());
        assert!(score("gc", "Google Chrome", "x").is_some());
    }

    #[test]
    fn forgives_a_slip_and_drops_weak_words() {
        assert!(score("chorme", "Google Chrome", "x").is_some());
        assert!(score("firefx", "Firefox", "x").is_some());
        assert!(score("chrm", "Google Chrome", "x").is_some());
        assert!(score("ms word", "Word", "x").is_some());
        assert!(score("xyzq", "Google Chrome", "x").is_none());
    }

    #[test]
    fn better_matches_come_first() {
        let exact = score("word", "Word", "x").unwrap();
        let prefix = score("word", "WordPad", "x").unwrap();
        let inside = score("word", "Crossword Puzzle", "x").unwrap();
        assert!(exact > prefix && prefix > inside);
        let start = score("ch", "Chrome", "x").unwrap();
        let later = score("ch", "Google Chrome", "x").unwrap();
        assert!(start > later);
    }

    #[test]
    fn knows_the_well_known_apps_only() {
        assert!(is_known("command prompt", "command prompt"));
        assert!(is_known("windows powershell", "powershell"));
        assert!(is_known("microsoft word", "word"));
        assert!(is_known("word 2019", "word"));
        assert!(is_known("windows powershell x86", "powershell"));
        assert!(is_known("google chrome", "chrome"));
        assert!(!is_known("developer command prompt for vs 2022", "command prompt"));
        assert!(!is_known("node js command prompt", "command prompt"));
        assert!(!is_known("x64 native tools command prompt for vs 2022", "command prompt"));
        assert!(!is_known("developer powershell for vs 2022", "powershell"));
        assert!(!is_known("wordpad", "word"));
    }

    #[test]
    fn splits_names_at_capitals() {
        assert_eq!(parts_of("PowerShell 7 (x64)"), ["power", "shell", "7", "x", "64"]);
        assert_eq!(ident_of("shell:AppsFolder\\Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), "windowscalculator");
        assert_eq!(ident_of("shell:AppsFolder\\{6D809377-6AF0-444B-8957-A3773F02200E}\\Google\\Chrome\\Application\\chrome.exe"), "chrome");
    }

    #[test]
    fn rot13_round_trip() {
        assert_eq!(rot13("HRZR_PGYFRFFVBA"), "UEME_CTLSESSION");
        assert_eq!(rot13(&rot13("Chrome.exe")), "Chrome.exe");
    }
}
