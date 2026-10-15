//! Shared abstractions so `routing::selector` can be unit-tested with
//! fake probes/tunnels instead of needing real network access or an
//! installed WireGuard client with Administrator rights. `bench.rs`
//! implements `Probe` for its existing `ping()`; `wireguard.rs`
//! implements `TunnelManager` for its existing connect/disconnect/status
//! functions. Nothing here touches the network or a process by itself.

use std::path::Path;

use crate::error::Result;
use crate::wireguard::ServiceState;

/// One round's worth of reachability/quality data for a single target.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub rtt_p95_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub loss_pct: f64,
    pub available: bool,
}

pub trait Probe {
    fn measure(&self, target: &str, count: u32) -> Result<Measurement>;
}

pub trait TunnelManager {
    /// Brings up the tunnel described by `config` and returns the tunnel
    /// (service) name, mirroring `wireguard::install_and_start`.
    fn connect(&self, config: &Path) -> Result<String>;
    fn disconnect(&self, name: &str) -> Result<()>;
    fn status(&self, name: &str) -> Result<ServiceState>;
}
