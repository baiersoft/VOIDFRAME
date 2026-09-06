//! WCPS v3's composite score -- a direct structural port of
//! `study/research/scripts/real_data_trial_2.py` lines 288-299 (the exact
//! per-metric z-score-sum loop that trial script uses to compute and report
//! "WCPS v3" for every real scenario in this session's research). Same
//! mechanism as `stats::wcps::compute_wcps_v2` (baseline-relative z-score,
//! sign-flipped when lower is better, summed with fixed weights) extended
//! from 4 metrics to 6 -- `wcps-v3-design.md` §3b: "nothing in Studies
//! #1/#3 found the scoring mechanism itself broken... extend it with the
//! two new pacing dimensions."
//!
//! `z_score` below is intentionally a near-identical duplicate of
//! `stats::wcps::z_score` (same formula, same `SIGMA_FLOOR`), not imported
//! from that module -- matches this port's own established convention
//! (`crates/voidframe-engine/src/stats/v3/mod.rs`'s doc comment: "sits
//! alongside `significance`/`wcps`... not yet wired into `ScenarioResult`")
//! of keeping `stats::v3` fully independent of WCPS v2's module, so v2 can
//! be changed or removed later without touching this file.

use crate::stats::{mean, stddev_sample};

const SIGMA_FLOOR: f64 = 1e-9;

/// WCPS v3's default metric weights -- `study/research/cache/phase3_wcps_v3_weights.csv`'s
/// `final_weight` column exactly (two-block inverse-null-CV weighting,
/// 0.7 throughput / 0.3 pacing, `wcps-v3-design.md` §3b). Sums to 1.000.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WcpsV3Weights {
    pub avg_fps: f64,
    pub p1_fps: f64,
    pub p01_fps: f64,
    pub adaptive_frame_time_cv: f64,
    pub stutter_count_pct: f64,
    pub mean_abs_animation_error_ms: f64,
}

impl Default for WcpsV3Weights {
    fn default() -> Self {
        Self {
            avg_fps: 0.218_045_812_319_222_85,
            p1_fps: 0.177_168_495_207_251_6,
            p01_fps: 0.114_743_683_984_650_4,
            adaptive_frame_time_cv: 0.190_042_008_488_875_15,
            stutter_count_pct: 0.202_516_379_430_962_15,
            mean_abs_animation_error_ms: 0.097_483_620_569_037_82,
        }
    }
}

/// `(value - baseline_mean) / baseline_stddev`, sigma floored, sign-flipped
/// when `lower_is_better` -- identical formula to `stats::wcps::z_score`,
/// see this module's doc comment for why it's duplicated, not imported.
fn z_score(value: f64, baseline_values: &[f64], lower_is_better: bool) -> f64 {
    let mu = mean(baseline_values);
    let mut sigma = stddev_sample(baseline_values);
    if sigma.abs() < SIGMA_FLOOR {
        sigma = SIGMA_FLOOR;
    }
    let z = (value - mu) / sigma;
    if lower_is_better { -z } else { z }
}

/// Scores a scenario's aggregated (mean) throughput/pacing values against
/// the baseline's own per-iteration distribution. Positive = better than
/// baseline -- same sign convention as `compute_wcps_v2`, which is exactly
/// what lets `stats::v3::evaluate_verdict`
/// (`docs/superpowers/plans/2026-09-04-wcps-v3-score-and-verdict.md`) use
/// this score's sign to decide `Verdict::Better` vs `Verdict::Worse`.
///
/// Column order fixed to match every other `stats::v3` function's own
/// established convention: `throughput_*` is
/// `[avg_fps, p1_fps, p01_fps, adaptive_frame_time_cv]`, `pacing_*` is
/// `[stutter_count_pct, mean_abs_animation_error_ms]`.
pub fn compute_wcps_v3(
    throughput_baseline: &[[f64; 4]],
    pacing_baseline: &[[f64; 2]],
    throughput_scenario_mean: [f64; 4],
    pacing_scenario_mean: [f64; 2],
    weights: &WcpsV3Weights,
) -> f64 {
    let throughput_weights = [
        weights.avg_fps,
        weights.p1_fps,
        weights.p01_fps,
        weights.adaptive_frame_time_cv,
    ];
    // avg_fps, p1_fps, p01_fps: higher is better. adaptive_frame_time_cv:
    // lower is better -- same per-metric direction table
    // `real_data_trial_2.py`'s own `lower_is_better` dict uses.
    let throughput_lower_is_better = [false, false, false, true];

    let pacing_weights = [
        weights.stutter_count_pct,
        weights.mean_abs_animation_error_ms,
    ];
    // Both pacing metrics: lower is better.
    let pacing_lower_is_better = [true, true];

    let mut score = 0.0;
    for i in 0..4 {
        let baseline_vals: Vec<f64> = throughput_baseline.iter().map(|row| row[i]).collect();
        score += throughput_weights[i]
            * z_score(
                throughput_scenario_mean[i],
                &baseline_vals,
                throughput_lower_is_better[i],
            );
    }
    for i in 0..2 {
        let baseline_vals: Vec<f64> = pacing_baseline.iter().map(|row| row[i]).collect();
        score += pacing_weights[i]
            * z_score(
                pacing_scenario_mean[i],
                &baseline_vals,
                pacing_lower_is_better[i],
            );
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_weights_sum_to_one() {
        let w = WcpsV3Weights::default();
        let sum = w.avg_fps
            + w.p1_fps
            + w.p01_fps
            + w.adaptive_frame_time_cv
            + w.stutter_count_pct
            + w.mean_abs_animation_error_ms;
        assert!(
            (sum - 1.0).abs() < 1e-9,
            "weights sum to {sum}, expected 1.0"
        );
    }

    #[test]
    fn scenario_matching_baseline_mean_scores_near_zero() {
        let throughput_baseline = vec![[400.0, 250.0, 180.0, 0.10]; 3];
        let pacing_baseline = vec![[7.0, 0.22]; 3];
        let score = compute_wcps_v3(
            &throughput_baseline,
            &pacing_baseline,
            [400.0, 250.0, 180.0, 0.10],
            [7.0, 0.22],
            &WcpsV3Weights::default(),
        );
        assert!(score.abs() < 1e-6, "expected ~0.0, got {score}");
    }

    #[test]
    fn strictly_better_scenario_scores_positive() {
        let throughput_baseline = vec![
            [390.0, 240.0, 170.0, 0.12],
            [400.0, 250.0, 180.0, 0.10],
            [410.0, 260.0, 190.0, 0.08],
        ];
        let pacing_baseline = vec![[7.2, 0.24], [7.0, 0.22], [6.8, 0.20]];
        // higher fps/lower cv, lower stutter%/lower anim error -- better on every axis
        let score = compute_wcps_v3(
            &throughput_baseline,
            &pacing_baseline,
            [500.0, 350.0, 280.0, 0.05],
            [3.0, 0.10],
            &WcpsV3Weights::default(),
        );
        assert!(score > 0.0, "expected positive score, got {score}");
    }

    #[test]
    fn strictly_worse_scenario_scores_negative() {
        let throughput_baseline = vec![
            [390.0, 240.0, 170.0, 0.12],
            [400.0, 250.0, 180.0, 0.10],
            [410.0, 260.0, 190.0, 0.08],
        ];
        let pacing_baseline = vec![[7.2, 0.24], [7.0, 0.22], [6.8, 0.20]];
        let score = compute_wcps_v3(
            &throughput_baseline,
            &pacing_baseline,
            [300.0, 150.0, 100.0, 0.25],
            [15.0, 0.60],
            &WcpsV3Weights::default(),
        );
        assert!(score < 0.0, "expected negative score, got {score}");
    }
}
