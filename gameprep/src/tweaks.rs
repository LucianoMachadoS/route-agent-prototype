//! Windows settings GamePrep can change - all per-user (HKCU), so none of
//! them need admin, and each has an exact undo:
//!
//! - Xbox Game Bar background recording (`GameDVR_Enabled` /
//!   `AppCaptureEnabled`): costs FPS while it records.
//! - Starting with Windows: a `Run` value plus Task Manager's own
//!   `StartupApproved` flag, so turning it off here shows as "Disabled" in
//!   Task Manager > Startup apps, and vice versa. The MSI creates the
//!   `Run` value and removes it on uninstall; when running a dev build,
//!   enabling creates it pointing at the current exe.
//!
//! Plus two process-level helpers: single instance and local clock.

use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, SYSTEMTIME};
use windows_sys::Win32::System::Registry::{
    RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_BINARY, REG_DWORD, REG_SZ, RRF_RT_ANY,
    RRF_RT_REG_BINARY, RRF_RT_REG_DWORD,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, SetForegroundWindow, ShowWindow, SW_RESTORE,
};

pub const APP_NAME: &str = "GamePrep";
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const DVR_KEY: &str = r"System\GameConfigStore";
const CAPTURE_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\GameDVR";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn read_dword(key: &str, name: &str) -> Option<u32> {
    let (k, n) = (wide(key), wide(name));
    let mut v = 0u32;
    let mut len = 4u32;
    // SAFETY: v/len describe a 4-byte buffer; RRF_RT_REG_DWORD enforces type.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            &mut v as *mut u32 as *mut _,
            &mut len,
        )
    };
    (rc == 0).then_some(v)
}

fn read_binary(key: &str, name: &str) -> Option<Vec<u8>> {
    let (k, n) = (wide(key), wide(name));
    let mut buf = vec![0u8; 64];
    let mut len = buf.len() as u32;
    // SAFETY: buf/len describe a writable buffer of that size.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut len,
        )
    };
    (rc == 0).then(|| {
        buf.truncate(len as usize);
        buf
    })
}

fn value_exists(key: &str, name: &str) -> bool {
    let (k, n) = (wide(key), wide(name));
    // SAFETY: null data pointer = existence/size query only.
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            RRF_RT_ANY,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    rc == 0
}

fn write(key: &str, name: &str, kind: u32, data: &[u8]) -> bool {
    let (k, n) = (wide(key), wide(name));
    // SAFETY: data is a live slice of the stated length.
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            kind,
            data.as_ptr() as *const _,
            data.len() as u32,
        )
    };
    rc == 0
}

/// `None` when Windows has never written the values (fresh profile).
pub fn game_dvr_enabled() -> Option<bool> {
    let dvr = read_dword(DVR_KEY, "GameDVR_Enabled");
    let cap = read_dword(CAPTURE_KEY, "AppCaptureEnabled");
    match (dvr, cap) {
        (None, None) => None,
        _ => Some(dvr == Some(1) || cap == Some(1)),
    }
}

pub fn set_game_dvr(enabled: bool) -> bool {
    let v = u32::from(enabled).to_le_bytes();
    write(DVR_KEY, "GameDVR_Enabled", REG_DWORD, &v)
        & write(CAPTURE_KEY, "AppCaptureEnabled", REG_DWORD, &v)
}

/// Same rule as Task Manager / windiag: approval byte odd = disabled,
/// missing approval = enabled.
pub fn autostart_enabled() -> bool {
    value_exists(RUN_KEY, APP_NAME)
        && read_binary(APPROVED_KEY, APP_NAME).is_none_or(|v| v.first().is_none_or(|b| b & 1 == 0))
}

pub fn set_autostart(on: bool) -> bool {
    if on && !value_exists(RUN_KEY, APP_NAME) {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
        let cmd = format!("\"{}\" --minimized", exe.display());
        let data: Vec<u8> = wide(&cmd).iter().flat_map(|c| c.to_le_bytes()).collect();
        if !write(RUN_KEY, APP_NAME, REG_SZ, &data) {
            return false;
        }
    }
    // 0x02 = enabled, 0x03 = disabled, followed by 11 bytes Task Manager
    // fills with a timestamp when disabling; zeros are accepted.
    let mut flag = [0u8; 12];
    flag[0] = if on { 2 } else { 3 };
    write(APPROVED_KEY, APP_NAME, REG_BINARY, &flag)
}

/// Returns false if another GamePrep is already running in this session
/// (and brings its window to the front). The mutex lives until exit.
pub fn claim_single_instance() -> bool {
    let name = wide("Local\\GamePrep-single-instance");
    // SAFETY: plain FFI; the handle is intentionally leaked for the
    // process lifetime (that's what keeps the name claimed).
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if h.is_null() || GetLastError() != ERROR_ALREADY_EXISTS {
            return true;
        }
        let title = wide(APP_NAME);
        let hwnd = FindWindowW(std::ptr::null(), title.as_ptr());
        if !hwnd.is_null() {
            ShowWindow(hwnd, SW_RESTORE);
            SetForegroundWindow(hwnd);
        }
    }
    false
}

/// Local wall-clock time as HH:MM:SS, for the in-window event list.
pub fn local_hms() -> String {
    // SAFETY: GetLocalTime fills the struct it's given.
    let t: SYSTEMTIME = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_round_trip_on_a_scratch_key() {
        let key = r"Software\GamePrep-test";
        assert!(write(key, "d", REG_DWORD, &7u32.to_le_bytes()));
        assert_eq!(read_dword(key, "d"), Some(7));
        assert!(write(key, "b", REG_BINARY, &[3, 0, 0]));
        assert_eq!(read_binary(key, "b"), Some(vec![3, 0, 0]));
        assert!(value_exists(key, "b"));
        assert!(!value_exists(key, "nope"));
        let k = wide(key);
        // SAFETY: deleting the scratch key this test created.
        unsafe {
            windows_sys::Win32::System::Registry::RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr());
        }
        assert_eq!(read_dword(key, "d"), None);
    }

    #[test]
    fn clock_format() {
        let s = local_hms();
        assert_eq!(s.len(), 8);
        assert_eq!(&s[2..3], ":");
    }
}
