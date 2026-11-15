//! Windows settings the dock reads or changes: theme, accent colour,
//! start-with-Windows, and the taskbar.

use crate::util::{data_dir, wide};
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use std::time::Instant;
use windows::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
use windows::Win32::Networking::NetworkListManager::{INetworkListManager, NetworkListManager};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Power::{GetSystemPowerStatus, SetSuspendState, SYSTEM_POWER_STATUS};
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY, VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT};
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn read_dword(subkey: &str, value: &str) -> Option<u32> {
    let (subkey, value) = (wide(subkey), wide(value));
    let mut data = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), PCWSTR(value.as_ptr()), RRF_RT_REG_DWORD, None, Some(&mut data as *mut u32 as *mut _), Some(&mut size))
    };
    (r == ERROR_SUCCESS).then_some(data)
}

/// True when Windows is set to its dark theme.
pub fn system_is_dark() -> bool {
    read_dword("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize", "SystemUsesLightTheme").map(|v| v == 0).unwrap_or(true)
}

/// The user's accent colour as (r, g, b) in 0..1.
pub fn accent_color() -> (f32, f32, f32) {
    // Stored as 0xAABBGGRR.
    if let Some(c) = read_dword("Software\\Microsoft\\Windows\\DWM", "AccentColor") {
        return ((c & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, ((c >> 16) & 0xff) as f32 / 255.0);
    }
    let mut argb = 0u32;
    let mut opaque = windows::core::BOOL(0);
    if unsafe { DwmGetColorizationColor(&mut argb, &mut opaque) }.is_ok() {
        return (((argb >> 16) & 0xff) as f32 / 255.0, ((argb >> 8) & 0xff) as f32 / 255.0, (argb & 0xff) as f32 / 255.0);
    }
    (0.0, 0.47, 0.84)
}

// ---------------------------------------------------------------- autostart

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_VALUE: &str = "Ben";

pub fn autostart_enabled() -> bool {
    let (subkey, value) = (wide(RUN_KEY), wide(RUN_VALUE));
    let mut size = 0u32;
    let r = unsafe { RegGetValueW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), PCWSTR(value.as_ptr()), RRF_RT_REG_SZ, None, None, Some(&mut size)) };
    r == ERROR_SUCCESS && size > 2
}

/// Adds or removes Bến from the programs Windows starts at sign-in (current user only).
pub fn set_autostart(enabled: bool) -> bool {
    let (subkey, value) = (wide(RUN_KEY), wide(RUN_VALUE));
    let r = if enabled {
        let Ok(exe) = std::env::current_exe() else { return false };
        let command = wide(&format!("\"{}\"", exe.display()));
        unsafe {
            RegSetKeyValueW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), PCWSTR(value.as_ptr()), REG_SZ.0, Some(command.as_ptr() as *const _), (command.len() * 2) as u32)
        }
    } else {
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), PCWSTR(value.as_ptr())) }
    };
    let ok = r == ERROR_SUCCESS || (!enabled && r == ERROR_FILE_NOT_FOUND);
    if !ok {
        crate::log!("autostart change failed: {:?}", r);
    }
    ok
}

// ------------------------------------------------------------------ taskbar

fn state_file() -> std::path::PathBuf {
    data_dir().join("taskbar.state")
}

fn taskbar_windows() -> Vec<HWND> {
    let mut out = Vec::new();
    unsafe {
        if let Ok(h) = FindWindowW(PCWSTR(wide("Shell_TrayWnd").as_ptr()), PCWSTR::null()) {
            out.push(h);
        }
        // One extra taskbar per additional monitor.
        let class = wide("Shell_SecondaryTrayWnd");
        let mut after: Option<HWND> = None;
        while let Ok(h) = FindWindowExW(None, after, PCWSTR(class.as_ptr()), PCWSTR::null()) {
            if h.is_invalid() {
                break;
            }
            out.push(h);
            after = Some(h);
        }
    }
    out
}

fn taskbar_state(tray: HWND) -> u32 {
    let mut data = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: tray, ..Default::default() };
    unsafe { SHAppBarMessage(ABM_GETSTATE, &mut data) as u32 }
}

fn set_taskbar_state(tray: HWND, state: u32) {
    let mut data = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: tray, lParam: LPARAM(state as isize), ..Default::default() };
    unsafe {
        SHAppBarMessage(ABM_SETSTATE, &mut data);
    }
}

/// Hides the Windows taskbar while the dock is running.
///
/// Two things are needed: switching the taskbar to auto-hide (so it stops
/// reserving a strip of the screen) and hiding its window (so it does not pop
/// up when the cursor reaches the bottom edge, where the dock is). The
/// original auto-hide setting is written to `taskbar.state` first, so it can be
/// put back even if the dock is killed.
pub struct Taskbar {
    pub hidden: bool,
}

impl Taskbar {
    pub fn new() -> Self {
        Self { hidden: false }
    }

    pub fn hide(&mut self) {
        let Some(&tray) = taskbar_windows().first() else {
            crate::log!("taskbar window not found; leaving it alone");
            return;
        };
        if !state_file().exists() {
            let original = taskbar_state(tray);
            if let Err(e) = std::fs::write(state_file(), original.to_string()) {
                crate::log!("could not record the taskbar setting ({e}); leaving the taskbar alone");
                return;
            }
        }
        set_taskbar_state(tray, ABS_AUTOHIDE);
        self.hidden = true;
        self.keep_hidden();
    }

    /// Explorer sometimes shows the taskbar again by itself; call this now and then.
    pub fn keep_hidden(&self) {
        if !self.hidden {
            return;
        }
        for h in taskbar_windows() {
            unsafe {
                if IsWindowVisible(h).as_bool() {
                    let _ = ShowWindow(h, SW_HIDE);
                }
            }
        }
    }

    pub fn restore(&mut self) {
        self.hidden = false;
        restore_taskbar();
    }
}

/// Puts the taskbar back the way it was. Safe to call at any time, from any
/// process (this is what `ben.exe --restore-taskbar` runs).
pub fn restore_taskbar() {
    restore_window_animations();
    let windows = taskbar_windows();
    if let Some(&tray) = windows.first() {
        if let Ok(text) = std::fs::read_to_string(state_file()) {
            let original = text.trim().parse::<u32>().unwrap_or(0);
            set_taskbar_state(tray, original);
        }
    }
    for h in windows {
        unsafe {
            let _ = ShowWindow(h, SW_SHOWNA);
        }
    }
    let _ = std::fs::remove_file(state_file());
}

// ------------------------------------------------------------------- appbar

/// Reserves a strip at the bottom of the screen, the same way the taskbar
/// does, so maximised windows stop above the dock.
pub struct AppBar {
    hwnd: HWND,
    registered: bool,
}

impl AppBar {
    pub fn new(hwnd: HWND) -> Self {
        Self { hwnd, registered: false }
    }

    fn data(&self) -> APPBARDATA {
        APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: self.hwnd, ..Default::default() }
    }

    pub fn register(&mut self, callback_message: u32) {
        if self.registered {
            return;
        }
        let mut data = self.data();
        data.uCallbackMessage = callback_message;
        self.registered = unsafe { SHAppBarMessage(ABM_NEW, &mut data) } != 0;
        if !self.registered {
            crate::log!("could not reserve screen space for the dock (ABM_NEW failed)");
        }
    }

    /// Asks for `height` pixels along the bottom of `monitor`. Returns the strip that was granted.
    pub fn reserve(&self, monitor: RECT, height: i32) -> Option<RECT> {
        self.reserve_edge(monitor, height, false)
    }

    /// Asks for `height` pixels along the top of `monitor` (for the top bar).
    pub fn reserve_top(&self, monitor: RECT, height: i32) -> Option<RECT> {
        self.reserve_edge(monitor, height, true)
    }

    fn reserve_edge(&self, monitor: RECT, height: i32, top: bool) -> Option<RECT> {
        if !self.registered {
            return None;
        }
        let mut data = self.data();
        data.uEdge = if top { ABE_TOP } else { ABE_BOTTOM };
        data.rc = if top {
            RECT { left: monitor.left, right: monitor.right, top: monitor.top, bottom: monitor.top + height }
        } else {
            RECT { left: monitor.left, right: monitor.right, bottom: monitor.bottom, top: monitor.bottom - height }
        };
        unsafe {
            // The shell moves the rectangle clear of other bars (for example a visible taskbar).
            SHAppBarMessage(ABM_QUERYPOS, &mut data);
            if top {
                data.rc.bottom = data.rc.top + height;
            } else {
                data.rc.top = data.rc.bottom - height;
            }
            SHAppBarMessage(ABM_SETPOS, &mut data);
        }
        Some(data.rc)
    }

    pub fn remove(&mut self) {
        if self.registered {
            let mut data = self.data();
            unsafe {
                SHAppBarMessage(ABM_REMOVE, &mut data);
            }
            self.registered = false;
        }
    }
}

// -------------------------------------------------------- window animations

fn animation_file() -> std::path::PathBuf {
    data_dir().join("animation.state")
}

fn set_minimize_animation(on: bool) {
    let mut info = ANIMATIONINFO { cbSize: std::mem::size_of::<ANIMATIONINFO>() as u32, iMinAnimate: on as i32 };
    // SPIF_SENDCHANGE without "update ini file": the change lasts for this sign-in session only,
    // so even in the worst case a sign-out brings the user's own setting back.
    let r = unsafe { SystemParametersInfoW(SPI_SETANIMATION, info.cbSize, Some(&mut info as *mut _ as *mut _), SPIF_SENDCHANGE) };
    if let Err(e) = r {
        crate::log!("could not change the minimise animation setting: {e}");
    }
}

/// Turns off Windows' own minimise / restore animation, so that it does not play on top of
/// the dock's "window flies into its icon" effect. Remembered in `animation.state` and undone
/// by `restore_window_animations`.
pub fn disable_window_animations() {
    let mut info = ANIMATIONINFO { cbSize: std::mem::size_of::<ANIMATIONINFO>() as u32, iMinAnimate: 0 };
    let read = unsafe { SystemParametersInfoW(SPI_GETANIMATION, info.cbSize, Some(&mut info as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    if read.is_err() || info.iMinAnimate == 0 {
        // Already off (by the user, or by an earlier run that is recorded in the file).
        return;
    }
    if std::fs::write(animation_file(), "1").is_ok() {
        set_minimize_animation(false);
    }
}

pub fn restore_window_animations() {
    if animation_file().exists() {
        set_minimize_animation(true);
        let _ = std::fs::remove_file(animation_file());
    }
}

// ------------------------------------------------------------ status readers

/// What the top bar shows on its right-hand side.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// Battery percentage and whether it is charging. `None` on a machine without a battery.
    pub battery: Option<(u8, bool)>,
    /// Volume from 0 to 1 and whether sound is muted. `None` if there is no sound device.
    pub volume: Option<(f32, bool)>,
    pub online: bool,
}

/// Reads battery, volume and network state. The sound device and the network manager are
/// looked up once and kept; if a call on them fails they are looked up again next time.
pub struct StatusReader {
    volume: Option<IAudioEndpointVolume>,
    network: Option<INetworkListManager>,
    online: bool,
    network_read: Option<Instant>,
}

impl StatusReader {
    pub fn new() -> Self {
        Self { volume: None, network: None, online: true, network_read: None }
    }

    pub fn read(&mut self) -> Status {
        Status { battery: battery(), volume: self.read_volume(), online: self.read_online() }
    }

    fn read_volume(&mut self) -> Option<(f32, bool)> {
        unsafe {
            if self.volume.is_none() {
                let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
                let speaker = devices.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
                self.volume = speaker.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok();
            }
            let endpoint = self.volume.as_ref()?;
            match (endpoint.GetMasterVolumeLevelScalar(), endpoint.GetMute()) {
                (Ok(level), Ok(muted)) => Some((level, muted.as_bool())),
                _ => {
                    // The default device changed or went away: look it up again next time.
                    self.volume = None;
                    None
                }
            }
        }
    }

    /// Sets the volume of the default speaker, 0 to 1.
    pub fn set_volume(&mut self, level: f32) {
        // Makes sure the device has been looked up.
        let _ = self.read_volume();
        if let Some(endpoint) = self.volume.as_ref() {
            if let Err(e) = unsafe { endpoint.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0), std::ptr::null()) } {
                crate::log!("could not set the volume: {e}");
            }
        }
    }

    /// Turns the sound of the default speaker off or back on.
    pub fn set_muted(&mut self, muted: bool) {
        let _ = self.read_volume();
        if let Some(endpoint) = self.volume.as_ref() {
            if let Err(e) = unsafe { endpoint.SetMute(muted, std::ptr::null()) } {
                crate::log!("could not change mute: {e}");
            }
        }
    }

    fn read_online(&mut self) -> bool {
        // Asking is not free, and the answer rarely changes: every five seconds is plenty.
        if self.network_read.map(|t| t.elapsed().as_secs_f32() < 5.0).unwrap_or(false) {
            return self.online;
        }
        self.network_read = Some(Instant::now());
        unsafe {
            if self.network.is_none() {
                self.network = CoCreateInstance(&NetworkListManager, None, CLSCTX_ALL).ok();
            }
            match self.network.as_ref().map(|n| n.IsConnectedToInternet()) {
                Some(Ok(connected)) => self.online = connected.as_bool(),
                Some(Err(_)) => self.network = None,
                None => {}
            }
        }
        self.online
    }
}

fn battery() -> Option<(u8, bool)> {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
    // Flag 128 = no battery in this machine; 255 = level unknown.
    if status.BatteryFlag & 128 != 0 || status.BatteryFlag == 255 || status.BatteryLifePercent > 100 {
        return None;
    }
    Some((status.BatteryLifePercent, status.ACLineStatus == 1))
}

/// More about the battery, for its menu.
#[derive(Clone, Copy, Debug)]
pub struct Battery {
    pub percent: u8,
    /// Running on the charger.
    pub plugged: bool,
    pub charging: bool,
    /// Minutes of battery left, when Windows has an estimate.
    pub minutes_left: Option<u32>,
}

pub fn battery_detail() -> Option<Battery> {
    let mut status = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
    if status.BatteryFlag & 128 != 0 || status.BatteryFlag == 255 || status.BatteryLifePercent > 100 {
        return None;
    }
    Some(Battery {
        percent: status.BatteryLifePercent,
        plugged: status.ACLineStatus == 1,
        charging: status.BatteryFlag & 8 != 0,
        minutes_left: (status.BatteryLifeTime != u32::MAX).then_some(status.BatteryLifeTime / 60),
    })
}

/// Today's date: year, month (1..=12), day.
pub fn today() -> (i32, u32, u32) {
    let now = unsafe { GetLocalTime() };
    (now.wYear as i32, now.wMonth as u32, now.wDay as u32)
}

/// Day and time for the top bar, for example "CN, 15 thg 11   13:49".
pub fn clock_text() -> String {
    let now = unsafe { GetLocalTime() };
    format_clock(now.wDayOfWeek, now.wDay, now.wMonth, now.wHour, now.wMinute)
}

/// `weekday`: 0 = Sunday.
pub fn format_clock(weekday: u16, day: u16, month: u16, hour: u16, minute: u16) -> String {
    const DAYS: [&str; 7] = ["CN", "Th 2", "Th 3", "Th 4", "Th 5", "Th 6", "Th 7"];
    format!("{}, {} thg {}   {:02}:{:02}", DAYS[(weekday % 7) as usize], day, month, hour, minute)
}

/// Presses Windows + a letter, as if typed. Used to open Windows' own panels:
/// Win+A = quick settings (Wi-Fi, sound, battery), Win+N = notifications and calendar.
pub fn press_windows_key(letter: char) {
    let key = VIRTUAL_KEY(letter.to_ascii_uppercase() as u16);
    let stroke = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } },
    };
    let inputs = [stroke(VK_LWIN, false), stroke(key, false), stroke(key, true), stroke(VK_LWIN, true)];
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Modifier keys for `press_keys`.
pub const CTRL: u8 = 1;
pub const SHIFT: u8 = 2;
pub const ALT: u8 = 4;
pub const WIN: u8 = 8;

/// Presses a keyboard shortcut, as if typed: the modifiers go down, the key is tapped, the
/// modifiers come up. It goes to whichever window is in front.
pub fn press_keys(mods: u8, key: u16) {
    let stroke = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) }, time: 0, dwExtraInfo: 0 } },
    };
    let held: Vec<VIRTUAL_KEY> = [(CTRL, VK_CONTROL), (SHIFT, VK_SHIFT), (ALT, VK_MENU), (WIN, VK_LWIN)].iter().filter(|(bit, _)| mods & bit != 0).map(|(_, vk)| *vk).collect();
    let mut inputs: Vec<INPUT> = held.iter().map(|vk| stroke(*vk, false)).collect();
    inputs.push(stroke(VIRTUAL_KEY(key), false));
    inputs.push(stroke(VIRTUAL_KEY(key), true));
    inputs.extend(held.iter().rev().map(|vk| stroke(*vk, true)));
    unsafe {
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Locks the screen (the same as Win+L).
pub fn lock_screen() {
    if let Err(e) = unsafe { LockWorkStation() } {
        crate::log!("could not lock the screen: {e}");
    }
}

/// Puts the computer to sleep.
pub fn sleep() {
    if !unsafe { SetSuspendState(false, false, false) } {
        crate::log!("Windows refused to go to sleep");
    }
}

/// Asks a yes / no question in a small box. True for yes.
pub fn confirm(question: &str) -> bool {
    let answer = unsafe { MessageBoxW(None, PCWSTR(wide(question).as_ptr()), PCWSTR(wide("Bến").as_ptr()), MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_TOPMOST | MB_SETFOREGROUND) };
    answer == IDYES
}

/// Runs a program with arguments, without showing a window for it.
pub fn run(program: &str, arguments: &str) {
    let result = unsafe { ShellExecuteW(None, PCWSTR(wide("open").as_ptr()), PCWSTR(wide(program).as_ptr()), PCWSTR(wide(arguments).as_ptr()), PCWSTR::null(), SW_HIDE) };
    if result.0 as isize <= 32 {
        crate::log!("could not run '{program} {arguments}' (ShellExecute returned {})", result.0 as isize);
    }
}
