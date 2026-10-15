//! route-agent: controller for an official WireGuard for Windows tunnel.
//!
//! Subcommands:
//!   connect    - install & start the tunnel as a Windows service from a config
//!   disconnect - stop & uninstall the tunnel's Windows service
//!   status      - report whether the tunnel's Windows service is running
//!   health-check - status + an actual ping to the relay's tunnel address
//!   bench       - compare direct vs. tunnel latency/jitter/loss and log it
//!   monitor    - Phase 3: probe multiple relay candidates, score them, and
//!                (unless --dry-run) switch the active tunnel automatically
//!
//! See README.md for setup (you need your own relay) and docs/experiments.md
//! / docs/BENCHMARK_PLAN.md for the measurement protocols `bench`/`monitor`
//! automate. Everything here is thin CLI glue - the actual logic lives in
//! the `route_agent` library (src/lib.rs), so `tests/*.rs` can exercise it
//! without going through this binary.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use route_agent::{bench, health, monitor, wireguard};

#[derive(Parser)]
#[command(
    name = "route-agent",
    version,
    about = "Controller for an official WireGuard for Windows tunnel: connect/disconnect it, measure direct-vs-tunnel latency, and (via `monitor`) automatically pick the best of several relays."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Install and start the tunnel as a Windows service from a WireGuard config file. Requires admin.
    Connect {
        /// Path to a filled-in WireGuard client .conf (see examples/client.conf.template).
        #[arg(long)]
        config: PathBuf,
        /// Path to wireguard.exe. Falls back to WIREGUARD_EXE, then standard
        /// install locations, then PATH.
        #[arg(long, env = "WIREGUARD_EXE")]
        wireguard: Option<PathBuf>,
    },
    /// Stop and uninstall the tunnel's Windows service. Requires admin.
    Disconnect {
        /// Tunnel name (the config file's stem, e.g. "route-client" for route-client.conf).
        #[arg(long)]
        name: String,
        /// Path to wireguard.exe. Falls back to WIREGUARD_EXE, then standard
        /// install locations, then PATH.
        #[arg(long, env = "WIREGUARD_EXE")]
        wireguard: Option<PathBuf>,
    },
    /// Show whether the tunnel's Windows service is running.
    Status {
        /// Tunnel name (the config file's stem, e.g. "route-client" for route-client.conf).
        #[arg(long)]
        name: String,
    },
    /// Check whether the tunnel is actually passing traffic, not just
    /// whether its Windows service is running: pings the relay's tunnel
    /// address on top of the service-state check `status` already does.
    /// Exits non-zero if either check fails, so it's scriptable.
    HealthCheck {
        /// Tunnel name (the config file's stem, e.g. "route-client" for route-client.conf).
        #[arg(long)]
        name: String,
        /// The relay's tunnel address to ping (e.g. 10.66.0.1).
        #[arg(long)]
        tunnel_host: String,
        /// ICMP probes to send.
        #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u32).range(1..=1000))]
        count: u32,
    },
    /// Compare latency/jitter/loss direct vs. through the tunnel, and log the result.
    Bench {
        /// Host/IP to ping directly (e.g. the game server).
        #[arg(long)]
        direct_host: String,
        /// Host/IP to ping through the tunnel (default: the relay's tunnel address).
        #[arg(long, default_value = "10.66.0.1")]
        tunnel_host: String,
        /// Number of ICMP probes per host (1-1000, to avoid accidentally
        /// flooding a host you don't control).
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u32).range(1..=1000))]
        count: u32,
        /// Free-text context for this run (relay region, ISP, etc.) - stored in the CSV log.
        #[arg(long, default_value = "")]
        note: String,
        /// Where to append the run log.
        #[arg(long, default_value = "docs/runs.csv")]
        csv: PathBuf,
    },
    /// Continuously probe multiple relay candidates (see relays.toml.template)
    /// plus a direct host, score them, and switch the active tunnel to
    /// whichever relay wins - unless --dry-run, which only logs the decision.
    Monitor {
        /// Path to a relays.toml (see relays.toml.template).
        #[arg(long)]
        relays: PathBuf,
        /// Host/IP to ping directly, for reference (not a switch candidate).
        #[arg(long)]
        direct_host: String,
        /// ICMP probes per target per round.
        #[arg(long, default_value_t = 10, value_parser = clap::value_parser!(u32).range(1..=1000))]
        count: u32,
        /// Seconds to wait between rounds.
        #[arg(long, default_value_t = 30)]
        interval: u64,
        /// Stop after this many rounds instead of running forever (useful
        /// for smoke-testing without a real relay).
        #[arg(long)]
        rounds: Option<u32>,
        /// Evaluate and log decisions, but never actually connect/disconnect
        /// the real tunnel.
        #[arg(long)]
        dry_run: bool,
        /// Where to append the decision log.
        #[arg(long, default_value = "docs/decisions.csv")]
        csv: PathBuf,
        /// Path to wireguard.exe. Falls back to WIREGUARD_EXE, then standard
        /// install locations, then PATH.
        #[arg(long, env = "WIREGUARD_EXE")]
        wireguard: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Connect { config, wireguard } => {
            wireguard::install_and_start(&config, wireguard.as_deref()).map(|name| {
                println!(
                    "Tunnel '{name}' installed and starting. Check with: route-agent status --name {name}"
                );
            })
        }
        Commands::Disconnect { name, wireguard } => {
            wireguard::stop_and_uninstall(&name, wireguard.as_deref()).map(|_| {
                println!("Tunnel '{name}' stopped and uninstalled.");
            })
        }
        Commands::Status { name } => wireguard::status(&name).map(|state| {
            println!("Tunnel '{name}': {state}");
        }),
        Commands::HealthCheck {
            name,
            tunnel_host,
            count,
        } => {
            let tunnel = wireguard::WireGuardTunnel::default();
            let probe = bench::PingProbe;
            health::check(&tunnel, &probe, &name, &tunnel_host, count).map(|report| {
                println!(
                    "Tunnel '{}': service={} reachable={} ({:.0}% loss)",
                    report.tunnel_name,
                    report.service,
                    report.reachability.available,
                    report.reachability.loss_pct
                );
                if report.healthy() {
                    println!("-> healthy");
                } else {
                    println!("-> NOT healthy");
                    std::process::exit(1);
                }
            })
        }
        Commands::Bench {
            direct_host,
            tunnel_host,
            count,
            note,
            csv,
        } => bench::run(&direct_host, &tunnel_host, count, &note, &csv),
        Commands::Monitor {
            relays,
            direct_host,
            count,
            interval,
            rounds,
            dry_run,
            csv,
            wireguard,
        } => monitor::run(
            &relays,
            &direct_host,
            count,
            interval,
            rounds,
            dry_run,
            &csv,
            wireguard,
        ),
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
