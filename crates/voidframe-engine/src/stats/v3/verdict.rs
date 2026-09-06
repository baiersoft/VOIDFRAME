//! The combined WCPS v3 verdict, as the real `model::results::Verdict`:
//! `Better`/`Worse` (Hotelling OR-of-2, Bonferroni-style via the p97.5
//! threshold on EACH axis independently, direction from the newly-ported
//! WCPS v3 score's sign -- `wcps-v3-design.md` §3b's own stated invariant,
//! "the score and the verdict must never visibly contradict") takes
//! priority over `ConfirmedSame` (TOST AND-of-all on both axes); anything
//! left over is `Inconclusive`. Not a new decision rule for the
//! Different-vs-not split -- that's the exact formula this research trail's
//! own Python scripts already validated end-to-end
//! (`study/research/scripts/real_data_trial_2.py`,
//! `tost_confirmed_same_reliability.py`). The direction split IS new in
//! this task: Python's own `verdict` field was always just the 3-way
//! string (`Different`/`ConfirmedSame`/`Inconclusive`, no direction) --
//! this Rust port adds direction by reusing the score this same function
//! now also computes, not by inventing a second statistical test.

use super::{
    CalibratedThresholds, TostResult, WcpsV3Weights, compute_wcps_v3, hotelling_t2_statistic,
    tost_evaluate,
};
use crate::model::results::Verdict;
use rand::Rng;
use std::collections::BTreeMap;

/// `throughput_baseline`/`throughput_scenario`: (n, 4) rows in
/// `[avg_fps, p1_fps, p01_fps, adaptive_frame_time_cv]` order.
/// `pacing_baseline`/`pacing_scenario`: (n, 2) rows in
/// `[stutter_count_pct, mean_abs_animation_error_ms]` order. `n` is the
/// actual measure-iteration count (must have a calibrated entry in
/// `thresholds` -- see `CalibratedThresholds::for_n`'s own doc for the
/// currently-calibrated ladder). `throughput_margins` is the TOST
/// equivalence margins for the 4 throughput metrics (pacing's margins come
/// from `thresholds.pacing_tost_margins_pct` directly) -- see the note
/// below on why this stays a separate parameter.
///
/// `calibrated_thresholds_v3.json` only ever stored `pacing_tost_margins_pct`
/// (the 2 NEW metrics, whose margins this research trail actually
/// calibrated and revised) -- the 4 throughput metrics' TOST margins
/// (`3.0/3.0/3.0/10.0`) were an earlier, still-unrevised judgment call from
/// `candidate_tost.py`'s original `DEFAULT_MARGINS_PCT`, never added to the
/// JSON cache. `evaluate_verdict` cannot source throughput margins from
/// `CalibratedThresholds` the same way it sources pacing margins for that
/// reason -- a future task, out of scope here, could add one if the
/// throughput margins are ever swept and calibrated.
#[expect(
    clippy::too_many_arguments,
    reason = "pure wiring of 4 already-established pieces (Hotelling, TOST, score, calibrated thresholds); see verdict.rs module doc"
)]
pub fn evaluate_verdict(
    throughput_baseline: &[[f64; 4]],
    throughput_scenario: &[[f64; 4]],
    pacing_baseline: &[[f64; 2]],
    pacing_scenario: &[[f64; 2]],
    n: u32,
    thresholds: &CalibratedThresholds,
    throughput_metric_names: [&str; 4],
    pacing_metric_names: [&str; 2],
    throughput_margins: &BTreeMap<String, f64>,
    score_weights: &WcpsV3Weights,
    rng: &mut impl Rng,
) -> Option<(Verdict, TostResult, TostResult)> {
    let thr = thresholds.for_n(n)?;

    let t2_throughput = hotelling_t2_statistic(throughput_baseline, throughput_scenario);
    let t2_pacing = hotelling_t2_statistic(pacing_baseline, pacing_scenario);
    let flag_throughput = t2_throughput > thr.throughput.p97_5;
    let flag_pacing = t2_pacing > thr.pacing.p97_5;
    let different = flag_throughput || flag_pacing;

    let throughput_arrays: BTreeMap<String, (Vec<f64>, Vec<f64>)> = throughput_metric_names
        .iter()
        .enumerate()
        .map(|(i, &name)| {
            (
                name.to_string(),
                (
                    throughput_baseline.iter().map(|row| row[i]).collect(),
                    throughput_scenario.iter().map(|row| row[i]).collect(),
                ),
            )
        })
        .collect();
    let pacing_arrays: BTreeMap<String, (Vec<f64>, Vec<f64>)> = pacing_metric_names
        .iter()
        .enumerate()
        .map(|(i, &name)| {
            (
                name.to_string(),
                (
                    pacing_baseline.iter().map(|row| row[i]).collect(),
                    pacing_scenario.iter().map(|row| row[i]).collect(),
                ),
            )
        })
        .collect();

    let tost_throughput = tost_evaluate(&throughput_arrays, throughput_margins, rng);
    let tost_pacing = tost_evaluate(&pacing_arrays, &thresholds.pacing_tost_margins_pct, rng);
    let confirmed_same = tost_throughput.declared_same_all && tost_pacing.declared_same_all;

    let verdict = if different {
        // Direction from the score's sign -- computed here (not skipped
        // when `!different`) is unnecessary work only in the sense of an
        // unused value on the ConfirmedSame/Inconclusive paths; computing
        // it unconditionally keeps this function's control flow simple and
        // the cost is negligible (6 more z-scores) next to the Hotelling/
        // TOST work already done above.
        let throughput_scenario_mean = column_means_4(throughput_scenario);
        let pacing_scenario_mean = column_means_2(pacing_scenario);
        let score = compute_wcps_v3(
            throughput_baseline,
            pacing_baseline,
            throughput_scenario_mean,
            pacing_scenario_mean,
            score_weights,
        );
        if score >= 0.0 {
            Verdict::Better
        } else {
            Verdict::Worse
        }
    } else if confirmed_same {
        Verdict::ConfirmedSame
    } else {
        Verdict::Inconclusive
    };
    Some((verdict, tost_throughput, tost_pacing))
}

fn column_means_4(rows: &[[f64; 4]]) -> [f64; 4] {
    let n = rows.len() as f64;
    let mut out = [0.0_f64; 4];
    for row in rows {
        for j in 0..4 {
            out[j] += row[j];
        }
    }
    for v in &mut out {
        *v /= n;
    }
    out
}

fn column_means_2(rows: &[[f64; 2]]) -> [f64; 2] {
    let n = rows.len() as f64;
    let mut out = [0.0_f64; 2];
    for row in rows {
        for j in 0..2 {
            out[j] += row[j];
        }
    }
    for v in &mut out {
        *v /= n;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::v3::load_calibrated_thresholds;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    const THROUGHPUT_NAMES: [&str; 4] = ["avg_fps", "p1_fps", "p01_fps", "adaptive_frame_time_cv"];
    const PACING_NAMES: [&str; 2] = ["stutter_count_pct", "mean_abs_animation_error_ms"];

    fn throughput_margins() -> BTreeMap<String, f64> {
        [
            ("avg_fps", 3.0),
            ("p1_fps", 3.0),
            ("p01_fps", 3.0),
            ("adaptive_frame_time_cv", 10.0),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
    }

    #[test]
    fn identical_baseline_and_scenario_is_confirmed_same_not_different() {
        let thresholds = load_calibrated_thresholds().unwrap();
        let throughput = vec![[400.0, 250.0, 180.0, 0.40]; 5];
        let pacing = vec![[7.0, 0.22]; 5];
        let mut rng = StdRng::seed_from_u64(1);
        let (verdict, ..) = evaluate_verdict(
            &throughput,
            &throughput,
            &pacing,
            &pacing,
            5,
            &thresholds,
            THROUGHPUT_NAMES,
            PACING_NAMES,
            &throughput_margins(),
            &WcpsV3Weights::default(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(verdict, Verdict::ConfirmedSame);
    }

    #[test]
    fn a_clear_regression_is_worse_not_just_different() {
        // Same scenario the golden fixture's `vulkan_vs_baseline` case
        // uses conceptually: much lower fps, higher stutter/anim-error --
        // must flag Different AND resolve to Worse specifically, not just
        // "different, direction unknown."
        let thresholds = load_calibrated_thresholds().unwrap();
        let throughput_baseline = vec![
            [880.0, 338.0, 298.0, 0.41],
            [878.0, 337.0, 297.0, 0.41],
            [881.0, 339.0, 299.0, 0.42],
        ];
        let throughput_scenario = vec![
            [550.0, 200.0, 170.0, 0.55],
            [548.0, 199.0, 169.0, 0.56],
            [551.0, 201.0, 171.0, 0.55],
        ];
        let pacing_baseline = vec![[7.0, 0.22], [7.1, 0.23], [6.9, 0.21]];
        let pacing_scenario = vec![[12.0, 0.40], [12.2, 0.41], [11.8, 0.39]];
        let mut rng = StdRng::seed_from_u64(2);
        let (verdict, ..) = evaluate_verdict(
            &throughput_baseline,
            &throughput_scenario,
            &pacing_baseline,
            &pacing_scenario,
            3,
            &thresholds,
            THROUGHPUT_NAMES,
            PACING_NAMES,
            &throughput_margins(),
            &WcpsV3Weights::default(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(verdict, Verdict::Worse);
    }

    #[test]
    fn an_unc_calibrated_n_returns_none() {
        let thresholds = load_calibrated_thresholds().unwrap();
        let throughput = vec![[400.0, 250.0, 180.0, 0.40]; 6];
        let pacing = vec![[7.0, 0.22]; 6];
        let mut rng = StdRng::seed_from_u64(1);
        let result = evaluate_verdict(
            &throughput,
            &throughput,
            &pacing,
            &pacing,
            6,
            &thresholds,
            THROUGHPUT_NAMES,
            PACING_NAMES,
            &throughput_margins(),
            &WcpsV3Weights::default(),
            &mut rng,
        );
        assert!(
            result.is_none(),
            "n=6 has no calibrated threshold -- must return None, not silently use a neighboring n's value"
        );
    }

    #[test]
    fn n_equals_2_returns_none_through_the_full_pipeline_not_just_thresholds_in_isolation() {
        let thresholds = load_calibrated_thresholds().unwrap();
        let throughput = vec![[400.0, 250.0, 180.0, 0.40]; 2];
        let pacing = vec![[7.0, 0.22]; 2];
        let mut rng = StdRng::seed_from_u64(1);
        let result = evaluate_verdict(
            &throughput,
            &throughput,
            &pacing,
            &pacing,
            2,
            &thresholds,
            THROUGHPUT_NAMES,
            PACING_NAMES,
            &throughput_margins(),
            &WcpsV3Weights::default(),
            &mut rng,
        );
        assert!(
            result.is_none(),
            "n=2 must return None even though its threshold is present in the data -- it's known unreliable, not merely uncalibrated"
        );
    }
}
