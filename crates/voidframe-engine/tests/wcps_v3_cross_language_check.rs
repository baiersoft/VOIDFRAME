//! One-time manual cross-language checks for the WCPS v3 Rust port --
//! genuinely independent verification: this file parses REAL raw PresentMon
//! CSVs directly via the production `capture::parse_presentmon_csv`, computes
//! every WCPS v3 metric via `stats::v3`, and runs the full `evaluate_verdict`
//! pipeline -- completely independent of Python, reading the same raw CSV
//! files `study/research/scripts/real_data_trial_2.py` reads directly (its
//! own `load_iteration_frames`, an unrelated pandas-based code path). Neither
//! side consumes the other's output, and neither reads from any shared
//! precomputed cache (`iteration_metrics_v3.csv` etc.) -- both start from the
//! same raw `.csv` files on disk and compute everything themselves.
//!
//! NOT part of the regular test suite (`#[ignore]`d) -- these read from
//! `study/real-runs/` (gitignored, present on this dev machine only, not
//! shipped with the crate) and their purpose (comparing against one specific
//! Python run's printed output) is a one-time manual verification, same
//! philosophy as `stats::v3::tost`'s own single-pair check.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rand::SeedableRng;
use rand::rngs::StdRng;
use voidframe_engine::capture::{aggregate_metrics, parse_presentmon_csv};
use voidframe_engine::model::results::Verdict;
use voidframe_engine::stats::v3::{
    Margin, WcpsV3Weights, adaptive_frame_time_cv, evaluate_verdict, hotelling_t2_statistic,
    load_calibrated_thresholds, mean_abs_animation_error_ms, stutter_count_pct, tost_evaluate,
};

const ADAPTIVE_WINDOW_MS: f64 = 500.0;
const STUTTER_K: f64 = 2.0;

fn real_runs_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = crates/voidframe-engine -- repo root is 2 levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../study/real-runs")
}

fn real_run_dir(run: &str, scenario: &str) -> PathBuf {
    real_runs_root()
        .join(run)
        .join(format!("scenario-{scenario}"))
}

fn throughput_margins() -> BTreeMap<String, Margin> {
    [
        ("avg_fps", 3.0),
        ("p1_fps", 3.0),
        ("p01_fps", 3.0),
        ("adaptive_frame_time_cv", 10.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Margin::RelativePct(v)))
    .collect()
}

/// Parses one real iteration's CSV via the actual PRODUCTION parser
/// (`capture::parse_presentmon_csv`) and computes this iteration's
/// throughput/pacing vectors -- `avg_fps`/`p1_fps`/`p01_fps` reused directly
/// from `capture::aggregate_metrics` (WCPS v2's own, unchanged formula, not
/// reimplemented here), `adaptive_frame_time_cv`/`stutter_count_pct`/
/// `mean_abs_animation_error_ms` from `stats::v3`.
fn load_iteration(csv_path: &Path) -> ([f64; 4], [f64; 2]) {
    let samples = parse_presentmon_csv(csv_path)
        .unwrap_or_else(|e| panic!("parsing {}: {e}", csv_path.display()));
    let old = aggregate_metrics(&samples)
        .unwrap_or_else(|e| panic!("aggregating {}: {e}", csv_path.display()));
    let frame_times: Vec<f64> = samples.iter().map(|s| s.frame_time_ms).collect();
    let adaptive_cv = adaptive_frame_time_cv(&frame_times, ADAPTIVE_WINDOW_MS);
    let stutter_pct = stutter_count_pct(&frame_times, STUTTER_K);
    let anim_err: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.animation_error_ms)
        .collect();
    let mean_anim = mean_abs_animation_error_ms(&anim_err);
    (
        [old.avg_fps, old.p1_fps, old.p01_fps, adaptive_cv],
        [stutter_pct, mean_anim],
    )
}

fn load_scenario(dir: &Path) -> (Vec<[f64; 4]>, Vec<[f64; 2]>) {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("iter-") && n.ends_with(".csv"))
        })
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "no iter-*.csv files found in {}",
        dir.display()
    );
    let mut throughput = Vec::new();
    let mut pacing = Vec::new();
    for p in paths {
        let (t, pa) = load_iteration(&p);
        throughput.push(t);
        pacing.push(pa);
    }
    (throughput, pacing)
}

struct RealPair {
    label: &'static str,
    scenario_dir: &'static str,
    /// The verdict `study/research/scripts/real_data_trial_2.py` printed for
    /// this exact pair, rerun fresh this session (2026-09-04, unmodified
    /// script, its own independent pandas-based CSV read) -- see this test's
    /// own doc comment for the full printed block this was copied from.
    /// Python's own `verdict` field is still the 3-way string
    /// (`Different`/`ConfirmedSame`/`Inconclusive`, no direction) -- where
    /// this Rust port's `Verdict::Better`/`Verdict::Worse` split is used
    /// below (in place of Python's undifferentiated `"Different"`), the
    /// direction is this Rust side's own independently-true refinement, not
    /// sourced from a 4th Python string value that doesn't exist. See the
    /// note on the `-vulkan` case below.
    python_verdict: Verdict,
}

#[test]
#[ignore = "one-time manual end-to-end cross-language check against a real, independently-computed Python verdict -- reads study/real-runs/ (gitignored, dev-machine only)"]
fn evaluate_verdict_matches_the_independently_computed_python_verdict_on_real_data() {
    // Both pairs from study/real-runs/9b700bea-wcps-v3-matrix-pruned/, the
    // exact dataset study/research/wcps-v3-real-data-trial-2.md documents.
    // Python side: `study/research/scripts/real_data_trial_2.py` rerun fresh
    // (2026-09-04, unmodified) -- its OWN independent CSV read
    // (`load_iteration_frames`, pandas, direct from the same raw files), no
    // shared code path with this Rust side at all. Printed output for these
    // two pairs (all 5 RNG seeds agreed in both cases):
    //
    //   SCENARIO scen_control_1 (Control 1, true no-op) vs baseline:
    //     T2_thr=19.525(thr 43.414,flag=False) T2_pace=0.369(thr 14.228,flag=False)
    //     verdict=ConfirmedSame
    //
    //   SCENARIO scen_1788488762212 (-vulkan rep 1) vs baseline:
    //     T2_thr=64755.562(thr 43.414,flag=True) T2_pace=81387.664(thr 14.228,flag=True)
    //     verdict=Different
    //
    // Python's own `verdict` field is still the 3-way string above
    // (`Different`, not `Worse`) -- this Rust port's `python_verdict` field
    // below asserts `Verdict::Worse` for this pair, not because Python
    // printed that word (it doesn't have a 4th verdict string), but because
    // this pair's real fps/stutter/anim-error deltas are unambiguously
    // negative (confirmed by `wcps-v3-real-data-trial-2.md`'s own published
    // numbers) -- this Rust port's own directional refinement of Python's
    // undifferentiated `Different`, independently true, not Python-sourced.
    let run = "9b700bea-wcps-v3-matrix-pruned";
    let pairs = [
        RealPair {
            label: "Control 1 (no-op) vs baseline",
            scenario_dir: "scen_control_1",
            python_verdict: Verdict::ConfirmedSame, // was Verdict3::ConfirmedSame -- Control 1 vs baseline
        },
        RealPair {
            label: "-vulkan (rep 1) vs baseline",
            scenario_dir: "scen_1788488762212",
            python_verdict: Verdict::Worse, // was Verdict3::Different -- -vulkan vs baseline is a real ~38% fps regression, not just "different, direction unspecified"; strengthens this check now that direction exists
        },
    ];

    let thresholds = load_calibrated_thresholds().unwrap();
    let margins = throughput_margins();
    let (base_t, base_p) = load_scenario(&real_run_dir(run, "baseline"));
    assert_eq!(base_t.len(), 5, "expected 5 baseline iterations");

    for pair in pairs {
        let (scen_t, scen_p) = load_scenario(&real_run_dir(run, pair.scenario_dir));
        assert_eq!(
            scen_t.len(),
            5,
            "{}: expected 5 scenario iterations",
            pair.label
        );

        // T2 statistics are deterministic (no RNG) -- compute directly for a
        // numeric cross-check against Python's printed T2_thr/T2_pace, not
        // just the derived verdict.
        let t2_throughput = hotelling_t2_statistic(&base_t, &scen_t);
        let t2_pacing = hotelling_t2_statistic(&base_p, &scen_p);

        let mut rng = StdRng::seed_from_u64(12345); // matches real_data_trial_2.py's first RNG_SEEDS entry; only affects TOST's own resample draws, not the deterministic T2/flag/verdict outcome
        let (verdict, tost_throughput, tost_pacing) = evaluate_verdict(
            &base_t,
            &scen_t,
            &base_p,
            &scen_p,
            5,
            &thresholds,
            ["avg_fps", "p1_fps", "p01_fps", "adaptive_frame_time_cv"],
            ["stutter_count_pct", "mean_abs_animation_error_ms"],
            &margins,
            &WcpsV3Weights::default(),
            &mut rng,
        )
        .expect("n=5 has a calibrated threshold");

        println!(
            "{}: RUST T2_throughput={t2_throughput:.3} T2_pacing={t2_pacing:.3} verdict={verdict:?} \
             tost_throughput_all_same={} tost_pacing_all_same={}",
            pair.label, tost_throughput.declared_same_all, tost_pacing.declared_same_all,
        );

        assert_eq!(
            verdict, pair.python_verdict,
            "{}: Rust verdict {verdict:?} disagrees with Python's independently-computed {:?}",
            pair.label, pair.python_verdict,
        );
    }

    // OBSERVED (run 2026-09-04): both pairs agreed EXACTLY, not just on the
    // final verdict but on the T2 statistics themselves (T2 is deterministic
    // -- no RNG involved -- so an exact numeric match here is real, not
    // "close enough"):
    //   Control 1 vs baseline:  RUST T2_throughput=19.525    T2_pacing=0.369     verdict=ConfirmedSame
    //                            PY   T2_thr=19.525           T2_pace=0.369       verdict=ConfirmedSame
    //   -vulkan (rep 1) vs baseline: RUST T2_throughput=64755.562 T2_pacing=81387.664 verdict=Worse
    //                            PY   T2_thr=64755.562        T2_pace=81387.664   verdict=Different
    // (Rust prints `Worse`, not `Different` -- Python's `verdict` field has
    // no direction concept at all; this Rust port's own directional
    // refinement, re-run live 2026-09-04 after
    // `docs/superpowers/plans/2026-09-04-wcps-v3-score-and-verdict.md`'s
    // `Verdict3` -> `Verdict` rewrite, confirmed passing with this exact output.)
    // Two independent CSV parsers (Rust's production `capture::parser`,
    // Python's pandas-based `load_iteration_frames`), two independent metric
    // implementations, reading the same raw files, landed on the same
    // floating-point values to 3 decimal places. This is the strongest
    // evidence in this whole port that the Rust side is a faithful
    // reproduction, not just a fixture-matching exercise.
}

#[test]
#[ignore = "one-time manual cross-language TOST statistical agreement check across multiple real pairs -- extends stats::v3::tost's own single-pair precedent; reads study/real-runs/ (gitignored, dev-machine only)"]
fn tost_declared_same_rate_across_multiple_real_pairs() {
    // 4 real pairs spanning different real conditions in the session: 3
    // true no-op controls (different session positions, so different
    // sampling noise even though the underlying config never changed) plus
    // one real tweak (-noreflex) whose Hotelling flag is already `Different`
    // -- included specifically because it showed real per-seed variability
    // in `confirmed_same` in the single-pair check this extends (the
    // combined verdict doesn't change, since `different` already takes
    // priority, but the TOST rate itself is worth seeing on a non-null
    // input too, not just on controls).
    let run = "9b700bea-wcps-v3-matrix-pruned";
    let pairs: [(&str, &str); 4] = [
        ("Control 1 (no-op) vs baseline", "scen_control_1"),
        ("Control 2 (no-op) vs baseline", "scen_control_2"),
        ("Control 3 (no-op) vs baseline", "scen_control_3"),
        ("-noreflex (rep 1, tweak) vs baseline", "scen_1788477760720"),
    ];

    let thresholds = load_calibrated_thresholds().unwrap();
    let pacing_margins = thresholds.pacing_tost_margins.clone();
    let throughput_margins = throughput_margins();
    let throughput_names = ["avg_fps", "p1_fps", "p01_fps", "adaptive_frame_time_cv"];
    let pacing_names = ["stutter_count_pct", "mean_abs_animation_error_ms"];
    const TRIALS: u64 = 1000;

    let (base_t, base_p) = load_scenario(&real_run_dir(run, "baseline"));

    for (label, scen_dir) in pairs {
        let (scen_t, scen_p) = load_scenario(&real_run_dir(run, scen_dir));

        let mut confirmed_throughput = 0u64;
        let mut confirmed_pacing = 0u64;
        let mut confirmed_both = 0u64;
        for seed in 0..TRIALS {
            let mut rng = StdRng::seed_from_u64(seed);
            let throughput_arrays: BTreeMap<String, (Vec<f64>, Vec<f64>)> = throughput_names
                .iter()
                .enumerate()
                .map(|(i, &n)| {
                    (
                        n.to_string(),
                        (
                            base_t.iter().map(|r| r[i]).collect(),
                            scen_t.iter().map(|r| r[i]).collect(),
                        ),
                    )
                })
                .collect();
            let pacing_arrays: BTreeMap<String, (Vec<f64>, Vec<f64>)> = pacing_names
                .iter()
                .enumerate()
                .map(|(i, &n)| {
                    (
                        n.to_string(),
                        (
                            base_p.iter().map(|r| r[i]).collect(),
                            scen_p.iter().map(|r| r[i]).collect(),
                        ),
                    )
                })
                .collect();
            let t_res = tost_evaluate(&throughput_arrays, &throughput_margins, &mut rng);
            let p_res = tost_evaluate(&pacing_arrays, &pacing_margins, &mut rng);
            if t_res.declared_same_all {
                confirmed_throughput += 1;
            }
            if p_res.declared_same_all {
                confirmed_pacing += 1;
            }
            if t_res.declared_same_all && p_res.declared_same_all {
                confirmed_both += 1;
            }
        }
        println!(
            "{label}: throughput_declared_same_rate={:.4} pacing_declared_same_rate={:.4} \
             both_declared_same_rate={:.4} ({TRIALS} trials)",
            confirmed_throughput as f64 / TRIALS as f64,
            confirmed_pacing as f64 / TRIALS as f64,
            confirmed_both as f64 / TRIALS as f64,
        );
    }

    // OBSERVED (run 2026-09-04, 1000 trials/pair):
    //   Control 1 vs baseline:            throughput=1.0000  pacing=1.0000  both=1.0000
    //   Control 2 vs baseline:            throughput=1.0000  pacing=1.0000  both=1.0000
    //   Control 3 vs baseline:            throughput=1.0000  pacing=1.0000  both=1.0000
    //   -noreflex (rep 1) vs baseline:    throughput=0.0660  pacing=1.0000  both=0.0660
    // All 3 real no-op controls: TOST correctly declares equivalence on
    // essentially every trial on both axes -- a much cleaner, more complete
    // picture than the single earlier pair (which happened to straddle a
    // margin). -noreflex (a genuine tweak, already flagged `Different` by
    // Hotelling on the throughput axis) shows real per-seed variability on
    // ITS OWN throughput TOST call (6.6% -- consistent with the earlier
    // single-pair check's finding that this fixture's p01_fps bootstrap CI
    // sits right at the edge of its 3% margin) while its pacing axis stays
    // fully equivalent (1.0000, consistent with -noreflex being a
    // latency/Reflex-only change that shouldn't move frame-pacing metrics).
    // This doesn't change the combined verdict (`different` already wins),
    // but confirms the TOST layer behaves sensibly -- confidently uniform on
    // true nulls, genuinely borderline on a metric that's actually close to
    // its margin -- not just parroting whatever Hotelling already decided.
}
