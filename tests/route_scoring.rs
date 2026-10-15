//! Integration-level sanity checks for `routing::score` through the
//! public API, complementing the pairwise unit tests already inside
//! `src/routing/score.rs` with a 3-way ranking check.

use route_agent::relays::ScoringWeights;
use route_agent::routing::score::{score_round, ScoreInput};

fn weights() -> ScoringWeights {
    ScoringWeights {
        w_rtt: 0.4,
        w_jitter: 0.25,
        w_loss: 0.30,
        w_instability: 0.05,
    }
}

#[test]
fn a_clearly_better_relay_gets_the_lowest_score() {
    let good = ScoreInput {
        rtt_p95_ms: 18.0,
        jitter_ms: 1.2,
        loss_pct: 0.0,
        instability: 0.0,
        available: true,
    };
    let mediocre = ScoreInput {
        rtt_p95_ms: 42.0,
        jitter_ms: 6.0,
        loss_pct: 1.0,
        instability: 0.0,
        available: true,
    };
    let bad = ScoreInput {
        rtt_p95_ms: 120.0,
        jitter_ms: 25.0,
        loss_pct: 8.0,
        instability: 0.3,
        available: true,
    };

    let scores = score_round(&[good, mediocre, bad], &weights());

    assert!(scores[0] < scores[1], "scores: {scores:?}");
    assert!(scores[1] < scores[2], "scores: {scores:?}");
}

#[test]
fn an_unreachable_relay_never_wins_even_against_a_poor_but_reachable_one() {
    let poor_but_up = ScoreInput {
        rtt_p95_ms: 300.0,
        jitter_ms: 40.0,
        loss_pct: 15.0,
        instability: 0.5,
        available: true,
    };
    let unreachable = ScoreInput {
        rtt_p95_ms: 5.0,
        jitter_ms: 0.1,
        loss_pct: 0.0,
        instability: 0.0,
        available: false,
    };

    let scores = score_round(&[poor_but_up, unreachable], &weights());

    assert!(
        scores[0] < scores[1],
        "the reachable relay should win despite worse raw metrics: {scores:?}"
    );
}
