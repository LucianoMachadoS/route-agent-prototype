//! Loads the registry of games/apps to watch for selective routing (see
//! `games.toml.template`). This is what makes adding a future game a
//! matter of adding one `[[game]]` entry, instead of writing new code or
//! a new Scheduled Task - `watch-games` just iterates whatever's in here.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{AgentError, Result};

#[derive(Debug, Clone, Deserialize)]
pub struct GameProfile {
    /// Short name for logging (e.g. "cs2", "valorant") - not used to
    /// find the process, just to label output.
    pub name: String,
    /// Process name to detect, with or without .exe (e.g. "cs2.exe" or
    /// "VALORANT-Win64-Shipping").
    pub process: String,
    /// The WireGuard client .conf whose AllowedIPs gets updated when a
    /// new server is detected for this game.
    pub config: PathBuf,
    /// Tunnel name (that config's file stem) to disconnect/reconnect.
    pub tunnel_name: String,
    /// Expand each detected IP to this CIDR prefix instead of an exact
    /// /32. Optional per game - omit for exact-IP precision.
    #[serde(default)]
    pub cidr: Option<u8>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GamesFile {
    #[serde(rename = "game", default)]
    pub games: Vec<GameProfile>,
}

/// Loads and validates a `games.toml`: must parse, must have at least
/// one `[[game]]` entry, and names must be unique (only used for
/// logging, but a duplicate is almost certainly a copy-paste mistake
/// worth catching early).
pub fn load(path: &Path) -> Result<GamesFile> {
    let text = std::fs::read_to_string(path)
        .map_err(|_| AgentError::ConfigNotFound(path.to_path_buf()))?;
    let parsed: GamesFile = toml::from_str(&text).map_err(|e| AgentError::RelayConfigInvalid {
        path: path.to_path_buf(),
        reason: e.to_string(),
    })?;

    if parsed.games.is_empty() {
        return Err(AgentError::RelayConfigInvalid {
            path: path.to_path_buf(),
            reason: "no [[game]] entries".to_string(),
        });
    }

    let mut seen = HashSet::new();
    for game in &parsed.games {
        if !seen.insert(game.name.clone()) {
            return Err(AgentError::RelayConfigInvalid {
                path: path.to_path_buf(),
                reason: format!("duplicate game name '{}'", game.name),
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
    fn loads_multiple_games() {
        let f = write_toml(
            r#"
[[game]]
name = "cs2"
process = "cs2.exe"
config = "C:/ProgramData/RouteAgent/saopaulo-gcp.conf"
tunnel_name = "saopaulo-gcp"
cidr = 24

[[game]]
name = "valorant"
process = "VALORANT-Win64-Shipping.exe"
config = "C:/ProgramData/RouteAgent/saopaulo-gcp.conf"
tunnel_name = "saopaulo-gcp"
"#,
        );
        let parsed = load(f.path()).unwrap();
        assert_eq!(parsed.games.len(), 2);
        assert_eq!(parsed.games[0].cidr, Some(24));
        assert_eq!(parsed.games[1].cidr, None);
    }

    #[test]
    fn rejects_empty_game_list() {
        let f = write_toml("");
        assert!(load(f.path()).is_err());
    }

    #[test]
    fn rejects_duplicate_game_names() {
        let f = write_toml(
            r#"
[[game]]
name = "cs2"
process = "cs2.exe"
config = "a.conf"
tunnel_name = "a"

[[game]]
name = "cs2"
process = "cs2.exe"
config = "b.conf"
tunnel_name = "b"
"#,
        );
        assert!(load(f.path()).is_err());
    }
}
