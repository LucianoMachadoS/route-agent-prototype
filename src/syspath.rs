//! Resolves well-known Windows system binaries by absolute path instead of
//! bare name.
//!
//! Background (CWE-427, "uncontrolled search path element"): when you spawn
//! a process by bare filename (`Command::new("ping.exe")`), Windows's module
//! search order checks the launching process's own directory and the
//! current working directory *before* `System32`. If `route-agent.exe` is
//! ever run with its working directory pointed at somewhere an attacker can
//! write (a shared folder, a Downloads folder, the repo itself if it were
//! ever compromised), a file named `ping.exe`, `sc.exe`, `icacls.exe` or
//! `powershell.exe` dropped there would run instead of the real Windows
//! binary - silently, and with whatever privileges route-agent has. Two of
//! our commands (`connect`/`disconnect`) run elevated, which makes this a
//! real local privilege-escalation path, not just a correctness nit.
//!
//! The fix is simple: never ask Windows to search for these; always pass
//! the fully-qualified path under `%SystemRoot%`. See docs/SECURITY.md.

use std::path::PathBuf;

fn system_root() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

/// Absolute path to a binary in `%SystemRoot%\System32` (`ping.exe`,
/// `sc.exe`, `icacls.exe`, ...).
pub fn system32(name: &str) -> PathBuf {
    system_root().join("System32").join(name)
}

/// Absolute path to the built-in Windows PowerShell 5.1, present on every
/// Windows 10/11 install (as opposed to PowerShell 7/`pwsh.exe`, which is
/// optional and not something we need - we only use trivial, version-
/// independent `[DateTime]`/`Get-AuthenticodeSignature` calls).
pub fn windows_powershell() -> PathBuf {
    system_root().join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system32_joins_under_system_root() {
        let p = system32("ping.exe");
        let s = p.to_string_lossy().to_lowercase();
        assert!(s.ends_with(r"\system32\ping.exe"));
    }

    #[test]
    fn windows_powershell_points_at_v1() {
        let p = windows_powershell();
        let s = p.to_string_lossy().to_lowercase();
        assert!(s.ends_with(r"\system32\windowspowershell\v1.0\powershell.exe"));
    }
}
