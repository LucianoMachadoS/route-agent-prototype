//! Implements the `bench` subcommand: ping a host directly and through the
//! tunnel, compute comparable stats, print them, and append a row to a CSV
//! log — operationalizing the manual worksheet in `docs/experiments.md`.

use std::path::Path;
use std::process::Command;

use crate::error::{AgentError, Result};
use crate::telemetry::{append_line, csv_field, fmt_opt};
use crate::traits::{Measurement, Probe};

/// Round-trip samples and loss info from one `ping.exe` run.
#[derive(Debug, Clone)]
pub struct PingStats {
    pub sent: u32,
    pub latencies_ms: Vec<f64>,
}

impl PingStats {
    pub fn received(&self) -> u32 {
        self.latencies_ms.len() as u32
    }

    pub fn loss_pct(&self) -> f64 {
        if self.sent == 0 {
            return 0.0;
        }
        let lost = self.sent.saturating_sub(self.received());
        (lost as f64 / self.sent as f64) * 100.0
    }

    /// Nearest-rank percentile (0-100) over the latency samples.
    pub fn percentile(&self, p: f64) -> Option<f64> {
        if self.latencies_ms.is_empty() {
            return None;
        }
        let mut sorted = self.latencies_ms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let rank = (p / 100.0 * (sorted.len() as f64 - 1.0)).round() as usize;
        Some(sorted[rank.min(sorted.len() - 1)])
    }

    pub fn median(&self) -> Option<f64> {
        self.percentile(50.0)
    }

    /// Mean absolute difference between consecutive samples - a simple,
    /// commonly used jitter estimate (same spirit as RFC 3550's, without
    /// the exponential smoothing).
    pub fn jitter(&self) -> Option<f64> {
        if self.latencies_ms.len() < 2 {
            return None;
        }
        let diffs: Vec<f64> = self
            .latencies_ms
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .collect();
        Some(diffs.iter().sum::<f64>() / diffs.len() as f64)
    }
}

/// Extracts round-trip times from `ping.exe` output. Tolerant of both
/// English ("time=23ms", "time<1ms") and Portuguese ("tempo=23ms") output:
/// it only looks for a number immediately before "ms" after the last '='
/// or '<' on each line, so it does not depend on any other wording.
///
/// Only lines containing "bytes=" are considered - these are the per-probe
/// reply lines ("Reply from ...: bytes=32 time=5ms ..." / "Resposta de
/// ...: bytes=32 tempo=5ms ..."). This deliberately excludes the trailing
/// "Minimum = Xms, Maximum = Yms, Average = Zms" statistics line, which
/// would otherwise be misread as one extra reply.
pub fn parse_ping_output(output: &str) -> Vec<f64> {
    let mut latencies = Vec::new();
    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("bytes=") {
            continue;
        }
        if let Some(ms_pos) = lower.find("ms") {
            let prefix = &line[..ms_pos];
            if let Some(sep) = prefix.rfind(|c: char| c == '=' || c == '<') {
                let num_str: String = prefix[sep + 1..]
                    .chars()
                    .filter(|c| c.is_ascii_digit() || *c == '.')
                    .collect();
                if let Ok(v) = num_str.parse::<f64>() {
                    latencies.push(v);
                }
            }
        }
    }
    latencies
}

/// True if `ping.exe`'s output looks like it actually attempted at least
/// one probe (a header line plus at least one reply/timeout line, i.e.
/// more than one non-empty line), as opposed to an immediate DNS/usage
/// failure, which prints exactly one line and exits before sending
/// anything. Deliberately locale-independent: it counts lines, it
/// doesn't match words.
fn looks_like_a_real_attempt(stdout: &str) -> bool {
    stdout.lines().filter(|l| !l.trim().is_empty()).count() > 1
}

/// Runs `ping.exe -n <count> <host>` and parses the round-trip times.
///
/// A target that is simply unreachable (every probe times out) is a
/// **valid measurement** - `loss_pct: 100.0`, not an error - because
/// `monitor`'s hard-failover and `direct`-candidate logic both depend on
/// seeing that result to act on it. `ping.exe` also exits non-zero for
/// this case (same as for any partial loss), so exit status alone can't
/// tell "target is down" apart from "ping.exe never actually attempted a
/// probe" (bad host/flag, DNS resolution failure) - a real tool-usage
/// error, which *is* worth surfacing. The distinguishing, locale-
/// independent signal is line count: a DNS/usage failure prints exactly
/// one line and exits immediately, while a real attempt - lost or not -
/// always prints a header plus one line per probe (and usually a stats
/// trailer), regardless of what language those lines are written in.
///
/// (An earlier version of this function treated *every* "zero replies +
/// non-zero exit" as a hard error, which silently broke hard-failover:
/// a fully-down relay would error out of the probing loop instead of
/// producing a 100%-loss measurement, so it would simply vanish from
/// `monitor`'s round instead of triggering failover. Found via a live
/// smoke test, not a unit test - see CLAUDE.md.)
pub fn ping(host: &str, count: u32) -> Result<PingStats> {
    let mut cmd = crate::syspath::hidden_command(crate::syspath::system32("ping.exe"));
    cmd.arg("-n").arg(count.to_string()).arg(host);
    let output = cmd.output().map_err(|source| AgentError::Spawn {
        command: format!("ping.exe -n {count} {host}"),
        source,
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let latencies_ms = parse_ping_output(&stdout);

    if latencies_ms.is_empty() && !looks_like_a_real_attempt(&stdout) {
        return Err(AgentError::CommandFailed {
            command: format!("ping.exe -n {count} {host}"),
            status: output.status.code().unwrap_or(-1),
            stderr: {
                let combined = format!("{}{}", String::from_utf8_lossy(&output.stderr), stdout);
                combined.trim().to_string()
            },
        });
    }

    Ok(PingStats {
        sent: count,
        latencies_ms,
    })
}

/// Current UTC time as `YYYY-MM-DDTHH:MM:SSZ`, via PowerShell with an
/// explicit format string so it doesn't depend on system locale and we
/// don't need to add a date-handling crate just for this.
pub(crate) fn utc_timestamp() -> Result<String> {
    let mut cmd = Command::new(crate::syspath::windows_powershell());
    cmd.args([
        "-NoProfile",
        "-Command",
        "[DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')",
    ]);
    let output = cmd.output().map_err(|source| AgentError::Spawn {
        command: "powershell.exe (timestamp)".to_string(),
        source,
    })?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub struct RunRow {
    pub timestamp_utc: String,
    pub note: String,
    pub probe_count: u32,
    pub direct_host: String,
    pub direct: PingStats,
    pub tunnel_host: String,
    pub tunnel: PingStats,
}

const CSV_HEADER: &str = "timestamp_utc,note,probe_count,direct_host,direct_median_ms,direct_p95_ms,direct_p99_ms,direct_jitter_ms,direct_loss_pct,tunnel_host,tunnel_median_ms,tunnel_p95_ms,tunnel_p99_ms,tunnel_jitter_ms,tunnel_loss_pct";

/// Appends one comparison row to `csv_path`, creating it with a header if
/// it doesn't exist yet. The actual "open, write header once, append
/// line" logic lives in `telemetry::append_line`, shared with `monitor`'s
/// decision log.
pub fn append_run_csv(csv_path: &Path, row: &RunRow) -> Result<()> {
    let line = format!(
        "{},{},{},{},{},{},{},{},{:.1},{},{},{},{},{},{:.1}",
        csv_field(&row.timestamp_utc),
        csv_field(&row.note),
        row.probe_count,
        csv_field(&row.direct_host),
        fmt_opt(row.direct.median()),
        fmt_opt(row.direct.percentile(95.0)),
        fmt_opt(row.direct.percentile(99.0)),
        fmt_opt(row.direct.jitter()),
        row.direct.loss_pct(),
        csv_field(&row.tunnel_host),
        fmt_opt(row.tunnel.median()),
        fmt_opt(row.tunnel.percentile(95.0)),
        fmt_opt(row.tunnel.percentile(99.0)),
        fmt_opt(row.tunnel.jitter()),
        row.tunnel.loss_pct(),
    );
    append_line(csv_path, CSV_HEADER, &line)
}

fn print_comparison(label: &str, host: &str, stats: &PingStats) {
    println!(
        "  {:<8} {:<24} median {:>6} ms   p95 {:>6} ms   p99 {:>6} ms   jitter {:>6} ms   loss {:>5.1}%  ({}/{})",
        label,
        host,
        fmt_opt(stats.median()),
        fmt_opt(stats.percentile(95.0)),
        fmt_opt(stats.percentile(99.0)),
        fmt_opt(stats.jitter()),
        stats.loss_pct(),
        stats.received(),
        stats.sent,
    );
}

/// Runs the direct-vs-tunnel comparison described in `docs/experiments.md`
/// and appends a row to `csv_path`.
pub fn run(
    direct_host: &str,
    tunnel_host: &str,
    count: u32,
    note: &str,
    csv_path: &Path,
) -> Result<()> {
    println!("Pinging direct host ({direct_host}, {count} probes)...");
    let direct = ping(direct_host, count)?;
    println!("Pinging tunnel host ({tunnel_host}, {count} probes)...");
    let tunnel = ping(tunnel_host, count)?;

    println!();
    print_comparison("direct", direct_host, &direct);
    print_comparison("tunnel", tunnel_host, &tunnel);

    if let (Some(d), Some(t)) = (direct.median(), tunnel.median()) {
        println!();
        if t < d {
            println!(
                "  -> tunnel is {:.1} ms faster (median) than direct.",
                d - t
            );
        } else if t > d {
            println!(
                "  -> direct is {:.1} ms faster (median) than tunnel.",
                t - d
            );
        } else {
            println!("  -> direct and tunnel medians are equal.");
        }
    }

    let row = RunRow {
        timestamp_utc: utc_timestamp()?,
        note: note.to_string(),
        probe_count: count,
        direct_host: direct_host.to_string(),
        direct,
        tunnel_host: tunnel_host.to_string(),
        tunnel,
    };
    append_run_csv(csv_path, &row)?;
    println!();
    println!("Appended run to {}", csv_path.display());
    Ok(())
}

impl Measurement {
    pub fn from_ping_stats(stats: &PingStats) -> Self {
        Measurement {
            rtt_p95_ms: stats.percentile(95.0),
            jitter_ms: stats.jitter(),
            loss_pct: stats.loss_pct(),
            available: stats.received() > 0,
        }
    }
}

/// `Probe` implementation backed by the real `ping()` above. `monitor`
/// depends on the `Probe` trait, not this struct directly, so its
/// selector logic can be tested with a fake instead of sending real ICMP
/// traffic.
pub struct PingProbe;

impl Probe for PingProbe {
    fn measure(&self, target: &str, count: u32) -> Result<Measurement> {
        let stats = ping(target, count)?;
        Ok(Measurement::from_ping_stats(&stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_english_ping_output() {
        let sample = "\
Pinging 1.1.1.1 with 32 bytes of data:
Reply from 1.1.1.1: bytes=32 time=23ms TTL=58
Reply from 1.1.1.1: bytes=32 time=19ms TTL=58
Request timed out.
Reply from 1.1.1.1: bytes=32 time<1ms TTL=58
";
        let v = parse_ping_output(sample);
        assert_eq!(v, vec![23.0, 19.0, 1.0]);
    }

    #[test]
    fn parses_portuguese_ping_output() {
        let sample = "Resposta de 1.1.1.1: bytes=32 tempo=15ms TTL=58";
        let v = parse_ping_output(sample);
        assert_eq!(v, vec![15.0]);
    }

    /// Regression test: a real `ping.exe -n 3` run ends with a one-line
    /// "Minimum = Xms, Maximum = Yms, Average = Zms" summary that contains
    /// "ms" too. It must not be counted as a 4th reply.
    #[test]
    fn ignores_the_trailing_statistics_line() {
        let sample = "\
Pinging 1.1.1.1 with 32 bytes of data:
Reply from 1.1.1.1: bytes=32 time=5ms TTL=58
Reply from 1.1.1.1: bytes=32 time=8ms TTL=58
Reply from 1.1.1.1: bytes=32 time=4ms TTL=58

Ping statistics for 1.1.1.1:
    Packets: Sent = 3, Received = 3, Lost = 0 (0% loss),
Approximate round trip times in milli-seconds:
    Minimum = 4ms, Maximum = 8ms, Average = 5ms
";
        let v = parse_ping_output(sample);
        assert_eq!(v, vec![5.0, 8.0, 4.0]);
    }

    /// Regression test for the bug found while smoke-testing `monitor`:
    /// a target that's simply unreachable (every probe times out) must
    /// still look like "a real attempt" so `ping()` reports it as a
    /// 100%-loss measurement, not a hard error - see `ping()`'s doc
    /// comment for why that distinction matters for hard-failover.
    #[test]
    fn a_fully_timed_out_attempt_looks_like_a_real_attempt_english() {
        let sample = "\
Pinging 203.0.113.1 with 32 bytes of data:
Request timed out.
Request timed out.
Request timed out.

Ping statistics for 203.0.113.1:
    Packets: Sent = 3, Received = 0, Lost = 3 (100% loss),
";
        assert!(looks_like_a_real_attempt(sample));
        assert_eq!(parse_ping_output(sample), Vec::<f64>::new());
    }

    #[test]
    fn a_fully_timed_out_attempt_looks_like_a_real_attempt_portuguese() {
        let sample = "\
Disparando 203.0.113.1 com 32 bytes de dados:
Esgotado o tempo limite do pedido.
Esgotado o tempo limite do pedido.
Esgotado o tempo limite do pedido.
Estatisticas do Ping para 203.0.113.1:
    Pacotes: Enviados = 3, Recebidos = 0, Perdidos = 3 (100% de perda),
";
        assert!(looks_like_a_real_attempt(sample));
    }

    #[test]
    fn an_immediate_dns_failure_does_not_look_like_a_real_attempt() {
        let sample =
            "Ping request could not find host no.such.host. Please check the name and try again.\n";
        assert!(!looks_like_a_real_attempt(sample));
    }

    #[test]
    fn stats_percentiles_and_jitter() {
        let stats = PingStats {
            sent: 5,
            latencies_ms: vec![10.0, 12.0, 11.0, 50.0, 13.0],
        };
        assert_eq!(stats.received(), 5);
        assert_eq!(stats.loss_pct(), 0.0);
        assert!(stats.median().is_some());
        assert!(stats.percentile(99.0).unwrap() >= stats.median().unwrap());
    }

    #[test]
    fn loss_pct_accounts_for_dropped_probes() {
        let stats = PingStats {
            sent: 10,
            latencies_ms: vec![10.0, 11.0],
        };
        assert_eq!(stats.loss_pct(), 80.0);
    }

    #[test]
    fn jitter_is_none_for_single_sample() {
        let stats = PingStats {
            sent: 1,
            latencies_ms: vec![10.0],
        };
        assert_eq!(stats.jitter(), None);
    }

    #[test]
    fn append_run_csv_writes_header_once_and_quotes_note() {
        let dir = tempdir().unwrap();
        let csv_path = dir.path().join("runs.csv");
        let row = RunRow {
            timestamp_utc: "2026-01-01T00:00:00Z".to_string(),
            note: "relay=fra1,isp=vivo-fibra".to_string(),
            probe_count: 5,
            direct_host: "1.1.1.1".to_string(),
            direct: PingStats {
                sent: 5,
                latencies_ms: vec![10.0, 11.0, 12.0, 13.0, 14.0],
            },
            tunnel_host: "10.66.0.1".to_string(),
            tunnel: PingStats {
                sent: 5,
                latencies_ms: vec![5.0, 6.0, 7.0, 8.0, 9.0],
            },
        };
        append_run_csv(&csv_path, &row).unwrap();
        append_run_csv(&csv_path, &row).unwrap();

        let content = std::fs::read_to_string(&csv_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 3, "expected 1 header + 2 data rows");
        assert_eq!(lines[0], CSV_HEADER);
        assert!(lines[1].contains("\"relay=fra1,isp=vivo-fibra\""));
        assert!(lines[1].contains("1.1.1.1"));
    }
}
