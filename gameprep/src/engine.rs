//! Decides what to do each tick, given the current process list and the
//! config. Pure on purpose (no Windows calls): the main loop feeds it a
//! snapshot and executes the returned `Action`s, so every transition -
//! game starts, game exits, game switches, nothing to close - is unit
//! tested without touching real processes.

use std::path::PathBuf;

use crate::config::{normalize, Config};
use crate::protect::is_protected;

/// One running process, as the engine needs to see it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcView {
    pub pid: u32,
    /// As Windows reports it (e.g. "Discord.exe").
    pub name: String,
    pub working_set: u64,
}

/// An app GamePrep closed, remembered so it can be reopened afterwards.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosedApp {
    pub name: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Log(String),
    /// Close every running process with one of these (normalized) names.
    /// `for_game` = closed because a game started (so it's eligible for
    /// reopening when the game ends), vs a manual "Otimizar agora".
    Close {
        names: Vec<String>,
        for_game: bool,
    },
    Relaunch(Vec<ClosedApp>),
    Boost(u32),
}

#[derive(Debug, Default)]
pub struct Engine {
    game: Option<String>,
    closed: Vec<ClosedApp>,
}

/// Configured apps that are running right now and may be closed: not
/// protected, not a game. Normalized, deduplicated, in config order.
pub fn closable_running(procs: &[ProcView], cfg: &Config) -> Vec<String> {
    cfg.apps
        .iter()
        .filter(|a| !is_protected(a) && !cfg.is_game(a))
        .filter(|a| procs.iter().any(|p| normalize(&p.name) == **a))
        .cloned()
        .collect()
}

/// The first configured game that's running, if any.
pub fn running_game(procs: &[ProcView], cfg: &Config) -> Option<(String, u32)> {
    procs
        .iter()
        .find(|p| cfg.is_game(&p.name))
        .map(|p| (normalize(&p.name), p.pid))
}

impl Engine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current_game(&self) -> Option<&str> {
        self.game.as_deref()
    }

    /// The executor reports what it actually closed for a game, so those
    /// can be reopened when the game exits.
    pub fn record_closed(&mut self, apps: Vec<ClosedApp>) {
        for a in apps {
            if !self.closed.iter().any(|c| c.name == a.name) {
                self.closed.push(a);
            }
        }
    }

    pub fn tick(&mut self, procs: &[ProcView], cfg: &Config) -> Vec<Action> {
        let mut out = Vec::new();
        let now = running_game(procs, cfg);
        match (self.game.take(), now) {
            (None, Some((game, pid))) => {
                out.push(Action::Log(format!("jogo detectado: {game}")));
                if cfg.auto_close {
                    let names = closable_running(procs, cfg);
                    if names.is_empty() {
                        out.push(Action::Log("nenhum app da lista estava aberto".to_string()));
                    } else {
                        out.push(Action::Close {
                            names,
                            for_game: true,
                        });
                    }
                }
                if cfg.boost_priority {
                    out.push(Action::Boost(pid));
                }
                self.game = Some(game);
            }
            (Some(old), None) => {
                out.push(Action::Log(format!("jogo fechado: {old}")));
                let closed = std::mem::take(&mut self.closed);
                if cfg.relaunch_after && !closed.is_empty() {
                    out.push(Action::Relaunch(closed));
                }
            }
            // Still playing (possibly switched straight to another
            // configured game): same session, nothing new to do.
            (Some(_), Some((game, _))) => self.game = Some(game),
            (None, None) => {}
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, name: &str) -> ProcView {
        ProcView {
            pid,
            name: name.to_string(),
            working_set: 100 << 20,
        }
    }

    fn cfg() -> Config {
        Config {
            games: vec!["cs2.exe".into(), "dota2.exe".into()],
            apps: vec![
                "discord.exe".into(),
                "spotify.exe".into(),
                "explorer.exe".into(),
            ],
            ..Config::default()
        }
    }

    #[test]
    fn closes_running_listed_apps_when_a_game_starts_once() {
        let mut e = Engine::new();
        let c = cfg();
        let idle = [
            p(1, "Discord.exe"),
            p(2, "explorer.exe"),
            p(3, "chrome.exe"),
        ];
        assert!(e.tick(&idle, &c).is_empty(), "no game, nothing happens");

        let playing = [p(1, "Discord.exe"), p(2, "explorer.exe"), p(9, "cs2.exe")];
        let a = e.tick(&playing, &c);
        assert_eq!(
            a,
            vec![
                Action::Log("jogo detectado: cs2.exe".into()),
                // spotify isn't running; explorer is protected even though listed.
                Action::Close {
                    names: vec!["discord.exe".into()],
                    for_game: true
                },
            ]
        );
        assert_eq!(e.current_game(), Some("cs2.exe"));
        // Next tick, same game: no repeated closing.
        assert!(e.tick(&playing, &c).is_empty());
    }

    #[test]
    fn auto_close_off_only_logs_and_boost_is_opt_in() {
        let mut e = Engine::new();
        let mut c = cfg();
        c.auto_close = false;
        c.boost_priority = true;
        let a = e.tick(&[p(1, "Discord.exe"), p(9, "cs2.exe")], &c);
        assert_eq!(
            a,
            vec![
                Action::Log("jogo detectado: cs2.exe".into()),
                Action::Boost(9)
            ]
        );
    }

    #[test]
    fn reopens_only_what_it_closed_and_only_if_enabled() {
        let c_on = Config {
            relaunch_after: true,
            ..cfg()
        };
        let mut e = Engine::new();
        e.tick(&[p(1, "Discord.exe"), p(9, "cs2.exe")], &c_on);
        let discord = ClosedApp {
            name: "discord.exe".into(),
            path: Some(PathBuf::from(r"C:\Discord\Discord.exe")),
        };
        e.record_closed(vec![discord.clone(), discord.clone()]);
        let a = e.tick(&[], &c_on);
        assert_eq!(
            a,
            vec![
                Action::Log("jogo fechado: cs2.exe".into()),
                Action::Relaunch(vec![discord.clone()])
            ]
        );
        assert_eq!(e.current_game(), None);

        // Same session with relaunch off: the closed list is just dropped.
        let mut e = Engine::new();
        e.tick(&[p(9, "cs2.exe")], &cfg());
        e.record_closed(vec![discord]);
        assert_eq!(
            e.tick(&[], &cfg()),
            vec![Action::Log("jogo fechado: cs2.exe".into())]
        );
        // ...and doesn't leak into the next session.
        e.tick(
            &[p(9, "cs2.exe")],
            &Config {
                relaunch_after: true,
                ..cfg()
            },
        );
        let a = e.tick(
            &[],
            &Config {
                relaunch_after: true,
                ..cfg()
            },
        );
        assert_eq!(a, vec![Action::Log("jogo fechado: cs2.exe".into())]);
    }

    #[test]
    fn switching_games_is_one_session_and_nothing_to_close_is_logged() {
        let mut e = Engine::new();
        let c = cfg();
        let a = e.tick(&[p(9, "cs2.exe")], &c);
        assert_eq!(
            a[1],
            Action::Log("nenhum app da lista estava aberto".into())
        );
        assert!(e.tick(&[p(7, "dota2.exe")], &c).is_empty());
        assert_eq!(e.current_game(), Some("dota2.exe"));
    }

    #[test]
    fn a_game_listed_as_app_is_never_closed() {
        let c = Config {
            games: vec!["cs2.exe".into()],
            apps: vec!["cs2.exe".into(), "discord.exe".into()],
            ..Config::default()
        };
        let names = closable_running(&[p(1, "cs2.exe"), p(2, "discord.exe")], &c);
        assert_eq!(names, vec!["discord.exe".to_string()]);
    }
}
