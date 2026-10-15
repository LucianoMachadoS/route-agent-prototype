//! Decides which relay should be active, from a rolling window of
//! measurements per relay. Kept free of any real I/O (no network, no
//! WireGuard) so `Selector` is fully unit-testable - `apply_decision`
//! below is the only piece that touches a real (or faked) `TunnelManager`.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::error::{AgentError, Result};
use crate::relays::{RelayProfile, ScoringWeights};
use crate::routing::score::{score_round, ScoreInput};
use crate::traits::{Measurement, TunnelManager};

#[derive(Debug, Clone)]
pub struct SelectorConfig {
    /// How many recent rounds to average per relay.
    pub window: usize,
    /// A candidate must beat the current relay's score by at least this
    /// fraction to trigger a "normal" switch (hysteresis).
    pub switch_margin: f64,
    /// Minimum time between two "normal" switches.
    pub cooldown: Duration,
    /// Consecutive 100%-loss rounds for the *current* relay that trigger
    /// an immediate switch, bypassing cooldown and the margin.
    pub hard_failover_losses: u32,
}

impl Default for SelectorConfig {
    fn default() -> Self {
        Self {
            window: 5,
            switch_margin: 0.15,
            cooldown: Duration::from_secs(60),
            hard_failover_losses: 3,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub chosen: String,
    pub keep_current: bool,
    pub reason: String,
    pub scores: HashMap<String, f64>,
}

#[derive(Default)]
struct RelayHistory {
    recent: Vec<Measurement>,
    consecutive_full_loss: u32,
}

pub struct Selector {
    config: SelectorConfig,
    history: HashMap<String, RelayHistory>,
    current: Option<String>,
    last_switch: Option<Instant>,
}

impl Selector {
    pub fn new(config: SelectorConfig) -> Self {
        Self {
            config,
            history: HashMap::new(),
            current: None,
            last_switch: None,
        }
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    /// Feeds one round of measurements (relay name -> Measurement) and
    /// returns the decision for this round.
    pub fn evaluate(
        &mut self,
        round: &HashMap<String, Measurement>,
        weights: &ScoringWeights,
    ) -> Decision {
        for (name, m) in round {
            let h = self.history.entry(name.clone()).or_default();
            h.recent.push(m.clone());
            if h.recent.len() > self.config.window {
                h.recent.remove(0);
            }
            if m.loss_pct >= 100.0 {
                h.consecutive_full_loss += 1;
            } else {
                h.consecutive_full_loss = 0;
            }
        }

        if let Some(cur) = self.current.clone() {
            let hit_threshold = self
                .history
                .get(&cur)
                .map(|h| h.consecutive_full_loss >= self.config.hard_failover_losses)
                .unwrap_or(false);
            if hit_threshold {
                return self.pick_best(
                    round,
                    weights,
                    true,
                    "hard failover: current relay lost all packets repeatedly",
                );
            }
        }

        if let Some(last) = self.last_switch {
            if last.elapsed() < self.config.cooldown {
                return Decision {
                    chosen: self.current.clone().unwrap_or_default(),
                    keep_current: true,
                    reason: "cooldown active since last switch".to_string(),
                    scores: HashMap::new(),
                };
            }
        }

        self.pick_best(round, weights, false, "periodic evaluation")
    }

    fn score_input_for(&self, name: &str) -> ScoreInput {
        let history = self.history.get(name);
        let recent: &[Measurement] = history.map(|h| h.recent.as_slice()).unwrap_or(&[]);
        if recent.is_empty() {
            return ScoreInput {
                rtt_p95_ms: f64::MAX / 4.0,
                jitter_ms: 0.0,
                loss_pct: 100.0,
                instability: 1.0,
                available: false,
            };
        }
        let n = recent.len() as f64;
        let rtt_p95_ms = recent.iter().filter_map(|m| m.rtt_p95_ms).sum::<f64>() / n;
        let jitter_ms = recent.iter().filter_map(|m| m.jitter_ms).sum::<f64>() / n;
        let loss_pct = recent.iter().map(|m| m.loss_pct).sum::<f64>() / n;
        let available = recent.last().map(|m| m.available).unwrap_or(false);
        let instability = history
            .map(|h| {
                (h.consecutive_full_loss as f64 / self.config.hard_failover_losses as f64).min(1.0)
            })
            .unwrap_or(0.0);
        ScoreInput {
            rtt_p95_ms,
            jitter_ms,
            loss_pct,
            instability,
            available,
        }
    }

    fn pick_best(
        &mut self,
        round: &HashMap<String, Measurement>,
        weights: &ScoringWeights,
        force: bool,
        context: &str,
    ) -> Decision {
        let mut names: Vec<String> = round.keys().cloned().collect();
        names.sort(); // deterministic order for ties/debugging
        let inputs: Vec<ScoreInput> = names.iter().map(|n| self.score_input_for(n)).collect();
        let scores = score_round(&inputs, weights);
        let scores_map: HashMap<String, f64> =
            names.iter().cloned().zip(scores.iter().cloned()).collect();

        let mut ranked: Vec<(String, f64)> = names.into_iter().zip(scores).collect();
        ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        let (best_name, best_score) = match ranked.into_iter().next() {
            Some(pair) => pair,
            None => {
                return Decision {
                    chosen: self.current.clone().unwrap_or_default(),
                    keep_current: true,
                    reason: "no relays measured this round".to_string(),
                    scores: scores_map,
                };
            }
        };

        match self.current.clone() {
            None => {
                self.current = Some(best_name.clone());
                self.last_switch = Some(Instant::now());
                Decision {
                    chosen: best_name,
                    keep_current: false,
                    reason: format!("initial selection ({context})"),
                    scores: scores_map,
                }
            }
            Some(cur) if cur == best_name => Decision {
                chosen: best_name,
                keep_current: true,
                reason: format!("current relay still best ({context})"),
                scores: scores_map,
            },
            Some(cur) => {
                let cur_score = *scores_map.get(&cur).unwrap_or(&f64::MAX);
                let improvement = if cur_score.is_finite() && cur_score > 0.0 {
                    (cur_score - best_score) / cur_score
                } else if cur_score.is_finite() {
                    0.0
                } else {
                    1.0
                };
                if force || improvement >= self.config.switch_margin {
                    self.current = Some(best_name.clone());
                    self.last_switch = Some(Instant::now());
                    Decision {
                        chosen: best_name,
                        keep_current: false,
                        reason: format!(
                            "{context}: {:.0}% improvement over '{cur}'",
                            improvement * 100.0
                        ),
                        scores: scores_map,
                    }
                } else {
                    Decision {
                        chosen: cur.clone(),
                        keep_current: true,
                        reason: format!(
                            "{context}: {:.0}% improvement below {:.0}% margin",
                            improvement * 100.0,
                            self.config.switch_margin * 100.0
                        ),
                        scores: scores_map,
                    }
                }
            }
        }
    }
}

/// Carries out a non-"keep" `Decision` against a `TunnelManager`:
/// disconnects the previous relay's tunnel (if any) and connects the new
/// one. Kept separate from `Selector::evaluate` so the pure selection
/// logic above stays trivially testable, and only this function needs a
/// `TunnelManager` (real or faked).
pub fn apply_decision(
    decision: &Decision,
    relays: &[RelayProfile],
    tunnel: &dyn TunnelManager,
    active_tunnel_name: &mut Option<String>,
) -> Result<()> {
    if decision.keep_current {
        return Ok(());
    }
    let profile = relays
        .iter()
        .find(|r| r.name == decision.chosen)
        .ok_or(AgentError::NoHealthyRelay)?;

    if let Some(old_name) = active_tunnel_name.take() {
        tunnel.disconnect(&old_name)?;
    }
    let new_name = tunnel.connect(&profile.config)?;
    *active_tunnel_name = Some(new_name);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measurement(rtt: f64, jitter: f64, loss: f64) -> Measurement {
        Measurement {
            rtt_p95_ms: Some(rtt),
            jitter_ms: Some(jitter),
            loss_pct: loss,
            available: loss < 100.0,
        }
    }

    fn weights() -> ScoringWeights {
        ScoringWeights {
            w_rtt: 0.4,
            w_jitter: 0.25,
            w_loss: 0.30,
            w_instability: 0.05,
        }
    }

    fn round(pairs: &[(&str, Measurement)]) -> HashMap<String, Measurement> {
        pairs
            .iter()
            .map(|(n, m)| (n.to_string(), m.clone()))
            .collect()
    }

    #[test]
    fn initial_round_picks_the_better_relay() {
        let mut sel = Selector::new(SelectorConfig::default());
        let d = sel.evaluate(
            &round(&[
                ("a", measurement(20.0, 2.0, 0.0)),
                ("b", measurement(80.0, 10.0, 5.0)),
            ]),
            &weights(),
        );
        assert_eq!(d.chosen, "a");
        assert!(!d.keep_current);
        assert_eq!(sel.current(), Some("a"));
    }

    #[test]
    fn small_improvement_does_not_trigger_a_switch() {
        let mut config = SelectorConfig::default();
        config.cooldown = Duration::from_secs(0);
        let mut sel = Selector::new(config);
        sel.evaluate(
            &round(&[
                ("a", measurement(30.0, 3.0, 0.0)),
                ("b", measurement(32.0, 3.0, 0.0)),
            ]),
            &weights(),
        );
        assert_eq!(sel.current(), Some("a"));

        // b edges out a slightly - within the margin, should NOT switch.
        let d = sel.evaluate(
            &round(&[
                ("a", measurement(30.0, 3.0, 0.0)),
                ("b", measurement(29.0, 3.0, 0.0)),
            ]),
            &weights(),
        );
        assert!(d.keep_current, "expected hysteresis to keep 'a': {d:?}");
    }

    #[test]
    fn large_improvement_beyond_margin_triggers_a_switch() {
        let mut config = SelectorConfig::default();
        config.cooldown = Duration::from_secs(0);
        let mut sel = Selector::new(config);
        sel.evaluate(
            &round(&[
                ("a", measurement(30.0, 3.0, 0.0)),
                ("b", measurement(200.0, 20.0, 10.0)),
            ]),
            &weights(),
        );
        assert_eq!(sel.current(), Some("a"));

        let d = sel.evaluate(
            &round(&[
                ("a", measurement(200.0, 20.0, 10.0)),
                ("b", measurement(10.0, 1.0, 0.0)),
            ]),
            &weights(),
        );
        assert!(!d.keep_current);
        assert_eq!(d.chosen, "b");
    }

    #[test]
    fn cooldown_blocks_a_second_normal_switch_immediately_after_the_first() {
        let mut config = SelectorConfig::default();
        config.cooldown = Duration::from_secs(3600);
        let mut sel = Selector::new(config);
        sel.evaluate(
            &round(&[
                ("a", measurement(30.0, 3.0, 0.0)),
                ("b", measurement(200.0, 20.0, 10.0)),
            ]),
            &weights(),
        );
        // a was picked first; now b clearly improves, but the 1h cooldown just started.
        let d = sel.evaluate(
            &round(&[
                ("a", measurement(200.0, 20.0, 10.0)),
                ("b", measurement(5.0, 0.5, 0.0)),
            ]),
            &weights(),
        );
        assert!(
            d.keep_current,
            "expected cooldown to block the switch: {d:?}"
        );
    }

    #[test]
    fn hard_failover_bypasses_cooldown_on_total_loss() {
        let mut config = SelectorConfig::default();
        config.cooldown = Duration::from_secs(3600);
        config.hard_failover_losses = 2;
        let mut sel = Selector::new(config);
        sel.evaluate(
            &round(&[
                ("a", measurement(30.0, 3.0, 0.0)),
                ("b", measurement(50.0, 5.0, 0.0)),
            ]),
            &weights(),
        );
        assert_eq!(sel.current(), Some("a"));

        sel.evaluate(
            &round(&[
                ("a", measurement(0.0, 0.0, 100.0)),
                ("b", measurement(50.0, 5.0, 0.0)),
            ]),
            &weights(),
        );
        let d = sel.evaluate(
            &round(&[
                ("a", measurement(0.0, 0.0, 100.0)),
                ("b", measurement(50.0, 5.0, 0.0)),
            ]),
            &weights(),
        );
        assert!(
            !d.keep_current,
            "expected hard failover to override cooldown: {d:?}"
        );
        assert_eq!(d.chosen, "b");
    }
}
