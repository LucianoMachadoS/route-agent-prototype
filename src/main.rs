//! route-agent: controller for an official WireGuard for Windows tunnel.
//!
//! Subcommands:
//!   connect    - install & start the tunnel as a Windows service from a config
//!   disconnect - stop & uninstall the tunnel's Windows service
//!   status     - report whether the tunnel's Windows service is running
//!   bench      - compare direct vs. tunnel latency/jitter/loss and log it
//!
//! See README.md for setup (you need your own relay) and docs/experiments.md
//! for the measurement protocol `bench` automates.

mod bench;
mod error;
mod wireguard;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "route-agent",
    version,
    about = "Controller for an official WireGuard for Windows tunnel: connect/disconnect it and measure direct-vs-tunnel latency to a game server."
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
    /// Compare latency/jitter/loss direct vs. through the tunnel, and log the result.
    Bench {
        /// Host/IP to ping directly (e.g. the game server).
        #[arg(long)]
        direct_host: String,
        /// Host/IP to ping through the tunnel (default: the relay's tunnel address).
        #[arg(long, default_value = "10.66.0.1")]
        tunnel_host: String,
        /// Number of ICMP probes per host.
        #[arg(long, default_value_t = 20)]
        count: u32,
        /// Free-text context for this run (relay region, ISP, etc.) - stored in the CSV log.
        #[arg(long, default_value = "")]
        note: String,
        /// Where to append the run log.
        #[arg(long, default_value = "docs/runs.csv")]
        csv: PathBuf,
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
        Commands::Bench { direct_host, tunnel_host, count, note, csv } => {
            bench::run(&direct_host, &tunnel_host, count, &note, &csv)
        }
    };

    if let Err(err) = result {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}
