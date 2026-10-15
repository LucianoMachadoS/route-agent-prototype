//! Combines a round's per-relay measurements into a single score (lower
//! is better). `S = w_rtt*RTT + w_jitter*Jitter + w_loss*Loss +
//! w_instability*Instability`. Each raw metric (RTT/jitter/loss) is
//! min-max normalized across the candidates being compared in the *same*
//! round before being weighted - summing raw milliseconds with a raw
//! percentage directly would be meaningless, and normalizing per-round
//! means this needs no external baseline/history to make sense.

use crate::relays::ScoringWeights;

/// Inputs for one relay's score in one evaluation round. `instability`
/// is expected to already be normalized to `0.0..=1.0` (e.g. a
/// recent-failure ratio) - unlike the other metrics, it isn't min-max
/// normalized across candidates here, since it's already comparable
/// across relays as-is.
#[derive(Debug, Clone, Copy)]
pub struct ScoreInput {
    pub rtt_p95_ms: f64,
    pub jitter_ms: f64,
    pub loss_pct: f64,
    pub instability: f64,
    pub available: bool,
}

/// Penalty added on top of the weighted sum for a relay with zero
/// replies this round - large enough that it always loses to any relay
/// that responded at all, regardless of how the weights are tuned.
const UNAVAILABLE_PENALTY: f64 = 1000.0;

fn normalize(values: &[f64]) -> Vec<f64> {
    if values.is_empty() {
        return Vec::new();
    }
    let min = values.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if (max - min).abs() < f64::EPSILON {
        return vec![0.0; values.len()];
    }
    values.iter().map(|v| (v - min) / (max - min)).collect()
}

/// Scores every candidate in one round. Returns scores in the same order
/// as `inputs`. Lower is better.
pub fn score_round(inputs: &[ScoreInput], weights: &ScoringWeights) -> Vec<f64> {
    let rtt: Vec<f64> = inputs.iter().map(|i| i.rtt_p95_ms).collect();
    let jitter: Vec<f64> = inputs.iter().map(|i| i.jitter_ms).collect();
    let loss: Vec<f64> = inputs.iter().map(|i| i.loss_pct).collect();

    let n_rtt = normalize(&rtt);
    let n_jitter = normalize(&jitter);
    let n_loss = normalize(&loss);

    inputs
        .iter()
        .enumerate()
        .map(|(i, input)| {
            let base = weights.w_rtt * n_rtt[i]
                + weights.w_jitter * n_jitter[i]
                + weights.w_loss * n_loss[i]
                + weights.w_instability * input.instability;
            if input.available {
                base
            } else {
                base + UNAVAILABLE_PENALTY
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weights() -> ScoringWeights {
        ScoringWeights {
            w_rtt: 0.4,
            w_jitter: 0.25,
            w_loss: 0.30,
            w_instability: 0.05,
        }
    }

    fn input(rtt: f64, jitter: f64, loss: f64) -> ScoreInput {
        ScoreInput {
            rtt_p95_ms: rtt,
            jitter_ms: jitter,
            loss_pct: loss,
            instability: 0.0,
            available: true,
        }
    }

    #[test]
    fn lower_rtt_jitter_loss_scores_better() {
        let inputs = vec![input(20.0, 2.0, 0.0), input(80.0, 10.0, 5.0)];
        let scores = score_round(&inputs, &weights());
        assert!(
            scores[0] < scores[1],
            "expected first relay to score better (lower): {scores:?}"
        );
    }

    #[test]
    fn identical_candidates_score_equal() {
        let inputs = vec![input(30.0, 3.0, 1.0), input(30.0, 3.0, 1.0)];
        let scores = score_round(&inputs, &weights());
        assert!((scores[0] - scores[1]).abs() < f64::EPSILON);
    }

    #[test]
    fn unavailable_relay_always_scores_worse() {
        let inputs = vec![
            input(500.0, 50.0, 20.0), // bad but available
            ScoreInput {
                rtt_p95_ms: 10.0,
                jitter_ms: 1.0,
                loss_pct: 0.0,
                instability: 0.0,
                available: false,
            },
        ];
        let scores = score_round(&inputs, &weights());
        assert!(
            scores[0] < scores[1],
            "available relay should beat an unavailable one even with worse raw metrics: {scores:?}"
        );
    }

    #[test]
    fn instability_breaks_ties_between_otherwise_equal_relays() {
        let a = ScoreInput {
            rtt_p95_ms: 20.0,
            jitter_ms: 2.0,
            loss_pct: 0.0,
            instability: 0.0,
            available: true,
        };
        let b = ScoreInput {
            instability: 1.0,
            ..a
        };
        let scores = score_round(&[a, b], &weights());
        assert!(scores[0] < scores[1]);
    }
}
