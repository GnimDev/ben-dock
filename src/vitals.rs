//! How busy the computer is: the share of the processor in use since the last look, and
//! of the memory taken. Two questions to Windows that are answered at once, so they are
//! asked from the app's own thread.

use windows::Win32::Foundation::FILETIME;
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
