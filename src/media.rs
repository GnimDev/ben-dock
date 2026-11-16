//! What is playing: the track and artist of whichever app Windows considers
//! in charge of playback, its cover, how far into the track it is, and
//! play / pause / skip / seek / shuffle / repeat for it.
//!
//! Windows tells us when anything changes, so nothing is polled. All of it
//! runs on a small thread of its own: the calls can take a while, and the
//! thread that animates must never wait for them. That thread only sends the
//! latest state over a channel and nudges the app's window.

use crate::shell::IconPixels;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use windows::core::Result;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapBounds, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform, ColorManagementMode, ExifOrientationMode};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Media::MediaPlaybackAutoRepeatMode as RepeatMode;
use windows::Storage::Streams::IRandomAccessStreamWithContentType;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// A cover is kept at this many pixels a side: enough for the island at twice the size.
const COVER: u32 = 96;

/// A picture that came with a track or a notification: square, ready to be shown. Two are
/// equal only when they are the very same picture, so nothing has to compare pixels.
#[derive(Clone)]
pub struct Art(pub Arc<IconPixels>);

impl PartialEq for Art {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for Art {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "Art({} px)", self.0.size)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// The cover, if the player gave one.
    pub art: Option<Art>,
    /// How far into the track it was when `read`, and how long the track is, in seconds
    /// (a length of 0: the player does not say).
    pub position: f32,
    pub length: f32,
    pub read: Option<Instant>,
    /// The player can be sent to another place in the track.
    pub seekable: bool,
    /// Shuffle, and repeat (0 off, 1 the one track, 2 the whole list), where the player
    /// lets them be changed.
    pub shuffle: Option<bool>,
    pub repeat: Option<u8>,
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Toggle,
    Next,
    Previous,
    /// Jump to this share of the track, 0 to 1.
    Seek(f32),
    Shuffle,
    Repeat,
}

enum Wake {
    /// Something about playback changed: read it again.
    Changed,
    /// The track or its cover changed: read the cover again too.
    Track,
    Do(Command),
    Stop,
}

pub struct Media {
    tx: Sender<Wake>,
    rx: Receiver<Option<NowPlaying>>,
}

impl Media {
    /// Starts watching. `window` gets `message` whenever there is something new to `take`.
    pub fn start(window: HWND, message: u32) -> Media {
        let (tx, wake) = channel::<Wake>();
        let (out, rx) = channel::<Option<NowPlaying>>();
        let signal = tx.clone();
        let window = window.0 as isize;
        let spawned = std::thread::Builder::new().name("media".into()).stack_size(256 * 1024).spawn(move || watch(wake, signal, out, window, message));
        if let Err(e) = spawned {
            crate::log!("could not start the now-playing watcher: {e}");
        }
        Media { tx, rx }
    }

    /// The newest state, if it changed since the last call. `Some(None)`: nothing is playing any more.
    pub fn take(&self) -> Option<Option<NowPlaying>> {
        self.rx.try_iter().last()
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(Wake::Do(command));
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        let _ = self.tx.send(Wake::Stop);
    }
}

/// Now, in Windows' own count of time: units of 100 ns since 1601.
pub fn ticks_now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| (d.as_nanos() / 100) as i64).unwrap_or(0) + 116_444_736_000_000_000
}

fn watch(wake: Receiver<Wake>, signal: Sender<Wake>, out: Sender<Option<NowPlaying>>, window: isize, message: u32) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let manager = match Manager::RequestAsync().and_then(|op| op.join()) {
        Ok(manager) => manager,
        Err(e) => {
            crate::log!("now-playing is unavailable: {e}");
            return;
        }
    };
    // Another app took over playback (or the last one closed).
    let nudge = signal.clone();
    let _ = manager.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        let _ = nudge.send(Wake::Track);
        Ok(())
    }));

    let mut session: Option<Session> = None;
    let mut tokens = (0i64, 0i64, 0i64);
    let mut last: Option<Option<NowPlaying>> = None;
    let mut art: Option<Art> = None;
    let mut next = Some(Wake::Track);
    loop {
        let event = match next.take() {
            Some(event) => event,
            None => match wake.recv() {
                Ok(event) => event,
                Err(_) => break,
            },
        };
        match event {
            Wake::Stop => break,
            Wake::Do(command) => {
                if let Some(session) = &session {
                    let asked = match command {
                        Command::Toggle => session.TryTogglePlayPauseAsync(),
                        Command::Next => session.TrySkipNextAsync(),
                        Command::Previous => session.TrySkipPreviousAsync(),
                        Command::Seek(share) => session.GetTimelineProperties().and_then(|line| {
                            let (start, end) = (line.StartTime()?.Duration, line.EndTime()?.Duration);
                            session.TryChangePlaybackPositionAsync(start + ((end - start) as f64 * share.clamp(0.0, 1.0) as f64) as i64)
                        }),
                        Command::Shuffle => session.GetPlaybackInfo().and_then(|info| session.TryChangeShuffleActiveAsync(!info.IsShuffleActive()?.Value()?)),
                        // Off, then the whole list, then the one track, then off again.
                        Command::Repeat => session.GetPlaybackInfo().and_then(|info| {
                            let after = match info.AutoRepeatMode()?.Value()? {
                                RepeatMode::None => RepeatMode::List,
                                RepeatMode::List => RepeatMode::Track,
                                _ => RepeatMode::None,
                            };
                            session.TryChangeAutoRepeatModeAsync(after)
                        }),
                    };
                    if let Err(e) = asked.and_then(|op| op.join()) {
                        crate::log!("the player did not take {command:?}: {e}");
                    }
                }
            }
            Wake::Changed | Wake::Track => {
                let mut track = matches!(event, Wake::Track);
                // Changes arrive in bursts (title, then artist, then status): read once for all of them.
                while let Ok(more) = wake.try_recv() {
                    match more {
                        Wake::Changed => {}
                        Wake::Track => track = true,
                        other => {
                            next = Some(other);
                            break;
                        }
                    }
                }
                let current = manager.GetCurrentSession().ok();
                if session != current {
                    if let Some(old) = session.take() {
                        let _ = old.RemoveMediaPropertiesChanged(tokens.0);
                        let _ = old.RemovePlaybackInfoChanged(tokens.1);
                        let _ = old.RemoveTimelinePropertiesChanged(tokens.2);
                    }
                    if let Some(new) = &current {
                        let (a, b, c) = (signal.clone(), signal.clone(), signal.clone());
                        tokens.0 = new
                            .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                                let _ = a.send(Wake::Track);
                                Ok(())
                            }))
                            .unwrap_or(0);
                        tokens.1 = new
                            .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                                let _ = b.send(Wake::Changed);
                                Ok(())
                            }))
                            .unwrap_or(0);
                        tokens.2 = new
                            .TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
                                let _ = c.send(Wake::Changed);
                                Ok(())
                            }))
                            .unwrap_or(0);
                    }
                    session = current;
                    track = true;
                }
                let now = session.as_ref().and_then(|session| read(session, &mut art, track));
                if last.as_ref() != Some(&now) {
                    last = Some(now.clone());
                    if out.send(now).is_err() {
                        break;
                    }
                    unsafe {
                        let _ = PostMessageW(Some(HWND(window as *mut _)), message, WPARAM(0), LPARAM(0));
                    }
                }
            }
        }
    }
}

/// Reads what the session is playing. With `track`, its cover is read again into `art`.
fn read(session: &Session, art: &mut Option<Art>, track: bool) -> Option<NowPlaying> {
    let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let title = props.Title().map(|t| t.to_string()).unwrap_or_default();
    let artist = props.Artist().map(|t| t.to_string()).unwrap_or_default();
    if track {
        let new = props.Thumbnail().ok().and_then(|picture| picture.OpenReadAsync().ok()).and_then(|op| op.join().ok()).and_then(|stream| picture(&stream, COVER)).map(|mut pixels| {
            round(&mut pixels, COVER as f32 * 0.27);
            pixels
        });
        // The same picture as before stays the same `Art`.
        *art = match (art.take(), new) {
            (Some(old), Some(new)) if old.0.bgra == new.bgra => Some(old),
            (_, new) => new.map(|pixels| Art(Arc::new(pixels))),
        };
    }

    let info = session.GetPlaybackInfo().ok();
    let playing = info.as_ref().and_then(|info| info.PlaybackStatus().ok()).map(|status| status == PlaybackStatus::Playing).unwrap_or(false);
    let (mut seekable, mut shuffle, mut repeat) = (false, None, None);
    if let Some(info) = &info {
        if let Ok(controls) = info.Controls() {
            seekable = controls.IsPlaybackPositionEnabled().unwrap_or(false);
            if controls.IsShuffleEnabled().unwrap_or(false) {
                shuffle = info.IsShuffleActive().and_then(|on| on.Value()).ok();
            }
            if controls.IsRepeatEnabled().unwrap_or(false) {
                repeat = info.AutoRepeatMode().and_then(|mode| mode.Value()).ok().map(|mode| mode.0 as u8);
            }
        }
    }

    let (mut position, mut length) = (0.0, 0.0);
    if let Ok(line) = session.GetTimelineProperties() {
        let ticks = |span: Result<TimeSpan>| span.map(|span| span.Duration).unwrap_or(0);
        let (start, end, mut at) = (ticks(line.StartTime()), ticks(line.EndTime()), ticks(line.Position()));
        // The position is the one at `LastUpdatedTime`: a player reports it only now and then.
        if playing {
            if let Ok(when) = line.LastUpdatedTime() {
                if when.UniversalTime > 0 {
                    at += (ticks_now() - when.UniversalTime).max(0);
                }
            }
        }
        length = (end - start).max(0) as f32 / 1e7;
        position = ((at - start).max(0) as f32 / 1e7).min(length);
    }
    (!title.is_empty() || !artist.is_empty()).then_some(NowPlaying { title, artist, playing, art: art.clone(), position, length, read: Some(Instant::now()), seekable: seekable && length > 0.0, shuffle, repeat })
}

/// A picture a player or an app hands over, as a square of `side` pixels: the middle of it,
/// if it is not square.
pub fn picture(stream: &IRandomAccessStreamWithContentType, side: u32) -> Option<IconPixels> {
    let decoder = BitmapDecoder::CreateAsync(stream).ok()?.join().ok()?;
    let (w, h) = (decoder.PixelWidth().ok()?, decoder.PixelHeight().ok()?);
    if w == 0 || h == 0 {
        return None;
    }
    let scale = side as f32 / w.min(h) as f32;
    let (sw, sh) = (((w as f32 * scale).round() as u32).max(side), ((h as f32 * scale).round() as u32).max(side));
    let transform = BitmapTransform::new().ok()?;
    transform.SetScaledWidth(sw).ok()?;
    transform.SetScaledHeight(sh).ok()?;
    transform.SetInterpolationMode(BitmapInterpolationMode::Fant).ok()?;
    transform.SetBounds(BitmapBounds { X: (sw - side) / 2, Y: (sh - side) / 2, Width: side, Height: side }).ok()?;
    let data = decoder.GetPixelDataTransformedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied, &transform, ExifOrientationMode::IgnoreExifOrientation, ColorManagementMode::DoNotColorManage).ok()?.join().ok()?;
    let bgra = data.DetachPixelData().ok()?.to_vec();
    (bgra.len() == (side * side * 4) as usize).then_some(IconPixels { size: side, bgra })
}

/// Rounds the corners of a square picture.
fn round(pixels: &mut IconPixels, radius: f32) {
    let side = pixels.size as usize;
    for y in 0..side {
        for x in 0..side {
            // How far into a corner's square this pixel is, measured from the centre of the arc.
            let dx = (radius - 0.5 - x.min(side - 1 - x) as f32).max(0.0);
            let dy = (radius - 0.5 - y.min(side - 1 - y) as f32).max(0.0);
            if dx == 0.0 || dy == 0.0 {
                continue;
            }
            let cover = (radius + 0.5 - (dx * dx + dy * dy).sqrt()).clamp(0.0, 1.0);
            for channel in &mut pixels.bgra[(y * side + x) * 4..][..4] {
                *channel = (*channel as f32 * cover) as u8;
            }
        }
    }
}
