//! Exit when Windows asks running apps to close: logoff/shutdown, and the
//! Restart Manager that MSI uses on uninstall/upgrade.
//!
//! Found live: uninstalling with GamePrep open left it running from the
//! already-deleted exe, after MSI waited 30 s ("RESTART MANAGER: Failed to
//! shutdown all applications"). Restart Manager closes GUI apps with
//! WM_QUERYENDSESSION/WM_ENDSESSION, which the window toolkit doesn't
//! surface - and GamePrep's X button deliberately only minimizes. So a
//! hidden top-level window on its own thread answers those messages and
//! exits the process. (Message-only windows don't receive them; it has
//! to be a real, just invisible, top-level window.)

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW,
    TranslateMessage, MSG, WM_ENDSESSION, WM_QUERYENDSESSION, WNDCLASSW, WS_POPUP,
};

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        // "May we end?" - yes; nothing to save that isn't saved already.
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wparam != 0 {
                route_agent::agentlog::info(
                    "Windows pediu para fechar (desligamento, logoff ou instalador)",
                );
                std::process::exit(0);
            }
            0
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

pub fn spawn_listener() {
    std::thread::spawn(|| {
        let class: Vec<u16> = "GamePrepEndSession\0".encode_utf16().collect();
        // SAFETY: standard register-class/create-window/message-loop
        // sequence; `class` outlives the window (this thread never returns
        // while the loop runs).
        unsafe {
            let hinst = GetModuleHandleW(std::ptr::null());
            let mut wc: WNDCLASSW = std::mem::zeroed();
            wc.lpfnWndProc = Some(wndproc);
            wc.hInstance = hinst;
            wc.lpszClassName = class.as_ptr();
            if RegisterClassW(&wc) == 0 {
                return;
            }
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                hinst,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return;
            }
            let mut msg: MSG = std::mem::zeroed();
            while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    });
}
