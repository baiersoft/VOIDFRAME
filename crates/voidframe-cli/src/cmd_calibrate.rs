//! `voidframe-cli calibrate` — docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §11a / finding D9: derive real
//! `warmup_loops`/`measure_loops` defaults from a large-N baseline-only run
//! on the real rig, instead of shipping the `2`/`3` placeholders.
//!
//! **Why every iteration is captured, contrary to the spec's literal
//! wording:** §11a describes running with "`warmup_loops` set high, ~8"
//! during calibration, then plotting each metric against iteration index to
//! find where the trend stops. But `run_scenario_inner`'s warmup loop calls
//! `run_one_iteration` with `capture: None` — no `Metrics` are ever recorded
//! for a warmup iteration, by design. You cannot find "where the trend
//! stops" over data that was never captured. This module therefore always
//! drives with `warmup_loops: 0` and does the warmup/measure split entirely
//! in post-hoc analysis over the captured series — the only way the method
//! described in §11a can actually work against this engine's real capture
//! behavior.
//!
//! **Two capture modes**, for comparing thermal behavior (real rig data
//! showed a session-length fps decline consistent with boost-clock thermal
//! throttling, not shader-cache warmup — see `run`'s own doc comment): the
//! default keeps one CS2 session alive for the whole run
//! (`measure_loops: N`); `--fresh-cs2-per-iteration` instead gives every
//! iteration its own scenario slot with `measure_loops: 1`, so CS2 is fully
//! closed and relaunched between every single data point — optionally with
//! an explicit `--break-seconds` cooldown while it's closed.

use rand::Rng;
use std::path::PathBuf;
use voidframe_engine::model::project::{Baseline, Project, Scenario};
use voidframe_engine::model::results::{Metrics, Verdict};
use voidframe_engine::model::{SCHEMA_VERSION, Settings};
use voidframe_engine::run::execute::RunConfig;
use voidframe_engine::run::{ControlMsg, EngineEvent, RunOutcome, RunStart};

use crate::calibrate_stats::{CALIBRATION_METRICS, compare_metric};
use crate::harness::select_backend;

/// Each candidate split's two halves must have at least this many
/// iterations — `3` is the smallest value in `Settings::validate`'s own
/// calibrated `measure_loops` ladder (`stats::v3::CALIBRATED_N_LADDER`), so
/// a real calibration run can never recommend an unusable `measure_loops`.
const MIN_HALF: usize = 3;

/// Smallest `w` such that `series[w..]`, split into two halves, shows
/// `NoMeasurableDifference` on every metric in `CALIBRATION_METRICS` — the
/// point past which the series is no longer trending (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §11a). `None`
/// if convergence isn't reached before fewer than `2 * MIN_HALF` iterations
/// remain (not enough data, or the series never stabilizes).
pub fn find_warmup_cutoff_with_rng(series: &[Metrics], rng: &mut impl Rng) -> Option<usize> {
    if series.len() < MIN_HALF * 2 {
        return None;
    }
    let max_w = series.len() - MIN_HALF * 2;
    (0..=max_w).find(|&w| {
        let remaining = &series[w..];
        let mid = remaining.len() / 2;
        let (first, second) = remaining.split_at(mid);
        CALIBRATION_METRICS
            .iter()
            .all(|(_name, extract, lower_is_better)| {
                let first_vals: Vec<f64> = first.iter().map(extract).collect();
                let second_vals: Vec<f64> = second.iter().map(extract).collect();
                let cmp = compare_metric(&first_vals, &second_vals, *lower_is_better, rng);
                cmp.verdict == Verdict::NoMeasurableDifference
            })
    })
}

/// For each `n` from 3 up to `pool.len() / 2`, self-split-half compares
/// `avg_fps` over two disjoint, equal-sized `n`-element windows
/// (`pool[0..n]` vs `pool[n..2n]`) and records the resulting 95% CI
/// half-width on the delta — the diminishing-returns curve docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §11a asks a
/// human to read `measure_loops` off of ("the *n* past which the CI
/// half-width improves by less than a set threshold... per added run").
/// Empty if `pool` has fewer than 6 iterations (no `n=3` split possible).
pub fn ci_half_width_curve_with_rng(pool: &[Metrics], rng: &mut impl Rng) -> Vec<(u32, f64)> {
    let max_n = pool.len() / 2;
    (MIN_HALF..=max_n)
        .map(|n| {
            let first: Vec<f64> = pool[0..n].iter().map(|it| it.avg_fps).collect();
            let second: Vec<f64> = pool[n..2 * n].iter().map(|it| it.avg_fps).collect();
            let cmp = compare_metric(&first, &second, false, rng);
            let half_width = (cmp.ci_high_pct - cmp.ci_low_pct) / 2.0;
            (n as u32, half_width)
        })
        .collect()
}

/// Mean `frame_time_cv` over the last `n` iterations of `pool` — the same
/// reproducibility-floor value `ResultsVisualizer`'s panel already surfaces
/// for every real result, computed here from the calibration run itself
/// (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §11a step 3). `n` must be `<= pool.len()` and nonzero.
pub fn reproducibility_floor(pool: &[Metrics], n: usize) -> f64 {
    let window = &pool[pool.len() - n..];
    voidframe_engine::stats::mean(&window.iter().map(|it| it.frame_time_cv).collect::<Vec<_>>())
}

/// Builds `n` synthetic, no-op scenarios (zero modules — same shape as
/// baseline), ids `calib-0..calib-{n-1}`, for `--fresh-cs2-per-iteration`
/// mode: combined with the mandatory baseline `execute()` always runs
/// first, these give `n + 1` total captured data points, each via its own
/// fresh CS2 launch → measure → kill cycle (CS2 genuinely closed between
/// them, not just idling at the menu — see `RunConfig::
/// inter_scenario_break_seconds`'s doc comment).
fn build_fresh_cs2_scenarios(n: u32) -> Vec<Scenario> {
    (0..n)
        .map(|i| Scenario {
            id: format!("calib-{i}"),
            name: format!("Calibration iteration {}", i + 2), // +2: baseline is iteration 1
            description: "No-op -- calibration sample point (fresh CS2 launch)".into(),
            enabled: true,
            modules: vec![],
        })
        .collect()
}

/// Drives a calibration run through the same `execute()`/`RunConfig` path
/// `cmd_run` uses (PREFLIGHT through REPORT, including the real ROLLBACK
/// phase — nothing here skips safety machinery), then runs the calibration
/// analysis over the captured per-iteration series and prints a report.
/// `warmup_loops` is always `0` (see this module's own doc comment for why).
///
/// Two independent modes, selectable for a thermal-isolation comparison
/// study: `fresh_cs2_per_iteration: false` (default) keeps every prior
/// behavior exactly — one scenario, `measure_loops: iterations`, a single
/// CS2 session for the whole run. `true` instead builds `iterations - 1`
/// synthetic no-op scenarios (`build_fresh_cs2_scenarios`) alongside the
/// mandatory baseline, each with `measure_loops: 1` — `iterations` total
/// data points via `iterations` separate CS2 launches. `break_seconds`
/// (`RunConfig::inter_scenario_break_seconds`) is independent of the mode;
/// it only has a meaningful effect combined with `fresh_cs2_per_iteration`
/// (with it off there's only one CS2 session, so nothing to break between).
#[allow(clippy::too_many_arguments)]
pub async fn run(
    controller: &str,
    presentmon_path: PathBuf,
    data_root: PathBuf,
    console_log_override: Option<PathBuf>,
    map_id: Option<String>,
    iterations: u32,
    sound_enabled: bool,
    fresh_cs2_per_iteration: bool,
    break_seconds: u32,
) -> anyhow::Result<()> {
    let mut settings: Settings = serde_json::from_str("{}")?;
    settings.warmup_loops = 0;
    let scenarios = if fresh_cs2_per_iteration {
        settings.measure_loops = 1;
        build_fresh_cs2_scenarios(iterations.saturating_sub(1))
    } else {
        settings.measure_loops = iterations;
        vec![]
    };
    if let Some(map) = map_id {
        settings.map_id = map;
    }
    // NOT `settings.validate()`: its calibrated-ladder check on
    // `measure_loops` (`stats::v3::CALIBRATED_N_LADDER`) is a real,
    // load-bearing rule for statistically-valid A/B comparisons in actual
    // projects -- weakening it would be wrong. Calibrate's own
    // `measure_loops` (1 in fresh-cs2 mode, deliberately) doesn't need that
    // constraint, so only the map_id console-injection-safety check applies
    // here.
    if settings.map_id.is_empty() || !settings.map_id.chars().all(|c| c.is_ascii_digit()) {
        anyhow::bail!("map_id must be a non-empty numeric Steam workshop addon id");
    }

    let project = Project {
        schema_version: SCHEMA_VERSION.to_string(),
        id: "calibration".into(),
        name: "Calibration".into(),
        description: "Baseline-only large-N calibration run (spec §11a)".into(),
        created_at: "calibration".into(),
        settings,
        baseline: Baseline {
            name: "Calibration Baseline".into(),
            description: "Stock, no modules -- calibration measures baseline variance only".into(),
        },
        scenarios,
    };

    let (sys, capture) = select_backend(controller, false, presentmon_path)?;
    let run_id = uuid::Uuid::new_v4().to_string();

    let (control_tx, control_rx) = tokio::sync::mpsc::channel(16);
    // `voidframe-cli` is headless -- nothing here can ever toggle
    // `shutdown_when_complete` mid-run, so the sender is just dropped. Seeded
    // `false` to match `config.shutdown_when_complete` below.
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

    let config = RunConfig {
        project,
        run_id: run_id.clone(),
        dry_run: false,
        data_root,
        webview_root_pid: None,
        console_log_override,
        mock_cs2_log: None,
        thermal_sample_override: None,
        inter_scenario_break_seconds: break_seconds,
        hwinfo_path: None,
        shutdown_when_complete: false,
        post_boot_settle: std::time::Duration::from_secs(180),
        exe_path: std::env::current_exe()?,
    };

    let mut run = voidframe_engine::run::spawn_run(
        sys,
        capture,
        RunStart::Fresh(config),
        control_rx,
        shutdown_toggle_rx,
    );

    let mut failed = false;
    while let Some(ev) = run.events.recv().await {
        if sound_enabled && let Some(cue) = crate::sound::sound_cue_for_event(&ev) {
            crate::sound::play(cue);
        }
        match &ev {
            EngineEvent::PhaseChanged { phase } => println!("-- phase: {phase}"),
            EngineEvent::LogLine { text } if text.starts_with("Cooldown break:") => {
                println!("   {text}");
            }
            EngineEvent::IterationComplete { metrics } => println!(
                "   iteration complete: avg_fps={:.2} frame_time_cv={:.4}",
                metrics.avg_fps, metrics.frame_time_cv
            ),
            EngineEvent::RunFailed { reason } => {
                failed = true;
                eprintln!("RUN FAILED: {reason}");
            }
            EngineEvent::OperatorPrompt { .. } => {
                let _ = control_tx.send(ControlMsg::OperatorAcknowledged).await;
            }
            _ => {}
        }
    }

    let result = run.join.await??;
    if failed {
        anyhow::bail!("calibration run failed");
    }
    // Calibration never configures a reboot-requiring module, so a
    // calibration run always either completes or fails -- `RebootPending`/
    // `ShutdownRequested` are M3 outcomes for real benchmark projects,
    // not this command.
    let RunOutcome::Complete(result) = result else {
        anyhow::bail!("calibration run ended in an unexpected outcome: {result:?}");
    };

    // Concatenated in order regardless of mode: with fresh_cs2_per_iteration
    // off, `result.scenarios` is empty, so this is exactly `baseline.
    // per_iteration` alone -- identical to every prior calibration run.
    let mut series = result.baseline.per_iteration.clone();
    for sc in &result.scenarios {
        series.extend(sc.per_iteration.iter().cloned());
    }
    let series = series.as_slice();
    let mut rng = rand::thread_rng();
    let cutoff = find_warmup_cutoff_with_rng(series, &mut rng);
    let pool: &[Metrics] = match cutoff {
        Some(w) => &series[w..],
        None => series,
    };
    let curve = ci_half_width_curve_with_rng(pool, &mut rng);

    print_report(series, cutoff, &curve, pool);
    Ok(())
}

fn print_report(all: &[Metrics], cutoff: Option<usize>, curve: &[(u32, f64)], pool: &[Metrics]) {
    println!("\n=== Calibration Report ===");
    println!(
        "{} total captured iterations (warmup_loops=0 -- every iteration measured, per this \
         module's own doc comment)\n",
        all.len()
    );
    println!(
        "{:>4}  {:>10}  {:>10}  {:>10}  {:>12}",
        "iter", "avg_fps", "p1_fps", "p01_fps", "frame_cv"
    );
    for (i, it) in all.iter().enumerate() {
        println!(
            "{:>4}  {:>10.2}  {:>10.2}  {:>10.2}  {:>12.4}",
            i, it.avg_fps, it.p1_fps, it.p01_fps, it.frame_time_cv
        );
    }

    match cutoff {
        Some(w) => println!(
            "\nSuggested warmup_loops cutoff: {w} (iterations 0..{w} look like a shader-cache \
             / load transient that later iterations no longer show)"
        ),
        None => println!(
            "\nSuggested warmup_loops cutoff: not found -- the series never stabilized within \
             the available data; re-run with a larger --iterations"
        ),
    }

    println!("\nCI half-width vs n (post-cutoff pool, avg_fps, 95%):");
    for &(n, hw) in curve {
        println!("  n={n:>3}  half-width={hw:.3}%");
    }
    println!(
        "\nPick measure_loops = the n past which half-width stops improving by more than ~0.5% \
         per added run (spec §11a) -- read the table above yourself; this is a human judgment \
         call the spec deliberately does not mechanize."
    );

    if pool.len() >= 3 {
        let floor = reproducibility_floor(pool, pool.len());
        println!(
            "\nReproducibility floor (mean frame_time_cv over the full post-cutoff pool, n={}): \
             {:.4}",
            pool.len(),
            floor
        );
    }

    println!(
        "\nThis is a printed report only -- nothing in docs/, config.rs, or settings.rs was \
         changed automatically. Review the curve above, pick warmup_loops/measure_loops \
         yourself, then update Settings' defaults and commit a calibration note under docs/ \
         per spec §11a's own deliverables."
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use voidframe_engine::model::results::Metrics;

    fn m(avg_fps: f64, frame_time_cv: f64) -> Metrics {
        Metrics {
            avg_fps,
            median_fps: avg_fps,
            p1_fps: avg_fps * 0.65,
            p01_fps: avg_fps * 0.5,
            frame_time_mean_ms: 1000.0 / avg_fps,
            frame_time_stddev_ms: frame_time_cv * (1000.0 / avg_fps),
            frame_time_cv,
            adaptive_frame_time_cv: frame_time_cv,
            stutter_count_pct: 0.0,
            mean_abs_animation_error_ms: None,
            gpu_busy_ms: None,
            bottleneck_ratio: None,
            render_latency_ms: None,
            dominant_present_mode: "Hardware: Independent Flip".into(),
            present_mode_consistent: true,
            present_mode_warning: None,
        }
    }

    #[test]
    fn a_trending_prefix_is_not_reported_as_needing_zero_warmup() {
        // Iterations 0..3 are a sharply elevated shader-cache-warming prefix
        // (avg_fps ~2x the settled value); iterations 3.. are flat data
        // around avg_fps ~400. The exact cutoff a bootstrap comparison lands
        // on is sensitive to how much of the trending prefix still overlaps
        // a given candidate window (a real property of the statistics, not
        // a bug) -- but it must never claim the series was already flat at
        // w=0 when a sharp trend clearly exists there, and it must converge
        // somewhere before running out of data.
        let mut series = vec![m(800.0, 0.04), m(750.0, 0.045), m(700.0, 0.05)];
        let flat_fps = [
            399.0, 400.0, 401.0, 399.0, 400.0, 401.0, 400.0, 399.0, 400.0, 401.0, 400.0, 399.0,
            400.0, 401.0, 400.0,
        ];
        for fps in flat_fps {
            series.push(m(fps, 0.10));
        }

        let mut rng = StdRng::seed_from_u64(42);
        let cutoff = find_warmup_cutoff_with_rng(&series, &mut rng);
        assert!(
            matches!(cutoff, Some(w) if w >= 1),
            "expected a nonzero cutoff given a sharp trend at the start, got {cutoff:?}"
        );
    }

    #[test]
    fn a_flat_series_from_the_start_needs_no_warmup() {
        let flat_fps = [
            400.0, 401.0, 399.0, 402.0, 398.0, 400.0, 401.0, 399.0, 400.0, 402.0, 398.0, 401.0,
        ];
        let series: Vec<Metrics> = flat_fps.iter().map(|&f| m(f, 0.09)).collect();

        let mut rng = StdRng::seed_from_u64(7);
        let cutoff = find_warmup_cutoff_with_rng(&series, &mut rng);
        assert_eq!(cutoff, Some(0));
    }

    #[test]
    fn ci_curve_covers_n_from_3_up_to_half_the_pool() {
        // 10-element pool -> n ranges 3..=5 (floor(10/2) = 5).
        let pool: Vec<Metrics> = (0..10).map(|i| m(400.0 + (i % 3) as f64, 0.1)).collect();
        let mut rng = StdRng::seed_from_u64(1);
        let curve = ci_half_width_curve_with_rng(&pool, &mut rng);
        let ns: Vec<u32> = curve.iter().map(|&(n, _)| n).collect();
        assert_eq!(ns, vec![3, 4, 5]);
    }

    #[test]
    fn half_width_is_smaller_for_tighter_noise_at_the_same_n() {
        let mut rng = StdRng::seed_from_u64(2);
        let tight: Vec<Metrics> = [400.0, 401.0, 399.0, 400.0, 401.0, 399.0, 400.0, 401.0]
            .iter()
            .map(|&f| m(f, 0.1))
            .collect();
        let noisy: Vec<Metrics> = [370.0, 430.0, 380.0, 420.0, 375.0, 425.0, 385.0, 415.0]
            .iter()
            .map(|&f| m(f, 0.1))
            .collect();

        let tight_curve = ci_half_width_curve_with_rng(&tight, &mut rng);
        let noisy_curve = ci_half_width_curve_with_rng(&noisy, &mut rng);

        let tight_at_3 = tight_curve.iter().find(|&&(n, _)| n == 3).unwrap().1;
        let noisy_at_3 = noisy_curve.iter().find(|&&(n, _)| n == 3).unwrap().1;
        assert!(
            tight_at_3 < noisy_at_3,
            "tight-noise half-width {tight_at_3} should be smaller than noisy half-width {noisy_at_3}"
        );
    }

    #[test]
    fn reproducibility_floor_is_the_mean_frame_time_cv_over_the_last_n() {
        let pool = vec![
            m(400.0, 0.30), // excluded -- outside the last 3
            m(400.0, 0.20), // excluded -- outside the last 3
            m(400.0, 0.10),
            m(400.0, 0.12),
            m(400.0, 0.08),
        ];
        let floor = reproducibility_floor(&pool, 3);
        assert!((floor - 0.10).abs() < 1e-9, "got {floor}");
    }

    #[test]
    fn build_fresh_cs2_scenarios_generates_n_no_op_scenarios_with_distinct_ids() {
        let scenarios = build_fresh_cs2_scenarios(3);
        assert_eq!(scenarios.len(), 3);
        let ids: Vec<&str> = scenarios.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["calib-0", "calib-1", "calib-2"]);
        for sc in &scenarios {
            assert!(sc.enabled, "every synthetic scenario must be enabled");
            assert!(
                sc.modules.is_empty(),
                "every synthetic scenario must be a no-op (zero modules)"
            );
        }
    }

    #[test]
    fn build_fresh_cs2_scenarios_of_zero_returns_an_empty_vec() {
        assert!(build_fresh_cs2_scenarios(0).is_empty());
    }

    #[test]
    fn too_little_data_to_split_returns_none() {
        // Fewer than 2*MIN_HALF (3) iterations total -- not enough to form
        // even one candidate first-half/second-half split.
        let series: Vec<Metrics> = vec![m(400.0, 0.1), m(401.0, 0.1), m(399.0, 0.1)];

        let mut rng = StdRng::seed_from_u64(1);
        let cutoff = find_warmup_cutoff_with_rng(&series, &mut rng);
        assert_eq!(cutoff, None);
    }
}
