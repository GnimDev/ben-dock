//! Notifications: what other apps have told the user through Windows, read from the
//! notification centre so the island can show each one as it arrives.
//!
//! Windows tells a packaged app when the list changes. A plain program may be refused
//! that, and then this looks at the list again every second and a half. Like the
//! now-playing watcher it does so on a small thread of its own and nudges the app's
//! window when there is something new. What a notification says is never written to the log.

use crate::media::{self, Art};
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Duration;
use windows::Foundation::{Size, TypedEventHandler};
use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus as Access};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// How often the list is read when Windows will not say that it changed, and how often it
/// is read anyway when Windows does.
const LOOK_EVERY: Duration = Duration::from_millis(1500);
const LOOK_ANYWAY: Duration = Duration::from_secs(20);
/// An app's own mark is kept at this many pixels a side.
const LOGO: u32 = 64;

#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub id: u32,
    /// The app that sent it, as Windows names it, and the name Windows knows it by.
    pub app: String,
    pub app_id: String,
    pub title: String,
    pub body: String,
    /// When it arrived, in Windows' own count of time (see `media::ticks_now`).
    pub arrived: i64,
    pub logo: Option<Art>,
}

enum Wake {
    Look,
    Remove(u32),
    Clear,
    Stop,
}

pub struct Notices {
    tx: Sender<Wake>,
    rx: Receiver<(Vec<Notice>, Vec<Notice>)>,
}

impl Notices {
    /// Starts watching. `window` gets `message` whenever there is something new to `take`.
    pub fn start(window: HWND, message: u32) -> Notices {
        let (tx, wake) = channel::<Wake>();
        let (out, rx) = channel();
        let signal = tx.clone();
        let window = window.0 as isize;
        let spawned = std::thread::Builder::new().name("notices".into()).stack_size(256 * 1024).spawn(move || watch(wake, signal, out, window, message));
        if let Err(e) = spawned {
            crate::log!("could not start the notification watcher: {e}");
        }
        Notices { tx, rx }
    }

    /// If anything changed since the last call: every notification there is (newest first),
    /// and those that arrived since.
    pub fn take(&self) -> Option<(Vec<Notice>, Vec<Notice>)> {
        let mut latest = None;
        let mut fresh = Vec::new();
        for (all, new) in self.rx.try_iter() {
            fresh.extend(new);
            latest = Some(all);
        }
        latest.map(|all| (all, fresh))
    }

    /// Takes one notification out of Windows' notification centre.
    pub fn remove(&self, id: u32) {
        let _ = self.tx.send(Wake::Remove(id));
    }

    /// Empties Windows' notification centre.
    pub fn clear(&self) {
        let _ = self.tx.send(Wake::Clear);
    }
}

impl Drop for Notices {
    fn drop(&mut self) {
        let _ = self.tx.send(Wake::Stop);
    }
}

fn watch(wake: Receiver<Wake>, signal: Sender<Wake>, out: Sender<(Vec<Notice>, Vec<Notice>)>, window: isize, message: u32) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let listener = match UserNotificationListener::Current() {
        Ok(listener) => listener,
        Err(e) => {
            crate::log!("notifications are unavailable: {e}");
            return;
        }
    };
    let mut access = listener.GetAccessStatus().unwrap_or(Access::Unspecified);
    if access == Access::Unspecified {
        access = listener.RequestAccessAsync().and_then(|op| op.join()).unwrap_or(access);
    }
    if access != Access::Allowed {
        crate::log!("Windows does not let this app read notifications ({access:?}); see Settings > Privacy & security > Notifications");
        return;
    }
    let told = listener
        .NotificationChanged(&TypedEventHandler::new(move |_, _| {
            let _ = signal.send(Wake::Look);
            Ok(())
        }))
        .is_ok();
    let pause = if told { LOOK_ANYWAY } else { LOOK_EVERY };
    crate::log!("watching notifications; {}", if told { "Windows says when they change" } else { "Windows will not say when they change, so looking every 1.5 s" });

    let mut known: HashMap<u32, Notice> = HashMap::new();
    let mut first = true;
    loop {
        match wake.recv_timeout(pause) {
            Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Wake::Remove(id)) => {
                let _ = listener.RemoveNotification(id);
            }
            Ok(Wake::Clear) => {
                let _ = listener.ClearNotifications();
            }
            Ok(Wake::Look) | Err(RecvTimeoutError::Timeout) => {}
        }
        let Ok(list) = listener.GetNotificationsAsync(NotificationKinds::Toast).and_then(|op| op.join()) else { continue };
        let (mut all, mut fresh) = (Vec::new(), Vec::new());
        for notification in &list {
            let Ok(id) = notification.Id() else { continue };
            // One already seen is not read again: only its number is asked for.
            let notice = match known.get(&id) {
                Some(seen) => seen.clone(),
                None => {
                    let new = read(&notification, id);
                    // What was already there when the app started is not news.
                    if !first {
                        fresh.push(new.clone());
                    }
                    new
                }
            };
            all.push(notice);
        }
        let changed = first || !fresh.is_empty() || all.len() != known.len();
        known = all.iter().map(|notice| (notice.id, notice.clone())).collect();
        first = false;
        if !changed {
            continue;
        }
        all.sort_by(|a, b| b.arrived.cmp(&a.arrived));
        if out.send((all, fresh)).is_err() {
            break;
        }
        unsafe {
            let _ = PostMessageW(Some(HWND(window as *mut _)), message, WPARAM(0), LPARAM(0));
        }
    }
}

/// What one notification says: its first line is its title, the rest its body.
fn read(notification: &UserNotification, id: u32) -> Notice {
    let display = notification.AppInfo().and_then(|app| app.DisplayInfo()).ok();
    let app = display.as_ref().and_then(|d| d.DisplayName().ok()).map(|name| name.to_string()).unwrap_or_default();
    let app_id = notification.AppInfo().and_then(|app| app.AppUserModelId()).map(|name| name.to_string()).unwrap_or_default();
    let logo = display
        .as_ref()
        .and_then(|d| d.GetLogo(Size { Width: LOGO as f32, Height: LOGO as f32 }).ok())
        .and_then(|logo| logo.OpenReadAsync().ok())
        .and_then(|op| op.join().ok())
        .and_then(|stream| media::picture(&stream, LOGO))
        .map(|pixels| Art(Arc::new(pixels)));
    let mut lines: Vec<String> = Vec::new();
    let texts = KnownNotificationBindings::ToastGeneric().and_then(|generic| notification.Notification()?.Visual()?.GetBinding(&generic)?.GetTextElements());
    if let Ok(texts) = texts {
        for text in &texts {
            let line = text.Text().map(|t| t.to_string()).unwrap_or_default().replace(['\r', '\n'], " ");
            if !line.trim().is_empty() {
                lines.push(line.trim().to_string());
            }
        }
    }
    let title = if lines.is_empty() { String::new() } else { lines.remove(0) };
    Notice { id, app, app_id, title, body: lines.join(" "), arrived: notification.CreationTime().map(|at| at.UniversalTime).unwrap_or(0), logo }
}
