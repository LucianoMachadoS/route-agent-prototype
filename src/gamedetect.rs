//! `detect-remote` subcommand: finds which remote IP(s) a running game
//! process currently has active UDP endpoints toward, via Windows's
//! `Get-NetUDPEndpoint` - the same manual trick documented in README for
//! finding a game server's current IP (CS2 shows this via `net_graph`/
//! `status` in its own console; Valorant doesn't expose it any other way).
//! Only useful while a match is actually in progress - there's nothing
//! meaningful to detect from a menu/lobby.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::error::{AgentError, Result};

/// Exit code the detection script uses for "no such process right now".
const NOT_RUNNING_EXIT: i32 = 3;

/// A normal run takes ~1 s; this only exists so a wedged PowerShell/CIM
/// call can't stall `watch-games` forever.
const DETECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// Queries Windows for the remote IPv4/IPv6 addresses a running process
/// currently has active UDP endpoints toward, filtering out addresses
/// that are never a real remote peer (unspecified, loopback, link-local,
/// multicast, broadcast). Returns an empty list (not an error) if the
/// process is running but has no such endpoints right now - that's the
/// normal state outside of an active match.
pub fn detect_remote_ips(process_name: &str) -> Result<Vec<IpAddr>> {
    let name = process_name.trim_end_matches(".exe").replace('\'', "''");
    // `Select-Object -ExpandProperty RemoteAddress` looked right but was
    // wrong: a UDP socket that's bound but never called connect() (very
    // common - UDP is connectionless, and plenty of game traffic goes
    // out via sendto() on an unconnected socket) has no RemoteAddress at
    // all, and -ExpandProperty *throws* on the first object missing the
    // property, aborting the whole pipeline with a non-zero exit -
    // confirmed live, 100% reproducible, every single round while CS2
    // was actually running in a real match. Plain `$_.RemoteAddress`
    // property access just evaluates to $null for those instead of
    // erroring, which Rust-side parsing already skips as unparseable.
    //
    // "Game isn't running" is the normal state for most of watch-games'
    // rounds, so it's signalled with a dedicated exit code instead of
    // letting Get-Process -ErrorAction Stop throw: that used to dump a
    // multi-line PowerShell exception into watch-game.log every 20s per
    // game, burying the lines that actually matter.
    let script = format!(
        "$p = @(Get-Process -Name '{name}' -ErrorAction SilentlyContinue); \
         if ($p.Count -eq 0) {{ exit {NOT_RUNNING_EXIT} }}; \
         $p | ForEach-Object {{ \
         Get-NetUDPEndpoint -OwningProcess $_.Id -ErrorAction SilentlyContinue }} | \
         ForEach-Object {{ $_.RemoteAddress }}"
    );
    let mut cmd = crate::syspath::hidden_command(crate::syspath::windows_powershell());
    cmd.args(["-NoProfile", "-Command", &script]);
    let output = crate::syspath::output_with_timeout(cmd, DETECT_TIMEOUT).map_err(|source| {
        if source.kind() == std::io::ErrorKind::TimedOut {
            AgentError::Timeout {
                command: "powershell.exe (detect-remote)".to_string(),
                secs: DETECT_TIMEOUT.as_secs(),
            }
        } else {
            AgentError::Spawn {
                command: "powershell.exe (detect-remote)".to_string(),
                source,
            }
        }
    })?;

    if output.status.code() == Some(NOT_RUNNING_EXIT) {
        return Err(AgentError::ProcessNotRunning(process_name.to_string()));
    }
    if !output.status.success() {
        return Err(AgentError::CommandFailed {
            command: format!("Get-NetUDPEndpoint for process '{process_name}'"),
            status: output.status.code().unwrap_or(-1),
            stderr: crate::console::decode(&output.stderr).trim().to_string(),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut ips: Vec<IpAddr> = stdout
        .lines()
        .filter_map(|line| line.trim().parse::<IpAddr>().ok())
        .filter(is_plausible_remote_peer)
        .collect();
    ips.sort();
    ips.dedup();
    Ok(ips)
}

/// True for addresses that could plausibly be a real remote peer/server:
/// excludes unspecified, loopback, link-local, multicast and broadcast
/// addresses, which `Get-NetUDPEndpoint` can legitimately return (DNS,
/// mDNS, DHCP, etc.) but never indicate an actual opponent's/server's IP.
fn is_plausible_remote_peer(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_plausible_v4(v4),
        IpAddr::V6(v6) => is_plausible_v6(v6),
    }
}

fn is_plausible_v4(v4: &Ipv4Addr) -> bool {
    !v4.is_unspecified()
        && !v4.is_loopback()
        && !v4.is_link_local()
        && !v4.is_multicast()
        && !v4.is_broadcast()
}

fn is_plausible_v6(v6: &Ipv6Addr) -> bool {
    // Ipv6Addr::is_unicast_link_local() isn't stable on every toolchain
    // this project might build with, so check the fe80::/10 prefix by
    // hand instead of depending on it.
    let is_link_local = (v6.segments()[0] & 0xffc0) == 0xfe80;
    !v6.is_unspecified() && !v6.is_loopback() && !v6.is_multicast() && !is_link_local
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_out_unspecified_loopback_linklocal_multicast_broadcast_v4() {
        assert!(!is_plausible_remote_peer(&"0.0.0.0".parse().unwrap()));
        assert!(!is_plausible_remote_peer(&"127.0.0.1".parse().unwrap()));
        assert!(!is_plausible_remote_peer(&"169.254.1.1".parse().unwrap()));
        assert!(!is_plausible_remote_peer(&"224.0.0.1".parse().unwrap()));
        assert!(!is_plausible_remote_peer(
            &"255.255.255.255".parse().unwrap()
        ));
    }

    #[test]
    fn accepts_a_real_public_v4_address() {
        assert!(is_plausible_remote_peer(&"155.133.227.35".parse().unwrap()));
    }

    #[test]
    fn filters_out_unspecified_loopback_linklocal_multicast_v6() {
        assert!(!is_plausible_remote_peer(&"::".parse().unwrap()));
        assert!(!is_plausible_remote_peer(&"::1".parse().unwrap()));
        assert!(!is_plausible_remote_peer(&"fe80::1".parse().unwrap()));
        assert!(!is_plausible_remote_peer(&"ff02::1".parse().unwrap()));
    }

    #[test]
    fn accepts_a_real_public_v6_address() {
        assert!(is_plausible_remote_peer(
            &"2001:4860:4860::8888".parse().unwrap()
        ));
    }
}
