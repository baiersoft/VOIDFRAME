//! Bootstrap-CI comparison used only by `cmd_calibrate`'s own warmup-cutoff/
//! CI-half-width diagnostics (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §11a) -- NOT part of WCPS v2's scoring
//! pipeline. This is a private copy of what used to be
//! `voidframe_engine::stats::{compare_metric, SIGNIFICANCE_METRICS}`
//! (`stats::significance`), relocated here when
//! `docs/superpowers/plans/2026-09-04-wcps-v3-pipeline-replace-v2.md`
//! deleted WCPS v2 from the engine outright: that plan's own research
//! didn't anticipate `cmd_calibrate` depending on this bootstrap-CI
//! machinery for an unrelated diagnostic purpose (finding where a
//! measure-loop series stabilizes), so removing it from the engine would
//! otherwise have broken `voidframe-cli calibrate`'s compile with no
//! replacement. The formula itself (95% bootstrap CI on a percentage delta,
//! paired when iteration counts match) is unchanged -- only its home moved.

use rand::Rng;
use voidframe_engine::model::results::{Metrics, Verdict};
use voidframe_engine::stats::{mean, percentile};

const BOOTSTRAP_RESAMPLES: usize = 2000;

/// One metric's bootstrap-CI comparison result -- the fields `cmd_calibrate`
/// actually reads (`verdict`, `ci_low_pct`/`ci_high_pct`).
pub struct CalibrationComparison {
    pub ci_low_pct: f64,
    pub ci_high_pct: f64,
    pub verdict: Verdict,
}

/// The metrics `cmd_calibrate`'s own warmup-cutoff search reports
/// significance for, and whether a lower value is the improvement (only
/// `frame_time_cv` is).
type MetricExtractor = (&'static str, fn(&Metrics) -> f64, bool);

pub const CALIBRATION_METRICS: &[MetricExtractor] = &[
    ("avg_fps", |m| m.avg_fps, false),
    ("p1_fps", |m| m.p1_fps, false),
    ("p01_fps", |m| m.p01_fps, false),
    ("frame_time_cv", |m| m.frame_time_cv, true),
];

fn resample(data: &[f64], rng: &mut impl Rng) -> Vec<f64> {
    let n = data.len();
    (0..n).map(|_| data[rng.gen_range(0..n)]).collect()
}

fn percentile_ci_95(mut deltas: Vec<f64>) -> (f64, f64) {
    deltas.sort_by(|a, b| a.partial_cmp(b).expect("bootstrap deltas are never NaN"));
    (percentile(&deltas, 2.5), percentile(&deltas, 97.5))
}

/// Compares one metric's per-iteration values between two windows via
/// bootstrap resampling. Paired (iteration indices resampled together) when
/// both have the same length; independent resampling otherwise.
/// `lower_is_better` flips which direction counts as an improvement.
pub fn compare_metric(
    baseline: &[f64],
    scenario: &[f64],
    lower_is_better: bool,
    rng: &mut impl Rng,
) -> CalibrationComparison {
    let raw_baseline_mean = mean(baseline);
    let baseline_mean_for_pct = if raw_baseline_mean.abs() < 1e-9 {
        1e-9_f64.copysign(raw_baseline_mean)
    } else {
        raw_baseline_mean
    };

    let deltas: Vec<f64> = if baseline.len() == scenario.len() {
        let n = baseline.len();
        (0..BOOTSTRAP_RESAMPLES)
            .map(|_| {
                let idx: Vec<usize> = (0..n).map(|_| rng.gen_range(0..n)).collect();
                let b: f64 = idx.iter().map(|&i| baseline[i]).sum::<f64>() / n as f64;
                let s: f64 = idx.iter().map(|&i| scenario[i]).sum::<f64>() / n as f64;
                (s - b) / baseline_mean_for_pct * 100.0
            })
            .collect()
    } else {
        (0..BOOTSTRAP_RESAMPLES)
            .map(|_| {
                let b = mean(&resample(baseline, rng));
                let s = mean(&resample(scenario, rng));
                (s - b) / baseline_mean_for_pct * 100.0
            })
            .collect()
    };

    let (ci_low_pct, ci_high_pct) = percentile_ci_95(deltas);

    let verdict = if lower_is_better {
        if ci_high_pct < 0.0 {
            Verdict::Better
        } else if ci_low_pct > 0.0 {
            Verdict::Worse
        } else {
            Verdict::NoMeasurableDifference
        }
    } else if ci_low_pct > 0.0 {
        Verdict::Better
    } else if ci_high_pct < 0.0 {
        Verdict::Worse
    } else {
        Verdict::NoMeasurableDifference
    };

    CalibrationComparison {
        ci_low_pct,
        ci_high_pct,
        verdict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    #[test]
    fn clearly_better_paired_data_yields_better() {
        let baseline = vec![99.0, 100.0, 101.0];
        let scenario = vec![149.0, 150.0, 151.0];
        let mut rng = StdRng::seed_from_u64(1);
        let cmp = compare_metric(&baseline, &scenario, false, &mut rng);
        assert_eq!(cmp.verdict, Verdict::Better);
    }

    #[test]
    fn overlapping_noisy_data_yields_no_measurable_difference() {
        let baseline = vec![95.0, 105.0, 98.0, 102.0, 100.0];
        let scenario = vec![97.0, 103.0, 101.0, 99.0, 96.0];
        let mut rng = StdRng::seed_from_u64(3);
        let cmp = compare_metric(&baseline, &scenario, false, &mut rng);
        assert_eq!(cmp.verdict, Verdict::NoMeasurableDifference);
    }

    #[test]
    fn lower_is_better_flips_the_verdict_direction() {
        let baseline = vec![0.20, 0.21, 0.19];
        let scenario = vec![0.09, 0.10, 0.08];
        let mut rng = StdRng::seed_from_u64(4);
        let cmp = compare_metric(&baseline, &scenario, true, &mut rng);
        assert_eq!(cmp.verdict, Verdict::Better);
    }
}
