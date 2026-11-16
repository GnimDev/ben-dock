//! How busy the computer is: the share of the processor in use since the last look, and
//! of the memory taken. Two questions to Windows that are answered at once, so they are
//! asked from the app's own thread.

use crate::util::wide;
use std::time::Instant;
use windows::core::PCWSTR;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, IF_TYPE_ETHERNET_CSMACD, IF_TYPE_IEEE80211, MIB_IF_TABLE2};
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

/// Percent of the processor and of the memory in use, and the memory itself in gigabytes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Reading {
    pub processor: u32,
    pub memory: u32,
    pub used: f32,
    pub total: f32,
}

/// Remembers the processor's counters at the last look: its use is told between two looks.
#[derive(Default)]
pub struct Vitals {
    /// Idle time and all time (in units of 100 ns, summed over the cores).
    last: Option<(u64, u64)>,
}

impl Vitals {
    pub fn read(&mut self) -> Reading {
        let mut reading = Reading::default();
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_ok() {
            let ticks = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
            // The time in the kernel counts the idle time too.
            let now = (ticks(idle), ticks(kernel) + ticks(user));
            if let Some((idle_then, all_then)) = self.last {
                let all = now.1.saturating_sub(all_then);
                if all > 0 {
                    let busy = all.saturating_sub(now.0.saturating_sub(idle_then));
                    reading.processor = ((busy as f64 / all as f64) * 100.0).round().clamp(0.0, 100.0) as u32;
                }
            }
            self.last = Some(now);
        }
        let mut memory = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        if unsafe { GlobalMemoryStatusEx(&mut memory) }.is_ok() {
            let gigabytes = |bytes: u64| bytes as f32 / (1u64 << 30) as f32;
            reading.memory = memory.dwMemoryLoad;
            reading.total = gigabytes(memory.ullTotalPhys);
            reading.used = gigabytes(memory.ullTotalPhys.saturating_sub(memory.ullAvailPhys));
        }
        reading
    }
}

/// How full the disk Windows is on is: gigabytes taken, and gigabytes in all.
pub fn disk() -> Option<(f32, f32)> {
    let root = format!("{}\\", std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()));
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide(&root).as_ptr()), None, Some(&mut total), Some(&mut free)) }.ok()?;
    let gigabytes = |bytes: u64| bytes as f32 / (1u64 << 30) as f32;
    (total > 0).then(|| (gigabytes(total.saturating_sub(free)), gigabytes(total)))
}

/// How fast the network is being used: bytes a second coming in and going out, told
/// between two looks, like the processor's use.
#[derive(Default)]
pub struct Traffic {
    /// Bytes in and out so far, and when that was read.
    last: Option<(u64, u64, Instant)>,
}

impl Traffic {
    pub fn read(&mut self) -> (f32, f32) {
        let Some((received, sent)) = counted() else { return (0.0, 0.0) };
        let now = Instant::now();
        let rate = match self.last {
            Some((received_then, sent_then, then)) => {
                let seconds = now.duration_since(then).as_secs_f32().max(0.05);
                // A counter that went backwards (an adapter was reset) counts as nothing.
                (received.saturating_sub(received_then) as f32 / seconds, sent.saturating_sub(sent_then) as f32 / seconds)
            }
            None => (0.0, 0.0),
        };
        self.last = Some((received, sent, now));
        rate
    }
}

/// Bytes received and sent since Windows started, over the network adapters that are real
/// (cable and Wi-Fi, not the virtual ones stacked on them, which would count the same
/// bytes again) and connected.
fn counted() -> Option<(u64, u64)> {
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).0 != 0 || table.is_null() {
            return None;
        }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let mut sum = (0u64, 0u64);
        for row in rows {
            // The lowest bit of the flags: this is a piece of hardware.
            let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if hardware && row.OperStatus == IfOperStatusUp && (row.Type == IF_TYPE_ETHERNET_CSMACD || row.Type == IF_TYPE_IEEE80211) {
                sum.0 += row.InOctets;
                sum.1 += row.OutOctets;
            }
        }
        FreeMibTable(table as *const _);
        Some(sum)
    }
}

/// A speed in bytes a second, in words: "850 KB/s", "12.4 MB/s".
pub fn speed_text(bytes: f32) -> String {
    let kilobytes = bytes / 1024.0;
    if kilobytes < 1000.0 {
        format!("{} KB/s", kilobytes.round() as u32)
    } else {
        format!("{:.1} MB/s", kilobytes / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_in_words() {
        assert_eq!(speed_text(0.0), "0 KB/s");
        assert_eq!(speed_text(870_000.0), "850 KB/s");
        assert_eq!(speed_text(13_000_000.0), "12.4 MB/s");
    }

    #[test]
    fn the_computer_can_be_asked() {
        // Only that the questions are answered sensibly on whatever machine this runs on.
        if let Some((used, total)) = disk() {
            assert!(used >= 0.0 && used <= total && total > 1.0, "{used} of {total}");
        }
        let mut traffic = Traffic::default();
        assert_eq!(traffic.read(), (0.0, 0.0), "nothing to compare the first look with");
        let (down, up) = traffic.read();
        assert!(down >= 0.0 && up >= 0.0);
    }
}
