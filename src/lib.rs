//! route-agent library: every module, so both the `route-agent` binary
//! (`src/main.rs`) and the integration tests under `tests/` can use them.
//! This crate was binary-only through Phase 2; Phase 3 adds `routing`,
//! whose scoring/failover logic needs to be testable from `tests/*.rs`
//! with fake probes/tunnels (no real network or WireGuard), which an
//! integration test can only reach through a library target. See
//! CLAUDE.md for the detail.

pub mod bench;
pub mod error;
pub mod monitor;
pub mod relays;
pub mod routing;
pub mod syspath;
pub mod telemetry;
pub mod traits;
pub mod wireguard;
