//! Statistics: pure math primitives and per-scenario aggregation. WCPS v3's
//! own scoring/verdict/significance machinery lives in `v3::score`/
//! `v3::verdict`, not here. Everything here is OS-independent and consumes
//! only `crate::model::results::Metrics` — no process or file I/O.

pub mod v3;

use crate::capture::present_mode::misconfiguration_warning;
use crate::error::{Error, Result};
use crate::model::results::Metrics;
use std::collections::BTreeMap;

pub fn mean(data: &[f64]) -> f64 {
    debug_assert!(!data.is_empty(), "mean() called on empty data");
    data.iter().sum::<f64>() / data.len() as f64
}

/// Sample standard deviation (`n - 1` denominator). `0.0` for fewer than 2
/// values — a single sample has no defined spread.
pub fn stddev_sample(data: &[f64]) -> f64 {
    if data.len() < 2 {
        return 0.0;
    }
    let m = mean(data);
    let sum_sq: f64 = data.iter().map(|v| (v - m).powi(2)).sum();
    (sum_sq / (data.len() - 1) as f64).sqrt()
}

/// Linear-interpolation percentile (numpy's default `'linear'` method).
/// `data` MUST already be sorted ascending — this function does not sort.
pub fn percentile(data: &[f64], p: f64) -> f64 {
    debug_assert!(!data.is_empty(), "percentile() called on empty data");
    debug_assert!(
        data.windows(2).all(|w| w[0] <= w[1]),
        "percentile() requires data sorted ascending"
    );
    let n = data.len();
    if n == 1 {
        return data[0];
    }
    let idx = (p / 100.0) * (n - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = (lo + 1).min(n - 1);
    let frac = idx - lo as f64;
    data[lo] + (data[hi] - data[lo]) * frac
}

/// Averages many iterations' `Metrics` into one scenario-level `Metrics`.
/// `Option` fields average only their present values — an iteration
/// missing GPU data (e.g. driver didn't expose it that run) doesn't drag
/// the average toward zero.
pub fn aggregate_iterations(iters: &[Metrics]) -> Result<Metrics> {
    if iters.is_empty() {
        return Err(Error::msg("cannot aggregate zero iterations".into()));
    }
    let avg_fps = mean(&iters.iter().map(|m| m.avg_fps).collect::<Vec<_>>());
    let median_fps = mean(&iters.iter().map(|m| m.median_fps).collect::<Vec<_>>());
    let p1_fps = mean(&iters.iter().map(|m| m.p1_fps).collect::<Vec<_>>());
    let p01_fps = mean(&iters.iter().map(|m| m.p01_fps).collect::<Vec<_>>());
    let frame_time_mean_ms = mean(
        &iters
            .iter()
            .map(|m| m.frame_time_mean_ms)
            .collect::<Vec<_>>(),
    );
    let frame_time_stddev_ms = mean(
        &iters
            .iter()
            .map(|m| m.frame_time_stddev_ms)
            .collect::<Vec<_>>(),
    );
    let frame_time_cv = mean(&iters.iter().map(|m| m.frame_time_cv).collect::<Vec<_>>());
    let adaptive_frame_time_cv = mean(
        &iters
            .iter()
            .map(|m| m.adaptive_frame_time_cv)
            .collect::<Vec<_>>(),
    );
    let stutter_count_pct = mean(
        &iters
            .iter()
            .map(|m| m.stutter_count_pct)
            .collect::<Vec<_>>(),
    );
    let anim_vals: Vec<f64> = iters
        .iter()
        .filter_map(|m| m.mean_abs_animation_error_ms)
        .collect();
    let mean_abs_animation_error_ms = (!anim_vals.is_empty()).then(|| mean(&anim_vals));

    let gpu_vals: Vec<f64> = iters.iter().filter_map(|m| m.gpu_busy_ms).collect();
    let gpu_busy_ms = (!gpu_vals.is_empty()).then(|| mean(&gpu_vals));

    let bottleneck_vals: Vec<f64> = iters.iter().filter_map(|m| m.bottleneck_ratio).collect();
    let bottleneck_ratio = (!bottleneck_vals.is_empty()).then(|| mean(&bottleneck_vals));

    let latency_vals: Vec<f64> = iters.iter().filter_map(|m| m.render_latency_ms).collect();
    let render_latency_ms = (!latency_vals.is_empty()).then(|| mean(&latency_vals));

    // Present-mode summary, rolled up one level from the per-frame
    // `aggregate_metrics` (`capture::parser`,
    // `docs/superpowers/plans/2026-09-01-m1-phase-2-capture-and-scoring.md`):
    // the scenario's dominant mode
    // is whichever mode dominates the most iterations, it's "consistent"
    // only if every iteration was itself consistent AND all iterations
    // agree on the mode, and the warning uses the same compositor-bypass
    // check against the scenario-level dominant mode.
    let mut mode_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for it in iters {
        *mode_counts
            .entry(it.dominant_present_mode.as_str())
            .or_insert(0) += 1;
    }
    let dominant_present_mode = mode_counts
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(mode, _)| (*mode).to_string())
        .expect("iters is non-empty, checked above");
    let present_mode_consistent =
        mode_counts.len() == 1 && iters.iter().all(|it| it.present_mode_consistent);
    let present_mode_warning = misconfiguration_warning(&dominant_present_mode);

    Ok(Metrics {
        avg_fps,
        median_fps,
        p1_fps,
        p01_fps,
        frame_time_mean_ms,
        frame_time_stddev_ms,
        frame_time_cv,
        adaptive_frame_time_cv,
        stutter_count_pct,
        mean_abs_animation_error_ms,
        gpu_busy_ms,
        bottleneck_ratio,
        render_latency_ms,
        dominant_present_mode,
        present_mode_consistent,
        present_mode_warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::results::Metrics;

    fn m(avg_fps: f64, p1_fps: f64, p01_fps: f64, frame_time_cv: f64, gpu: Option<f64>) -> Metrics {
        Metrics {
            avg_fps,
            median_fps: avg_fps,
            p1_fps,
            p01_fps,
            frame_time_mean_ms: 1000.0 / avg_fps,
            frame_time_stddev_ms: frame_time_cv * (1000.0 / avg_fps),
            frame_time_cv,
            // Neutral stand-ins for tests that don't care about these
            // specific fields -- both CV-shaped, so `adaptive_frame_time_cv`
            // reuses `frame_time_cv` as a reasonable default.
            adaptive_frame_time_cv: frame_time_cv,
            stutter_count_pct: 0.0,
            mean_abs_animation_error_ms: None,
            gpu_busy_ms: gpu,
            bottleneck_ratio: gpu.map(|g| g / (1000.0 / avg_fps)),
            render_latency_ms: None,
            // Default value for callers that don't care about present-mode
            // fields specifically -- most tests below override these via
            // struct update syntax. `aggregate_iterations`'s present-mode
            // rollup (majority vote across iterations, cross-iteration
            // consistency, compositor-bypass warning) is exercised by the
            // dedicated tests below it, not by the frame-aggregation tests -- those only
            // cover the per-frame rollup in `capture::parser::aggregate_metrics`.
            dominant_present_mode: "Hardware: Independent Flip".into(),
            present_mode_consistent: true,
            present_mode_warning: None,
        }
    }

    #[test]
    fn aggregate_iterations_averages_each_field() {
        let iters = vec![
            m(390.0, 240.0, 170.0, 0.12, Some(2.0)),
            m(410.0, 260.0, 190.0, 0.08, None),
        ];
        let agg = aggregate_iterations(&iters).unwrap();
        assert!((agg.avg_fps - 400.0).abs() < 1e-9);
        // gpu_busy_ms: only one iteration had a value -- mean of just that
        // one value, not averaged against a phantom 0.0 for the other.
        assert_eq!(agg.gpu_busy_ms, Some(2.0));
    }

    #[test]
    fn aggregate_iterations_rejects_empty_input() {
        assert!(aggregate_iterations(&[]).is_err());
    }

    #[test]
    fn aggregate_iterations_averages_the_3_new_v3_metrics() {
        let iters = vec![
            Metrics {
                adaptive_frame_time_cv: 0.40,
                stutter_count_pct: 5.0,
                mean_abs_animation_error_ms: Some(0.10),
                ..m(390.0, 240.0, 170.0, 0.12, Some(2.0))
            },
            Metrics {
                adaptive_frame_time_cv: 0.30,
                stutter_count_pct: 3.0,
                mean_abs_animation_error_ms: Some(0.20),
                ..m(410.0, 260.0, 190.0, 0.08, None)
            },
        ];
        let agg = aggregate_iterations(&iters).unwrap();
        assert!((agg.adaptive_frame_time_cv - 0.35).abs() < 1e-9);
        assert!((agg.stutter_count_pct - 4.0).abs() < 1e-9);
        assert!((agg.mean_abs_animation_error_ms.unwrap() - 0.15).abs() < 1e-9);
    }

    #[test]
    fn aggregate_iterations_dominant_mode_is_a_majority_vote_and_disagreement_is_inconsistent() {
        let iters = vec![
            Metrics {
                dominant_present_mode: "Hardware: Independent Flip".into(),
                present_mode_consistent: true,
                ..m(400.0, 250.0, 180.0, 0.10, Some(2.0))
            },
            Metrics {
                dominant_present_mode: "Hardware: Independent Flip".into(),
                present_mode_consistent: true,
                ..m(410.0, 260.0, 190.0, 0.09, Some(2.0))
            },
            Metrics {
                dominant_present_mode: "Composed: Flip".into(),
                present_mode_consistent: true,
                ..m(390.0, 240.0, 170.0, 0.11, Some(2.0))
            },
        ];
        let agg = aggregate_iterations(&iters).unwrap();
        // 2 of 3 iterations report "Hardware: Independent Flip" -- majority wins.
        assert_eq!(agg.dominant_present_mode, "Hardware: Independent Flip");
        // Iterations disagree on the mode, so the scenario as a whole isn't
        // consistent even though every individual iteration was internally
        // consistent.
        assert!(!agg.present_mode_consistent);
    }

    #[test]
    fn aggregate_iterations_unknown_or_empty_dominant_mode_produces_no_warning() {
        // An unrecognised dominant present mode (Unknown, not Composed) --
        // no evidence of a compositor misconfiguration, so no warning
        // should claim CS2 isn't in exclusive Fullscreen.
        let iters = vec![
            Metrics {
                dominant_present_mode: "Something Future PresentMon Adds".into(),
                present_mode_consistent: true,
                ..m(400.0, 250.0, 180.0, 0.10, Some(2.0))
            },
            Metrics {
                dominant_present_mode: "Something Future PresentMon Adds".into(),
                present_mode_consistent: true,
                ..m(410.0, 260.0, 190.0, 0.09, Some(2.0))
            },
        ];
        let agg = aggregate_iterations(&iters).unwrap();
        assert_eq!(agg.present_mode_warning, None);

        // Reachable today via `Metrics`'s `#[serde(default)]` when
        // deserializing a legacy results.json that predates these fields --
        // an empty dominant mode also classifies as Unknown.
        let empty_iters = vec![
            Metrics {
                dominant_present_mode: "".into(),
                present_mode_consistent: true,
                ..m(400.0, 250.0, 180.0, 0.10, Some(2.0))
            },
            Metrics {
                dominant_present_mode: "".into(),
                present_mode_consistent: true,
                ..m(410.0, 260.0, 190.0, 0.09, Some(2.0))
            },
        ];
        let agg = aggregate_iterations(&empty_iters).unwrap();
        assert_eq!(agg.present_mode_warning, None);
    }

    #[test]
    fn aggregate_iterations_compositor_dominant_mode_produces_a_warning() {
        let iters = vec![
            Metrics {
                dominant_present_mode: "Composed: Flip".into(),
                present_mode_consistent: true,
                ..m(400.0, 250.0, 180.0, 0.10, Some(2.0))
            },
            Metrics {
                dominant_present_mode: "Composed: Flip".into(),
                present_mode_consistent: true,
                ..m(410.0, 260.0, 190.0, 0.09, Some(2.0))
            },
            Metrics {
                dominant_present_mode: "Hardware: Independent Flip".into(),
                present_mode_consistent: true,
                ..m(390.0, 240.0, 170.0, 0.11, Some(2.0))
            },
        ];
        let agg = aggregate_iterations(&iters).unwrap();
        // 2 of 3 iterations report "Composed: Flip" -- majority wins, and
        // that mode isn't compositor-bypassed, so a warning is expected.
        assert_eq!(agg.dominant_present_mode, "Composed: Flip");
        let warning = agg
            .present_mode_warning
            .expect("expected a warning for a Composed dominant mode");
        assert!(warning.contains("compositor"));
    }

    #[test]
    fn mean_of_known_values() {
        assert_eq!(mean(&[2.0, 4.0, 6.0]), 4.0);
    }

    #[test]
    fn stddev_sample_of_known_values() {
        // sample stddev of [2,4,4,4,5,5,7,9] is 2.13809... (textbook example)
        let data = [2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0];
        let s = stddev_sample(&data);
        assert!((s - 2.1380899353).abs() < 1e-6);
    }

    #[test]
    fn stddev_sample_single_value_is_zero() {
        assert_eq!(stddev_sample(&[5.0]), 0.0);
    }

    #[test]
    fn percentile_matches_linear_interpolation() {
        let data = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&data, 0.0), 1.0);
        assert_eq!(percentile(&data, 100.0), 5.0);
        assert_eq!(percentile(&data, 50.0), 3.0);
        // index = 0.25 * 4 = 1.0 exactly -> data[1] = 2.0
        assert_eq!(percentile(&data, 25.0), 2.0);
        // index = 0.90 * 4 = 3.6 -> interpolate data[3]..data[4] = 4.0 + 0.6*(5.0-4.0)
        assert!((percentile(&data, 90.0) - 4.6).abs() < 1e-9);
    }

    #[test]
    fn percentile_single_element() {
        assert_eq!(percentile(&[42.0], 99.0), 42.0);
    }
}
