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

use route_agent::{bench, gamedetect, gameroute, games, health, monitor, wireguard};

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
        /// Host/IP to ping directly. Scored as a candidate too (named
        /// "direct"): if no relay beats it, monitor disconnects any
        /// active tunnel instead of keeping a losing relay active.
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
    /// Detect the remote IP(s) a game process currently has active UDP
    /// connections to. Only useful while a match is in progress. Does
    /// not touch any config or tunnel.
    DetectRemote {
        /// Process name, with or without .exe (e.g. cs2 or
        /// VALORANT-Win64-Shipping).
        #[arg(long)]
        process: String,
    },
    /// Detect the game's current remote IP(s) and add them to a
    /// WireGuard config's AllowedIPs, then reconnect the tunnel so it
    /// takes effect. Requires the same Administrator elevation as
    /// connect/disconnect. Does not touch the relay's NAT rules - see
    /// README/SECURITY.md for that half.
    AddGameRoute {
        /// Path to the WireGuard client .conf to update.
        #[arg(long)]
        config: PathBuf,
        /// Tunnel name (the config file's stem) to disconnect/reconnect.
        #[arg(long)]
        name: String,
        /// Process name, with or without .exe.
        #[arg(long)]
        process: String,
        /// Expand each detected IP to a CIDR of this prefix length
        /// instead of an exact /32 (e.g. 24 to cover the server's /24).
        #[arg(long)]
        cidr: Option<u8>,
        /// Path to wireguard.exe. Falls back to WIREGUARD_EXE, then standard
        /// install locations, then PATH.
        #[arg(long, env = "WIREGUARD_EXE")]
        wireguard: Option<PathBuf>,
    },
    /// Repeatedly detect new remote servers for a game process and add/
    /// reconnect routes automatically, so you don't have to re-run
    /// add-game-route by hand every time you're matched into a different
    /// server. Requires Administrator (same as connect/disconnect).
    WatchGame {
        /// Path to the WireGuard client .conf to update.
        #[arg(long)]
        config: PathBuf,
        /// Tunnel name (the config file's stem) to disconnect/reconnect.
        #[arg(long)]
        name: String,
        /// Process name, with or without .exe.
        #[arg(long)]
        process: String,
        /// Expand each detected IP to a CIDR of this prefix length
        /// instead of an exact /32.
        #[arg(long)]
        cidr: Option<u8>,
        /// Seconds between detection checks.
        #[arg(long, default_value_t = 20)]
        interval: u64,
        /// Stop after this many rounds instead of running forever.
        #[arg(long)]
        rounds: Option<u32>,
        /// Path to wireguard.exe. Falls back to WIREGUARD_EXE, then standard
        /// install locations, then PATH.
        #[arg(long, env = "WIREGUARD_EXE")]
        wireguard: Option<PathBuf>,
    },
    /// Like watch-game, but for every game registered in a games.toml at
    /// once (see games.toml.template) - add a future game/app by adding
    /// a [[game]] entry there, not by writing new code or a new
    /// Scheduled Task. Requires Administrator.
    WatchGames {
        /// Path to a games.toml (see games.toml.template).
        #[arg(long)]
        games: PathBuf,
        /// Seconds between detection checks.
        #[arg(long, default_value_t = 20)]
        interval: u64,
        /// Stop after this many rounds instead of running forever.
        #[arg(long)]
        rounds: Option<u32>,
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
        Commands::DetectRemote { process } => gamedetect::detect_remote_ips(&process).map(|ips| {
            if ips.is_empty() {
                println!(
                    "No active UDP remote addresses found for '{process}' - make sure a match is in progress."
                );
            } else {
                for ip in &ips {
                    println!("{ip}");
                }
            }
        }),
        Commands::AddGameRoute {
            config,
            name,
            process,
            cidr,
            wireguard,
        } => gameroute::add_game_route(&config, &process, cidr).and_then(|outcome| {
            if outcome.detected.is_empty() {
                println!(
                    "No active UDP remote addresses found for '{process}' - make sure a match is in progress."
                );
                return Ok(());
            }
            let detected_str = outcome
                .detected
                .iter()
                .map(|ip| ip.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!("Detected: {detected_str}");
            if outcome.added.is_empty() {
                println!("Already covered by existing AllowedIPs - nothing to do.");
                return Ok(());
            }
            println!("Added to AllowedIPs: {}", outcome.added.join(", "));
            let tunnel = wireguard::WireGuardTunnel {
                wireguard_exe: wireguard,
            };
            gameroute::reconnect(&tunnel, &name, &config).map(|new_name| {
                println!("Reconnected tunnel '{new_name}' with the updated route.");
            })
        }),
        Commands::WatchGame {
            config,
            name,
            process,
            cidr,
            interval,
            rounds,
            wireguard,
        } => {
            let tunnel = wireguard::WireGuardTunnel {
                wireguard_exe: wireguard,
            };
            gameroute::watch_game(&config, &name, &process, cidr, interval, rounds, &tunnel)
        }
        Commands::WatchGames {
            games: games_path,
            interval,
            rounds,
            wireguard,
        } => games::load(&games_path).and_then(|games_file| {
            let tunnel = wireguard::WireGuardTunnel {
                wireguard_exe: wireguard,
            };
            gameroute::watch_games(&games_file.games, interval, rounds, &tunnel)
        }),
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
