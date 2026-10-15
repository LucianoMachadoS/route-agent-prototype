//! Thin wrapper around the official `wireguard.exe` CLI (WireGuard for
//! Windows) for installing/uninstalling a tunnel as a Windows service, and
//! around `sc.exe` for querying that service's state.
//!
//! We deliberately shell out instead of talking to the Windows Service
//! Control Manager directly: `wireguard.exe /installtunnelservice` already
//! handles config encryption-at-rest (DPAPI) and service registration the
//! way the official client expects, and re-implementing that is out of
//! scope for a prototype.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::error::{AgentError, Result};
use crate::traits::TunnelManager;

/// Service name prefix WireGuard for Windows registers tunnels under.
const SERVICE_PREFIX: &str = "WireGuardTunnel$";

/// Known install locations for wireguard.exe, checked in order before
/// falling back to `PATH`.
fn candidate_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from(r"C:\Program Files\WireGuard\wireguard.exe"),
        PathBuf::from(r"C:\Program Files (x86)\WireGuard\wireguard.exe"),
    ]
}

/// Finds the official WireGuard client. `override_path`, when given, is
/// trusted and used as-is (still validated to exist).
pub fn find_wireguard_exe(override_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = override_path {
        return if p.is_file() {
            Ok(p.to_path_buf())
        } else {
            Err(AgentError::WireGuardNotFound(p.to_path_buf()))
        };
    }

    for candidate in candidate_paths() {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // Fall back to PATH.
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in path_var.split(';') {
            let candidate = Path::new(dir).join("wireguard.exe");
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(AgentError::WireGuardNotFound(PathBuf::from(
        "wireguard.exe",
    )))
}

/// Derives the tunnel/service name WireGuard will use from a config path:
/// the file stem of a `.conf` file (e.g. `wg-client.conf` -> `wg-client`).
pub fn tunnel_name_from_config(config: &Path) -> Result<String> {
    let ext_ok = config
        .extension()
        .map(|e| e.eq_ignore_ascii_case("conf"))
        .unwrap_or(false);
    if !ext_ok {
        return Err(AgentError::InvalidConfigName(config.to_path_buf()));
    }
    config
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| AgentError::InvalidConfigName(config.to_path_buf()))
}

fn run(command_label: &str, mut cmd: Command) -> Result<Output> {
    let output = cmd.output().map_err(|source| AgentError::Spawn {
        command: command_label.to_string(),
        source,
    })?;

    if !output.status.success() {
        return Err(AgentError::CommandFailed {
            command: command_label.to_string(),
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output)
}

/// Warns when `wireguard.exe` was not resolved from one of our own
/// hardcoded, admin-writable-only candidate paths (i.e. it came from
/// `--wireguard`/`WIREGUARD_EXE` and isn't one of the standard install
/// locations). `connect`/`disconnect` run this binary elevated: if an
/// attacker-controlled process can set the `WIREGUARD_EXE` user
/// environment variable before an administrator runs `route-agent` in a
/// new elevated terminal (which inherits user env vars), this is a
/// classic path for getting an arbitrary binary run with Administrator
/// rights. We can't safely block a legitimate custom install outright, so
/// we make the trust decision loud instead of silent. See docs/SECURITY.md.
fn warn_if_untrusted_wireguard_path(exe: &Path, was_overridden: bool) {
    if !was_overridden {
        return; // resolved from our own candidate_paths() or found on PATH by us, not by env/flag
    }
    let trusted = candidate_paths().into_iter().any(|c| {
        c.to_string_lossy()
            .eq_ignore_ascii_case(&exe.to_string_lossy())
    });
    if !trusted {
        eprintln!(
            "warning: using a non-standard wireguard.exe at '{}' (from --wireguard or WIREGUARD_EXE). \
This will run with Administrator privileges - make sure you trust this exact file and that nothing \
untrusted could have set WIREGUARD_EXE before you elevated this terminal.",
            exe.display()
        );
    }
}

/// Restricts a config file's ACL to SYSTEM and (local) Administrators only,
/// dropping inherited entries. Private keys live in this file in
/// plaintext; if it sits under a folder like `C:\ProgramData\...` its
/// permissions are inherited from the parent and can allow any local
/// account to read it. Uses the well-known SIDs (`S-1-5-18` = SYSTEM,
/// `S-1-5-32-544` = Administrators) instead of the localized group names
/// ("Administrators" vs. "Administradores" etc.) so this works regardless
/// of Windows UI language. Best-effort: callers should treat failure as a
/// warning, not a reason to consider `connect` itself failed - the tunnel
/// is already up either way.
pub fn restrict_config_permissions(path: &Path) -> Result<()> {
    let icacls = crate::syspath::system32("icacls.exe");
    let mut cmd = Command::new(&icacls);
    cmd.arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg("*S-1-5-18:(F)")
        .arg("/grant:r")
        .arg("*S-1-5-32-544:(F)");
    run("icacls.exe", cmd)?;
    Ok(())
}

/// Installs the tunnel service from `config` and starts it
/// (`wireguard.exe /installtunnelservice <config>`), then locks down the
/// config file's permissions. Requires admin.
pub fn install_and_start(config: &Path, wireguard_exe: Option<&Path>) -> Result<String> {
    if !config.is_file() {
        return Err(AgentError::ConfigNotFound(config.to_path_buf()));
    }
    let name = tunnel_name_from_config(config)?;
    let exe = find_wireguard_exe(wireguard_exe)?;
    warn_if_untrusted_wireguard_path(&exe, wireguard_exe.is_some());

    let mut cmd = Command::new(&exe);
    cmd.arg("/installtunnelservice").arg(config);
    run("wireguard.exe /installtunnelservice", cmd)?;

    if let Err(e) = restrict_config_permissions(config) {
        eprintln!(
            "warning: tunnel is up, but could not lock down permissions on '{}' ({e}). \
This file contains your private key in plaintext and may still be readable by other local \
accounts - consider restricting it manually (see docs/SECURITY.md).",
            config.display()
        );
    }

    Ok(name)
}

/// Stops and uninstalls the tunnel service by name
/// (`wireguard.exe /uninstalltunnelservice <name>`). Requires admin.
pub fn stop_and_uninstall(name: &str, wireguard_exe: Option<&Path>) -> Result<()> {
    let exe = find_wireguard_exe(wireguard_exe)?;
    warn_if_untrusted_wireguard_path(&exe, wireguard_exe.is_some());
    let mut cmd = Command::new(&exe);
    cmd.arg("/uninstalltunnelservice").arg(name);
    run("wireguard.exe /uninstalltunnelservice", cmd)?;
    Ok(())
}

/// Service state, decoded from the numeric SCM state code so this works
/// regardless of Windows UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Stopped,
    StartPending,
    StopPending,
    Running,
    ContinuePending,
    PausePending,
    Paused,
    Unknown(u32),
}

impl ServiceState {
    fn from_code(code: u32) -> Self {
        match code {
            1 => Self::Stopped,
            2 => Self::StartPending,
            3 => Self::StopPending,
            4 => Self::Running,
            5 => Self::ContinuePending,
            6 => Self::PausePending,
            7 => Self::Paused,
            other => Self::Unknown(other),
        }
    }
}

impl std::fmt::Display for ServiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Stopped => "stopped",
            Self::StartPending => "starting",
            Self::StopPending => "stopping",
            Self::Running => "running",
            Self::ContinuePending => "resuming",
            Self::PausePending => "pausing",
            Self::Paused => "paused",
            Self::Unknown(_) => "unknown",
        };
        write!(f, "{s}")
    }
}

/// Parses the numeric state code out of `sc.exe query` output, independent
/// of the localized state name sc.exe prints alongside it.
pub fn parse_sc_query_state(output: &str) -> Option<ServiceState> {
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("STATE") {
            // Typical line: "STATE              : 4  RUNNING"
            let after_colon = rest.split_once(':')?.1.trim();
            let code_str: String = after_colon
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(code) = code_str.parse::<u32>() {
                return Some(ServiceState::from_code(code));
            }
        }
    }
    None
}

/// Queries the tunnel's Windows service state via `sc.exe query`.
pub fn status(name: &str) -> Result<ServiceState> {
    let service = format!("{SERVICE_PREFIX}{name}");
    let mut cmd = Command::new(crate::syspath::system32("sc.exe"));
    cmd.arg("query").arg(&service);

    let output = cmd.output().map_err(|source| AgentError::Spawn {
        command: "sc.exe query".to_string(),
        source,
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_sc_query_state(&stdout).ok_or_else(|| AgentError::StatusUnknown(name.to_string()))
}

/// `TunnelManager` implementation backed by the real `wireguard.exe`/
/// `sc.exe` functions above. `routing::selector` depends on the
/// `TunnelManager` trait, not this struct directly, so tests can swap in
/// a fake instead of needing a real WireGuard install + Administrator.
#[derive(Debug, Clone, Default)]
pub struct WireGuardTunnel {
    pub wireguard_exe: Option<PathBuf>,
}

impl TunnelManager for WireGuardTunnel {
    fn connect(&self, config: &Path) -> Result<String> {
        install_and_start(config, self.wireguard_exe.as_deref())
    }

    fn disconnect(&self, name: &str) -> Result<()> {
        stop_and_uninstall(name, self.wireguard_exe.as_deref())
    }

    fn status(&self, name: &str) -> Result<ServiceState> {
        status(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tunnel_name_from_config_strips_conf_extension() {
        let path = Path::new(r"C:\Users\me\wg-client.conf");
        assert_eq!(tunnel_name_from_config(path).unwrap(), "wg-client");
    }

    #[test]
    fn tunnel_name_from_config_rejects_wrong_extension() {
        let path = Path::new(r"C:\Users\me\wg-client.txt");
        assert!(tunnel_name_from_config(path).is_err());
    }

    #[test]
    fn parse_sc_query_state_reads_running() {
        let sample = "\
SERVICE_NAME: WireGuardTunnel$wg-client
        TYPE               : 10  WIN32_OWN_PROCESS
        STATE              : 4  RUNNING
                                (STOPPABLE, NOT_PAUSABLE, ACCEPTS_SHUTDOWN)
        WIN32_EXIT_CODE    : 0  (0x0)
";
        assert_eq!(parse_sc_query_state(sample), Some(ServiceState::Running));
    }

    #[test]
    fn parse_sc_query_state_reads_stopped() {
        let sample = "        STATE              : 1  STOPPED";
        assert_eq!(parse_sc_query_state(sample), Some(ServiceState::Stopped));
    }

    #[test]
    fn parse_sc_query_state_handles_missing_state_line() {
        assert_eq!(parse_sc_query_state("no state here"), None);
    }

    #[test]
    fn find_wireguard_exe_errors_when_override_missing() {
        let bogus = Path::new(r"C:\definitely\not\here\wireguard.exe");
        assert!(find_wireguard_exe(Some(bogus)).is_err());
    }
}
