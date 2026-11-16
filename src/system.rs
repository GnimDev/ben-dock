//! Windows settings the dock reads or changes: theme, accent colour,
//! start-with-Windows, and the taskbar.

use crate::util::{data_dir, from_wide, wide};
use std::time::Instant;
use windows::core::{implement, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::DwmGetColorizationColor;
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, HDC, HMONITOR, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl};
use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator, AUDIO_VOLUME_NOTIFICATION_DATA};
use windows::Win32::Networking::NetworkListManager::{INetworkListManager, NetworkListManager};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
use windows::Win32::System::Memory::{SetProcessWorkingSetSizeEx, QUOTA_LIMITS_HARDWS_MAX_ENABLE, QUOTA_LIMITS_HARDWS_MIN_DISABLE, SETPROCESSWORKINGSETSIZEEX_FLAGS};
use windows::Win32::System::Power::{GetSystemPowerStatus, SetSuspendState, SYSTEM_POWER_STATUS};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessInformation, ProcessMemoryPriority, MEMORY_PRIORITY_INFORMATION, MEMORY_PRIORITY_LOW};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

// ------------------------------------------------------------------ display

/// A screen.
#[derive(Clone, Copy, Debug)]
pub struct Monitor {
    /// Windows' handle for it, as a number.
    pub handle: isize,
    pub rect: RECT,
    pub work: RECT,
    pub dpi: u32,
    pub primary: bool,
}

impl Monitor {
    /// Physical pixels per logical pixel.
    pub fn scale(&self) -> f32 {
        self.dpi as f32 / 96.0
    }

    pub fn width(&self) -> i32 {
        self.rect.right - self.rect.left
    }
}

unsafe fn describe(handle: HMONITOR) -> Monitor {
    unsafe {
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(handle, &mut info);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(handle, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        // Flag 1 = this is the primary monitor.
        Monitor { handle: handle.0 as isize, rect: info.rcMonitor, work: info.rcWork, dpi: dx.max(48), primary: info.dwFlags & 1 != 0 }
    }
}

pub fn primary_monitor() -> Monitor {
    unsafe { describe(MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY)) }
}

/// Every screen: the primary one first, then the others from left to right.
pub fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn each(handle: HMONITOR, _dc: HDC, _rect: *mut RECT, data: LPARAM) -> windows::core::BOOL {
        let list = unsafe { &mut *(data.0 as *mut Vec<Monitor>) };
        list.push(unsafe { describe(handle) });
        windows::core::BOOL(1)
    }
    let mut list: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    if list.is_empty() {
        list.push(primary_monitor());
    }
    list.sort_by_key(|m| (!m.primary, m.rect.left, m.rect.top));
    list
}

/// True when "Animation effects" is switched off in Windows' accessibility settings: then
/// nothing here bounces or stretches either.
pub fn reduce_motion() -> bool {
    let mut on = windows::core::BOOL(1);
    let read = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, Some(&mut on as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    read.is_ok() && !on.as_bool()
}

// ------------------------------------------------------------------- memory

/// How much memory the app is using, in megabytes.
#[derive(Clone, Copy, Debug, Default)]
pub struct MemoryUse {
    /// What is in RAM right now (what Task Manager's "Memory" is about).
    pub in_ram: f32,
    /// What the app has asked for and nothing else can use, in RAM or paged out.
    pub private: f32,
}

pub fn memory_use() -> Option<MemoryUse> {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    unsafe { GetProcessMemoryInfo(GetCurrentProcess(), &mut counters as *mut _ as *mut PROCESS_MEMORY_COUNTERS, counters.cb) }.ok()?;
    const MB: f32 = 1024.0 * 1024.0;
    Some(MemoryUse { in_ram: counters.WorkingSetSize as f32 / MB, private: counters.PrivateUsage as f32 / MB })
}

/// Hands back to Windows every page the app is not using this instant. Pages it needs again
/// come back on their own, at the price of a moment's delay the first time, so this is done
/// once the app has been at rest for a while, not while it is busy.
pub fn release_memory() {
    if let Err(e) = unsafe { SetProcessWorkingSetSizeEx(GetCurrentProcess(), usize::MAX, usize::MAX, SETPROCESSWORKINGSETSIZEEX_FLAGS(0)) } {
        crate::log!("could not release memory: {e}");
    }
}

/// Tells Windows never to let the app keep more than `megabytes` in RAM, whatever it does:
/// past that, Windows pages the oldest parts out instead. A safety net, not a diet. Also
/// marks the app's memory as the first to give up when the machine runs short.
pub fn limit_memory(megabytes: u32) {
    unsafe {
        let process = GetCurrentProcess();
        if megabytes > 0 {
            let most = (megabytes as usize).max(48) * 1024 * 1024;
            if let Err(e) = SetProcessWorkingSetSizeEx(process, 4 * 1024 * 1024, most, QUOTA_LIMITS_HARDWS_MAX_ENABLE | QUOTA_LIMITS_HARDWS_MIN_DISABLE) {
                crate::log!("could not set the {megabytes} MB memory limit: {e}");
            }
        }
        let priority = MEMORY_PRIORITY_INFORMATION { MemoryPriority: MEMORY_PRIORITY_LOW };
        let _ = SetProcessInformation(process, ProcessMemoryPriority, &priority as *const _ as *const _, std::mem::size_of::<MEMORY_PRIORITY_INFORMATION>() as u32);
    }
}

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
        // (Not the window that stands in for it while Bến runs, see `background.rs`.)
        if let Some(h) = crate::background::taskbar() {
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

/// Asks the shell about a bar: the taskbar itself, or a strip of the screen kept free for
/// the dock or the island. (Past the taskbar's stand-in, which would take the question
/// otherwise: see `background::past`.)
fn ask_shell(message: u32, data: &mut APPBARDATA) -> usize {
    crate::background::past(|| unsafe { SHAppBarMessage(message, data) })
}

fn taskbar_state(tray: HWND) -> u32 {
    let mut data = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: tray, ..Default::default() };
    ask_shell(ABM_GETSTATE, &mut data) as u32
}

fn set_taskbar_state(tray: HWND, state: u32) {
    let mut data = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: tray, lParam: LPARAM(state as isize), ..Default::default() };
    ask_shell(ABM_SETSTATE, &mut data);
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
        self.registered = ask_shell(ABM_NEW, &mut data) != 0;
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
        // The shell moves the rectangle clear of other bars (for example a visible taskbar).
        ask_shell(ABM_QUERYPOS, &mut data);
        if top {
            data.rc.bottom = data.rc.top + height;
        } else {
            data.rc.top = data.rc.bottom - height;
        }
        ask_shell(ABM_SETPOS, &mut data);
        Some(data.rc)
    }

    pub fn remove(&mut self) {
        if self.registered {
            let mut data = self.data();
            ask_shell(ABM_REMOVE, &mut data);
            self.registered = false;
        }
    }
}

impl Drop for AppBar {
    fn drop(&mut self) {
        self.remove();
    }
}

// -------------------------------------------------------- window animations

fn animation_file() -> std::path::PathBuf {
    data_dir().join("animation.state")
}

fn set_minimize_animation(on: bool) {
    let mut info = ANIMATIONINFO { cbSize: std::mem::size_of::<ANIMATIONINFO>() as u32, iMinAnimate: on as i32 };
    // SPIF_SENDCHANGE without "update ini file": the change is for this sign-in session only; the
    // user's own saved setting is never written.
    let r = unsafe { SystemParametersInfoW(SPI_SETANIMATION, info.cbSize, Some(&mut info as *mut _ as *mut _), SPIF_SENDCHANGE) };
    if let Err(e) = r {
        crate::log!("could not change the minimise animation setting: {e}");
    }
}

/// Versions before 0.2 switched Windows' own minimise animation off while they ran, and noted
/// it in `animation.state`. This puts it back if such a run was stopped without doing so.
pub fn restore_window_animations() {
    if animation_file().exists() {
        set_minimize_animation(true);
        let _ = std::fs::remove_file(animation_file());
    }
}

/// True if Windows animates windows as they are minimised and restored (a setting of the user's).
pub fn window_animations_on() -> bool {
    let mut info = ANIMATIONINFO { cbSize: std::mem::size_of::<ANIMATIONINFO>() as u32, iMinAnimate: 0 };
    let read = unsafe { SystemParametersInfoW(SPI_GETANIMATION, info.cbSize, Some(&mut info as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    read.is_ok() && info.iMinAnimate != 0
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

/// Tells the app's window the moment the volume changes, so it does not have to keep asking.
#[implement(IAudioEndpointVolumeCallback)]
struct VolumeWatch {
    window: isize,
    message: u32,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeWatch_Impl {
    fn OnNotify(&self, _data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        // This runs on one of Windows' own threads: only pass the news on.
        unsafe {
            let _ = PostMessageW(Some(HWND(self.window as *mut _)), self.message, WPARAM(0), LPARAM(0));
        }
        Ok(())
    }
}

/// Reads battery, volume and network state. The sound device and the network manager are
/// looked up once and kept; if a call on them fails they are looked up again next time.
pub struct StatusReader {
    devices: Option<IMMDeviceEnumerator>,
    volume: Option<IAudioEndpointVolume>,
    /// Identity of the device `volume` belongs to, and when it was last checked to still be the default.
    device: String,
    device_checked: Option<Instant>,
    /// The window to tell about volume changes, and the message to send it.
    notify: Option<(isize, u32)>,
    watch: Option<IAudioEndpointVolumeCallback>,
    network: Option<INetworkListManager>,
    online: bool,
    network_read: Option<Instant>,
}

impl StatusReader {
    pub fn new() -> Self {
        Self { devices: None, volume: None, device: String::new(), device_checked: None, notify: None, watch: None, network: None, online: true, network_read: None }
    }

    /// From now on `window` gets `message` whenever the volume or mute changes.
    pub fn watch_volume(&mut self, window: HWND, message: u32) {
        self.notify = Some((window.0 as isize, message));
        self.drop_volume();
    }

    pub fn read(&mut self) -> Status {
        Status { battery: battery(), volume: self.read_volume(), online: self.read_online() }
    }

    fn drop_volume(&mut self) {
        if let (Some(endpoint), Some(watch)) = (self.volume.take(), self.watch.take()) {
            unsafe {
                let _ = endpoint.UnregisterControlChangeNotify(&watch);
            }
        }
        self.device_checked = None;
    }

    /// The identity of the speaker Windows currently plays through.
    unsafe fn default_speaker(&mut self) -> Option<(windows::Win32::Media::Audio::IMMDevice, String)> {
        unsafe {
            if self.devices.is_none() {
                self.devices = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok();
            }
            let speaker = self.devices.as_ref()?.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let id = speaker.GetId().ok()?;
            let name = from_wide(id.as_wide());
            CoTaskMemFree(Some(id.0 as *const _));
            Some((speaker, name))
        }
    }

    fn read_volume(&mut self) -> Option<(f32, bool)> {
        unsafe {
            // The user may switch to headphones or another output at any time: every few
            // seconds make sure the device being read is still the one in use.
            let stale = self.device_checked.map(|t| t.elapsed().as_secs_f32() > 4.0).unwrap_or(true);
            if self.volume.is_none() || stale {
                self.device_checked = Some(Instant::now());
                match self.default_speaker() {
                    Some((speaker, id)) => {
                        if self.volume.is_none() || id != self.device {
                            self.drop_volume();
                            self.device_checked = Some(Instant::now());
                            self.device = id;
                            self.volume = speaker.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok();
                            if let (Some(endpoint), Some((window, message))) = (self.volume.as_ref(), self.notify) {
                                let watch: IAudioEndpointVolumeCallback = VolumeWatch { window, message }.into();
                                if endpoint.RegisterControlChangeNotify(&watch).is_ok() {
                                    self.watch = Some(watch);
                                }
                            }
                        }
                    }
                    None => self.drop_volume(),
                }
            }
            let endpoint = self.volume.as_ref()?;
            match (endpoint.GetMasterVolumeLevelScalar(), endpoint.GetMute()) {
                (Ok(level), Ok(muted)) => Some((level, muted.as_bool())),
                _ => {
                    // The device went away: look it up again next time.
                    self.drop_volume();
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

/// More about the battery, for its page.
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

/// Seconds since midnight, and the day of the week (0 = Monday).
pub fn time_of_day() -> (u32, u32) {
    let now = unsafe { GetLocalTime() };
    (now.wHour as u32 * 3600 + now.wMinute as u32 * 60 + now.wSecond as u32, (now.wDayOfWeek as u32 + 6) % 7)
}

/// The time and date as the island writes them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Clock {
    /// "13:49"
    pub time: String,
    /// "CN, 15 thg 11"
    pub day: String,
    /// "Chủ nhật, 15 tháng 11"
    pub date: String,
}

pub fn clock() -> Clock {
    let now = unsafe { GetLocalTime() };
    format_clock(now.wDayOfWeek, now.wDay, now.wMonth, now.wHour, now.wMinute)
}

/// `weekday`: 0 = Sunday.
pub fn format_clock(weekday: u16, day: u16, month: u16, hour: u16, minute: u16) -> Clock {
    const SHORT: [&str; 7] = ["CN", "Th 2", "Th 3", "Th 4", "Th 5", "Th 6", "Th 7"];
    const LONG: [&str; 7] = ["Chủ nhật", "Thứ hai", "Thứ ba", "Thứ tư", "Thứ năm", "Thứ sáu", "Thứ bảy"];
    let w = (weekday % 7) as usize;
    Clock { time: format!("{hour:02}:{minute:02}"), day: format!("{}, {} thg {}", SHORT[w], day, month), date: format!("{}, {} tháng {}", LONG[w], day, month) }
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
