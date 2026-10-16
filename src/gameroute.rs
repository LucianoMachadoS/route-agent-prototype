//! `add-game-route` / `watch-game`: the local half of "selective
//! application routing" (README milestone 3). Detects which remote IP(s)
//! a game process currently talks to (`gamedetect`) and adds them to a
//! WireGuard client config's `AllowedIPs`, then reconnects the tunnel so
//! the change takes effect (WireGuard for Windows has no "reload config
//! in place" - only reinstall).
//!
//! The relay side (NAT/forwarding to that destination) is **not**
//! automated here - this project doesn't manage SSH access to relays in
//! code. See docs/SECURITY.md and README for the manual procedure.

use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::error::{AgentError, Result};
use crate::gamedetect::detect_remote_ips;
use crate::games::GameProfile;
use crate::traits::TunnelManager;

#[derive(Debug, Clone, Default)]
pub struct AddGameRouteOutcome {
    pub detected: Vec<IpAddr>,
    pub added: Vec<String>,
}

/// Detects `process`'s current remote IP(s) and adds them to
/// `config_path`'s `AllowedIPs`, skipping any already covered by an
/// existing entry. Does not touch the tunnel service - call `reconnect`
/// separately if `added` comes back non-empty.
pub fn add_game_route(
    config_path: &Path,
    process: &str,
    cidr: Option<u8>,
) -> Result<AddGameRouteOutcome> {
    let detected = detect_remote_ips(process)?;
    apply_detected_ips(config_path, &detected, cidr)
}

/// Disconnects and reconnects the tunnel so an `AllowedIPs` change takes
/// effect. Requires the same Administrator elevation `connect` always
/// does.
pub fn reconnect(tunnel: &dyn TunnelManager, name: &str, config_path: &Path) -> Result<String> {
    tunnel.disconnect(name)?;
    tunnel.connect(config_path)
}

/// Repeatedly detects and adds routes for `process` every `interval`,
/// reconnecting whenever a genuinely new route was added - so the user
/// doesn't have to re-run `add-game-route` by hand every time they're
/// matched into a different server. Stops after `rounds` rounds if
/// given, otherwise runs until killed. Logs to stdout/stderr; doesn't
/// write a CSV (this isn't a measurement tool like `bench`/`monitor`).
#[allow(clippy::too_many_arguments)]
pub fn watch_game(
    config_path: &Path,
    tunnel_name: &str,
    process: &str,
    cidr: Option<u8>,
    interval_secs: u64,
    rounds: Option<u32>,
    tunnel: &dyn TunnelManager,
) -> Result<()> {
    crate::log_info!(
        "watch-game: watching '{process}' for new remote servers every {interval_secs}s"
    );
    let mut round_no: u32 = 0;
    let mut sampler = crate::selfstats::SelfSampler::new();
    loop {
        round_no += 1;
        crate::selfstats::report_periodically(&mut sampler, round_no, interval_secs);
        match add_game_route(config_path, process, cidr) {
            Ok(outcome) if outcome.detected.is_empty() => {
                crate::log_info!("[round {round_no}] no active match detected for '{process}'");
            }
            Ok(outcome) if outcome.added.is_empty() => {
                let joined = join_ips(&outcome.detected);
                crate::log_info!("[round {round_no}] server already covered ({joined})");
            }
            Ok(outcome) => {
                crate::log_info!(
                    "[round {round_no}] new route(s): {} - reconnecting",
                    outcome.added.join(", ")
                );
                match reconnect(tunnel, tunnel_name, config_path) {
                    Ok(name) => crate::log_info!("[round {round_no}] tunnel '{name}' reconnected"),
                    Err(e) => crate::log_warn!("[round {round_no}] warning: reconnect failed: {e}"),
                }
            }
            Err(e) => crate::log_warn!("[round {round_no}] warning: detection failed: {e}"),
        }

        if rounds.map(|limit| round_no >= limit).unwrap_or(false) {
            break;
        }
        thread::sleep(Duration::from_secs(interval_secs));
    }
    Ok(())
}

/// Like `watch_game`, but for every game registered in a `games.toml`
/// (see `games::load`) at once, in a single loop - the point being that
/// adding support for a future game/app is just adding a `[[game]]`
/// entry to that file, not writing new code or registering a new
/// Scheduled Task.
pub fn watch_games(
    games: &[GameProfile],
    interval_secs: u64,
    rounds: Option<u32>,
    tunnel: &dyn TunnelManager,
) -> Result<()> {
    crate::log_info!(
        "watch-games: watching {} game(s) every {interval_secs}s: {}",
        games.len(),
        games
            .iter()
            .map(|g| g.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut round_no: u32 = 0;
    let mut sampler = crate::selfstats::SelfSampler::new();
    loop {
        round_no += 1;
        crate::selfstats::report_periodically(&mut sampler, round_no, interval_secs);
        for game in games {
            run_one_game_round(round_no, game, tunnel);
        }
        if rounds.map(|limit| round_no >= limit).unwrap_or(false) {
            break;
        }
        thread::sleep(Duration::from_secs(interval_secs));
    }
    Ok(())
}

fn run_one_game_round(round_no: u32, game: &GameProfile, tunnel: &dyn TunnelManager) {
    match add_game_route(&game.config, &game.process, game.cidr) {
        Ok(outcome) if outcome.detected.is_empty() => {
            crate::log_info!("[round {round_no}] {}: no active match", game.name);
        }
        Ok(outcome) if outcome.added.is_empty() => {
            crate::log_info!(
                "[round {round_no}] {}: already covered ({})",
                game.name,
                join_ips(&outcome.detected)
            );
        }
        Ok(outcome) => {
            crate::log_info!(
                "[round {round_no}] {}: new route(s) {} - reconnecting",
                game.name,
                outcome.added.join(", ")
            );
            match reconnect(tunnel, &game.tunnel_name, &game.config) {
                Ok(name) => crate::log_info!(
                    "[round {round_no}] {}: tunnel '{name}' reconnected",
                    game.name
                ),
                Err(e) => crate::log_warn!(
                    "[round {round_no}] {}: warning: reconnect failed: {e}",
                    game.name
                ),
            }
        }
        Err(AgentError::ProcessNotRunning(_)) => {
            crate::log_info!("[round {round_no}] {}: not running", game.name);
        }
        Err(e) => crate::log_warn!(
            "[round {round_no}] {}: warning: detection failed: {e}",
            game.name
        ),
    }
}

fn join_ips(ips: &[IpAddr]) -> String {
    ips.iter()
        .map(|ip| ip.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The file-editing core of `add_game_route`, taking an already-detected
/// IP list instead of calling `gamedetect` itself - separated out so
/// it's testable with synthetic IPs and a temp file, without needing
/// Windows/PowerShell or a real game running.
fn apply_detected_ips(
    config_path: &Path,
    detected: &[IpAddr],
    cidr: Option<u8>,
) -> Result<AddGameRouteOutcome> {
    if detected.is_empty() {
        return Ok(AddGameRouteOutcome::default());
    }

    let text = std::fs::read_to_string(config_path)
        .map_err(|_| AgentError::ConfigNotFound(config_path.to_path_buf()))?;

    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let allowed_idx = lines
        .iter()
        .position(|l| {
            l.trim_start()
                .to_ascii_lowercase()
                .starts_with("allowedips")
        })
        .ok_or_else(|| AgentError::RelayConfigInvalid {
            path: config_path.to_path_buf(),
            reason: "no AllowedIPs line found".to_string(),
        })?;

    let (label, value) =
        lines[allowed_idx]
            .split_once('=')
            .ok_or_else(|| AgentError::RelayConfigInvalid {
                path: config_path.to_path_buf(),
                reason: "malformed AllowedIPs line (no '=')".to_string(),
            })?;
    let mut entries: Vec<String> = value
        .split(',')
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect();

    let mut added = Vec::new();
    for ip in detected {
        if entries.iter().any(|e| entry_covers(e, ip)) {
            continue;
        }
        let candidate = new_entry_for(*ip, cidr);
        entries.push(candidate.clone());
        added.push(candidate);
    }

    if !added.is_empty() {
        lines[allowed_idx] = format!("{} = {}", label.trim(), entries.join(", "));
        let mut new_text = lines.join("\n");
        new_text.push('\n');
        std::fs::write(config_path, new_text)?;
    }

    Ok(AddGameRouteOutcome {
        detected: detected.to_vec(),
        added,
    })
}

fn new_entry_for(ip: IpAddr, cidr: Option<u8>) -> String {
    match ip {
        IpAddr::V4(v4) => {
            let prefix = cidr.unwrap_or(32).min(32);
            format!("{}/{}", network_address(v4, prefix), prefix)
        }
        IpAddr::V6(v6) => format!("{v6}/128"),
    }
}

fn network_address(ip: Ipv4Addr, prefix: u8) -> Ipv4Addr {
    let mask: u32 = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix as u32)
    };
    Ipv4Addr::from(u32::from(ip) & mask)
}

fn parse_cidr_v4(s: &str) -> Option<(Ipv4Addr, u8)> {
    let s = s.trim();
    match s.split_once('/') {
        Some((ip_str, prefix_str)) => {
            let ip = ip_str.trim().parse::<Ipv4Addr>().ok()?;
            let prefix = prefix_str.trim().parse::<u8>().ok()?;
            (prefix <= 32).then_some((ip, prefix))
        }
        None => s.parse::<Ipv4Addr>().ok().map(|ip| (ip, 32)),
    }
}

fn ipv4_in_cidr(ip: Ipv4Addr, network: Ipv4Addr, prefix: u8) -> bool {
    let mask: u32 = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix as u32)
    };
    (u32::from(ip) & mask) == (u32::from(network) & mask)
}

/// True if `entry` (an existing `AllowedIPs` item - bare IP or CIDR)
/// already covers `ip`.
fn entry_covers(entry: &str, ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => parse_cidr_v4(entry)
            .map(|(net, prefix)| ipv4_in_cidr(*v4, net, prefix))
            .unwrap_or(false),
        IpAddr::V6(v6) => entry.trim().trim_end_matches("/128") == v6.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_conf(allowed_ips: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(
            f,
            "[Interface]\nPrivateKey = AAAA\nAddress = 10.66.0.2/24\n\n[Peer]\nPublicKey = BBBB\nEndpoint = 1.2.3.4:51820\nAllowedIPs = {allowed_ips}\nPersistentKeepalive = 25"
        )
        .unwrap();
        f
    }

    fn read_allowed_ips(path: &Path) -> String {
        let text = std::fs::read_to_string(path).unwrap();
        text.lines()
            .find(|l| {
                l.trim_start()
                    .to_ascii_lowercase()
                    .starts_with("allowedips")
            })
            .unwrap()
            .split_once('=')
            .unwrap()
            .1
            .trim()
            .to_string()
    }

    #[test]
    fn adds_a_new_ip_as_slash_32_by_default() {
        let f = write_conf("10.66.0.0/24");
        let outcome =
            apply_detected_ips(f.path(), &["155.133.227.35".parse().unwrap()], None).unwrap();
        assert_eq!(outcome.added, vec!["155.133.227.35/32".to_string()]);
        assert_eq!(
            read_allowed_ips(f.path()),
            "10.66.0.0/24, 155.133.227.35/32"
        );
    }

    #[test]
    fn expands_to_requested_cidr_network_address() {
        let f = write_conf("10.66.0.0/24");
        let outcome =
            apply_detected_ips(f.path(), &["155.133.227.35".parse().unwrap()], Some(24)).unwrap();
        assert_eq!(outcome.added, vec!["155.133.227.0/24".to_string()]);
    }

    #[test]
    fn skips_an_ip_already_covered_by_an_existing_broader_range() {
        let f = write_conf("10.66.0.0/24, 155.133.0.0/16");
        let outcome =
            apply_detected_ips(f.path(), &["155.133.227.35".parse().unwrap()], None).unwrap();
        assert!(outcome.added.is_empty());
        // File should be untouched when nothing was added.
        assert_eq!(read_allowed_ips(f.path()), "10.66.0.0/24, 155.133.0.0/16");
    }

    #[test]
    fn running_it_twice_in_a_row_is_idempotent() {
        let f = write_conf("10.66.0.0/24");
        apply_detected_ips(f.path(), &["155.133.227.35".parse().unwrap()], None).unwrap();
        let second =
            apply_detected_ips(f.path(), &["155.133.227.35".parse().unwrap()], None).unwrap();
        assert!(second.added.is_empty(), "second call should be a no-op");
    }

    #[test]
    fn empty_detected_list_is_not_an_error_and_touches_nothing() {
        let f = write_conf("10.66.0.0/24");
        let outcome = apply_detected_ips(f.path(), &[], None).unwrap();
        assert!(outcome.detected.is_empty());
        assert!(outcome.added.is_empty());
        assert_eq!(read_allowed_ips(f.path()), "10.66.0.0/24");
    }

    #[test]
    fn missing_allowed_ips_line_is_an_error() {
        let mut f = NamedTempFile::new().unwrap();
        writeln!(f, "[Interface]\nPrivateKey = AAAA\n").unwrap();
        let result = apply_detected_ips(f.path(), &["1.2.3.4".parse().unwrap()], None);
        assert!(result.is_err());
    }

    #[test]
    fn ipv4_in_cidr_basic_containment() {
        let net: Ipv4Addr = "205.185.192.0".parse().unwrap();
        assert!(ipv4_in_cidr("205.185.194.21".parse().unwrap(), net, 19));
        assert!(!ipv4_in_cidr("205.185.224.1".parse().unwrap(), net, 19));
    }
}
