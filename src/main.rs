//! Bến — a dock for Windows 11.
//!
//! Runs in the background from sign-in, replaces the taskbar with a row of app
//! icons that magnify under the cursor, and puts the taskbar back when it exits.
//!
//!   ben.exe                    start the dock
//!   ben.exe --quit             stop the running dock
//!   ben.exe --restore-taskbar  bring the Windows taskbar back (after a crash)
//!   ben.exe --autostart on|off start with Windows, or stop doing so
//!   ben.exe --bench            start, then run a 9 s self-test and write frame timing to ben.log

#![windows_subsystem = "windows"]
#![allow(clippy::too_many_arguments)]

mod bar;
mod calendar;
mod config;
mod dates;
mod dock;
mod fx;
mod gfx;
mod glass;
mod launcher;
mod layout;
mod menu;
mod menus;
mod preview;
mod settings;
mod shell;
mod spring;
mod system;
mod text;
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
use windows::Win32::System::Threading::CreateMutexW;
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
        if let Ok(hwnd) = FindWindowW(PCWSTR(wide(dock::CLASS_NAME).as_ptr()), PCWSTR::null()) {
            let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

fn run() -> Result<()> {
    unsafe {
        // Only one dock per session.
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
        let class_name = wide(dock::CLASS_NAME);
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(dock::wndproc),
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

        let bench = std::env::args().any(|a| a == "--bench");
        let mut dock = dock::Dock::new(hwnd, cfg, bench)?;
        log!("dock is up");

        let mut last = Instant::now();
        let mut idle = true;
        let mut stats = Stats::default();
        loop {
            let frame_start = Instant::now();
            if !idle {
                // One pass per display refresh: sleep until the desktop compositor has finished a
                // frame, then read the cursor and send the next set of changes. Reading input
                // right after the wait keeps the icons as close behind the cursor as Windows allows.
                let _ = DwmFlush();
            }
            let waited = frame_start.elapsed();

            if !pump() {
                break;
            }
            for ev in dock::take_events() {
                dock.handle(ev);
            }
            if dock.quit {
                break;
            }

            // Time since the previous pass. After a sleep, or a hitch, pretend one frame passed
            // so nothing teleports.
            let now = Instant::now();
            let dt = if idle { 1.0 / 60.0 } else { (now - last).as_secs_f32().min(1.0 / 20.0) };
            let interval = (now - last).as_secs_f32();
            last = now;

            let animating = dock.update(dt);
            let updated = now.elapsed();
            // Pumping messages is what hands the batch of changes to the compositor.
            if !pump() {
                break;
            }

            if let Some(phase) = dock.bench_phase_finished() {
                log!("bench: {phase}");
                stats.flush();
            }
            if dock.stats_enabled() {
                stats.add(!idle, interval, waited.as_secs_f32(), updated.as_secs_f32(), dock.item_count());
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

        dock.shutdown();
        let _ = DestroyWindow(hwnd);
        log!("stopped");
        Ok(())
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
                "stats: {} frames, {:.1} fps | frame avg {:.1} ms, worst {:.1}, slow {} | waiting for the compositor {:.1} ms | dock's own work {:.2} ms (max {:.1}) | {} items",
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
