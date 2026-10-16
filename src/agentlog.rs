//! Timestamped, size-capped logging for the long-running loops
//! (`watch-game`, `watch-games`, `monitor`).
//!
//! Before this, those loops only printed to stdout and the Scheduled
//! Task's `.bat` appended that to `watch-game.log` with `>>` - no
//! timestamps, and no size limit: the file only ever grew (123 KB after
//! two evenings, most of it one PowerShell error repeated every 20 s).
//! With `--log <path>` the loop writes there itself, prefixing each line
//! with UTC time, and rotates `path` -> `path.1` -> `path.2` once it
//! passes `max_bytes`. Without `--log`, output goes to stdout/stderr
//! exactly as before, so interactive use is unchanged.
//!
//! Also home to `utc_now()`, a pure-Rust replacement for the old
//! `bench::utc_timestamp`, which spawned a whole `powershell.exe` (tens
//! of MB, hundreds of ms) every time a CSV row needed a timestamp.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Result;

struct FileSink {
    path: PathBuf,
    max_bytes: u64,
    keep: u32,
}

static SINK: Mutex<Option<FileSink>> = Mutex::new(None);

/// From now on, `info`/`warn` write timestamped lines to `path` instead of
/// stdout/stderr, rotating once the file passes `max_bytes` and keeping
/// `keep` old copies (`path.1` newest .. `path.<keep>` oldest).
pub fn init_file(path: &Path, max_bytes: u64, keep: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    *SINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(FileSink {
        path: path.to_path_buf(),
        max_bytes: max_bytes.max(1024),
        keep,
    });
    Ok(())
}

pub fn info(msg: &str) {
    emit(false, msg);
}

pub fn warn(msg: &str) {
    emit(true, msg);
}

/// `log_info!("...", args)` - `format!` + `agentlog::info`.
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::agentlog::info(&format!($($arg)*)) };
}

/// `log_warn!("...", args)` - `format!` + `agentlog::warn`.
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::agentlog::warn(&format!($($arg)*)) };
}

fn emit(is_warn: bool, msg: &str) {
    let guard = SINK.lock().unwrap_or_else(|e| e.into_inner());
    match guard.as_ref() {
        Some(sink) => {
            let line = format!("{} {msg}\n", utc_now());
            if let Err(e) = sink.write(&line) {
                // Never lose the message just because the log file is
                // locked/unwritable (e.g. AV interference) - fall back.
                eprintln!("{msg} (log write failed: {e})");
            }
        }
        None if is_warn => eprintln!("{msg}"),
        None => println!("{msg}"),
    }
}

impl FileSink {
    fn write(&self, line: &str) -> io::Result<()> {
        let len = fs::metadata(&self.path).map(|m| m.len()).unwrap_or(0);
        if len > 0 && len + line.len() as u64 > self.max_bytes {
            rotate(&self.path, self.keep)?;
        }
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        f.write_all(line.as_bytes())
    }
}

fn numbered(path: &Path, i: u32) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{i}"));
    PathBuf::from(s)
}

/// `path` -> `path.1`, `path.1` -> `path.2`, ..., dropping `path.<keep>`.
/// `keep == 0` just deletes `path`.
pub fn rotate(path: &Path, keep: u32) -> io::Result<()> {
    if keep == 0 {
        return fs::remove_file(path);
    }
    let _ = fs::remove_file(numbered(path, keep));
    for i in (1..keep).rev() {
        let from = numbered(path, i);
        if from.exists() {
            fs::rename(&from, numbered(path, i + 1))?;
        }
    }
    fs::rename(path, numbered(path, 1))
}

/// Current UTC time as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format_utc(secs)
}

/// Days since 1970-01-01 (UTC) for today.
pub fn today_days() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86_400) as i64)
        .unwrap_or(0)
}

pub fn format_utc(unix_secs: u64) -> String {
    let (y, m, d) = civil_from_days((unix_secs / 86_400) as i64);
    let rem = unix_secs % 86_400;
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

// Howard Hinnant's civil-calendar algorithms (public domain) - exact for
// the proleptic Gregorian calendar, no date crate needed.
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe as i64 + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u64;
    let (m, d) = (u64::from(m), u64::from(d));
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// Parses `YYYY-MM-DD` (optionally followed by a time) into days since
/// the epoch.
pub fn parse_ymd(s: &str) -> Option<i64> {
    let mut parts = s.get(..10)?.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn formats_known_instants() {
        assert_eq!(format_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        // Leap day.
        assert_eq!(format_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn civil_round_trips() {
        for days in [-1_000_000, -1, 0, 1, 11_016, 20_742, 1_000_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
        assert_eq!(parse_ymd("2026-10-16"), Some(days_from_civil(2026, 10, 16)));
        assert_eq!(
            parse_ymd("2017-08-10T00:00:00"),
            Some(days_from_civil(2017, 8, 10))
        );
        assert_eq!(parse_ymd("10/08/2017"), None);
        assert_eq!(parse_ymd(""), None);
    }

    #[test]
    fn rotation_shifts_and_drops_oldest() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("x.log");
        for gen in ["a", "b", "c", "d"] {
            fs::write(&p, gen).unwrap();
            rotate(&p, 2).unwrap();
        }
        assert!(!p.exists());
        assert_eq!(fs::read_to_string(numbered(&p, 1)).unwrap(), "d");
        assert_eq!(fs::read_to_string(numbered(&p, 2)).unwrap(), "c");
        assert!(!numbered(&p, 3).exists());
    }

    #[test]
    fn sink_rotates_when_full() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("w.log");
        let sink = FileSink {
            path: p.clone(),
            max_bytes: 1024,
            keep: 2,
        };
        let line = format!("{}\n", "x".repeat(99));
        for _ in 0..25 {
            sink.write(&line).unwrap();
        }
        assert!(fs::metadata(&p).unwrap().len() <= 1024);
        assert!(numbered(&p, 1).exists());
        assert!(fs::metadata(numbered(&p, 1)).unwrap().len() <= 1024);
    }
}
