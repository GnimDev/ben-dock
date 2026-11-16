//! The app icons that have been read, kept in a file (`icons.cache`, next to `ben.ini`), so
//! that the tray need not ask the shell for them again: the shell takes some 40 ms for each,
//! the file a fraction of a millisecond.
//!
//! The file is a list of records and only ever grows at its end: an 8-byte mark, then for
//! each icon its target's length and text, its size in pixels, the time it was read
//! (seconds since 1970) and its pixels. An icon older than three weeks is read again.

use crate::shell::IconPixels;
use crate::util::data_dir;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MARK: &[u8; 8] = b"BENICON1";
const KEEP_SECONDS: u64 = 21 * 86_400;
/// Past this the file is started over.
const MOST_BYTES: u64 = 24 << 20;

pub struct IconCache {
    path: PathBuf,
    /// Where each icon's pixels are in the file, and how many bytes they are.
    index: HashMap<(String, u32), (u64, u32)>,
    /// The end of the file.
    end: u64,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl IconCache {
    /// Opens the file (making it if need be) and finds the icons in it.
    pub fn open() -> IconCache {
        let path = data_dir().join("icons.cache");
        let mut cache = IconCache { path: path.clone(), index: HashMap::new(), end: MARK.len() as u64 };
        let length = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if length > MARK.len() as u64 && length <= MOST_BYTES {
            match cache.read_index(length) {
                Some(good_end) => {
                    cache.end = good_end;
                    if good_end < length {
                        // A record that was being written when the app stopped: cut it off.
                        let _ = OpenOptions::new().write(true).open(&path).and_then(|f| f.set_len(good_end));
                    }
                    return cache;
                }
                None => cache.index.clear(),
            }
        }
        // Nothing usable there: start a new file.
        if File::create(&path).and_then(|mut f| f.write_all(MARK)).is_err() {
            crate::log!("could not make the icon cache");
        }
        cache
    }

    /// Reads the records' headers. Returns where the last whole record ends, or None if the
    /// file is not one of ours.
    fn read_index(&mut self, length: u64) -> Option<u64> {
        let mut reader = BufReader::new(File::open(&self.path).ok()?);
        let mut mark = [0u8; 8];
        reader.read_exact(&mut mark).ok()?;
        if &mark != MARK {
            return None;
        }
        let mut at = MARK.len() as u64;
        let limit = now().saturating_sub(KEEP_SECONDS);
        loop {
            let mut word = [0u8; 4];
            if reader.read_exact(&mut word).is_err() {
                break;
            }
            let target_len = u32::from_le_bytes(word) as usize;
            if target_len == 0 || target_len > 4096 {
                break;
            }
            let mut target = vec![0u8; target_len];
            let (mut size, mut stamp, mut data_len) = ([0u8; 4], [0u8; 8], [0u8; 4]);
            if reader.read_exact(&mut target).is_err() || reader.read_exact(&mut size).is_err() || reader.read_exact(&mut stamp).is_err() || reader.read_exact(&mut data_len).is_err() {
                break;
            }
            let (size, stamp, data_len) = (u32::from_le_bytes(size), u64::from_le_bytes(stamp), u32::from_le_bytes(data_len));
            let data_at = at + 4 + target_len as u64 + 4 + 8 + 4;
            // Pixels of the size the record claims, and all of them in the file.
            if size == 0 || size > 1024 || data_len as u64 != size as u64 * size as u64 * 4 || data_at + data_len as u64 > length {
                break;
            }
            if reader.seek_relative(data_len as i64).is_err() {
                break;
            }
            at = data_at + data_len as u64;
            if stamp >= limit {
                if let Ok(target) = String::from_utf8(target) {
                    self.index.insert((target, size), (data_at, data_len));
                }
            }
        }
        Some(at)
    }

    pub fn get(&self, target: &str, size: u32) -> Option<IconPixels> {
        let &(at, len) = self.index.get(&(target.to_string(), size))?;
        let mut file = File::open(&self.path).ok()?;
        file.seek(SeekFrom::Start(at)).ok()?;
        let mut bgra = vec![0u8; len as usize];
        file.read_exact(&mut bgra).ok()?;
        Some(IconPixels { size, bgra })
    }

    pub fn put(&mut self, target: &str, size: u32, pixels: &IconPixels) {
        if pixels.size != size || pixels.bgra.len() != (size as usize) * (size as usize) * 4 || target.is_empty() || target.len() > 4096 {
            return;
        }
        let mut record = Vec::with_capacity(pixels.bgra.len() + target.len() + 20);
        record.extend_from_slice(&(target.len() as u32).to_le_bytes());
        record.extend_from_slice(target.as_bytes());
        record.extend_from_slice(&size.to_le_bytes());
        record.extend_from_slice(&now().to_le_bytes());
        record.extend_from_slice(&(pixels.bgra.len() as u32).to_le_bytes());
        let header = record.len() as u64;
        record.extend_from_slice(&pixels.bgra);
        if OpenOptions::new().append(true).open(&self.path).and_then(|mut f| f.write_all(&record)).is_ok() {
            self.index.insert((target.to_string(), size), (self.end + header, pixels.bgra.len() as u32));
            self.end += record.len() as u64;
        }
    }
}
