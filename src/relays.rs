//! Loads the relay candidate list and scoring weights from a TOML file
//! (see `relays.toml.template`). This is the "multiple relay profiles"
//! piece that Phase 2 (a single `--config`) didn't need yet.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AgentError, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct RelayProfile {
    pub name: String,
    pub config: PathBuf,
    pub tunnel_host: String,
}

/// Weights for `S = w_rtt*RTT + w_jitter*Jitter + w_loss*Loss +
/// w_instability*Instability` (lower score wins). Defaults are starting
/// points, not tuned values - see docs/BENCHMARK_PLAN.md.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ScoringWeights {
    pub w_rtt: f64,
    pub w_jitter: f64,
    pub w_loss: f64,
    pub w_instability: f64,
}

impl Default for ScoringWeights {
    fn default() -> Self {
        Self {
            w_rtt: 0.4,
            w_jitter: 0.25,
            w_loss: 0.30,
            w_instability: 0.05,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RelaysFile {
    #[serde(default)]
    pub scoring: ScoringWeights,
    #[serde(rename = "relay", default)]
    pub relays: Vec<RelayProfile>,
}

/// Loads and validates a `relays.toml`: must parse, must have at least
/// one `[[relay]]` entry, and relay names must be unique (the selector
/// and the decision log key everything by name).
pub fn load(path: &Path) -> Result<RelaysFile> {
    let text = std::fs::read_to_string(path)
        .map_err(|_| AgentError::ConfigNotFound(path.to_path_buf()))?;
    let parsed: RelaysFile = toml::from_str(&text).map_err(|e| AgentError::RelayConfigInvalid {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    if parsed.relays.is_empty() {
        return Err(AgentError::RelayConfigInvalid {
            path: path.to_path_buf(),
            reason: "no [[relay]] entries".to_string(),
        });
    }

    let mut seen = HashSet::new();
    for relay in &parsed.relays {
        if !seen.insert(relay.name.clone()) {
            return Err(AgentError::RelayConfigInvalid {
                path: path.to_path_buf(),
                reason: format!("duplicate relay name '{}'", relay.name),
            });
        }
    }

    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_toml(contents: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f
    }

    #[test]
    fn loads_relays_and_default_weights() {
        let f = write_toml(
            r#"
[[relay]]
name = "fra1"
config = "C:/ProgramData/RouteAgent/fra1.conf"
tunnel_host = "10.66.0.1"

[[relay]]
name = "ams1"
config = "C:/ProgramData/RouteAgent/ams1.conf"
tunnel_host = "10.66.0.1"
"#,
        );
        let parsed = load(f.path()).unwrap();
        assert_eq!(parsed.relays.len(), 2);
        assert_eq!(parsed.scoring.w_rtt, 0.4);
    }

    #[test]
    fn rejects_duplicate_relay_names() {
        let f = write_toml(
            r#"
[[relay]]
name = "fra1"
config = "a.conf"
tunnel_host = "10.66.0.1"

[[relay]]
name = "fra1"
config = "b.conf"
tunnel_host = "10.66.0.1"
"#,
        );
        assert!(load(f.path()).is_err());
    }

    #[test]
    fn rejects_empty_relay_list() {
        let f = write_toml("");
        assert!(load(f.path()).is_err());
    }

    #[test]
    fn rejects_unparsable_toml() {
        let f = write_toml("this is not valid toml {{{");
        assert!(load(f.path()).is_err());
    }

    #[test]
    fn partial_scoring_overrides_only_specified_weights() {
        let f = write_toml(
            r#"
[scoring]
w_rtt = 0.9

[[relay]]
name = "fra1"
config = "a.conf"
tunnel_host = "10.66.0.1"
"#,
        );
        let parsed = load(f.path()).unwrap();
        assert_eq!(parsed.scoring.w_rtt, 0.9);
        assert_eq!(parsed.scoring.w_jitter, 0.25); // default preserved
    }
}
