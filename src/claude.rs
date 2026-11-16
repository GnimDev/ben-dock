//! Claude: how much of it was used today, read from what Claude Code keeps on this
//! computer: the transcripts of its sessions (every reply notes the tokens it took) and
//! the share of the plan's limits it last noted down for itself. Nothing is asked of
//! Anthropic, and the sign-in Claude Code keeps is never read.
//!
//! The transcripts are large, so they are read on a thread of their own, and each file
//! only from where the last look stopped.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// A session written to within this many seconds counts as running.
const RUNNING: i64 = 90;

/// Replies, and the tokens they took: read (what was sent in, without what was only
/// read back from the cache) and written.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Count {
    pub replies: u32,
    pub read: u64,
    pub written: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    /// Since midnight, and in the last five hours.
    pub today: Count,
    pub recent: Count,
    /// Sessions being written to right now.
    pub running: usize,
    /// The share of the plan's five-hour and weekly limits used, as Claude Code last noted
    /// it: percent, and the seconds until the count starts over. None: nothing noted since
    /// the count last started over.
    pub session: Option<(u32, i64)>,
    pub week: Option<(u32, i64)>,
}

enum Wake {
    Look,
    Stop,
}

pub struct Claude {
    tx: Sender<Wake>,
    rx: Receiver<Usage>,
}

impl Claude {
    /// Starts the reader. `window` gets `message` whenever there is something new to `take`.
    pub fn start(window: HWND, message: u32) -> Claude {
        let (tx, wake) = channel::<Wake>();
        let (out, rx) = channel::<Usage>();
        let window = window.0 as isize;
        let spawned = std::thread::Builder::new().name("claude".into()).spawn(move || watch(wake, out, window, message));
        if let Err(e) = spawned {
            crate::log!("could not start reading Claude's usage: {e}");
        }
        Claude { tx, rx }
    }

    /// Asks for the numbers to be read again; they arrive a moment later.
    pub fn look(&self) {
        let _ = self.tx.send(Wake::Look);
    }

    pub fn take(&self) -> Option<Usage> {
        self.rx.try_iter().last()
    }
}

impl Drop for Claude {
    fn drop(&mut self) {
        let _ = self.tx.send(Wake::Stop);
    }
}

/// One reply found in a transcript: when (seconds since 1970) and the tokens it took.
struct Reply {
    at: i64,
    read: u64,
    written: u64,
}

/// What has been read of one transcript so far.
#[derive(Default)]
struct Seen {
    upto: u64,
    last_id: String,
    replies: Vec<Reply>,
}

fn watch(wake: Receiver<Wake>, out: Sender<Usage>, window: isize, message: u32) {
    let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) else { return };
    let mut seen: HashMap<PathBuf, Seen> = HashMap::new();
    while let Ok(Wake::Look) = wake.recv() {
        if out.send(look(&home, &mut seen)).is_err() {
            break;
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(window as *mut _)), message, WPARAM(0), LPARAM(0));
        }
    }
}

fn look(home: &Path, seen: &mut HashMap<PathBuf, Seen>) -> Usage {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let clock = unsafe { GetLocalTime() };
    let midnight = now - (clock.wHour as i64 * 3600 + clock.wMinute as i64 * 60 + clock.wSecond as i64);
    let recent = now - 5 * 3600;
    let since = midnight.min(recent);

    let mut usage = Usage::default();
    let mut files = Vec::new();
    transcripts(&home.join(".claude").join("projects"), 0, &mut files);
    for (path, modified, size) in files {
        // Not written to since then: nothing in it counts.
        if modified < since {
            seen.remove(&path);
            continue;
        }
        // A session's helpers have transcripts of their own; they are not sessions.
        if now - modified < RUNNING && !path.components().any(|part| part.as_os_str() == "subagents") {
            usage.running += 1;
        }
        let file = seen.entry(path.clone()).or_default();
        if size < file.upto {
            // Written afresh: read it from the start.
            *file = Seen::default();
        }
        if size > file.upto {
            read_more(&path, file, since);
        }
        file.replies.retain(|reply| reply.at >= since);
        for reply in &file.replies {
            for (count, from) in [(&mut usage.today, midnight), (&mut usage.recent, recent)] {
                if reply.at >= from {
                    count.replies += 1;
                    count.read += reply.read;
                    count.written += reply.written;
                }
            }
        }
    }
    (usage.session, usage.week) = limits(home, now);
    usage
}

/// Every transcript under `folder`: a session's own, and those of the helpers it started a
/// few folders further down. With each, when it was last written to and its size.
fn transcripts(folder: &Path, depth: usize, found: &mut Vec<(PathBuf, i64, u64)>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let path = entry.path();
        if meta.is_dir() {
            if depth < 3 {
                transcripts(&path, depth + 1, found);
            }
        } else if path.extension().is_some_and(|extension| extension == "jsonl") {
            let modified = meta.modified().ok().and_then(|at| at.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            found.push((path, modified, meta.len()));
        }
    }
}

/// Reads the lines added to a transcript since the last look, keeping the replies made
/// after `since`.
fn read_more(path: &Path, file: &mut Seen, since: i64) {
    let Ok(mut opened) = File::open(path) else { return };
    if opened.seek(SeekFrom::Start(file.upto)).is_err() {
        return;
    }
    let mut reader = BufReader::with_capacity(1 << 16, opened);
    let mut line = Vec::new();
    loop {
        line.clear();
        match reader.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            // A line still being written: next time.
            Ok(_) if line.last() != Some(&b'\n') => break,
            Ok(length) => file.upto += length as u64,
        }
        let text = String::from_utf8_lossy(&line);
        let Some((id, found)) = reply(&text) else { continue };
        if found.at < since {
            continue;
        }
        match file.replies.last_mut() {
            // One reply is written as several lines, each with what it had taken so far.
            Some(last) if !id.is_empty() && id == file.last_id => {
                last.read = last.read.max(found.read);
                last.written = last.written.max(found.written);
            }
            _ => {
                file.last_id = id.to_string();
                file.replies.push(found);
            }
        }
    }
}

/// What one line of a transcript says a reply took, if it is a reply's line; with the
/// reply's own name, by which its other lines are known.
fn reply(line: &str) -> Option<(&str, Reply)> {
    const STAMP: &str = "\"timestamp\":\"";
    if !line.contains("\"type\":\"assistant\"") {
        return None;
    }
    let usage = &line[line.find("\"usage\":{")?..];
    let count = |key: &str| number(usage, key).unwrap_or(0.0) as u64;
    // The line's own time is the last one in it.
    let at = unix(quoted(&line[line.rfind(STAMP)? + STAMP.len()..])?)?;
    let id = line.find("\"id\":\"msg_").and_then(|at| quoted(&line[at + 6..])).unwrap_or("");
    Some((id, Reply { at, read: count("\"input_tokens\":") + count("\"cache_creation_input_tokens\":"), written: count("\"output_tokens\":") }))
}

/// The share of the plan's five-hour and weekly limits used, as Claude Code last noted it
/// in its own settings file: percent, and the seconds until the count starts over. A count
/// that has started over since then says nothing any more and is left out.
fn limits(home: &Path, now: i64) -> (Option<(u32, i64)>, Option<(u32, i64)>) {
    let Ok(text) = std::fs::read_to_string(home.join(".claude.json")) else { return (None, None) };
    let Some(at) = text.find("\"cachedUsageUtilization\":{") else { return (None, None) };
    let noted = &text[at..];
    let window = |key: &str| {
        const RESETS: &str = "\"resets_at\":\"";
        let part = &noted[noted.find(key)?..];
        let percent = number(part, "\"utilization\":")?;
        let left = unix(quoted(&part[part.find(RESETS)? + RESETS.len()..])?)? - now;
        (left > 0).then_some((percent.round() as u32, left))
    };
    (window("\"five_hour\":{"), window("\"seven_day\":{"))
}

/// The number that follows `key` in a piece of JSON.
fn number(text: &str, key: &str) -> Option<f64> {
    let rest = &text[text.find(key)? + key.len()..];
    let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-')).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// The text up to the closing quote.
fn quoted(rest: &str) -> Option<&str> {
    rest.find('"').map(|end| &rest[..end])
}

/// Seconds since 1970 for a time written as 2026-11-16T01:33:35.321Z, or with +07:00 at its end.
fn unix(stamp: &str) -> Option<i64> {
    let part = |from: usize, to: usize| stamp.get(from..to)?.parse::<i64>().ok();
    let (year, month, day, hour, minute, second) = (part(0, 4)?, part(5, 7)?, part(8, 10)?, part(11, 13)?, part(14, 16)?, part(17, 19)?);
    // Days since 1970, counting years from March so the leap day comes last.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let days = era * 146_097 + year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year - 719_468;
    let mut at = days * 86_400 + hour * 3600 + minute * 60 + second;
    // Ahead of or behind UTC, if the time says so.
    let tail = &stamp[19..];
    if let Some(sign) = tail.find(['+', '-']) {
        let offset = &tail[sign..];
        if let (Some(hours), Some(minutes)) = (offset.get(1..3).and_then(|v| v.parse::<i64>().ok()), offset.get(4..6).and_then(|v| v.parse::<i64>().ok())) {
            let shift = hours * 3600 + minutes * 60;
            at -= if offset.starts_with('+') { shift } else { -shift };
        }
    }
    Some(at)
}
