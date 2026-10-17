//! The agent's own CPU/RAM use, for the periodic self-report in the
//! long-running loops (the "resource consumption" metric the benchmark
//! plan asks for: the agent itself must not eat into the game's budget).
//!
//! Read straight from kernel32 (`GetProcessTimes`,
//! `K32GetProcessMemoryInfo`) instead of shelling out to
//! `Get-Process -Id $PID` - spawning PowerShell to measure our own
//! footprint would cost far more than the footprint itself.

use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub struct SelfStats {
    /// Share of the whole machine (all logical CPUs), like Task Manager.
    /// `None` on the first sample - there's no previous one to diff against.
    pub cpu_pct: Option<f64>,
    pub working_set_mb: f64,
    pub private_mb: f64,
}

/// One-line summary for the log.
pub fn describe(s: &SelfStats) -> String {
    let cpu = s
        .cpu_pct
        .map(|c| format!("{c:.2}%"))
        .unwrap_or_else(|| "-".to_string());
    format!(
        "self: cpu {cpu} ram {:.1} MB (private {:.1} MB)",
        s.working_set_mb, s.private_mb
    )
}

/// Keeps the previous sample so each call reports CPU since the last one.
#[derive(Debug, Default)]
pub struct SelfSampler {
    last: Option<(Instant, u64)>,
}

impl SelfSampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// `None` if the OS calls fail (or on non-Windows builds).
    pub fn sample(&mut self) -> Option<SelfStats> {
        let (cpu_100ns, working_set, private) = imp::read()?;
        let now = Instant::now();
        let cpu_pct = self.last.map(|(t, prev_cpu)| {
            let wall_100ns = now.duration_since(t).as_nanos() as f64 / 100.0;
            let cpus = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1) as f64;
            cpu_share(cpu_100ns.saturating_sub(prev_cpu) as f64, wall_100ns, cpus)
        });
        self.last = Some((now, cpu_100ns));
        Some(SelfStats {
            cpu_pct,
            working_set_mb: working_set as f64 / 1_048_576.0,
            private_mb: private as f64 / 1_048_576.0,
        })
    }
}

/// How often the long-running loops log a self-report.
pub const REPORT_EVERY_SECS: u64 = 300;

/// Logs a self-report on round 1 and then roughly every
/// `REPORT_EVERY_SECS`, given the loop's round interval.
pub fn report_periodically(sampler: &mut SelfSampler, round_no: u32, interval_secs: u64) {
    let every = (REPORT_EVERY_SECS / interval_secs.max(1)).max(1) as u32;
    if round_no == 1 || round_no.is_multiple_of(every) {
        if let Some(s) = sampler.sample() {
            crate::log_info!("[round {round_no}] {}", describe(&s));
        }
    }
}

fn cpu_share(cpu_delta: f64, wall_delta: f64, cpus: f64) -> f64 {
    if wall_delta <= 0.0 || cpus <= 0.0 {
        return 0.0;
    }
    (cpu_delta / wall_delta / cpus * 100.0).clamp(0.0, 100.0)
}

#[cfg(windows)]
mod imp {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn ticks(&self) -> u64 {
            (u64::from(self.high) << 32) | u64::from(self.low)
        }
    }

    // PROCESS_MEMORY_COUNTERS from psapi.h.
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn GetProcessTimes(
            process: isize,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn K32GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }

    /// (kernel+user CPU time in 100 ns ticks, working set bytes, private bytes)
    pub fn read() -> Option<(u64, usize, usize)> {
        let mut c = FileTime::default();
        let mut e = FileTime::default();
        let mut k = FileTime::default();
        let mut u = FileTime::default();
        let mut mem = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            ..Default::default()
        };
        // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no
        // closing; every out-pointer refers to a live, correctly sized
        // #[repr(C)] local.
        unsafe {
            let h = GetCurrentProcess();
            if GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) == 0 {
                return None;
            }
            if K32GetProcessMemoryInfo(h, &mut mem, mem.cb) == 0 {
                return None;
            }
        }
        let _ = (c, e);
        Some((
            FileTime::ticks(&k) + FileTime::ticks(&u),
            mem.working_set_size,
            mem.pagefile_usage,
        ))
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn read() -> Option<(u64, usize, usize)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_share_is_machine_wide_and_clamped() {
        // 1 s of CPU over 1 s of wall time on 4 CPUs = one core = 25%.
        assert!((cpu_share(1e7, 1e7, 4.0) - 25.0).abs() < 1e-9);
        assert_eq!(cpu_share(5.0, 0.0, 4.0), 0.0);
        assert_eq!(cpu_share(1e9, 1e7, 1.0), 100.0);
    }

    #[test]
    fn describe_without_cpu_yet() {
        let s = SelfStats {
            cpu_pct: None,
            working_set_mb: 4.26,
            private_mb: 1.5,
        };
        assert_eq!(describe(&s), "self: cpu - ram 4.3 MB (private 1.5 MB)");
    }

    #[cfg(windows)]
    #[test]
    fn samples_this_process() {
        let mut s = SelfSampler::new();
        let first = s.sample().expect("kernel32 calls should work");
        assert!(first.cpu_pct.is_none());
        assert!(first.working_set_mb > 0.0);
        let second = s.sample().unwrap();
        assert!(second.cpu_pct.is_some());
    }
}
