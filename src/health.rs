//! `health-check` subcommand: a stronger signal than `status` alone.
//!
//! `status` only reports the Windows service's SCM state - a service can
//! be "Running" while the tunnel is actually down (handshake expired,
//! relay unreachable, wrong keys). The official `wireguard.exe` CLI on
//! Windows doesn't expose a documented way to query live handshake/peer
//! stats (unlike Linux's `wg show`, which reads the kernel module
//! directly) - there is no bundled `wg.exe` equivalent. So this combines
//! the two signals that *are* available without extra tooling:
//!
//!   1. the Windows service state (via `TunnelManager::status`)
//!   2. whether the relay's tunnel address actually responds (via `Probe`)
//!
//! Both have to check out for the tunnel to be considered healthy. This
//! is an honest proxy, not a cryptographic handshake check - see
//! docs/SECURITY.md / README's "Current limitations" for that caveat.

use crate::error::Result;
use crate::traits::{Measurement, Probe, TunnelManager};
use crate::wireguard::ServiceState;

#[derive(Debug, Clone)]
pub struct HealthReport {
    pub tunnel_name: String,
    pub service: ServiceState,
    pub reachability: Measurement,
}

impl HealthReport {
    /// True only if the Windows service is running AND the tunnel
    /// address actually replied to at least one probe this round.
    pub fn healthy(&self) -> bool {
        self.service == ServiceState::Running && self.reachability.available
    }
}

/// Runs both checks. `tunnel` and `probe` are trait objects so this is
/// testable with fakes - no real WireGuard install or network needed.
pub fn check(
    tunnel: &dyn TunnelManager,
    probe: &dyn Probe,
    name: &str,
    tunnel_host: &str,
    probe_count: u32,
) -> Result<HealthReport> {
    let service = tunnel.status(name)?;
    let reachability = probe.measure(tunnel_host, probe_count)?;
    Ok(HealthReport {
        tunnel_name: name.to_string(),
        service,
        reachability,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AgentError;
    use std::path::Path;

    struct FakeTunnel(ServiceState);
    impl TunnelManager for FakeTunnel {
        fn connect(&self, _config: &Path) -> Result<String> {
            Ok("fake".to_string())
        }
        fn disconnect(&self, _name: &str) -> Result<()> {
            Ok(())
        }
        fn status(&self, _name: &str) -> Result<ServiceState> {
            Ok(self.0)
        }
    }

    struct FakeProbe(Measurement);
    impl Probe for FakeProbe {
        fn measure(&self, _target: &str, _count: u32) -> Result<Measurement> {
            Ok(self.0.clone())
        }
    }

    struct FailingTunnel;
    impl TunnelManager for FailingTunnel {
        fn connect(&self, _config: &Path) -> Result<String> {
            Ok("fake".to_string())
        }
        fn disconnect(&self, _name: &str) -> Result<()> {
            Ok(())
        }
        fn status(&self, _name: &str) -> Result<ServiceState> {
            Err(AgentError::NoHealthyRelay)
        }
    }

    fn reachable() -> Measurement {
        Measurement {
            rtt_p95_ms: Some(20.0),
            jitter_ms: Some(1.0),
            loss_pct: 0.0,
            available: true,
        }
    }

    fn unreachable() -> Measurement {
        Measurement {
            rtt_p95_ms: None,
            jitter_ms: None,
            loss_pct: 100.0,
            available: false,
        }
    }

    #[test]
    fn running_and_reachable_is_healthy() {
        let report = check(
            &FakeTunnel(ServiceState::Running),
            &FakeProbe(reachable()),
            "t",
            "10.66.0.1",
            4,
        )
        .unwrap();
        assert!(report.healthy());
    }

    #[test]
    fn running_but_unreachable_is_unhealthy() {
        let report = check(
            &FakeTunnel(ServiceState::Running),
            &FakeProbe(unreachable()),
            "t",
            "10.66.0.1",
            4,
        )
        .unwrap();
        assert!(
            !report.healthy(),
            "service up but no reply should not be healthy"
        );
    }

    #[test]
    fn reachable_but_stopped_service_is_unhealthy() {
        let report = check(
            &FakeTunnel(ServiceState::Stopped),
            &FakeProbe(reachable()),
            "t",
            "10.66.0.1",
            4,
        )
        .unwrap();
        assert!(
            !report.healthy(),
            "stopped service should not be healthy even if the host happens to reply"
        );
    }

    #[test]
    fn status_error_propagates() {
        let result = check(&FailingTunnel, &FakeProbe(reachable()), "t", "10.66.0.1", 4);
        assert!(result.is_err());
    }
}
