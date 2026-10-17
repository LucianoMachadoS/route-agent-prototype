//! Thin, safe wrappers over the Win32 process/window APIs GamePrep uses.
//! Everything works on processes of the *current user* without elevation -
//! GamePrep never asks for admin. Processes we can't open (other users,
//! elevated, protected) simply report 0 memory / no path and can't be
//! closed, which is fine: they were never ours to close.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, SetPriorityClass,
    TerminateProcess, ABOVE_NORMAL_PRIORITY_CLASS, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_INFORMATION, PROCESS_TERMINATE, PROCESS_VM_READ,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, WM_CLOSE,
};

use crate::config::normalize;
use crate::engine::{ClosedApp, ProcView};

/// Closes the handle when dropped.
struct Handle(HANDLE);

impl Handle {
    fn open(access: u32, pid: u32) -> Option<Self> {
        // SAFETY: plain FFI call; a null return means failure.
        let h = unsafe { OpenProcess(access, 0, pid) };
        (!h.is_null()).then_some(Self(h))
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: self.0 is a handle we opened and haven't closed.
        unsafe {
            CloseHandle(self.0);
        }
    }
}

fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Every process visible to this user, with its working set (0 if it
/// can't be opened). Skips GamePrep itself.
pub fn list() -> Vec<ProcView> {
    let mut out = Vec::new();
    // SAFETY: snapshot handle is checked and closed; entry is a correctly
    // sized, zero-initialised PROCESSENTRY32W with dwSize set.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return out;
        }
        let snap = Handle(snap);
        let me = GetCurrentProcessId();
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap.0, &mut e);
        while ok != 0 {
            if e.th32ProcessID != 0 && e.th32ProcessID != me {
                out.push(ProcView {
                    pid: e.th32ProcessID,
                    name: from_wide(&e.szExeFile),
                    working_set: working_set(e.th32ProcessID),
                });
            }
            ok = Process32NextW(snap.0, &mut e);
        }
    }
    out
}

fn working_set(pid: u32) -> u64 {
    let Some(h) = Handle::open(PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ, pid) else {
        return 0;
    };
    // SAFETY: counters is a local of the right size, cb set accordingly.
    unsafe {
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if K32GetProcessMemoryInfo(h.0, &mut c, c.cb) != 0 {
            c.WorkingSetSize as u64
        } else {
            0
        }
    }
}

/// Full path of a process's executable, for reopening it later.
pub fn image_path(pid: u32) -> Option<PathBuf> {
    let h = Handle::open(PROCESS_QUERY_LIMITED_INFORMATION, pid)?;
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // SAFETY: buf has `len` u16s; the API writes at most that and updates len.
    let ok =
        unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len) };
    (ok != 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

struct CloseCtx<'a> {
    pids: &'a HashSet<u32>,
    posted: usize,
}

unsafe extern "system" fn post_close(hwnd: HWND, lparam: LPARAM) -> i32 {
    // SAFETY: lparam is the &mut CloseCtx passed to EnumWindows below,
    // alive for the whole (synchronous) enumeration.
    let ctx = unsafe { &mut *(lparam as *mut CloseCtx) };
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
        if ctx.pids.contains(&pid) && IsWindowVisible(hwnd) != 0 {
            PostMessageW(hwnd, WM_CLOSE, 0, 0);
            ctx.posted += 1;
        }
    }
    1 // keep enumerating
}

/// Asks every visible top-level window of `pids` to close, exactly like
/// clicking its X - apps get to save state (browsers keep their session).
pub fn request_close(pids: &HashSet<u32>) -> usize {
    let mut ctx = CloseCtx { pids, posted: 0 };
    // SAFETY: callback only dereferences lparam as the CloseCtx above.
    unsafe {
        EnumWindows(Some(post_close), &mut ctx as *mut CloseCtx as LPARAM);
    }
    ctx.posted
}

/// Exit code given to force-closed apps - non-zero, so nothing mistakes a
/// forced kill for a clean exit.
pub const FORCED_EXIT_CODE: u32 = 1;

pub fn terminate(pid: u32) -> bool {
    match Handle::open(PROCESS_TERMINATE, pid) {
        // SAFETY: handle opened with PROCESS_TERMINATE.
        Some(h) => unsafe { TerminateProcess(h.0, FORCED_EXIT_CODE) != 0 },
        None => false,
    }
}

pub fn boost(pid: u32) -> bool {
    match Handle::open(PROCESS_SET_INFORMATION, pid) {
        // SAFETY: handle opened with PROCESS_SET_INFORMATION.
        Some(h) => unsafe { SetPriorityClass(h.0, ABOVE_NORMAL_PRIORITY_CLASS) != 0 },
        None => false,
    }
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct CloseReport {
    pub closed: Vec<ClosedApp>,
    pub processes: usize,
    pub freed_mb: f64,
    pub forced: Vec<String>,
    pub failed: Vec<String>,
}

/// Closes every process named in `names` (normalized): polite WM_CLOSE
/// first, then - for whatever is still alive after `grace` (apps that
/// "close to tray", like Discord or Spotify, never exit on WM_CLOSE) -
/// TerminateProcess. Blocks for up to `grace`; run it off the UI thread.
pub fn close_apps(names: &[String], grace: Duration) -> CloseReport {
    let wanted: HashSet<&str> = names.iter().map(String::as_str).collect();
    let targets: Vec<ProcView> = list()
        .into_iter()
        .filter(|p| wanted.contains(normalize(&p.name).as_str()))
        .collect();
    let mut report = CloseReport::default();
    if targets.is_empty() {
        return report;
    }
    report.processes = targets.len();
    report.freed_mb = targets.iter().map(|p| p.working_set as f64).sum::<f64>() / 1_048_576.0;

    // Remember one path per app before anything dies, for "reopen later".
    let mut seen = HashSet::new();
    for p in &targets {
        let n = normalize(&p.name);
        if seen.insert(n.clone()) {
            report.closed.push(ClosedApp {
                name: n,
                path: image_path(p.pid),
            });
        }
    }

    let pids: HashSet<u32> = targets.iter().map(|p| p.pid).collect();
    request_close(&pids);
    let deadline = Instant::now() + grace;
    let alive = |pids: &HashSet<u32>| -> Vec<ProcView> {
        list()
            .into_iter()
            .filter(|p| pids.contains(&p.pid))
            .collect()
    };
    let mut left = alive(&pids);
    while !left.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        left = alive(&pids);
    }
    for p in &left {
        let n = normalize(&p.name);
        if terminate(p.pid) {
            if !report.forced.contains(&n) {
                report.forced.push(n);
            }
        } else if !report.failed.contains(&n) {
            report.failed.push(n);
        }
    }
    report
}

/// Starts an app again, detached from GamePrep.
pub fn relaunch(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new(path)
        .current_dir(path.parent().unwrap_or(std::path::Path::new(".")))
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_real_processes_with_memory() {
        let all = list();
        assert!(all
            .iter()
            .any(|p| normalize(&p.name) == "explorer.exe" || normalize(&p.name) == "svchost.exe"));
        assert!(
            all.iter().any(|p| p.working_set > 0),
            "own-user processes must report memory"
        );
    }

    #[test]
    fn closes_a_real_child_process() {
        // A hidden ping running ~30 s: no window, so this exercises the
        // grace-period-then-terminate path end to end.
        let mut child =
            route_agent::syspath::hidden_command(route_agent::syspath::system32("PING.EXE"))
                .args(["-n", "30", "127.0.0.1"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap();
        let pid = child.id();
        assert!(image_path(pid)
            .unwrap()
            .to_string_lossy()
            .to_lowercase()
            .ends_with("ping.exe"));

        // Only this one pid: close by name would also hit unrelated pings.
        let pids: HashSet<u32> = [pid].into();
        assert_eq!(request_close(&pids), 0, "ping has no windows");
        assert!(terminate(pid));
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(FORCED_EXIT_CODE as i32));
    }
}
