//! Small helpers: wide strings, data folder, log file.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// UTF-16, NUL-terminated.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads a UTF-16 buffer up to the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Folder that holds `ben.ini`, `pins.txt` and `ben.log`: next to the exe when
/// that folder is writable, otherwise `%LOCALAPPDATA%\Ben`.
pub fn data_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf()));
        if let Some(dir) = exe_dir {
            let probe = dir.join(".ben-write-test");
            if File::create(&probe).is_ok() {
                let _ = std::fs::remove_file(&probe);
                return dir;
            }
        }
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let dir = base.join("Ben");
        let _ = std::fs::create_dir_all(&dir);
        dir
    })
    .clone()
}

static LOG: OnceLock<Mutex<Option<File>>> = OnceLock::new();

pub fn log_init() {
    let path = data_dir().join("ben.log");
    // Keep the log from growing without bound: start over past 1 MB.
    let too_big = std::fs::metadata(&path).map(|m| m.len() > 1_000_000).unwrap_or(false);
    let file = OpenOptions::new().create(true).append(!too_big).write(true).truncate(too_big).open(&path).ok();
    let _ = LOG.set(Mutex::new(file));
}

pub fn log_line(msg: &str) {
    let Some(lock) = LOG.get() else { return };
    let Ok(mut guard) = lock.lock() else { return };
    if let Some(f) = guard.as_mut() {
        let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let secs = (t / 1000) % 86_400;
        let _ = writeln!(f, "[{:02}:{:02}:{:02}.{:03} UTC] {}", secs / 3600, (secs / 60) % 60, secs % 60, t % 1000, msg);
        let _ = f.flush();
    }
}

#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => { $crate::util::log_line(&format!($($arg)*)) };
}
