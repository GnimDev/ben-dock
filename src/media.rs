//! What is playing: the track and artist of whichever app Windows considers
//! in charge of playback, and play / pause / skip for it.
//!
//! Windows tells us when anything changes, so nothing is polled. All of it
//! runs on a small thread of its own: the calls can take a while, and the
//! thread that animates must never wait for them. That thread only sends the
//! latest state over a channel and nudges the app's window.

use std::sync::mpsc::{channel, Receiver, Sender};
use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub playing: bool,
}

#[derive(Clone, Copy, Debug)]
pub enum Command {
    Toggle,
    Next,
    Previous,
}

enum Wake {
    /// Something about playback changed: read it again.
    Changed,
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
        let _ = nudge.send(Wake::Changed);
        Ok(())
    }));

    let mut session: Option<Session> = None;
    let mut tokens = (0i64, 0i64);
    let mut last: Option<Option<NowPlaying>> = None;
    let mut next = Some(Wake::Changed);
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
                    };
                    if let Err(e) = asked.and_then(|op| op.join()) {
                        crate::log!("the player did not take {command:?}: {e}");
                    }
                }
            }
            Wake::Changed => {
                // Changes arrive in bursts (title, then artist, then status): read once for all of them.
                while let Ok(more) = wake.try_recv() {
                    if !matches!(more, Wake::Changed) {
                        next = Some(more);
                        break;
                    }
                }
                let current = manager.GetCurrentSession().ok();
                if session != current {
                    if let Some(old) = session.take() {
                        let _ = old.RemoveMediaPropertiesChanged(tokens.0);
                        let _ = old.RemovePlaybackInfoChanged(tokens.1);
                    }
                    if let Some(new) = &current {
                        let (a, b) = (signal.clone(), signal.clone());
                        tokens.0 = new
                            .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                                let _ = a.send(Wake::Changed);
                                Ok(())
                            }))
                            .unwrap_or(0);
                        tokens.1 = new
                            .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                                let _ = b.send(Wake::Changed);
                                Ok(())
                            }))
                            .unwrap_or(0);
                    }
                    session = current;
                }
                let now = session.as_ref().and_then(read);
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

fn read(session: &Session) -> Option<NowPlaying> {
    let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let title = props.Title().map(|t| t.to_string()).unwrap_or_default();
    let artist = props.Artist().map(|t| t.to_string()).unwrap_or_default();
    let playing = session.GetPlaybackInfo().and_then(|info| info.PlaybackStatus()).map(|status| status == PlaybackStatus::Playing).unwrap_or(false);
    (!title.is_empty() || !artist.is_empty()).then_some(NowPlaying { title, artist, playing })
}
