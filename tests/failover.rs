//! Integration test for `routing::selector::apply_decision` against a
//! fake `TunnelManager` - confirms the orchestration (disconnect old,
//! then connect new, only when actually switching) without needing a
//! real WireGuard install, a real relay, or Administrator rights.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use route_agent::error::Result;
use route_agent::relays::{RelayProfile, DIRECT_CANDIDATE};
use route_agent::routing::selector::{apply_decision, Decision};
use route_agent::traits::TunnelManager;
use route_agent::wireguard::ServiceState;

#[derive(Default)]
struct FakeTunnel {
    calls: RefCell<Vec<String>>,
    next_id: RefCell<u32>,
}

impl TunnelManager for FakeTunnel {
    fn connect(&self, config: &Path) -> Result<String> {
        let mut n = self.next_id.borrow_mut();
        *n += 1;
        let name = format!("fake-{n}");
        self.calls
            .borrow_mut()
            .push(format!("connect({}) -> {name}", config.display()));
        Ok(name)
    }

    fn disconnect(&self, name: &str) -> Result<()> {
        self.calls.borrow_mut().push(format!("disconnect({name})"));
        Ok(())
    }

    fn status(&self, _name: &str) -> Result<ServiceState> {
        Ok(ServiceState::Running)
    }
}

fn relay(name: &str, config: &str) -> RelayProfile {
    RelayProfile {
        name: name.to_string(),
        config: PathBuf::from(config),
        tunnel_host: "10.66.0.1".to_string(),
    }
}

fn decision(chosen: &str, keep_current: bool) -> Decision {
    Decision {
        chosen: chosen.to_string(),
        keep_current,
        reason: "test".to_string(),
        scores: Default::default(),
    }
}

#[test]
fn switching_disconnects_old_then_connects_new() {
    let tunnel = FakeTunnel::default();
    let relays = vec![relay("a", "a.conf"), relay("b", "b.conf")];
    let mut active: Option<String> = Some("fake-0".to_string());

    apply_decision(&decision("b", false), &relays, &tunnel, &mut active).unwrap();

    let calls = tunnel.calls.borrow();
    assert_eq!(calls.len(), 2, "calls: {calls:?}");
    assert!(calls[0].starts_with("disconnect(fake-0)"));
    assert!(calls[1].contains("b.conf"));
    assert_eq!(active.as_deref(), Some("fake-1"));
}

#[test]
fn keeping_current_makes_no_tunnel_calls() {
    let tunnel = FakeTunnel::default();
    let relays = vec![relay("a", "a.conf")];
    let mut active: Option<String> = Some("fake-0".to_string());

    apply_decision(&decision("a", true), &relays, &tunnel, &mut active).unwrap();

    assert!(tunnel.calls.borrow().is_empty());
    assert_eq!(active.as_deref(), Some("fake-0"));
}

#[test]
fn first_connect_has_no_prior_disconnect() {
    let tunnel = FakeTunnel::default();
    let relays = vec![relay("a", "a.conf")];
    let mut active: Option<String> = None;

    apply_decision(&decision("a", false), &relays, &tunnel, &mut active).unwrap();

    let calls = tunnel.calls.borrow();
    assert_eq!(calls.len(), 1, "calls: {calls:?}");
    assert!(calls[0].contains("a.conf"));
}

#[test]
fn unknown_relay_name_is_an_error() {
    let tunnel = FakeTunnel::default();
    let relays = vec![relay("a", "a.conf")];
    let mut active: Option<String> = None;

    let result = apply_decision(&decision("ghost", false), &relays, &tunnel, &mut active);
    assert!(result.is_err());
    assert!(tunnel.calls.borrow().is_empty());
}

#[test]
fn switching_to_direct_disconnects_without_connecting_anything() {
    let tunnel = FakeTunnel::default();
    let relays = vec![relay("a", "a.conf")];
    let mut active: Option<String> = Some("fake-0".to_string());

    apply_decision(
        &decision(DIRECT_CANDIDATE, false),
        &relays,
        &tunnel,
        &mut active,
    )
    .unwrap();

    let calls = tunnel.calls.borrow();
    assert_eq!(calls.len(), 1, "calls: {calls:?}");
    assert!(calls[0].starts_with("disconnect(fake-0)"));
    assert_eq!(active, None);
}

#[test]
fn switching_to_direct_with_no_active_tunnel_makes_no_calls() {
    let tunnel = FakeTunnel::default();
    let relays = vec![relay("a", "a.conf")];
    let mut active: Option<String> = None;

    apply_decision(
        &decision(DIRECT_CANDIDATE, false),
        &relays,
        &tunnel,
        &mut active,
    )
    .unwrap();

    assert!(tunnel.calls.borrow().is_empty());
    assert_eq!(active, None);
}
