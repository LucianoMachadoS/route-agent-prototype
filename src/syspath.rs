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

/// `Command::new(path)` that won't pop up a console window for the child.
/// Matters for `route-agent-gui`, which has no console of its own in
/// release builds - without this every `sc.exe`/`ping.exe` it runs on a
/// refresh would flash a black window. Harmless for the CLI: every call
/// site captures output through pipes (`.output()`), never the console.
pub fn hidden_command(path: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut cmd = std::process::Command::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// `cmd.output()`, but kills the child and returns `ErrorKind::TimedOut`
/// if it hasn't exited within `timeout`. `Command::output` waits forever,
/// and a wedged `powershell.exe` (WMI/CIM providers can hang) would
/// otherwise stall `watch-games` indefinitely, silently.
pub fn output_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::process::Stdio;

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Drain both pipes on threads: a child blocked on a full pipe buffer
    // would never exit, and we'd "time out" a perfectly healthy process.
    let mut out = child.stdout.take().expect("stdout is piped");
    let mut err = child.stderr.take().expect("stderr is piped");
    let out_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = out.read_to_end(&mut v);
        v
    });
    let err_t = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = err.read_to_end(&mut v);
        v
    });

    let deadline = std::time::Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!("did not finish within {}s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    Ok(std::process::Output {
        status,
        stdout: out_t.join().unwrap_or_default(),
        stderr: err_t.join().unwrap_or_default(),
    })
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

    #[cfg(windows)]
    #[test]
    fn output_with_timeout_returns_output_or_kills() {
        use std::time::{Duration, Instant};

        let mut quick = std::process::Command::new(system32("cmd.exe"));
        quick.args(["/c", "echo hi"]);
        let out = output_with_timeout(quick, Duration::from_secs(10)).unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");

        // ping -n 30 takes ~29 s; must be cut off after ~1 s.
        let mut slow = hidden_command(system32("ping.exe"));
        slow.args(["-n", "30", "127.0.0.1"]);
        let started = Instant::now();
        let err = output_with_timeout(slow, Duration::from_secs(1)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn windows_powershell_points_at_v1() {
        let p = windows_powershell();
        let s = p.to_string_lossy().to_lowercase();
        assert!(s.ends_with(r"\system32\windowspowershell\v1.0\powershell.exe"));
    }
}
