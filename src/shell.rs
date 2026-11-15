//! Talking to the Windows shell: which apps have windows, their icons and
//! names, and launching / switching to them.

use crate::util::{from_wide, wide};
use std::collections::HashMap;
use windows::core::{BOOL, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToString};
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VK_MENU};
use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const APPS_FOLDER: &str = "shell:AppsFolder\\";

/// PKEY_AppUserModel_ID: the identity Windows uses to group taskbar buttons.
const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY { fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3), pid: 5 };

/// One open window, attributed to an app.
#[derive(Clone, Debug)]
pub struct AppWindow {
    pub hwnd: HWND,
    /// Identity used to group windows into one dock icon.
    pub key: String,
    /// What to hand to the shell to start this app.
    pub target: String,
}

/// Identity for a launch target: case-insensitive.
pub fn key_for_target(target: &str) -> String {
    target.to_lowercase()
}

/// Remembers which app each window belongs to, so the periodic scan does not
/// reopen every process twice a second.
#[derive(Default)]
pub struct WindowTracker {
    cache: HashMap<isize, (String, String)>,
}

impl WindowTracker {
    /// Open app windows, front-most first.
    pub fn scan(&mut self, own: HWND) -> Vec<AppWindow> {
        let mut handles: Vec<HWND> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(collect_window), LPARAM(&mut handles as *mut _ as isize));
        }
        let mut out = Vec::new();
        let mut seen = Vec::with_capacity(handles.len());
        for hwnd in handles {
            if hwnd == own || !is_app_window(hwnd) {
                continue;
            }
            let id = hwnd.0 as isize;
            seen.push(id);
            let entry = match self.cache.get(&id) {
                Some(e) => Some(e.clone()),
                None => {
                    let found = identify(hwnd);
                    if let Some(e) = &found {
                        self.cache.insert(id, e.clone());
                    }
                    found
                }
            };
            if let Some((key, target)) = entry {
                out.push(AppWindow { hwnd, key, target });
            }
        }
        self.cache.retain(|id, _| seen.contains(id));
        out
    }
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
    list.push(hwnd);
    BOOL(1)
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

/// The same test the taskbar uses, roughly: visible, top-level, not a tool
/// window, not hidden by the system ("cloaked"), and it has a title.
fn is_app_window(hwnd: HWND) -> bool {
    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return false;
        }
        let ex = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
        let app_window = ex.contains(WS_EX_APPWINDOW);
        if ex.contains(WS_EX_TOOLWINDOW) && !app_window {
            return false;
        }
        if ex.contains(WS_EX_NOACTIVATE) && !app_window {
            return false;
        }
        let owned = GetWindow(hwnd, GW_OWNER).map(|o| !o.is_invalid()).unwrap_or(false);
        if owned && !app_window {
            return false;
        }
        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
        if cloaked != 0 {
            return false;
        }
        if GetWindowTextLengthW(hwnd) == 0 {
            return false;
        }
        !matches!(class_name(hwnd).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Windows.UI.Core.CoreWindow")
    }
}

fn identify(hwnd: HWND) -> Option<(String, String)> {
    // Store apps all live inside ApplicationFrameHost.exe; their real identity is on the window.
    if class_name(hwnd) == "ApplicationFrameWindow" {
        let id = window_app_id(hwnd)?;
        let target = format!("{APPS_FOLDER}{id}");
        return Some((key_for_target(&target), target));
    }
    let target = process_target(hwnd)?;
    Some((key_for_target(&target), target))
}

fn window_app_id(hwnd: HWND) -> Option<String> {
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(hwnd).ok()?;
        let mut value = store.GetValue(&PKEY_APP_USER_MODEL_ID).ok()?;
        let mut buf = [0u16; 512];
        let ok = PropVariantToString(&value, &mut buf).is_ok();
        let _ = PropVariantClear(&mut value);
        let id = from_wide(&buf);
        (ok && !id.is_empty()).then_some(id)
    }
}

/// What to launch for the process that owns `hwnd`: the Store identity for
/// packaged apps (Terminal, Notepad, Paint...), otherwise the exe path.
fn process_target(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 || pid == GetCurrentProcessId() {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;

        let mut id = [0u16; 512];
        let mut id_len = id.len() as u32;
        let packaged = GetApplicationUserModelId(process, &mut id_len, Some(PWSTR(id.as_mut_ptr()))) == ERROR_SUCCESS;

        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let have_path = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);

        if packaged {
            let id = from_wide(&id);
            if !id.is_empty() {
                return Some(format!("{APPS_FOLDER}{id}"));
            }
        }
        (have_path && len > 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Human-readable app name: the Store name, or the exe's "File description".
pub fn display_name(target: &str) -> String {
    if target.to_lowercase().starts_with(&APPS_FOLDER.to_lowercase()) {
        if let Some(name) = shell_display_name(target) {
            return name;
        }
    } else if let Some(name) = file_description(target) {
        return name;
    }
    std::path::Path::new(target).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| target.to_string())
}

fn shell_display_name(target: &str) -> Option<String> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide(target).as_ptr()), None).ok()?;
        let name = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let text = name.to_string().ok();
        CoTaskMemFree(Some(name.0 as *const _));
        text.filter(|t| !t.is_empty())
    }
}

fn file_description(path: &str) -> Option<String> {
    unsafe {
        let wpath = wide(path);
        let size = GetFileVersionInfoSizeW(PCWSTR(wpath.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wpath.as_ptr()), None, size, data.as_mut_ptr() as *mut _).ok()?;

        // The description is stored per language; take the first language the file declares.
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let query = wide("\\VarFileInfo\\Translation");
        if !VerQueryValueW(data.as_ptr() as *const _, PCWSTR(query.as_ptr()), &mut ptr, &mut len).as_bool() || len < 4 || ptr.is_null() {
            return None;
        }
        let lang = *(ptr as *const u16);
        let codepage = *(ptr as *const u16).add(1);

        let query = wide(&format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription"));
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if !VerQueryValueW(data.as_ptr() as *const _, PCWSTR(query.as_ptr()), &mut ptr, &mut len).as_bool() || len == 0 || ptr.is_null() {
            return None;
        }
        let text = from_wide(std::slice::from_raw_parts(ptr as *const u16, len as usize));
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    }
}

/// Premultiplied BGRA pixels, top row first.
pub struct IconPixels {
    pub size: u32,
    pub bgra: Vec<u8>,
}

/// Loads the largest icon the shell has for an app.
pub fn load_icon(target: &str, size: u32) -> Option<IconPixels> {
    unsafe {
        let item: IShellItemImageFactory = SHCreateItemFromParsingName(PCWSTR(wide(target).as_ptr()), None).ok()?;
        let hbm = item.GetImage(SIZE { cx: size as i32, cy: size as i32 }, SIIGBF_ICONONLY).ok()?;
        let pixels = bitmap_pixels(hbm);
        let _ = DeleteObject(hbm.into());
        let (w, h, mut bgra) = pixels?;
        fix_alpha(&mut bgra);
        Some(crop_to_content(w, h, bgra))
    }
}

unsafe fn bitmap_pixels(hbm: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(hbm.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth.max(0) as u32, bm.bmHeight.unsigned_abs());
        if w == 0 || h == 0 {
            return None;
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // negative = top row first
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = vec![0u8; (w * h * 4) as usize];
        let dc = GetDC(None);
        let lines = GetDIBits(dc, hbm, 0, h, Some(bits.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        (lines > 0).then_some((w, h, bits))
    }
}

/// Shell bitmaps are normally premultiplied, but some icons come back with no
/// alpha at all or with straight alpha. Normalise to premultiplied.
fn fix_alpha(bgra: &mut [u8]) {
    let any_alpha = bgra.chunks_exact(4).any(|p| p[3] != 0);
    if !any_alpha {
        for p in bgra.chunks_exact_mut(4) {
            p[3] = 255;
        }
        return;
    }
    let straight = bgra.chunks_exact(4).any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]);
    if straight {
        for p in bgra.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
}

/// Apps that only ship a small icon get it placed in a corner of a large
/// empty canvas. Crop to the drawn part so every icon fills its slot.
fn crop_to_content(w: u32, h: u32, bgra: Vec<u8>) -> IconPixels {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0u32, 0u32);
    for y in 0..h {
        for x in 0..w {
            if bgra[((y * w + x) * 4 + 3) as usize] > 8 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    let square = w.min(h);
    if x1 <= x0 || y1 <= y0 {
        return IconPixels { size: square, bgra: copy_square(&bgra, w, 0, 0, square) };
    }
    let side = (x1 - x0).max(y1 - y0);
    if side * 10 >= square * 6 {
        // The drawing uses most of the canvas: keep it as is.
        return IconPixels { size: square, bgra: copy_square(&bgra, w, 0, 0, square) };
    }
    let cx = (x0 + x1) / 2;
    let cy = (y0 + y1) / 2;
    let left = cx.saturating_sub(side / 2).min(w - side);
    let top = cy.saturating_sub(side / 2).min(h - side);
    IconPixels { size: side, bgra: copy_square(&bgra, w, left, top, side) }
}

fn copy_square(src: &[u8], src_w: u32, left: u32, top: u32, side: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((side * side * 4) as usize);
    for y in top..top + side {
        let start = ((y * src_w + left) * 4) as usize;
        out.extend_from_slice(&src[start..start + (side * 4) as usize]);
    }
    out
}

/// A plain rounded tile, for apps whose icon cannot be read.
pub fn placeholder_icon(size: u32) -> IconPixels {
    let n = size as f32;
    let radius = n * 0.22;
    let mut bgra = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            // Distance to the rounded rectangle's edge, for a soft one-pixel border.
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let dx = (px - n * 0.5).abs() - (n * 0.5 - radius);
            let dy = (py - n * 0.5).abs() - (n * 0.5 - radius);
            let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() - radius;
            let alpha = ((0.5 - outside).clamp(0.0, 1.0) * 0.45 * 255.0) as u8;
            let i = ((y * size + x) * 4) as usize;
            // Premultiplied mid-grey.
            let c = (150u32 * alpha as u32 / 255) as u8;
            bgra[i..i + 4].copy_from_slice(&[c, c, c, alpha]);
        }
    }
    IconPixels { size, bgra }
}

/// The dock's own icon for the app launcher: a rounded tile in the accent colour with a
/// three-by-three grid of dots.
pub fn launcher_icon(size: u32, accent: (f32, f32, f32)) -> IconPixels {
    let n = size as f32;
    let radius = n * 0.225;
    let inset = n * 0.06;
    let half = n * 0.5 - inset;
    let dot = n * 0.062;
    let spacing = n * 0.2;
    let mut bgra = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5 - n * 0.5, y as f32 + 0.5 - n * 0.5);
            // Rounded-square tile, with a soft one-pixel edge.
            let (dx, dy) = (px.abs() - (half - radius), py.abs() - (half - radius));
            let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - radius;
            let cover = (0.5 - outside).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            // Lighter at the top, deeper at the bottom.
            let shade = 1.12 - 0.38 * (y as f32 / n);
            let mut rgb = [accent.0 * shade, accent.1 * shade, accent.2 * shade];
            // Nearest dot of the 3x3 grid.
            let (gx, gy) = ((px / spacing).round().clamp(-1.0, 1.0) * spacing, (py / spacing).round().clamp(-1.0, 1.0) * spacing);
            let d = ((px - gx).powi(2) + (py - gy).powi(2)).sqrt();
            let white = (dot + 0.5 - d).clamp(0.0, 1.0) * 0.96;
            for c in &mut rgb {
                *c = (*c * (1.0 - white) + white).clamp(0.0, 1.0);
            }
            let i = ((y * size + x) * 4) as usize;
            // Premultiplied BGRA.
            bgra[i] = (rgb[2] * cover * 255.0).round() as u8;
            bgra[i + 1] = (rgb[1] * cover * 255.0).round() as u8;
            bgra[i + 2] = (rgb[0] * cover * 255.0).round() as u8;
            bgra[i + 3] = (cover * 255.0).round() as u8;
        }
    }
    IconPixels { size, bgra }
}

/// One entry of the app launcher.
pub struct AppEntry {
    pub name: String,
    pub target: String,
    pub icon: IconPixels,
}

/// Lists every app in the Start menu's "all apps" list, with icons at `icon_px` pixels, on a
/// background thread. Entries arrive on the returned channel as they are read, so the dock
/// never waits for the few seconds this takes.
pub fn scan_apps(icon_px: u32) -> std::sync::mpsc::Receiver<AppEntry> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || unsafe {
        use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
        if CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_err() {
            return;
        }
        let started = std::time::Instant::now();
        let mut count = 0;
        let listed = (|| -> windows::core::Result<()> {
            let folder: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide("shell:AppsFolder").as_ptr()), None)?;
            let items: IEnumShellItems = folder.BindToHandler(None, &BHID_EnumItems)?;
            loop {
                let mut slot = [None];
                let mut fetched = 0u32;
                if items.Next(&mut slot, Some(&mut fetched)).is_err() || fetched == 0 {
                    break;
                }
                let Some(item) = slot[0].take() else { break };
                let text = |kind: SIGDN| {
                    item.GetDisplayName(kind).ok().and_then(|p| {
                        let s = p.to_string().ok();
                        CoTaskMemFree(Some(p.0 as *const _));
                        s
                    })
                };
                let (Some(name), Some(id)) = (text(SIGDN_NORMALDISPLAY), text(SIGDN_PARENTRELATIVEPARSING)) else { continue };
                if name.is_empty() || id.is_empty() || !is_launchable_name(&name) {
                    continue;
                }
                let target = format!("{APPS_FOLDER}{id}");
                let Some(icon) = load_icon(&target, 256) else { continue };
                let icon = resample(&icon, icon_px);
                count += 1;
                if tx.send(AppEntry { name, target, icon }).is_err() {
                    break;
                }
            }
            Ok(())
        })();
        match listed {
            Ok(()) => crate::log!("app list read: {count} apps in {:.1} s", started.elapsed().as_secs_f32()),
            Err(e) => crate::log!("could not read the app list: {e}"),
        }
        CoUninitialize();
    });
    rx
}

/// The "all apps" list also holds uninstallers, read-me files and links to websites. Leave
/// those out of the launcher.
pub fn is_launchable_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    const SKIP: [&str; 12] = ["uninstall", "gỡ cài đặt", "readme", "read me", "release notes", "license", "documentation", "user guide", "manual", "website", "help", "changelog"];
    !SKIP.iter().any(|word| lower.contains(word))
}

/// Resizes an icon with an area average: every output pixel is the mean of
/// the source area it covers. Slower than what the GPU does per frame, but it
/// runs once per icon and gives clean, shimmer-free small icons.
pub fn resample(src: &IconPixels, size: u32) -> IconPixels {
    let size = size.max(1);
    if size == src.size || src.size == 0 {
        return IconPixels { size: src.size, bgra: src.bgra.clone() };
    }
    let n = src.size as usize;
    let ratio = src.size as f32 / size as f32;
    // For each output index: the source span it covers, as (first pixel, weights).
    let spans: Vec<(usize, Vec<f32>)> = (0..size as usize)
        .map(|i| {
            let (a, b) = (i as f32 * ratio, (i as f32 + 1.0) * ratio);
            if ratio <= 1.0 {
                // Enlarging: blend the two nearest source pixels.
                let c = ((a + b) * 0.5 - 0.5).clamp(0.0, (n - 1) as f32);
                let first = (c.floor() as usize).min(n - 1);
                let f = c - first as f32;
                if first + 1 < n {
                    (first, vec![1.0 - f, f])
                } else {
                    (first, vec![1.0])
                }
            } else {
                let first = a.floor() as usize;
                let last = ((b.ceil() as usize).max(first + 1)).min(n);
                let weights = (first..last).map(|p| (b.min(p as f32 + 1.0) - a.max(p as f32)).max(0.0)).collect();
                (first, weights)
            }
        })
        .collect();

    let mut out = vec![0u8; (size * size * 4) as usize];
    for (oy, (y0, wy)) in spans.iter().enumerate() {
        for (ox, (x0, wx)) in spans.iter().enumerate() {
            let mut acc = [0f32; 4];
            let mut total = 0f32;
            for (dy, &fy) in wy.iter().enumerate() {
                let row = (y0 + dy) * n;
                for (dx, &fx) in wx.iter().enumerate() {
                    let w = fx * fy;
                    let i = (row + x0 + dx) * 4;
                    for (c, sum) in acc.iter_mut().enumerate() {
                        *sum += src.bgra[i + c] as f32 * w;
                    }
                    total += w;
                }
            }
            let o = (oy * size as usize + ox) * 4;
            if total > 0.0 {
                for (c, sum) in acc.iter().enumerate() {
                    out[o + c] = (sum / total).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    IconPixels { size, bgra: out }
}

/// Starts an app (or opens another window of it).
pub fn launch(target: &str) -> bool {
    let wtarget = wide(target);
    let verb = wide("open");
    let result = unsafe { ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(wtarget.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    // ShellExecute reports success as a value above 32.
    let ok = result.0 as isize > 32;
    if !ok {
        crate::log!("could not launch '{target}' (ShellExecute returned {})", result.0 as isize);
    }
    ok
}

/// Brings a window to the front, restoring it first if it is minimised.
pub fn activate(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if !SetForegroundWindow(hwnd).as_bool() {
            // Windows only lets the app that received the last input change the
            // foreground window. A synthetic Alt tap counts as input and lifts that lock.
            keybd_event(VK_MENU.0 as u8, 0, KEYBD_EVENT_FLAGS(0), 0);
            keybd_event(VK_MENU.0 as u8, 0, KEYEVENTF_KEYUP, 0);
            let _ = SetForegroundWindow(hwnd);
        }
    }
}

pub fn minimize(hwnd: HWND) {
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(SC_MINIMIZE as usize), LPARAM(0));
    }
}

/// Maximises a window, or puts a maximised one back to its normal size.
pub fn toggle_maximize(hwnd: HWND) {
    unsafe {
        let command = if IsZoomed(hwnd).as_bool() { SC_RESTORE } else { SC_MAXIMIZE };
        let _ = PostMessageW(Some(hwnd), WM_SYSCOMMAND, WPARAM(command as usize), LPARAM(0));
    }
}

/// True for a window of this program (the dock, the bar, a menu...).
pub fn is_own_window(hwnd: HWND) -> bool {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        pid != 0 && pid == GetCurrentProcessId()
    }
}

pub fn close(hwnd: HWND) {
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

/// Copies a rectangle of the screen. Returns (width, height, opaque BGRA pixels, top row first).
pub fn capture_screen(area: RECT) -> Option<(u32, u32, Vec<u8>)> {
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    if w < 1 || h < 1 {
        return None;
    }
    unsafe {
        let screen = GetDC(None);
        let memory = CreateCompatibleDC(Some(screen));
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // negative = top row first
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut pixels = None;
        if let Ok(bitmap) = CreateDIBSection(Some(memory), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            let previous = SelectObject(memory, bitmap.into());
            let copied = BitBlt(memory, 0, 0, w, h, Some(screen), area.left, area.top, SRCCOPY).is_ok();
            if copied && !bits.is_null() {
                let mut data = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
                // The screen has no transparency; the alpha bytes come back undefined.
                for px in data.chunks_exact_mut(4) {
                    px[3] = 255;
                }
                pixels = Some(data);
            }
            SelectObject(memory, previous);
            let _ = DeleteObject(bitmap.into());
        }
        let _ = DeleteDC(memory);
        ReleaseDC(None, screen);
        pixels.map(|p| (w as u32, h as u32, p))
    }
}

/// Where a minimised window will be when it is restored.
pub fn restored_rect(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let mut placement = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        GetWindowPlacement(hwnd, &mut placement).ok()?;
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        if placement.flags.contains(WPF_RESTORETOMAXIMIZED) {
            return Some(info.rcWork);
        }
        // The stored rectangle is relative to the work area's corner.
        let (dx, dy) = (info.rcWork.left - info.rcMonitor.left, info.rcWork.top - info.rcMonitor.top);
        let r = placement.rcNormalPosition;
        Some(RECT { left: r.left + dx, top: r.top + dy, right: r.right + dx, bottom: r.bottom + dy })
    }
}

pub fn is_minimized(hwnd: HWND) -> bool {
    unsafe { IsIconic(hwnd).as_bool() }
}

pub fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect).ok().map(|_| rect) }
}

/// The part of a window that is actually drawn. A window's own rectangle also includes
/// invisible borders a few pixels wide (the area where the resize cursor appears).
pub fn frame_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    let ok = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut rect as *mut RECT as *mut _, std::mem::size_of::<RECT>() as u32) }.is_ok();
    (ok && rect.right > rect.left && rect.bottom > rect.top).then_some(rect)
}

/// True when `hwnd` covers its whole monitor (a game, a video, a presentation).
pub fn is_fullscreen(hwnd: HWND) -> bool {
    unsafe {
        if hwnd.is_invalid() || IsZoomed(hwnd).as_bool() {
            return false;
        }
        // The dock's own launcher covers the screen too.
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return false;
        }
        if matches!(class_name(hwnd).as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "XamlExplorerHostIslandWindow" | "Windows.UI.Core.CoreWindow") {
            return false;
        }
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let mut rect = RECT::default();
        if !GetMonitorInfoW(monitor, &mut info).as_bool() || GetWindowRect(hwnd, &mut rect).is_err() {
            return false;
        }
        let m = info.rcMonitor;
        rect.left <= m.left && rect.top <= m.top && rect.right >= m.right && rect.bottom >= m.bottom
    }
}
