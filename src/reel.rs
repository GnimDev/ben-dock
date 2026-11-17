//! A video file, as the player is given it.
//!
//! Most files are handed over by their path. One kind is not: an MP4 file that comes in
//! pieces (the kind a browser is fed while it streams, and what is on disk when such a
//! stream is kept) and that also carries a note saying which part of it is to be played,
//! an "edit list". Windows' player takes that note to mean that there is nothing to play:
//! it opens the file, says how long and how large the video is, and is at its end at once.
//! Without the note the same file plays.
//!
//! So such a file is read through a stream of the app's own, which hands every byte over
//! as it is in the file, except that the box holding the note goes by another name
//! ("free": room that nobody uses), which the player skips. The file itself is only ever
//! read, and no copy of it is made.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Mutex;
use windows::core::{implement, Error, Ref, Result, HRESULT, HSTRING};
use windows::Foundation::Uri;
use windows::Media::Core::MediaSource;
use windows::Storage::Streams::IRandomAccessStream;
use windows::Win32::Foundation::{E_FAIL, E_NOTIMPL, E_POINTER, STG_E_ACCESSDENIED, STG_E_INVALIDFUNCTION, S_FALSE, S_OK};
use windows::Win32::System::Com::{ISequentialStream_Impl, IStream, IStream_Impl, LOCKTYPE, STATFLAG, STATSTG, STGC, STGM_READ, STGTY_STREAM, STREAM_SEEK, STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET};
use windows::Win32::System::WinRT::{CreateRandomAccessStreamOverStream, BSOS_DEFAULT};

/// The most of a file's table of contents that is read to look for the note.
const MOST_CONTENTS: u64 = 32 * 1024 * 1024;

/// The video at `path`, for a player to play.
pub fn source(path: &str) -> Result<MediaSource> {
    let renamed = File::open(path).map(|mut file| edits(&mut file)).unwrap_or_default();
    if renamed.is_empty() {
        return MediaSource::CreateFromUri(&Uri::CreateUri(&HSTRING::from(address(path)))?);
    }
    crate::log!("video: a file in pieces with a note of what to play, which Windows' player would stop at; it is read without the note");
    let stream: IStream = Mended::open(PathBuf::from(path), renamed, 0)?.into();
    let stream: IRandomAccessStream = unsafe { CreateRandomAccessStreamOverStream(&stream, BSOS_DEFAULT)? };
    MediaSource::CreateFromStream(&stream, &HSTRING::from("video/mp4"))
}

/// A file's path as the address a player is given ("file:///C:/..."), with every character
/// that could mean something else in an address written out as its bytes.
fn address(path: &str) -> String {
    // (A file on another computer, "\\name\folder\...", has that computer's name in the
    // place where a file of this one has nothing.)
    let mut text = String::from(if path.starts_with("\\\\") { "file:" } else { "file:///" });
    for byte in path.replace('\\', "/").bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => text.push(byte as char),
            other => text.push_str(&format!("%{other:02X}")),
        }
    }
    text
}

/// One box of an MP4 file, read from `data` at `at`: its name, where its content begins,
/// and where the box ends. `None` where there is no whole box.
fn a_box(data: &[u8], at: usize) -> Option<([u8; 4], usize, usize)> {
    let head = data.get(at..at + 8)?;
    let name = [head[4], head[5], head[6], head[7]];
    let (size, content) = match u32::from_be_bytes([head[0], head[1], head[2], head[3]]) {
        // (0: the box runs to the end. 1: its size is too large for four bytes and follows the name.)
        0 => (data.len() - at, at + 8),
        1 => (u64::from_be_bytes(data.get(at + 8..at + 16)?.try_into().ok()?) as usize, at + 16),
        size => (size as usize, at + 8),
    };
    let end = at.checked_add(size)?;
    (end <= data.len() && end >= content).then_some((name, content, end))
}

/// The boxes named `name` among those that lie one after another from `from` to `to`:
/// for each, where its name is, where its content begins and where it ends.
fn named(data: &[u8], from: usize, to: usize, name: &[u8; 4]) -> Vec<(usize, usize, usize)> {
    let mut found = Vec::new();
    let mut at = from;
    while let Some((this, content, end)) = a_box(&data[..to], at) {
        if this == *name {
            found.push((at + 4, content, end));
        }
        at = end;
    }
    found
}

/// Where in a file's table of contents (its `moov` box, given whole) the names of the boxes
/// are that hold a note of what to play (`edts`, one to a track), if the file comes in
/// pieces (it has an `mvex` box). A file in one piece is played by its note, as it should be.
fn notes(moov: &[u8]) -> Vec<usize> {
    let Some((_, content, end)) = a_box(moov, 0) else { return Vec::new() };
    if named(moov, content, end, b"mvex").is_empty() {
        return Vec::new();
    }
    named(moov, content, end, b"trak").into_iter().flat_map(|(_, content, end)| named(moov, content, end, b"edts")).map(|(name, ..)| name).collect()
}

/// The same for a file: where in it those names are. Empty for a file that is not an MP4
/// file, or needs nothing done to it.
fn edits(file: &mut File) -> Vec<u64> {
    let Ok(length) = file.metadata().map(|data| data.len()) else { return Vec::new() };
    let mut at = 0u64;
    // (The table of contents is one of the first few boxes in a file that comes in pieces.)
    for _ in 0..16 {
        let mut head = [0u8; 16];
        if file.seek(SeekFrom::Start(at)).is_err() || file.read_exact(&mut head).is_err() {
            break;
        }
        let size = match u32::from_be_bytes([head[0], head[1], head[2], head[3]]) {
            0 => length - at,
            1 => u64::from_be_bytes([head[8], head[9], head[10], head[11], head[12], head[13], head[14], head[15]]),
            size => size as u64,
        };
        // Not a box: not an MP4 file.
        if size < 8 || at + size > length || !head[4..8].iter().all(|byte| byte.is_ascii_graphic() || *byte == b' ') {
            break;
        }
        if &head[4..8] == b"moov" {
            let mut moov = vec![0u8; size.min(MOST_CONTENTS) as usize];
            if size > MOST_CONTENTS || file.seek(SeekFrom::Start(at)).is_err() || file.read_exact(&mut moov).is_err() {
                break;
            }
            return notes(&moov).into_iter().map(|name| at + name as u64).collect();
        }
        at += size;
    }
    Vec::new()
}

/// A file read as it is, but for the boxes whose names are at `renamed`: they go by the
/// name "free".
#[implement(IStream)]
struct Mended {
    path: PathBuf,
    /// The file, and where in it the next read begins.
    file: Mutex<(File, u64)>,
    size: u64,
    renamed: Vec<u64>,
}

impl Mended {
    fn open(path: PathBuf, renamed: Vec<u64>, at: u64) -> Result<Self> {
        let failed = |e: std::io::Error| Error::new(E_FAIL, e.to_string());
        let file = File::open(&path).map_err(failed)?;
        let size = file.metadata().map_err(failed)?.len();
        Ok(Mended { path, file: Mutex::new((file, at)), size, renamed })
    }
}

/// Gives the boxes whose names are at `renamed` the name "free" in what was read: `read`
/// is a piece of the file that begins at `at`.
fn rename(read: &mut [u8], at: u64, renamed: &[u64]) {
    for name in renamed {
        for (index, letter) in b"free".iter().enumerate() {
            let place = name + index as u64;
            if place >= at && place < at + read.len() as u64 {
                read[(place - at) as usize] = *letter;
            }
        }
    }
}

impl ISequentialStream_Impl for Mended_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> HRESULT {
        if pv.is_null() {
            return E_POINTER;
        }
        let Ok(mut held) = self.file.lock() else { return E_FAIL };
        let (file, at) = &mut *held;
        // SAFETY: the caller hands over room for `cb` bytes.
        let out = unsafe { std::slice::from_raw_parts_mut(pv as *mut u8, cb as usize) };
        let mut got = 0;
        if file.seek(SeekFrom::Start(*at)).is_err() {
            return E_FAIL;
        }
        // (One read may bring less than was asked for; a stream gives all there is.)
        while got < out.len() {
            match file.read(&mut out[got..]) {
                Ok(0) => break,
                Ok(more) => got += more,
                Err(_) => return E_FAIL,
            }
        }
        rename(&mut out[..got], *at, &self.renamed);
        *at += got as u64;
        if !pcbread.is_null() {
            // SAFETY: the caller's own place for the count.
            unsafe { *pcbread = got as u32 };
        }
        if got < out.len() {
            S_FALSE
        } else {
            S_OK
        }
    }

    fn Write(&self, _pv: *const core::ffi::c_void, _cb: u32, _pcbwritten: *mut u32) -> HRESULT {
        STG_E_ACCESSDENIED
    }
}

impl IStream_Impl for Mended_Impl {
    fn Seek(&self, dlibmove: i64, dworigin: STREAM_SEEK, plibnewposition: *mut u64) -> Result<()> {
        let mut held = self.file.lock().map_err(|_| Error::from(E_FAIL))?;
        let from = match dworigin {
            STREAM_SEEK_SET => 0,
            STREAM_SEEK_CUR => held.1 as i64,
            STREAM_SEEK_END => self.size as i64,
            _ => return Err(STG_E_INVALIDFUNCTION.into()),
        };
        held.1 = from.checked_add(dlibmove).filter(|to| *to >= 0).ok_or_else(|| Error::from(STG_E_INVALIDFUNCTION))? as u64;
        if !plibnewposition.is_null() {
            // SAFETY: the caller's own place for the answer.
            unsafe { *plibnewposition = held.1 };
        }
        Ok(())
    }

    fn SetSize(&self, _libnewsize: u64) -> Result<()> {
        Err(STG_E_ACCESSDENIED.into())
    }

    fn CopyTo(&self, _pstm: Ref<IStream>, _cb: u64, _pcbread: *mut u64, _pcbwritten: *mut u64) -> Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn Commit(&self, _grfcommitflags: &STGC) -> Result<()> {
        Ok(())
    }

    fn Revert(&self) -> Result<()> {
        Ok(())
    }

    fn LockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: &LOCKTYPE) -> Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn UnlockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: u32) -> Result<()> {
        Err(STG_E_INVALIDFUNCTION.into())
    }

    fn Stat(&self, pstatstg: *mut STATSTG, _grfstatflag: &STATFLAG) -> Result<()> {
        if pstatstg.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: the caller's own place for the answer.
        unsafe { *pstatstg = STATSTG { r#type: STGTY_STREAM.0 as u32, cbSize: self.size, grfMode: STGM_READ, ..Default::default() } };
        Ok(())
    }

    fn Clone(&self) -> Result<IStream> {
        // Another reader of the same file, at the same place.
        let at = self.file.lock().map_err(|_| Error::from(E_FAIL))?.1;
        Ok(Mended::open(self.path.clone(), self.renamed.clone(), at)?.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A box: four bytes of size, four of name, then what is in it.
    fn boxed(name: &[u8; 4], inside: &[u8]) -> Vec<u8> {
        let mut bytes = ((inside.len() + 8) as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(name);
        bytes.extend_from_slice(inside);
        bytes
    }

    #[test]
    fn a_path_becomes_an_address_that_cannot_be_misread() {
        assert_eq!(address("C:\\Users\\me\\Videos\\sea.mp4"), "file:///C:/Users/me/Videos/sea.mp4");
        // A space, a hash, a percent sign and letters with marks would each mean something
        // else in an address, or nothing.
        assert_eq!(address("D:\\My films\\50% #1.mp4"), "file:///D:/My%20films/50%25%20%231.mp4");
        assert_eq!(address("C:\\Phim\\biển.mp4"), "file:///C:/Phim/bi%E1%BB%83n.mp4");
        // A file on another computer.
        assert_eq!(address("\\\\box\\films\\sea.mp4"), "file://box/films/sea.mp4");
    }

    #[test]
    fn the_note_is_found_only_in_a_file_that_comes_in_pieces() {
        let track = |with_note: bool| {
            let mut inside = boxed(b"tkhd", &[0; 20]);
            if with_note {
                inside.extend(boxed(b"edts", &boxed(b"elst", &[0; 12])));
            }
            inside.extend(boxed(b"mdia", &[0; 30]));
            boxed(b"trak", &inside)
        };
        // In pieces (it has `mvex`), two tracks, one of them with a note.
        let mut inside = boxed(b"mvhd", &[0; 100]);
        inside.extend(boxed(b"mvex", &boxed(b"trex", &[0; 24])));
        inside.extend(track(false));
        inside.extend(track(true));
        let moov = boxed(b"moov", &inside);
        let found = notes(&moov);
        assert_eq!(found.len(), 1);
        assert_eq!(&moov[found[0]..found[0] + 4], b"edts");
        // (The four letters "edts" inside another box's content are not taken for a box.)
        let mut odd = boxed(b"mvhd", b"....edts....");
        odd.extend(boxed(b"mvex", &[]));
        assert!(notes(&boxed(b"moov", &odd)).is_empty());
        // In one piece: the note is left to the player.
        let mut inside = boxed(b"mvhd", &[0; 100]);
        inside.extend(track(true));
        assert!(notes(&boxed(b"moov", &inside)).is_empty());
        // Not a table of contents at all, or one cut short.
        assert!(notes(b"not a box").is_empty());
        assert!(notes(&moov[..moov.len() - 5]).is_empty());
    }

    #[test]
    fn what_is_read_has_the_box_under_its_other_name_wherever_the_read_begins() {
        let file: Vec<u8> = (0..40u8).collect();
        let renamed = [10u64];
        // The whole of it at once.
        let mut read = file.clone();
        rename(&mut read, 0, &renamed);
        assert_eq!(&read[10..14], b"free");
        assert_eq!(&read[..10], &file[..10]);
        assert_eq!(&read[14..], &file[14..]);
        // A read that ends in the middle of the name, and the next one that begins there.
        let (mut first, mut second) = (file[4..12].to_vec(), file[12..20].to_vec());
        rename(&mut first, 4, &renamed);
        rename(&mut second, 12, &renamed);
        assert_eq!(&first[6..], b"fr");
        assert_eq!(&second[..2], b"ee");
        assert_eq!(&second[2..], &file[14..20]);
        // A read nowhere near it is as it was.
        let mut far = file[20..30].to_vec();
        rename(&mut far, 20, &renamed);
        assert_eq!(far, &file[20..30]);
    }

    #[test]
    fn a_file_is_looked_into_without_being_changed() {
        let dir = std::env::temp_dir().join(format!("ben-reel-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a folder");
        // A file in pieces with a note: `ftyp`, then the table of contents, then a piece.
        let mut inside = boxed(b"mvhd", &[0; 100]);
        inside.extend(boxed(b"mvex", &boxed(b"trex", &[0; 24])));
        let mut track = boxed(b"tkhd", &[0; 20]);
        track.extend(boxed(b"edts", &boxed(b"elst", &[0; 12])));
        inside.extend(boxed(b"trak", &track));
        let mut bytes = boxed(b"ftyp", b"dash\0\0\0\0iso6");
        bytes.extend(boxed(b"moov", &inside));
        bytes.extend(boxed(b"moof", &[0; 16]));
        bytes.extend(boxed(b"mdat", &[7; 64]));
        let path = dir.join("pieces.mp4");
        std::fs::write(&path, &bytes).expect("the file written");
        let found = edits(&mut File::open(&path).expect("the file"));
        assert_eq!(found.len(), 1);
        assert_eq!(&bytes[found[0] as usize..found[0] as usize + 4], b"edts");
        assert_eq!(std::fs::read(&path).expect("the file read"), bytes, "the file is as it was");
        // Something that is not an MP4 file at all, and nothing.
        let other = dir.join("other.mkv");
        std::fs::write(&other, [0x1a, 0x45, 0xdf, 0xa3, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16]).expect("the file written");
        assert!(edits(&mut File::open(&other).expect("the file")).is_empty());
        std::fs::write(&other, []).expect("the file written");
        assert!(edits(&mut File::open(&other).expect("the file")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
