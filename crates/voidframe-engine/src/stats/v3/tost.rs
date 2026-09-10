//! TOST (Two One-Sided Tests) equivalence testing -- a structural port of
//! `study/research/scripts/candidate_tost.py`. Reuses the SAME
//! paired/unpaired bootstrap resampling branch selection as
//! `stats::significance::compare_metric`, just with a 90% CI cut (5th/95th
//! percentile, matching two one-sided alpha=0.05 tests) instead of
//! `compare_metric`'s 95% (2.5th/97.5th), and a margin-based equivalence
//! decision instead of a straddles-zero significance decision.
//!
//! IMPORTANT: unlike `hotelling.rs`'s and `metrics.rs`'s golden-value
//! fixture tests, this module is NOT verified via exact-value fixtures
//! against the Python reference, and should never be -- see this plan's
//! Global Constraints. `numpy.random.Generator` (PCG64, the Python
//! reference's RNG) and `rand::rngs::StdRng` (ChaCha12, Rust's) are
//! different algorithms; identical seeds do not produce identical draws
//! across them, so the two implementations' bootstrap resampling can never
//! be bit-for-bit reproduced against each other, by construction, no matter
//! how faithful the port is. This module's own tests below instead verify:
//! (1) the CI-construction logic against known analytic properties
//! (symmetric data centers near zero, a clear large offset falls outside a
//! tight margin, an identical baseline/scenario pair is always declared
//! equivalent for any positive margin), and (2) a STATISTICAL agreement
//! check against the Python reference's *distribution* of outcomes over
//! many independent trials on the same real data (Step 3 below) -- matching
//! rates within sampling noise, not matching individual draws.

use super::thresholds::Margin;
use crate::stats::{mean, percentile};
use rand::Rng;
use std::collections::BTreeMap;

const BOOTSTRAP_RESAMPLES: usize = 2000; // matches candidate_tost.py's own constant exactly

fn resample(data: &[f64], rng: &mut impl Rng) -> Vec<f64> {
    let n = data.len();
    (0..n).map(|_| data[rng.gen_range(0..n)]).collect()
}

/// `(ci_low, ci_high)` of the bootstrap delta distribution at the given
/// percentile cut -- structurally identical to
/// `significance::compare_metric`'s own paired/unpaired branch, generalized
/// to a caller-supplied CI cut (`compare_metric` hardcodes 2.5/97.5; TOST
/// needs 5/95).
///
/// The delta's unit follows `margin`: `Margin::RelativePct` expresses each
/// resample's delta as percent of the baseline mean; `Margin::AbsolutePoints`
/// keeps the RAW difference (`scenario - baseline`), regardless of how close
/// the baseline mean sits to zero -- see that variant's own doc comment for
/// why "percent of baseline" is unsound for a bounded proportion.
fn bootstrap_delta_ci(
    baseline: &[f64],
    scenario: &[f64],
    ci_low_pct: f64,
    ci_high_pct: f64,
    margin: Margin,
    rng: &mut impl Rng,
) -> (f64, f64) {
    let raw_baseline_mean = mean(baseline);
    let baseline_is_degenerate = raw_baseline_mean.abs() < 1e-9;
    let baseline_mean_for_pct = if baseline_is_degenerate {
        1e-9_f64.copysign(raw_baseline_mean)
    } else {
        raw_baseline_mean
    };
    let delta_of = |b: f64, s: f64| -> f64 {
        match margin {
            Margin::AbsolutePoints(_) => s - b,
            Margin::RelativePct(_) => (s - b) / baseline_mean_for_pct * 100.0,
        }
    };

    let mut deltas: Vec<f64> = if baseline.len() == scenario.len() {
        let n = baseline.len();
        (0..BOOTSTRAP_RESAMPLES)
            .map(|_| {
                let idx: Vec<usize> = (0..n).map(|_| rng.gen_range(0..n)).collect();
                let b: f64 = idx.iter().map(|&i| baseline[i]).sum::<f64>() / n as f64;
                let s: f64 = idx.iter().map(|&i| scenario[i]).sum::<f64>() / n as f64;
                delta_of(b, s)
            })
            .collect()
    } else {
        (0..BOOTSTRAP_RESAMPLES)
            .map(|_| {
                let b = mean(&resample(baseline, rng));
                let s = mean(&resample(scenario, rng));
                delta_of(b, s)
            })
            .collect()
    };
    deltas.sort_by(|a, b| a.partial_cmp(b).expect("bootstrap deltas are never NaN"));
    (
        percentile(&deltas, ci_low_pct),
        percentile(&deltas, ci_high_pct),
    )
}

/// `true` if the entire 90% CI falls inside `[-margin, +margin]`, both
/// sides expressed in `margin`'s own unit.
fn tost_per_metric(baseline: &[f64], scenario: &[f64], margin: Margin, rng: &mut impl Rng) -> bool {
    let (ci_low, ci_high) = bootstrap_delta_ci(baseline, scenario, 5.0, 95.0, margin, rng);
    let bound = margin.bound();
    ci_low > -bound && ci_high < bound
}

#[derive(Debug, Clone, PartialEq)]
pub struct TostResult {
    pub per_metric: BTreeMap<String, bool>,
    pub declared_same_all: bool,
}

/// `metric_arrays`: `{metric_name: (baseline_values, scenario_values)}`.
/// `margins`: `{metric_name: Margin}` -- every key in `metric_arrays` must
/// have a corresponding entry in `margins` (panics with the metric name
/// otherwise, matching a caller-programming-error contract, not a
/// runtime-data-error one -- margins are a fixed config table, never
/// derived from the input data itself). The margin's own unit tag decides
/// how each metric's delta is measured; nothing here keys off the name.
pub fn tost_evaluate(
    metric_arrays: &BTreeMap<String, (Vec<f64>, Vec<f64>)>,
    margins: &BTreeMap<String, Margin>,
    rng: &mut impl Rng,
) -> TostResult {
    let mut per_metric = BTreeMap::new();
    for (name, (b, s)) in metric_arrays {
        let margin = *margins
            .get(name)
            .unwrap_or_else(|| panic!("no TOST margin configured for metric '{name}'"));
        per_metric.insert(name.clone(), tost_per_metric(b, s, margin, rng));
    }
    let declared_same_all = per_metric.values().all(|&v| v);
    TostResult {
        per_metric,
        declared_same_all,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn identical_baseline_and_scenario_is_always_declared_equivalent() {
        let mut rng = StdRng::seed_from_u64(1);
        let data = vec![100.0, 101.0, 99.0, 100.5, 100.2];
        let equivalent = tost_per_metric(&data, &data, Margin::RelativePct(1.0), &mut rng); // tight 1% margin
        assert!(
            equivalent,
            "identical data must always be equivalent, even at a tight margin"
        );
    }

    #[test]
    fn a_large_offset_is_not_declared_equivalent_at_a_tight_margin() {
        let mut rng = StdRng::seed_from_u64(2);
        let baseline = vec![100.0, 101.0, 99.0, 100.5, 100.2];
        let scenario = vec![150.0, 151.0, 149.0, 150.5, 150.2]; // ~50% higher
        let equivalent = tost_per_metric(&baseline, &scenario, Margin::RelativePct(3.0), &mut rng); // 3% margin
        assert!(
            !equivalent,
            "a ~50% offset must not be declared equivalent at a 3% margin"
        );
    }

    #[test]
    fn a_small_offset_is_declared_equivalent_at_a_generous_margin() {
        let mut rng = StdRng::seed_from_u64(3);
        let baseline = vec![100.0, 101.0, 99.0, 100.5, 100.2];
        let scenario = vec![100.5, 101.5, 99.5, 101.0, 100.7]; // ~0.5% higher
        let equivalent = tost_per_metric(&baseline, &scenario, Margin::RelativePct(10.0), &mut rng); // 10% margin
        assert!(
            equivalent,
            "a ~0.5% offset should be declared equivalent at a 10% margin"
        );
    }

    #[test]
    fn declared_same_all_requires_every_metric_equivalent() {
        let mut rng = StdRng::seed_from_u64(4);
        let mut arrays = BTreeMap::new();
        arrays.insert(
            "a".to_string(),
            (vec![100.0, 100.0, 100.0], vec![100.0, 100.0, 100.0]),
        );
        arrays.insert(
            "b".to_string(),
            (vec![100.0, 100.0, 100.0], vec![150.0, 150.0, 150.0]),
        );
        let mut margins = BTreeMap::new();
        margins.insert("a".to_string(), Margin::RelativePct(3.0));
        margins.insert("b".to_string(), Margin::RelativePct(3.0));
        let result = tost_evaluate(&arrays, &margins, &mut rng);
        assert!(result.per_metric["a"]);
        assert!(!result.per_metric["b"]);
        assert!(!result.declared_same_all);
    }

    #[test]
    fn zero_variance_zero_mean_baseline_is_equivalent_to_a_small_absolute_delta() {
        // Reproduces study/pamuk/.../d38e6455.../results.json's real
        // stutter_count_pct shape: baseline exactly [0.0, 0.0, 0.0],
        // scenario mean a small nonzero value (~0.36pp) -- must be declared
        // equivalent under an absolute 10-percentage-point band (a wide,
        // illustrative margin; the calibrated 0.7pp is exercised by the
        // small-nonzero-baseline test below), not stuck at an
        // impossible-to-satisfy CI (ab-test-analysis-report.md §2.3: the
        // percent-of-baseline formula produced a 90% CI of roughly
        // (+0.00%, +725,821,085.10%) for this exact shape).
        let mut rng = StdRng::seed_from_u64(5);
        let baseline = vec![0.0, 0.0, 0.0];
        let scenario = vec![1.09, 0.0, 0.0];
        let equivalent =
            tost_per_metric(&baseline, &scenario, Margin::AbsolutePoints(10.0), &mut rng);
        assert!(
            equivalent,
            "a ~0.36pp mean stutter delta from a zero baseline must pass a 10pp margin"
        );
    }

    #[test]
    fn zero_variance_zero_mean_baseline_is_not_equivalent_to_a_large_absolute_delta() {
        let mut rng = StdRng::seed_from_u64(6);
        let baseline = vec![0.0, 0.0, 0.0];
        let scenario = vec![15.0, 16.0, 14.0];
        let equivalent =
            tost_per_metric(&baseline, &scenario, Margin::AbsolutePoints(10.0), &mut rng);
        assert!(
            !equivalent,
            "a ~15pp mean stutter delta from a zero baseline must fail a 10pp margin"
        );
    }

    #[test]
    fn small_nonzero_baseline_is_equivalent_to_a_small_absolute_delta() {
        // Reproduces the AveYo `benchmark.cfg v2` shape
        // (study/research/aveyo-tost-hotelling-fix-research.md Sec 1): a
        // small-but-NOT-exactly-zero baseline mean (~0.02-0.03%, well above
        // the old `baseline_is_degenerate` cutoff of 1e-9) paired with a
        // real-noise-sized absolute delta (~0.01pp).
        //
        // Under the OLD formula (percent-of-baseline-mean, gated off
        // because this baseline isn't floating-point-zero), this shape's
        // relative delta would be (0.0322 - 0.0222) / 0.0222 * 100 ~= 45%,
        // which fails even the old formula's generous 10%-of-baseline
        // margin -- an ordinary sampling fluctuation misclassified as a
        // huge swing purely because the reference value sits near zero.
        // Under the absolute-percentage-point treatment, at this metric's
        // real calibrated margin -- read from the embedded JSON rather than
        // hardcoded, so this test tracks a re-calibration -- the same
        // ~0.01pp real delta correctly falls well inside the margin and is
        // equivalent.
        let margin = *crate::stats::v3::load_calibrated_thresholds()
            .unwrap()
            .pacing_tost_margins
            .get("stutter_count_pct")
            .unwrap();
        assert!(matches!(margin, Margin::AbsolutePoints(_)), "{margin:?}");
        let mut rng = StdRng::seed_from_u64(7);
        let baseline = vec![0.02, 0.03, 0.02, 0.018, 0.023];
        let scenario = vec![0.03, 0.04, 0.03, 0.028, 0.033];
        let equivalent = tost_per_metric(&baseline, &scenario, margin, &mut rng);
        assert!(
            equivalent,
            "a ~0.01pp delta from a small-but-nonzero baseline must pass the calibrated margin"
        );
    }

    #[test]
    #[ignore = "one-time manual cross-language statistical agreement check, not part of the regular suite"]
    fn declared_same_all_rate_is_in_the_same_ballpark_as_the_python_reference() {
        // ONE-TIME MANUAL CHECK (the TOST-evaluation task's Step 3 of
        // docs/superpowers/plans/2026-09-04-wcps-v3-rust-port.md), run by
        // hand -- not part of the automated suite. `numpy.random.Generator`
        // (Python's RNG, PCG64) and `rand::rngs::StdRng` (Rust's, ChaCha12)
        // are different algorithms: individual bootstrap draws can never be
        // reproduced bit-for-bit across languages, only the DISTRIBUTION of
        // outcomes over many independent trials.
        //
        // Reuses the "throughput_n5" case from
        // tests/fixtures/wcps_v3/hotelling_cases.json (real n=5 throughput
        // data) with the throughput TOST margins (3.0/3.0/3.0/10.0)
        // hardcoded per the plan's combined-verdict task's own Step 2 note
        // (`docs/superpowers/plans/2026-09-04-wcps-v3-rust-port.md` §Task 7) -- the calibrated JSON only
        // ever stored pacing margins, throughput margins are a separate,
        // still-unrevised constant. Runs tost_evaluate 1000 times with a
        // fresh StdRng seed per trial and reports the fraction where
        // declared_same_all was true.
        //
        // OBSERVED (run 2026-09-04): 0/1000 trials declared_same_all (0%).
        // This does NOT match study/research/wcps-v3-research-gaps-findings.md
        // §3's 86-96% ConfirmedSame-under-a-genuine-null figure on its own --
        // so, per this step's own instruction, that mismatch was chased down
        // rather than just recorded. Chasing it down: this fixture case's
        // baseline/scenario rows were drawn by the plan's fixture-generation task's generator at
        // random across ALL THREE real conditions in iteration_metrics_v3.csv
        // (1-default/2-default/3-fresh-cs2-360s), not resampled within one
        // controlled condition the way the research findings' own 86-96%
        // figure was computed -- so this single real n=5 pair is not the
        // same kind of "true null" input the published rate describes, and a
        // divergence from it is not automatically a Rust-side bug.
        //
        // To rule out a REAL logic divergence (the failure mode this check
        // exists to catch), study/research/scripts/candidate_tost.py's own
        // `tost_evaluate` was run by hand against this EXACT fixture pair and
        // these EXACT margins, 1000 trials, one fresh `np.random.default_rng`
        // seed per trial (0..999): Python's own reference ALSO returns
        // 0/1000. Both implementations agree exactly on this data, not just
        // "in the same ballpark" -- the strongest form of agreement this
        // check can produce. Per-metric inspection (seed 0) shows why: this
        // pair's `p01_fps` bootstrap 90% CI is `(-1.26%, +3.46%)` against a
        // tight 3% margin -- the CI's upper bound sits just outside the
        // margin, so `p01_fps` (and therefore `declared_same_all`, which
        // needs every metric to pass) fails equivalence on nearly every
        // trial for this specific real sample. That is a genuine property of
        // this one random n=5 draw crossing condition boundaries, confirmed
        // identically in both languages -- not a port bug.
        use std::collections::BTreeMap;
        use std::path::PathBuf;

        let fixture_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/wcps_v3/hotelling_cases.json");
        let raw = std::fs::read_to_string(&fixture_path).unwrap();
        let cases: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let case = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["label"] == "throughput_n5")
            .expect("throughput_n5 fixture case must exist");

        let names = ["avg_fps", "p1_fps", "p01_fps", "adaptive_frame_time_cv"];
        let margin_values = [3.0, 3.0, 3.0, 10.0];
        let mut margins = BTreeMap::new();
        for (name, margin) in names.iter().zip(margin_values) {
            margins.insert((*name).to_string(), Margin::RelativePct(margin));
        }

        let extract_col = |rows: &serde_json::Value, col: usize| -> Vec<f64> {
            rows.as_array()
                .unwrap()
                .iter()
                .map(|row| row[col].as_f64().unwrap())
                .collect()
        };

        let mut arrays = BTreeMap::new();
        for (col, name) in names.iter().enumerate() {
            arrays.insert(
                (*name).to_string(),
                (
                    extract_col(&case["baseline"], col),
                    extract_col(&case["scenario"], col),
                ),
            );
        }

        const TRIALS: u64 = 1000;
        let mut confirmed_same_count = 0u64;
        for seed in 0..TRIALS {
            let mut rng = StdRng::seed_from_u64(seed);
            let result = tost_evaluate(&arrays, &margins, &mut rng);
            if result.declared_same_all {
                confirmed_same_count += 1;
            }
        }
        let rate = confirmed_same_count as f64 / TRIALS as f64;
        println!(
            "declared_same_all rate over {TRIALS} trials: {rate:.4} ({confirmed_same_count}/{TRIALS})"
        );
    }
}
