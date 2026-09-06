//! Golden-value tests for the WCPS v3 Rust port
//! (docs/superpowers/plans/2026-09-04-wcps-v3-rust-port.md) -- every fixture
//! here was generated FROM the already-validated Python reference
//! implementation (see the plan above's fixture-generation task for the
//! exact generator script), not hand-computed. A mismatch means the Rust port has diverged from the
//! calibrated Python behavior, not that the fixture is wrong.

use serde::Deserialize;
use std::path::PathBuf;
use voidframe_engine::stats::v3::{
    WcpsV3Weights, adaptive_frame_time_cv, compute_wcps_v3, hotelling_t2_statistic,
    mean_abs_animation_error_ms, stutter_count_pct,
};

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/wcps_v3")
        .join(name)
}

#[derive(Deserialize)]
struct HotellingCase {
    label: String,
    p: usize,
    baseline: Vec<Vec<f64>>,
    scenario: Vec<Vec<f64>>,
    expected_t2: f64,
}

fn to_fixed<const P: usize>(rows: &[Vec<f64>]) -> Vec<[f64; P]> {
    rows.iter()
        .map(|r| {
            let arr: [f64; P] = r
                .as_slice()
                .try_into()
                .expect("fixture row has wrong width");
            arr
        })
        .collect()
}

#[test]
fn hotelling_t2_matches_the_python_reference_on_every_fixture_case() {
    let raw = std::fs::read_to_string(fixture_path("hotelling_cases.json")).unwrap();
    let cases: Vec<HotellingCase> = serde_json::from_str(&raw).unwrap();
    assert!(
        !cases.is_empty(),
        "fixture file is empty -- did Task 1 run?"
    );

    for case in &cases {
        // Relative tolerance: these statistics range from ~0 to several
        // thousand across the fixture set (small-n covariance can produce
        // large T^2 values, matching what this session's own calibration
        // work already observed at n=2) -- an absolute tolerance tight
        // enough for the small values would be needlessly loose for the
        // large ones, and vice versa.
        let tol = (case.expected_t2.abs() * 1e-6).max(1e-9);
        let actual = match case.p {
            4 => hotelling_t2_statistic::<4>(
                &to_fixed::<4>(&case.baseline),
                &to_fixed::<4>(&case.scenario),
            ),
            2 => hotelling_t2_statistic::<2>(
                &to_fixed::<2>(&case.baseline),
                &to_fixed::<2>(&case.scenario),
            ),
            other => panic!("fixture case {} has unsupported p={other}", case.label),
        };
        assert!(
            (actual - case.expected_t2).abs() < tol,
            "case {}: expected {}, got {actual} (tol {tol})",
            case.label,
            case.expected_t2
        );
    }
}

#[derive(Deserialize)]
struct MetricsCase {
    label: String,
    frame_times_ms: Vec<f64>,
    window_ms: f64,
    k: f64,
    expected_adaptive_frame_time_cv: f64,
    expected_stutter_count_pct: f64,
}

#[test]
fn pacing_metrics_match_the_python_reference_on_every_fixture_case() {
    let raw = std::fs::read_to_string(fixture_path("metrics_cases.json")).unwrap();
    let cases: Vec<MetricsCase> = serde_json::from_str(&raw).unwrap();
    assert!(
        !cases.is_empty(),
        "fixture file is empty -- did Task 1 run?"
    );

    for case in &cases {
        let actual_cv = adaptive_frame_time_cv(&case.frame_times_ms, case.window_ms);
        assert!(
            (actual_cv - case.expected_adaptive_frame_time_cv).abs() < 1e-9,
            "case {}: adaptive_frame_time_cv expected {}, got {actual_cv}",
            case.label,
            case.expected_adaptive_frame_time_cv
        );

        let actual_stutter = stutter_count_pct(&case.frame_times_ms, case.k);
        assert!(
            (actual_stutter - case.expected_stutter_count_pct).abs() < 1e-9,
            "case {}: stutter_count_pct expected {}, got {actual_stutter}",
            case.label,
            case.expected_stutter_count_pct
        );
    }
}

#[derive(Deserialize)]
struct AnimationErrorCase {
    label: String,
    animation_error_ms: Vec<f64>,
    expected_mean_abs_animation_error_ms: f64,
}

#[test]
fn mean_abs_animation_error_ms_matches_the_python_reference_on_real_csv_data() {
    // Unlike the other 2 metrics' cases (drawn from RawFrametimeStore, FPS
    // only), these are real MsAnimationError values read directly from raw
    // PresentMon CSVs by a hand-rolled csv.DictReader in the fixture
    // generator (study/research/scripts/generate_wcps_v3_fixtures.py) --
    // see that script's own comment for why it deliberately avoids both
    // RawFrametimeStore and pandas here.
    let raw = std::fs::read_to_string(fixture_path("animation_error_cases.json")).unwrap();
    let cases: Vec<AnimationErrorCase> = serde_json::from_str(&raw).unwrap();
    assert!(
        !cases.is_empty(),
        "fixture file is empty -- did the generator run?"
    );

    for case in &cases {
        let actual = mean_abs_animation_error_ms(&case.animation_error_ms);
        assert!(
            (actual - case.expected_mean_abs_animation_error_ms).abs() < 1e-9,
            "case {}: expected {}, got {actual}",
            case.label,
            case.expected_mean_abs_animation_error_ms
        );
    }
}

#[test]
fn embedded_thresholds_match_the_committed_fixture_snapshot() {
    // Confirms crates/voidframe-engine/data/calibrated_thresholds_v3.json
    // (what actually ships in the binary) is byte-for-byte the same data
    // as tests/fixtures/wcps_v3/thresholds_snapshot.json (the fixture-generation
    // task's copy, taken at the same moment) -- catches the two ever silently drifting
    // apart (e.g. one gets updated on a later re-calibration and the other
    // doesn't).
    let embedded = voidframe_engine::stats::v3::load_calibrated_thresholds().unwrap();
    let snapshot_raw = std::fs::read_to_string(fixture_path("thresholds_snapshot.json")).unwrap();
    let snapshot: voidframe_engine::stats::v3::CalibratedThresholds =
        serde_json::from_str(&snapshot_raw).unwrap();
    assert_eq!(embedded, snapshot);
}

#[derive(Deserialize)]
struct ScoreCase {
    label: String,
    throughput_baseline: Vec<Vec<f64>>,
    pacing_baseline: Vec<Vec<f64>>,
    throughput_scenario_mean: Vec<f64>,
    pacing_scenario_mean: Vec<f64>,
    expected_score: f64,
}

#[test]
fn wcps_v3_score_matches_the_python_reference_on_real_scenario_pairs() {
    let raw = std::fs::read_to_string(fixture_path("score_cases.json")).unwrap();
    let cases: Vec<ScoreCase> = serde_json::from_str(&raw).unwrap();
    assert!(
        !cases.is_empty(),
        "fixture file is empty -- did Task 2 Step 4 run?"
    );

    for case in &cases {
        let throughput_baseline = to_fixed::<4>(&case.throughput_baseline);
        let pacing_baseline = to_fixed::<2>(&case.pacing_baseline);
        let throughput_mean: [f64; 4] = case
            .throughput_scenario_mean
            .as_slice()
            .try_into()
            .expect("fixture throughput_scenario_mean has wrong width");
        let pacing_mean: [f64; 2] = case
            .pacing_scenario_mean
            .as_slice()
            .try_into()
            .expect("fixture pacing_scenario_mean has wrong width");

        let actual = compute_wcps_v3(
            &throughput_baseline,
            &pacing_baseline,
            throughput_mean,
            pacing_mean,
            &WcpsV3Weights::default(),
        );
        let tol = (case.expected_score.abs() * 1e-6).max(1e-6);
        assert!(
            (actual - case.expected_score).abs() < tol,
            "case {}: expected {}, got {actual} (tol {tol})",
            case.label,
            case.expected_score
        );
    }
}

#[test]
fn evaluate_verdict_on_identical_fixture_rows_is_always_confirmed_same_across_seeds() {
    // Not a Python-fixture comparison (the plan's TOST-evaluation task
    // explains why TOST can't be exact-fixture-matched) -- this instead confirms the
    // DETERMINISTIC half of evaluate_verdict (the Hotelling OR-of-2 check)
    // behaves identically regardless of which RNG seed drives the TOST
    // half, on a real fixture's identical-baseline-vs-itself case (T^2 is
    // then always exactly 0.0, so `different` must always be false,
    // regardless of TOST's stochastic outcome on the SAME data -- an
    // identical pair must be within any positive margin every time).
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use voidframe_engine::model::results::Verdict;
    use voidframe_engine::stats::v3::load_calibrated_thresholds;
    use voidframe_engine::stats::v3::{WcpsV3Weights, evaluate_verdict};

    let thresholds = load_calibrated_thresholds().unwrap();
    let throughput = vec![[880.2, 338.1, 298.2, 0.41]; 3];
    let pacing = vec![[7.0, 0.22]; 3];
    let throughput_margins: std::collections::BTreeMap<String, f64> = [
        ("avg_fps", 3.0),
        ("p1_fps", 3.0),
        ("p01_fps", 3.0),
        ("adaptive_frame_time_cv", 10.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    for seed in [1, 2, 3, 4, 5] {
        let mut rng = StdRng::seed_from_u64(seed);
        let (verdict, ..) = evaluate_verdict(
            &throughput,
            &throughput,
            &pacing,
            &pacing,
            3,
            &thresholds,
            ["avg_fps", "p1_fps", "p01_fps", "adaptive_frame_time_cv"],
            ["stutter_count_pct", "mean_abs_animation_error_ms"],
            &throughput_margins,
            &WcpsV3Weights::default(),
            &mut rng,
        )
        .unwrap();
        assert_eq!(
            verdict,
            Verdict::ConfirmedSame,
            "seed {seed}: identical data must never flag Different"
        );
    }
}
