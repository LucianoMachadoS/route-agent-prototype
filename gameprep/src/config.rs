//! User settings, stored as TOML in `%APPDATA%\GamePrep\config.toml`.
//!
//! Process names are kept normalized (lowercase, with `.exe`) so matching
//! against what Windows reports is a plain string compare.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Game executables that switch "game mode" on while running.
    pub games: Vec<String>,
    /// Background apps to close when a game starts (or on "Otimizar agora").
    pub apps: Vec<String>,
    /// Close `apps` automatically when a game starts.
    pub auto_close: bool,
    /// Reopen what was closed once the game exits.
    pub relaunch_after: bool,
    /// Give the game "above normal" CPU priority while it runs. Off by
    /// default: it rarely helps and can starve audio/voice chat.
    pub boost_priority: bool,
    /// The window's X button minimizes instead of quitting, so detection
    /// keeps running.
    pub close_minimizes: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            games: [
                "cs2.exe",
                "VALORANT-Win64-Shipping.exe",
                "League of Legends.exe",
                "FortniteClient-Win64-Shipping.exe",
                "r5apex.exe",
                "Overwatch.exe",
                "RocketLeague.exe",
                "dota2.exe",
            ]
            .iter()
            .map(|s| normalize(s))
            .collect(),
            // The list agreed on for this machine (GamePrep.ps1).
            apps: [
                "LM Studio.exe",
                "Docker Desktop.exe",
                "Postman Agent.exe",
                "Spotify.exe",
                "opera.exe",
                "Discord.exe",
                "ms-teams.exe",
                "M365Copilot.exe",
                "OneDrive.exe",
                "parsecd.exe",
            ]
            .iter()
            .map(|s| normalize(s))
            .collect(),
            auto_close: true,
            relaunch_after: false,
            boost_priority: false,
            close_minimizes: true,
        }
    }
}

/// `"Discord"`, `"Discord.EXE"`, `" discord.exe "` -> `"discord.exe"`.
pub fn normalize(name: &str) -> String {
    let n = name.trim().to_lowercase();
    if n.is_empty() || n.ends_with(".exe") {
        n
    } else {
        format!("{n}.exe")
    }
}

impl Config {
    /// Normalizes and dedups both lists (e.g. after hand-editing the file).
    pub fn tidy(&mut self) {
        for list in [&mut self.games, &mut self.apps] {
            let mut seen = std::collections::HashSet::new();
            *list = list
                .iter()
                .map(|s| normalize(s))
                .filter(|s| !s.is_empty() && seen.insert(s.clone()))
                .collect();
        }
        // A game is never also an app to close.
        let games = self.games.clone();
        self.apps.retain(|a| !games.contains(a));
    }

    pub fn is_game(&self, name: &str) -> bool {
        self.games.contains(&normalize(name))
    }

    pub fn is_app(&self, name: &str) -> bool {
        self.apps.contains(&normalize(name))
    }

    pub fn set_app(&mut self, name: &str, on: bool) {
        let n = normalize(name);
        self.apps.retain(|a| *a != n);
        if on && !n.is_empty() && !self.games.contains(&n) {
            self.apps.push(n);
        }
    }

    pub fn add_game(&mut self, name: &str) {
        let n = normalize(name);
        if !n.is_empty() && !self.games.contains(&n) {
            self.apps.retain(|a| *a != n);
            self.games.push(n);
        }
    }

    pub fn remove_game(&mut self, name: &str) {
        let n = normalize(name);
        self.games.retain(|g| *g != n);
    }
}

pub fn default_path() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("GamePrep").join("config.toml")
}

/// Missing or unreadable file -> defaults. A file that exists but doesn't
/// parse is kept aside as `config.toml.bad` instead of being silently
/// overwritten on the next save.
pub fn load(path: &Path) -> Config {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Config::default();
    };
    match toml::from_str::<Config>(&text) {
        Ok(mut cfg) => {
            cfg.tidy();
            cfg
        }
        Err(_) => {
            let _ = std::fs::rename(path, path.with_extension("toml.bad"));
            Config::default()
        }
    }
}

pub fn save(path: &Path, cfg: &Config) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = toml::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    // Write-then-rename so a crash mid-write can't leave a half file.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_names() {
        assert_eq!(normalize("Discord"), "discord.exe");
        assert_eq!(normalize(" CS2.EXE "), "cs2.exe");
        assert_eq!(normalize("LM Studio"), "lm studio.exe");
        assert_eq!(normalize("  "), "");
    }

    #[test]
    fn games_and_apps_never_overlap() {
        let mut c = Config::default();
        c.set_app("cs2", true);
        assert!(!c.is_app("cs2.exe"), "a game can't be closed as an app");
        c.set_app("Chrome", true);
        assert!(c.is_app("chrome.exe"));
        c.add_game("chrome.exe");
        assert!(c.is_game("chrome.exe") && !c.is_app("chrome.exe"));
        c.remove_game("CHROME");
        assert!(!c.is_game("chrome.exe"));
    }

    #[test]
    fn round_trips_and_survives_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        assert_eq!(load(&p), Config::default());

        let mut c = Config::default();
        c.relaunch_after = true;
        c.set_app("chrome", true);
        save(&p, &c).unwrap();
        assert_eq!(load(&p), c);

        std::fs::write(&p, "this is = = not toml").unwrap();
        assert_eq!(load(&p), Config::default());
        assert!(p.with_extension("toml.bad").exists());
    }

    #[test]
    fn hand_edited_file_is_tidied() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("config.toml");
        std::fs::write(
            &p,
            "games = ['CS2', 'cs2.exe']\napps = ['Discord', 'discord.exe', 'cs2']\n",
        )
        .unwrap();
        let c = load(&p);
        assert_eq!(c.games, vec!["cs2.exe"]);
        assert_eq!(c.apps, vec!["discord.exe"]);
        // Missing keys fall back to defaults.
        assert!(c.auto_close);
    }
}
