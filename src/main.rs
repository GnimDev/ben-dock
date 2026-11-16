//! Bến — a liquid interface for Windows 11.
//!
//! Runs in the background from sign-in and replaces the taskbar with two
//! pieces of glass: a dock of app icons along the bottom of the screen and an
//! island at the top that is the clock, the status lights and the control
//! panel. Everything moves on springs. The taskbar comes back when it exits.
//!
//!   ben.exe                    start
//!   ben.exe --quit             stop the running one
//!   ben.exe --restore-taskbar  bring the Windows taskbar back (after a crash)
//!   ben.exe --autostart on|off start with Windows, or stop doing so
//!   ben.exe --bench            start, then run a self-test and write frame timing to ben.log
//!   ben.exe --demo             start, then show every surface in turn

#![windows_subsystem = "windows"]
#![allow(clippy::too_many_arguments)]

mod app;
mod audio;
mod bluetooth;
mod bubble;
mod camera;
mod claude;
mod config;
mod dates;
mod dock;
mod fx;
mod gfx;
mod island;
mod launcher;
mod layout;
mod media;
mod motion;
mod notices;
mod preview;
mod scene;
mod settings;
mod shade;
mod shell;
mod spring;
mod system;
mod text;
mod theme;
mod util;
mod warp;
mod wifi;

use std::time::Instant;
use util::wide;
use windows::core::{Result, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, ResetEvent, SetEvent, WaitForSingleObject, INFINITE};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::WindowsAndMessaging::*;

fn main() {
    util::log_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--restore-taskbar") => {
            system::restore_taskbar();
            return;
        }
        Some("--quit") => {
            quit_running_instance();
            return;
        }
        Some("--autostart") => {
            system::set_autostart(args.get(1).map(|a| a != "off").unwrap_or(true));
            return;
        }
        _ => {}
    }

    // Whatever goes wrong, never leave the user without a taskbar.
    std::panic::set_hook(Box::new(|info| {
        log!("panic: {info}");
        system::restore_taskbar();
    }));

    if let Err(e) = run() {
        log!("fatal: {e}");
        system::restore_taskbar();
        let text = wide(&format!("Bến không khởi động được.\n\n{e}\n\nChi tiết nằm trong file ben.log cạnh ben.exe."));
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(wide("Bến").as_ptr()), MB_OK | MB_ICONERROR);
        }
    }
}

fn quit_running_instance() {
    unsafe {
        if let Ok(hwnd) = FindWindowW(PCWSTR(wide(app::CLASS_NAME).as_ptr()), PCWSTR::null()) {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

fn run() -> Result<()> {
    unsafe {
        // Only one per session.
        let _mutex = CreateMutexW(None, true, PCWSTR(wide("Local\\BenDock.SingleInstance").as_ptr()))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            log!("already running; exiting");
            return Ok(());
        }

        // Draw at the display's real resolution instead of being upscaled (blurry) by Windows.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;

        let cfg = config::Config::load();
        log!("Bến {} starting; settings: {:?}", env!("CARGO_PKG_VERSION"), cfg);

        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = wide(app::CLASS_NAME);
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(app::wndproc),
            hInstance: instance,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(windows::core::Error::from_thread());
        }

        // WS_EX_NOREDIRECTIONBITMAP: the window has no surface of its own, only what the
        //   compositor draws, which is what makes per-pixel transparency possible.
        // WS_EX_NOACTIVATE: clicking the dock must not take focus away from the app in use.
        // WS_EX_TOOLWINDOW: keep the dock itself out of Alt+Tab.
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(wide("Bến").as_ptr()),
            WS_POPUP,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(instance),
            None,
        )?;

        let mode = if std::env::args().any(|a| a == "--bench") {
            app::Run::Bench
        } else if std::env::args().any(|a| a == "--demo") {
            app::Run::Demo
        } else {
            app::Run::Normal
        };
        let mut app = app::App::new(hwnd, cfg, mode)?;
        if let Some(memory) = system::memory_use() {
            log!("up: {:.0} MB in RAM, {:.0} MB private", memory.in_ram, memory.private);
        }

        let clock = FrameClock::start();
        if clock.is_none() {
            log!("no frame clock; waiting for the compositor on the main thread");
        }
        let mut last = Instant::now();
        let mut idle = true;
        let mut stats = Stats::default();
        loop {
            let frame_start = Instant::now();
            if !idle {
                // One pass per display refresh: sleep until the desktop compositor has finished a
                // frame, then read the cursor and send the next set of changes. Reading input
                // right after the wait keeps the icons as close behind the cursor as Windows allows.
                match &clock {
                    Some(clock) => clock.wait(),
                    None => {
                        let _ = DwmFlush();
                    }
                }
            }
            let waited = frame_start.elapsed();

            if !pump() {
                break;
            }
            for ev in app::take_events() {
                app.handle(ev);
            }
            if app.quit {
                break;
            }

            // Time since the previous pass. After a sleep, or a hitch, pretend one frame passed
            // so nothing teleports.
            let now = Instant::now();
            let dt = if idle { 1.0 / 60.0 } else { (now - last).as_secs_f32().min(1.0 / 20.0) };
            let interval = (now - last).as_secs_f32();
            last = now;

            let animating = app.update(dt) || app.scripted();
            let updated = now.elapsed();
            if frame_start.elapsed().as_millis() >= 80 {
                // Windows gives a window a tenth of a second to say where it is minimised to;
                // a pass this long may have missed the question.
                log!("a pass of the loop took {} ms (waiting for a frame {} ms, the app's own work {} ms)", frame_start.elapsed().as_millis(), waited.as_millis(), updated.as_millis());
            }
            // Pumping messages is what hands the batch of changes to the compositor.
            if !pump() {
                break;
            }

            if let Some(phase) = app.bench_phase_finished() {
                log!("bench: {phase}");
                stats.flush();
            }
            if app.stats_enabled() {
                stats.add(!idle, interval, waited.as_secs_f32(), updated.as_secs_f32(), app.item_count());
            }

            let was_idle = idle;
            idle = !animating;
            if !idle && !was_idle && frame_start.elapsed().as_millis() < 2 {
                // The compositor did not pace us (screen off, remote session...). Do not spin.
                std::thread::sleep(std::time::Duration::from_millis(4));
            }
            if idle {
                stats.flush();
                // Nothing is moving: sleep until the mouse, a timer or the shell wakes us.
                let _ = WaitMessage();
            }
        }

        app.shutdown();
        let _ = DestroyWindow(hwnd);
        log!("stopped");
        Ok(())
    }
}

/// Paces the main loop to the desktop compositor's frames.
///
/// Waiting for the compositor (`DwmFlush`) on the main thread makes the thread deaf while it
/// waits. That matters at one moment: when a window is minimised, Windows asks the app's
/// window where the window's icon is, and the compositor does not finish its frame until that
/// has been settled. Asked while it waits for that very frame, the app would sit out the tenth
/// of a second Windows allows, the window's animation would start late and go to the wrong
/// place, and everything of the app's own would stand still meanwhile. So another thread
/// waits for the compositor, and the main thread waits for that thread in a way that still
/// lets it answer.
struct FrameClock {
    ask: HANDLE,
    done: HANDLE,
}

impl FrameClock {
    fn start() -> Option<Self> {
        unsafe {
            // Both reset themselves when a wait takes them.
            let ask = CreateEventW(None, false, false, None).ok()?;
            let done = CreateEventW(None, false, false, None).ok()?;
            let (asked, finished) = (ask.0 as isize, done.0 as isize);
            let run = move || loop {
                WaitForSingleObject(HANDLE(asked as *mut _), INFINITE);
                let _ = DwmFlush();
                let _ = SetEvent(HANDLE(finished as *mut _));
            };
            std::thread::Builder::new().name("frames".into()).stack_size(64 * 1024).spawn(run).ok()?;
            Some(FrameClock { ask, done })
        }
    }

    /// Sleeps until the compositor has finished a frame (a tenth of a second at most).
    /// Questions sent to the app's windows are answered while it sleeps.
    fn wait(&self) {
        unsafe {
            // A frame reported after an earlier wait gave up does not count for this one.
            let _ = ResetEvent(self.done);
            let _ = SetEvent(self.ask);
            let started = Instant::now();
            loop {
                let left = 100u32.saturating_sub(started.elapsed().as_millis() as u32);
                if left == 0 {
                    break;
                }
                let woke = MsgWaitForMultipleObjectsEx(Some(&[self.done]), left, QS_SENDMESSAGE, MSG_WAIT_FOR_MULTIPLE_OBJECTS_EX_FLAGS(0));
                if woke.0 != WAIT_OBJECT_0.0 + 1 {
                    // The frame is done (or the wait ran out).
                    break;
                }
                // Something was sent to one of the windows: looking at the queue is what lets
                // it through. Posted messages stay where they are, for the loop to take.
                let mut msg = MSG::default();
                let _ = PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE | PM_QS_SENDMESSAGE);
            }
        }
    }
}

/// Frame timing, written to ben.log when `stats = true` in ben.ini.
#[derive(Default)]
struct Stats {
    frames: u32,
    /// Sum and worst of the time between consecutive passes, in ms.
    interval: f32,
    interval_max: f32,
    /// Passes that took more than 1.5x the typical interval (a visible hitch).
    slow: u32,
    wait: f32,
    update: f32,
    update_max: f32,
    items: usize,
}

impl Stats {
    fn add(&mut self, continuous: bool, interval: f32, wait: f32, update: f32, items: usize) {
        if !continuous {
            // First pass after a sleep: its interval is the sleep, not a frame time.
            return;
        }
        let ms = |s: f32| s * 1000.0;
        self.frames += 1;
        self.interval += ms(interval);
        self.interval_max = self.interval_max.max(ms(interval));
        if self.frames > 10 && ms(interval) > 1.5 * self.interval / self.frames as f32 {
            self.slow += 1;
        }
        self.wait += ms(wait);
        self.update += ms(update);
        self.update_max = self.update_max.max(ms(update));
        self.items = items;
        if self.interval >= 2000.0 {
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.frames >= 20 {
            let n = self.frames as f32;
            log!(
                "stats: {} frames, {:.1} fps | frame avg {:.1} ms, worst {:.1}, slow {} | waiting for the compositor {:.1} ms | the app's own work {:.2} ms (max {:.1}) | {} items",
                self.frames,
                1000.0 * n / self.interval,
                self.interval / n,
                self.interval_max,
                self.slow,
                self.wait / n,
                self.update / n,
                self.update_max,
                self.items
            );
        }
        *self = Stats::default();
    }
}

/// Dispatches every pending window message. Returns false once the app should exit.
fn pump() -> bool {
    unsafe {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                return false;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        true
    }
}
