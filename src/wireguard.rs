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

    Err(AgentError::WireGuardNotFound(PathBuf::from("wireguard.exe")))
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

/// Installs the tunnel service from `config` and starts it
/// (`wireguard.exe /installtunnelservice <config>`). Requires admin.
pub fn install_and_start(config: &Path, wireguard_exe: Option<&Path>) -> Result<String> {
    if !config.is_file() {
        return Err(AgentError::ConfigNotFound(config.to_path_buf()));
    }
    let name = tunnel_name_from_config(config)?;
    let exe = find_wireguard_exe(wireguard_exe)?;

    let mut cmd = Command::new(&exe);
    cmd.arg("/installtunnelservice").arg(config);
    run("wireguard.exe /installtunnelservice", cmd)?;

    Ok(name)
}

/// Stops and uninstalls the tunnel service by name
/// (`wireguard.exe /uninstalltunnelservice <name>`). Requires admin.
pub fn stop_and_uninstall(name: &str, wireguard_exe: Option<&Path>) -> Result<()> {
    let exe = find_wireguard_exe(wireguard_exe)?;
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
    let mut cmd = Command::new("sc.exe");
    cmd.arg("query").arg(&service);

    let output = cmd.output().map_err(|source| AgentError::Spawn {
        command: "sc.exe query".to_string(),
        source,
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_sc_query_state(&stdout).ok_or_else(|| AgentError::StatusUnknown(name.to_string()))
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
