//! `monitor` subcommand: continuously probes all configured relay
//! candidates plus the direct path, scores them, and (unless
//! `--dry-run`) switches the active WireGuard tunnel to whichever
//! candidate the selector picks - including the direct path itself
//! (`relays::DIRECT_CANDIDATE`), so a relay that's worse than just
//! staying on the direct connection doesn't win only because it's the
//! best of the configured relays. This is Phase 3 of the project
//! roadmap - route selection and failover - on top of the Phase 1/2
//! pieces (`bench`, `connect`/`disconnect`) that already existed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use crate::bench::{self, PingProbe};
use crate::error::Result;
use crate::relays;
use crate::routing::selector::{apply_decision, Decision, Selector, SelectorConfig};
use crate::telemetry::{append_line, csv_field};
use crate::traits::Probe;
use crate::wireguard::WireGuardTunnel;

const DECISIONS_HEADER: &str = "timestamp_utc,round,relay,score,chosen,switched,reason";

#[allow(clippy::too_many_arguments)]
pub fn run(
    relays_path: &Path,
    direct_host: &str,
    probe_count: u32,
    interval_secs: u64,
    rounds: Option<u32>,
    dry_run: bool,
    decisions_csv: &Path,
    wireguard_exe: Option<PathBuf>,
) -> Result<()> {
    let relays_file = relays::load(relays_path)?;
    let probe = PingProbe;
    let tunnel = WireGuardTunnel { wireguard_exe };
    let mut selector = Selector::new(SelectorConfig::default());
    let mut active_tunnel_name: Option<String> = None;

    println!(
        "monitor: {} relay candidate(s) vs. direct host {direct_host}{}",
        relays_file.relays.len(),
        if dry_run {
            " (dry-run: not actually switching tunnels)"
        } else {
            ""
        }
    );

    let mut round_no: u32 = 0;
    loop {
        round_no += 1;
        let mut round = HashMap::new();
        for relay in &relays_file.relays {
            match probe.measure(&relay.tunnel_host, probe_count) {
                Ok(m) => {
                    round.insert(relay.name.clone(), m);
                }
                Err(e) => eprintln!(
                    "warning: probing relay '{}' ({}) failed: {e}",
                    relay.name, relay.tunnel_host
                ),
            }
        }

        match probe.measure(direct_host, probe_count) {
            Ok(direct) => {
                println!(
                    "[round {round_no}] direct({direct_host}): loss={:.1}% jitter={}",
                    direct.loss_pct,
                    direct
                        .jitter_ms
                        .map(|j| format!("{j:.1}ms"))
                        .unwrap_or_else(|| "-".to_string())
                );
                // The direct path is a candidate like any relay (named
                // relays::DIRECT_CANDIDATE), so a relay that's worse than
                // just staying on the direct connection doesn't win only
                // because it's the best of the configured relays.
                round.insert(relays::DIRECT_CANDIDATE.to_string(), direct);
            }
            Err(e) => eprintln!("warning: probing direct host '{direct_host}' failed: {e}"),
        }

        let decision = selector.evaluate(&round, &relays_file.scoring);
        println!(
            "[round {round_no}] chosen={} switched={} reason={}",
            decision.chosen, !decision.keep_current, decision.reason
        );

        if !dry_run {
            apply_decision(
                &decision,
                &relays_file.relays,
                &tunnel,
                &mut active_tunnel_name,
            )?;
        }

        log_decision(decisions_csv, round_no, &decision)?;

        if rounds.map(|limit| round_no >= limit).unwrap_or(false) {
            break;
        }
        thread::sleep(Duration::from_secs(interval_secs));
    }

    Ok(())
}

fn log_decision(path: &Path, round: u32, decision: &Decision) -> Result<()> {
    let ts = bench::utc_timestamp().unwrap_or_else(|_| "unknown".to_string());
    if decision.scores.is_empty() {
        let line = format!(
            "{},{},,,{},{},{}",
            csv_field(&ts),
            round,
            csv_field(&decision.chosen),
            !decision.keep_current,
            csv_field(&decision.reason)
        );
        return append_line(path, DECISIONS_HEADER, &line);
    }
    for (relay, score) in &decision.scores {
        let line = format!(
            "{},{},{},{:.4},{},{},{}",
            csv_field(&ts),
            round,
            csv_field(relay),
            score,
            csv_field(&decision.chosen),
            !decision.keep_current,
            csv_field(&decision.reason)
        );
        append_line(path, DECISIONS_HEADER, &line)?;
    }
    Ok(())
}
