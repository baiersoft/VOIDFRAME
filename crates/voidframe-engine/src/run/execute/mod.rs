//! `execute` — the run loop's phase sequencer (PREFLIGHT -> SNAPSHOT ->
//! BASELINE -> SCENARIO* -> ROLLBACK -> REPORT; see
//! `docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md` §7.1).
//! This module owns phase transitions and event emission; the per-scenario
//! CS2 lifecycle (launch -> warmup -> measure -> kill) is `run_scenario`,
//! implemented per `docs/superpowers/plans/2026-09-01-m1-phase-3b-cs2-control-run-loop.md`.
//!
//! Split across sibling files for legibility: `scenario` owns the
//! per-scenario CS2 lifecycle (`run_scenario`/`run_scenario_inner`/the
//! iteration loop), `steam` owns Steam/CS2 process-lifecycle polling and
//! kill helpers, `signatures` resolves and loads `signatures.json`,
//! `rollback` is ROLLBACK's own real work, `checks` holds the standalone
//! Steam-readiness/HWiNFO diagnostic checks used by the CLI, `launch` owns
//! CS2 session preparation ahead of the iteration loop, `context` is the
//! `RunContext` snapshot taken once at SNAPSHOT that later phases read, and
//! `control` holds the small Pause/Abort control-channel helpers
//! (`honor_pause`/`abortable`) shared across the run loop's wait points.
//! This file keeps the top-level phase sequencer (`execute`) plus scoring
//! (the run-start timestamp itself lives in
//! `crate::model::results::utc_timestamp_now`).

mod checks;
mod context;
mod control;
mod launch;
mod rollback;
mod scenario;
mod signatures;
mod steam;
pub(crate) mod thermal;

#[cfg(test)]
mod tests;

use crate::capture::runner::CaptureRunner;
use crate::error::{Error, Result};
use crate::model::project::Project;
use crate::model::results::{Metrics, RunResults, ScenarioResult};
use crate::preflight::{self, PreflightFacts};
use crate::run::{ControlMsg, DetectionTier, EngineEvent, Phase};
use crate::system::SystemController;
pub use checks::{
    HwinfoCheckReport, HwinfoCheckStep, SteamLaunchCheckReport, SteamLaunchCheckStep, hwinfo_check,
    steam_launch_check,
};
use context::RunContext;
use rollback::rollback;
use scenario::run_scenario;
use std::path::PathBuf;
use std::sync::Arc;
use steam::ensure_steam_running;
use tokio::sync::mpsc;

use std::time::Duration;

pub struct RunConfig {
    pub project: Project,
    pub run_id: String,
    pub dry_run: bool,
    pub data_root: PathBuf,
    /// The Tauri app's own WebView2 process tree root pid, for the §2.2
    /// "quiet window" suspend around each measure iteration's capture
    /// window. `None` for every M1 caller — `voidframe-cli` is
    /// headless and has no WebView2 tree to suspend at all, and nothing in
    /// this plan populates it for a desktop UI either. `Some(pid)` is set by
    /// the Tauri shell (`src-tauri/src/webview_pid.rs`, per
    /// `docs/superpowers/plans/2026-09-01-m1-phase-4a-tauri-backend-bridge.md`);
    /// this field is only the forward-compatible hook so
    /// that later integration doesn't have to touch `run_scenario`'s
    /// signature again.
    pub webview_root_pid: Option<u32>,
    /// Test-only seam: when set, `run_scenario` tails this path instead of
    /// resolving `console.log` via the real Steam installation. `None` in
    /// every real caller. Without this hook, `LogTail` — which genuinely
    /// tails a real file on disk — would have no way to be pointed at a
    /// test's own temp file, since the real path is otherwise derived from
    /// `SystemController::app_library_path`.
    pub console_log_override: Option<PathBuf>,
    /// Test-only seam (E2E mock-run mode, `src-tauri`'s `mock-run` Cargo
    /// feature): when `Some(event_delay)`, `run_scenario_inner`/`launch.rs`
    /// open a `voidframe_engine::mock_harness::MockLogTail` (answering
    /// every CS2-readiness wait after `event_delay`, never tailing a real
    /// -- nonexistent, since no real CS2 was launched -- `console.log`)
    /// instead of a real `RealLogTail`. `None` for every real caller.
    /// `event_delay` lets mock-run pick between a fast preset (E2E tests)
    /// and a "realistic timing" preset (manual observation) without a
    /// second config field.
    pub mock_cs2_log: Option<Duration>,
    /// Test-only seam (E2E mock-run mode): overrides the thermal-sampling
    /// cadence (`thermal::THERMAL_SAMPLE_INTERVAL`/`THERMAL_SAMPLE_COUNT`)
    /// used for the pre-run baseline and every inter-scenario cooldown
    /// check. `None` (every real caller, and mock-run's own "realistic
    /// timing" preset) uses the real production cadence unchanged --
    /// `Some((interval, count))` is the E2E mock-run fast preset's only
    /// use: `MockController`'s HWiNFO reading is a fixed value, so any
    /// sample count/interval reaches the same "already cool" conclusion, a
    /// tiny override just skips paying the real ~30s per check for that
    /// foregone conclusion.
    pub thermal_sample_override: Option<(Duration, u32)>,
    /// Sleep this long, with a `LogLine`, after BASELINE's `run_scenario`
    /// call and after every enabled scenario's `run_scenario` call — while
    /// CS2 is genuinely closed (`run_scenario_inner` always kills it before
    /// returning), not just idling at the menu. `0` (no break) for every
    /// caller except `voidframe-cli calibrate`'s own `--break-seconds`
    /// flag: a general "cool down between scenarios" primitive, reused by
    /// calibrate's thermal-isolation study rather than being calibrate-only
    /// plumbing.
    pub inter_scenario_break_seconds: u32,
    /// The HWiNFO64 executable's path, used to launch it if it isn't
    /// already running (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2). `None` means thermal mode is never
    /// attempted this run -- either it isn't configured, or the feature is
    /// disabled. Every existing caller defaults this to `None` until a
    /// later task wires real Settings-driven config in.
    pub hwinfo_path: Option<PathBuf>,
}

pub async fn execute(
    sys: Arc<dyn SystemController>,
    capture: Arc<dyn CaptureRunner>,
    config: RunConfig,
    events: mpsc::Sender<EngineEvent>,
    mut control: mpsc::Receiver<ControlMsg>,
    abort: tokio::sync::watch::Receiver<bool>,
) -> Result<RunResults> {
    let emit = |ev: EngineEvent| {
        let events = events.clone();
        async move {
            let _ = events.send(ev).await;
        }
    };

    // PREFLIGHT
    emit(EngineEvent::PhaseChanged {
        phase: Phase::Preflight,
    })
    .await;
    // Best-effort: attempt to bring Steam up de-elevated before gathering
    // facts, so a freshly booted machine with no Steam session yet doesn't
    // automatically fail pre-flight (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §2.1a rev 6). Failure here isn't
    // fatal on its own -- gather_facts below simply observes
    // `steam_running: false` and `evaluate()` blocks with the right
    // message, exactly as if this attempt had never been made.
    //
    // Checked once up front (rather than letting `ensure_steam_running`'s
    // own internal check decide silently) so the LogLine below is only
    // ever emitted when a launch attempt is genuinely about to happen --
    // never a misleading "Steam is not running" line when Steam is
    // actually already up and the call below is about to no-op.
    if !sys.steam_status().await?.running {
        emit(EngineEvent::LogLine {
            text: "Steam is not running -- attempting to start it automatically...".into(),
        })
        .await;
    }
    if let Err(e) = ensure_steam_running(sys.as_ref()).await {
        tracing::warn!("pre-flight auto-launch of Steam failed: {e}");
        emit(EngineEvent::LogLine {
            text: format!("Pre-flight auto-launch of Steam failed: {e}"),
        })
        .await;
    }
    let facts: PreflightFacts =
        preflight::gather_facts(sys.as_ref(), None, 2_000_000_000, &config.project.scenarios)
            .await?;
    let report = preflight::evaluate(&facts);
    if report.is_blocked() {
        let reason = report
            .blocking()
            .iter()
            .map(|c| c.detail.clone())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(crate::error::Error::preflight(reason));
    }

    // SNAPSHOT (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1: read + store every value the enabled scenarios'
    // modules will touch, so the Desktop .bat (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §6.3) and the run's own
    // audit trail have a true before-state). The Desktop .bat generation
    // itself is a Phase-4-adjacent UI/UX feature (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §6.3), out of this
    // plan's scope -- deliberately not implemented here. What this phase
    // DOES do: record the run-start active power plan (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1 / §6.2
    // item 2), needed for ROLLBACK's belt-and-braces restore below.
    emit(EngineEvent::PhaseChanged {
        phase: Phase::Snapshot,
    })
    .await;

    // `ctx.start_build_id`: captured once, before BASELINE begins, so every
    // scenario boundary (baseline included) has a stable value to re-check
    // against (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.5). Pre-flight already blocked the run if CS2 wasn't
    // fully installed, so this should always resolve; a failure here (e.g.
    // Steam vanished between pre-flight and now) means every later
    // build-freeze check in this run would be meaningless anyway, hence the
    // hard `?` rather than silently degrading to `None`.
    //
    // `ctx.start_launch_args`: captured once here, before any scenario can
    // write its own `launch_args` module's value over Steam's
    // `LaunchOptions` — `run_scenario_inner` uses this snapshot (not a live
    // read) as the base a scenario with no `launch_args` module falls back
    // to, since a live read would pick up whatever an *earlier scenario in
    // this same run* left behind, not the user's real baseline.
    //
    // `ctx.start_power_plan`: the plan active right now, restored in
    // ROLLBACK below.
    let raw_launch_args = sys.read_cs2_launch_options().await?;
    emit(EngineEvent::LogLine {
        text: format!(
            "Current CS2 launch options: \"{}\"",
            crate::cs2::keybind_cfg::redact(&raw_launch_args)
        ),
    })
    .await;
    let ctx = RunContext {
        config: &config,
        start_build_id: sys.app_manifest(730).await?.and_then(|m| m.build_id),
        start_launch_args: crate::cs2::keybind_cfg::strip_reserved(&raw_launch_args),
        start_launch_args_raw: raw_launch_args.clone(),
        start_power_plan: sys.active_power_plan().await?,
        abort: abort.clone(),
    };

    // THERMAL BASELINE (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2): only attempted when this run has an
    // `hwinfo_path` configured at all, and only when HWiNFO isn't already
    // running externally -- VOIDFRAME only ever drives HWiNFO's own
    // lifecycle when it's the one that started it (the same "only touch
    // what we started" rule `close_steam_window` follows), so an already-
    // running instance means thermal mode falls back to the plain
    // fixed-time `maybe_break` for the whole run instead. This decision is
    // made once, here, up front (spec A2's "decided once per run" rule) --
    // never re-checked per scenario boundary.
    let (thermal_sample_interval, thermal_sample_count) =
        config.thermal_sample_override.unwrap_or((
            thermal::THERMAL_SAMPLE_INTERVAL,
            thermal::THERMAL_SAMPLE_COUNT,
        ));
    let mut thermal_baseline: Option<thermal::ThermalReading> = None;
    if let Some(hwinfo_path) = &config.hwinfo_path {
        match sys.hwinfo_already_running().await {
            Ok(true) => {
                emit(EngineEvent::LogLine {
                    text: "HWiNFO is already running -- thermal cooldown will use the plain \
                           fixed-time break instead (VOIDFRAME only drives HWiNFO's lifecycle \
                           when it's the one that started it)."
                        .into(),
                })
                .await;
            }
            Ok(false) => {
                emit(EngineEvent::PhaseChanged {
                    phase: Phase::ThermalBaseline,
                })
                .await;
                match thermal::collect_thermal_sample(
                    sys.as_ref(),
                    hwinfo_path,
                    thermal_sample_interval,
                    thermal_sample_count,
                )
                .await
                {
                    Ok(reading) => thermal_baseline = Some(reading),
                    Err(e) => {
                        emit(EngineEvent::LogLine {
                            text: format!(
                                "Thermal baseline collection failed ({e}) -- falling back to \
                                 the plain fixed-time break."
                            ),
                        })
                        .await;
                    }
                }
            }
            Err(e) => {
                emit(EngineEvent::LogLine {
                    text: format!(
                        "Could not check whether HWiNFO is already running ({e}) -- falling \
                         back to the plain fixed-time break."
                    ),
                })
                .await;
            }
        }
    }

    // Cooldown-check readings taken during each break, keyed by the
    // scenario id that break followed (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2's `thermal.json`) --
    // `maybe_break` has no notion of "which scenario"; that's this loop's
    // job, not its own. Declared outside the `body` block below (borrowed
    // mutably from inside it) so it's still readable afterward for
    // `thermal.json` on the success path.
    let mut cooldown_checks: Vec<(String, Vec<thermal::ThermalReading>)> = Vec::new();

    // BASELINE + SCENARIO* + scoring, wrapped so ROLLBACK below always runs
    // -- success or failure. Every one of these steps used to `?` straight
    // out of `execute()` on its first failure (including an operator
    // Abort), which skipped ROLLBACK's belt-and-braces power-plan restore
    // and defensive journal sweep entirely -- docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1 lists ROLLBACK as an
    // unconditional phase, not a happy-path-only one.
    let body: Result<(ScenarioResult, Vec<ScenarioResult>)> = async {
        // BASELINE
        emit(EngineEvent::PhaseChanged {
            phase: Phase::Baseline,
        })
        .await;
        let baseline_dummy_scenario = crate::model::project::Scenario {
            id: "baseline".into(),
            name: config.project.baseline.name.clone(),
            description: config.project.baseline.description.clone(),
            enabled: true,
            modules: vec![],
        };
        let baseline_result = run_scenario(
            sys.as_ref(),
            capture.as_ref(),
            &baseline_dummy_scenario,
            true,
            &ctx,
            &events,
            &mut control,
        )
        .await?;
        let checks = maybe_break(
            sys.as_ref(),
            &events,
            &mut control,
            ctx.abort.clone(),
            config.inter_scenario_break_seconds,
            thermal_baseline.as_ref(),
            config.hwinfo_path.as_deref(),
            thermal_sample_interval,
            thermal_sample_count,
        )
        .await?;
        if !checks.is_empty() {
            cooldown_checks.push(("baseline".to_string(), checks));
        }

        // SCENARIO*
        let mut scenario_results = Vec::new();
        for sc in config.project.enabled_scenarios() {
            emit(EngineEvent::PhaseChanged {
                phase: Phase::Scenario { id: sc.id.clone() },
            })
            .await;
            let result = run_scenario(
                sys.as_ref(),
                capture.as_ref(),
                sc,
                false,
                &ctx,
                &events,
                &mut control,
            )
            .await?;
            emit(EngineEvent::ScenarioComplete {
                result: result.clone(),
            })
            .await;
            scenario_results.push(result);
            let checks = maybe_break(
                sys.as_ref(),
                &events,
                &mut control,
                ctx.abort.clone(),
                config.inter_scenario_break_seconds,
                thermal_baseline.as_ref(),
                config.hwinfo_path.as_deref(),
                thermal_sample_interval,
                thermal_sample_count,
            )
            .await?;
            if !checks.is_empty() {
                cooldown_checks.push((sc.id.clone(), checks));
            }
        }

        // Scoring (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §8): now that every scenario's `per_iteration` data
        // is in, compute each non-baseline scenario's
        // `metric_deltas`/`wcps`/`verdict` against the baseline's
        // per-iteration distribution, replacing the
        // `vec![]`/`0.0`/`ConfirmedSame` placeholders `run_scenario` left in
        // place.
        score_scenarios(&baseline_result.per_iteration, &mut scenario_results)?;
        Ok((baseline_result, scenario_results))
    }
    .await;

    // ROLLBACK (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1): every scenario's own `run_scenario` call above
    // already reverted its journal, success or failure -- this phase's real
    // job is narrower than its name suggests: belt-and-braces restoration
    // of the run-start active power plan, plus a defensive sweep for any
    // journal file this run left with unconfirmed entries. Always runs,
    // regardless of `body`'s outcome (see the comment above it).
    emit(EngineEvent::PhaseChanged {
        phase: Phase::Rollback,
    })
    .await;
    let rollback_result = rollback(sys.as_ref(), &ctx, &events).await;
    if let (Err(body_err), Err(rollback_err)) = (&body, &rollback_result) {
        // Mirrors `run_scenario`'s own `inner_result`/`revert_report`
        // priority handling: `body`'s error is the more actionable one to
        // return (it's why this run is failing at all), but a rollback
        // failure that co-occurs with it is real and must not vanish with
        // no trace just because only one `Err` can be returned below.
        tracing::error!(
            body_error = ?body_err,
            rollback_error = ?rollback_err,
            "run body failed AND its own rollback also failed"
        );
        if body_err.is_aborted() {
            // Same rule as `run_scenario`'s identical guard: an operator
            // Abort whose own ROLLBACK also failed must not come back as
            // `is_aborted() == true` -- `spawn_run`'s `e.is_aborted()`
            // check decides whether this run's journal files get pruned,
            // and a failed ROLLBACK is exactly the case where they must
            // survive for manual rollback.
            return Err(Error::msg(format!(
                "{body_err}; this run's own rollback also failed ({rollback_err}) -- \
                 journal left in place for manual rollback"
            )));
        }
    }
    let (baseline_result, scenario_results) = body?;
    rollback_result?;

    // REPORT
    emit(EngineEvent::PhaseChanged {
        phase: Phase::Report,
    })
    .await;
    let results = RunResults {
        schema_version: crate::model::SCHEMA_VERSION.to_string(),
        run_id: config.run_id.clone(),
        project_id: config.project.id.clone(),
        completed_at: crate::model::results::utc_timestamp_now(),
        detection_tier: DetectionTier::LogTail,
        baseline: baseline_result,
        scenarios: scenario_results,
    };
    let run_dir = config.data_root.join("runs").join(&config.run_id);
    results.save(&run_dir.join("results.json"))?;
    // Best-effort (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2/A5): only written when thermal mode was
    // actually used this run (a `Some` baseline), and a write failure here
    // is a warning, never a run failure -- `results.json` above is the
    // run's real record, this file is diagnostic.
    if thermal_baseline.is_some() {
        let thermal_log = crate::model::thermal::ThermalLog {
            baseline: thermal_baseline,
            cooldown_checks,
        };
        if let Err(e) = thermal_log.write(&run_dir) {
            tracing::warn!(error = %e, "failed to write thermal.json for this run");
            emit(EngineEvent::LogLine {
                text: format!("Failed to write thermal.json for this run: {e}"),
            })
            .await;
        }
    }
    emit(EngineEvent::RunComplete {
        run_id: config.run_id.clone(),
    })
    .await;
    Ok(results)
}

/// Computes `wcps`/`verdict`/`metric_deltas` for every entry in
/// `scenario_results` against `baseline_per_iteration`, using WCPS v3's real
/// calibrated pipeline (`stats::v3::evaluate_verdict` +
/// `stats::v3::compute_wcps_v3`). Called on the *whole* `scenario_results`
/// vec (baseline never appears in it -- `execute` builds it separately,
/// docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1) rather than filtering by `is_baseline`, since every element
/// already is non-baseline by construction.
///
/// `n` (the measure-iteration count) comes from `baseline_per_iteration`'s
/// own length -- `Settings::validate` already guarantees this is a
/// calibrated value (see `docs/superpowers/plans/2026-09-04-wcps-v3-pipeline-replace-v2.md`'s
/// `Settings::validate` tightening step), so `evaluate_verdict`'s `None` return
/// (uncalibrated `n`) should never actually happen in a real run; treated
/// as a hard `Err` rather than silently skipped, since a scenario this
/// function can't score has no meaningful `ScenarioResult` to produce.
///
/// Factored out of `execute` so it is unit-testable on its own: exercising
/// this logic through a full `execute()` round trip would mean driving the
/// entire phase sequencer (PREFLIGHT through ROLLBACK, `run_scenario` and
/// all) just to reach this one step. This function's own inputs/outputs
/// are plain data, so it needs none of that.
///
/// Uses a fresh `rand::thread_rng()` per call -- reproducibility across
/// runs is not a requirement here (unlike `stats`'s own tests, which seed
/// `StdRng` explicitly to assert bit-identical output).
fn score_scenarios(
    baseline_per_iteration: &[Metrics],
    scenario_results: &mut [ScenarioResult],
) -> Result<()> {
    use crate::stats::v3::{
        WcpsV3Weights, compute_wcps_v3, evaluate_verdict, load_calibrated_thresholds,
    };

    const THROUGHPUT_NAMES: [&str; 4] = ["avg_fps", "p1_fps", "p01_fps", "adaptive_frame_time_cv"];
    const PACING_NAMES: [&str; 2] = ["stutter_count_pct", "mean_abs_animation_error_ms"];
    // Same judgment call `stats::v3::verdict.rs`'s own tests use -- these 4
    // throughput margins were never added to the calibrated JSON cache (see
    // `evaluate_verdict`'s own doc comment for why), so they stay a
    // caller-supplied constant here too.
    let throughput_margins: std::collections::BTreeMap<String, f64> = [
        ("avg_fps", 3.0),
        ("p1_fps", 3.0),
        ("p01_fps", 3.0),
        ("adaptive_frame_time_cv", 10.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();

    let thresholds = load_calibrated_thresholds()?;
    let weights = WcpsV3Weights::default();
    let mut rng = rand::thread_rng();

    let to_throughput = |m: &Metrics| [m.avg_fps, m.p1_fps, m.p01_fps, m.adaptive_frame_time_cv];
    let to_pacing = |m: &Metrics| {
        [
            m.stutter_count_pct,
            m.mean_abs_animation_error_ms.unwrap_or(0.0),
        ]
    };

    let baseline_throughput: Vec<[f64; 4]> =
        baseline_per_iteration.iter().map(to_throughput).collect();
    let baseline_pacing: Vec<[f64; 2]> = baseline_per_iteration.iter().map(to_pacing).collect();
    let n = baseline_per_iteration.len() as u32;

    for result in scenario_results {
        let scenario_throughput: Vec<[f64; 4]> =
            result.per_iteration.iter().map(to_throughput).collect();
        let scenario_pacing: Vec<[f64; 2]> = result.per_iteration.iter().map(to_pacing).collect();

        let (verdict, ..) = evaluate_verdict(
            &baseline_throughput,
            &scenario_throughput,
            &baseline_pacing,
            &scenario_pacing,
            n,
            &thresholds,
            THROUGHPUT_NAMES,
            PACING_NAMES,
            &throughput_margins,
            &weights,
            &mut rng,
        )
        .ok_or_else(|| {
            Error::msg(format!(
                "no calibrated WCPS v3 threshold for n={n} measure iterations -- \
                 Settings::validate should have rejected this before the run started"
            ))
        })?;

        let throughput_scenario_mean = mean_cols_4(&scenario_throughput);
        let pacing_scenario_mean = mean_cols_2(&scenario_pacing);
        let wcps = compute_wcps_v3(
            &baseline_throughput,
            &baseline_pacing,
            throughput_scenario_mean,
            pacing_scenario_mean,
            &weights,
        );

        let metric_deltas = THROUGHPUT_NAMES
            .iter()
            .zip(0..4)
            .map(|(&name, i)| metric_delta(name, &baseline_throughput, &scenario_throughput, i))
            .chain(
                PACING_NAMES
                    .iter()
                    .zip(0..2)
                    .map(|(&name, i)| metric_delta_2(name, &baseline_pacing, &scenario_pacing, i)),
            )
            .collect();

        result.wcps = wcps;
        result.verdict = verdict;
        result.metric_deltas = metric_deltas;
    }
    Ok(())
}

fn mean_cols_4(rows: &[[f64; 4]]) -> [f64; 4] {
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

fn mean_cols_2(rows: &[[f64; 2]]) -> [f64; 2] {
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

/// A metric's plain percent delta (scenario mean vs. baseline mean) -- purely
/// descriptive, no significance test attached (WCPS v3's verdict is a single
/// holistic call across all 6 metrics together via Hotelling/TOST, not a
/// per-metric one the way WCPS v2's `MetricComparison` used to be).
fn metric_delta(
    name: &str,
    baseline: &[[f64; 4]],
    scenario: &[[f64; 4]],
    col: usize,
) -> crate::model::results::MetricDelta {
    let b = mean_cols_4(baseline)[col];
    let s = mean_cols_4(scenario)[col];
    crate::model::results::MetricDelta {
        metric: name.to_string(),
        delta_pct: if b.abs() > 1e-9 {
            (s - b) / b * 100.0
        } else {
            0.0
        },
    }
}

fn metric_delta_2(
    name: &str,
    baseline: &[[f64; 2]],
    scenario: &[[f64; 2]],
    col: usize,
) -> crate::model::results::MetricDelta {
    let b = mean_cols_2(baseline)[col];
    let s = mean_cols_2(scenario)[col];
    crate::model::results::MetricDelta {
        metric: name.to_string(),
        delta_pct: if b.abs() > 1e-9 {
            (s - b) / b * 100.0
        } else {
            0.0
        },
    }
}

/// A pre-run thermal baseline's CPU temperature must come back within this
/// many degrees Celsius of itself before a thermal-mode cooldown break
/// (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2) is considered over.
const THERMAL_COOLDOWN_THRESHOLD_C: f64 = 3.0;
/// Thermal-mode cooldown never waits longer than this, however hot the
/// system still reads -- proceeds anyway rather than stalling a run
/// indefinitely.
const THERMAL_COOLDOWN_MAX_WAIT: Duration = Duration::from_secs(10 * 60);
/// How long to wait between successive thermal-cooldown checks.
const THERMAL_COOLDOWN_CHECK_GAP: Duration = Duration::from_secs(30);

/// Sleeps `seconds`, emitting a `LogLine` first — a no-op when `seconds` is
/// `0`. Called after BASELINE's `run_scenario` call and after every
/// scenario's, while CS2 is genuinely closed (`run_scenario_inner` always
/// kills it before returning). See `RunConfig::inter_scenario_break_seconds`
/// for why this exists.
///
/// When `thermal_baseline`/`hwinfo_path` are both `Some` (this run has a
/// usable pre-run thermal baseline, docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2), `seconds` is ignored in favor
/// of [`wait_for_thermal_cooldown`]'s own poll-until-cool loop -- the
/// fixed-time break is thermal mode's fallback, not something it layers on
/// top of. That's still true if thermal mode itself fails partway through
/// the wait (HWiNFO's already-running check errors, it fails to start, or a
/// sensor read fails mid-loop): `seconds` isn't ignored on those paths, it's
/// exactly what `wait_for_thermal_cooldown` falls back to -- see its own doc
/// comment for which of its exit paths that applies to and which don't.
///
/// Returns the sequence of cooldown-check readings taken during this call
/// (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2's `thermal.json`), for the caller to record against whichever
/// scenario this break follows -- this function itself has no notion of
/// "which scenario", only the caller does. Always an empty `Vec` on the
/// plain fixed-time-sleep-or-noop path, since there is nothing to record
/// there.
///
/// Checks for a pending `Pause` before doing anything else -- the run's own
/// wait through this break (which can be as long as
/// `THERMAL_COOLDOWN_MAX_WAIT`) would otherwise be invisible to the operator
/// until the next scenario's first iteration. `Err` (`honor_pause` seeing an
/// `Abort` -- whether at the head of the channel while not paused, or while
/// already paused -- or `abort` already set) propagates straight out --
/// `execute()`'s caller still runs ROLLBACK regardless.
#[allow(clippy::too_many_arguments)]
async fn maybe_break(
    sys: &dyn SystemController,
    events: &mpsc::Sender<EngineEvent>,
    control: &mut mpsc::Receiver<ControlMsg>,
    abort: tokio::sync::watch::Receiver<bool>,
    seconds: u32,
    thermal_baseline: Option<&thermal::ThermalReading>,
    hwinfo_path: Option<&std::path::Path>,
    sample_interval: Duration,
    sample_count: u32,
) -> Result<Vec<thermal::ThermalReading>> {
    control::honor_pause(control).await?;
    if *abort.borrow() {
        return Err(Error::aborted("operator requested Abort".into()));
    }
    if let (Some(baseline), Some(hwinfo_path)) = (thermal_baseline, hwinfo_path) {
        return wait_for_thermal_cooldown(
            sys,
            events,
            control,
            abort,
            baseline,
            hwinfo_path,
            seconds,
            sample_interval,
            sample_count,
        )
        .await;
    }
    fixed_time_break(events, seconds, abort).await?;
    Ok(Vec::new())
}

/// Sleeps `seconds`, emitting a `LogLine` first -- a no-op when `seconds` is
/// `0`. The plain fixed-time break itself, shared by `maybe_break`'s own
/// no-thermal-baseline path and by [`wait_for_thermal_cooldown`]'s
/// failure-to-even-start-cooling exit paths (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A5: every thermal-mode
/// failure condition falls back to "Fixed-time break, unchanged"). Racing
/// the sleep against `abort` (via [`control::abortable`]) means an operator
/// Abort during this break takes effect immediately rather than after the
/// full `seconds` elapses.
async fn fixed_time_break(
    events: &mpsc::Sender<EngineEvent>,
    seconds: u32,
    abort: tokio::sync::watch::Receiver<bool>,
) -> Result<()> {
    if seconds == 0 {
        return Ok(());
    }
    let _ = events
        .send(EngineEvent::LogLine {
            text: format!("Cooldown break: sleeping {seconds}s (CS2 closed)"),
        })
        .await;
    control::abortable(abort, async {
        tokio::time::sleep(Duration::from_secs(seconds as u64)).await;
        Ok(())
    })
    .await
}

/// Polls [`thermal::sample_loop`] every `THERMAL_COOLDOWN_CHECK_GAP`,
/// logging each reading's delta from `baseline`, until the system is back
/// within `THERMAL_COOLDOWN_THRESHOLD_C` degrees of its pre-run baseline or
/// `THERMAL_COOLDOWN_MAX_WAIT` elapses (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2) -- whichever comes first.
///
/// Three failure conditions -- `hwinfo_already_running` erroring, a failed
/// `start_hwinfo`, or a sensor read failing partway through the loop -- mean
/// thermal mode never got a real cooldown wait in at all; per docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A5's
/// failure-mode table, each of these falls all the way back to
/// [`fixed_time_break`]'s plain `seconds`-based sleep (skipped if `seconds`
/// is `0`, matching `maybe_break`'s own no-op-at-zero behavior) rather than
/// returning with no wait whatsoever. The threshold-met and
/// `THERMAL_COOLDOWN_MAX_WAIT`-reached exit paths below are deliberately
/// exempt from this fallback -- those aren't failures, the wait already ran
/// for real (up to the full cap in the worst case), so a bonus fixed sleep
/// on top would just be a pointless double-wait.
///
/// Manages HWiNFO's lifecycle itself, once, around the whole wait --
/// starting it (only if not already running) before the first check and
/// closing it (only in that same case) after the last one, rather than
/// delegating that to [`thermal::collect_thermal_sample`] on every check
/// cycle. Each check cycle happens every `THERMAL_COOLDOWN_CHECK_GAP`
/// (~60s including sampling); restarting HWiNFO's process that often is
/// itself a brief CPU spike that could keep re-heating the very system
/// being measured, so this wait could otherwise never converge.
///
/// Returns every check's reading, in order, regardless of which exit path
/// is taken -- the caller (`maybe_break`) records these into `thermal.json`
/// via `model::thermal::ThermalLog`. Checks for `Pause` and `abort` once per
/// loop iteration (before each `sample_loop` call, not mid-sample) and races
/// `THERMAL_COOLDOWN_CHECK_GAP`'s own sleep against `abort` too -- an
/// operator Abort during this wait is caught within at most one sampling
/// pass (`sample_count * sample_interval`, ~30s at the real production
/// cadence) rather than the full remaining cooldown wait, which could
/// otherwise run up to `THERMAL_COOLDOWN_MAX_WAIT`.
#[allow(clippy::too_many_arguments)]
async fn wait_for_thermal_cooldown(
    sys: &dyn SystemController,
    events: &mpsc::Sender<EngineEvent>,
    control: &mut mpsc::Receiver<ControlMsg>,
    abort: tokio::sync::watch::Receiver<bool>,
    baseline: &thermal::ThermalReading,
    hwinfo_path: &std::path::Path,
    seconds: u32,
    sample_interval: Duration,
    sample_count: u32,
) -> Result<Vec<thermal::ThermalReading>> {
    let mut checks = Vec::new();

    let already_running = match sys.hwinfo_already_running().await {
        Ok(r) => r,
        Err(e) => {
            let _ = events
                .send(EngineEvent::LogLine {
                    text: format!(
                        "Thermal cooldown: could not check whether HWiNFO is already running \
                         ({e}), falling back to the fixed-time break"
                    ),
                })
                .await;
            fixed_time_break(events, seconds, abort).await?;
            return Ok(checks);
        }
    };
    if !already_running && let Err(e) = sys.start_hwinfo(hwinfo_path).await {
        let _ = events
            .send(EngineEvent::LogLine {
                text: format!(
                    "Thermal cooldown: failed to start HWiNFO ({e}), falling back to the \
                     fixed-time break"
                ),
            })
            .await;
        fixed_time_break(events, seconds, abort).await?;
        return Ok(checks);
    }

    let deadline = tokio::time::Instant::now() + THERMAL_COOLDOWN_MAX_WAIT;
    // Set on the sensor-read-failure exit only -- distinguishes it from the
    // threshold-met/cap-reached exits below, which share this same `break`
    // but must NOT get the fixed-time fallback (see this function's own doc
    // comment on why only the three genuinely-failed-to-cool paths do).
    let mut sensor_read_failed = false;
    // Set on either abort-check exit below -- both share the same
    // close-hwinfo-then-return-Err tail as the other exit paths' own
    // cleanup, so it's checked once, after the loop, alongside
    // `sensor_read_failed`.
    let mut aborted = false;
    loop {
        if control::honor_pause(control).await.is_err() || *abort.borrow() {
            aborted = true;
            break;
        }
        let sample = match thermal::sample_loop(sys, sample_interval, sample_count).await {
            Ok(s) => s,
            Err(e) => {
                let _ = events
                    .send(EngineEvent::LogLine {
                        text: format!(
                            "Thermal cooldown: sensor read failed ({e}), falling back to the \
                             fixed-time break"
                        ),
                    })
                    .await;
                sensor_read_failed = true;
                break;
            }
        };
        let delta = (sample.cpu_temp_celsius - baseline.cpu_temp_celsius).abs();
        let _ = events
            .send(EngineEvent::LogLine {
                text: format!(
                    "Thermal cooldown: {:.1}C (baseline {:.1}C, delta {:.1}C)",
                    sample.cpu_temp_celsius, baseline.cpu_temp_celsius, delta
                ),
            })
            .await;
        checks.push(sample);
        if delta <= THERMAL_COOLDOWN_THRESHOLD_C {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let _ = events
                .send(EngineEvent::LogLine {
                    text: "Thermal cooldown: 10-minute cap reached, proceeding anyway".into(),
                })
                .await;
            break;
        }
        if control::abortable(abort.clone(), async {
            tokio::time::sleep(THERMAL_COOLDOWN_CHECK_GAP).await;
            Ok(())
        })
        .await
        .is_err()
        {
            aborted = true;
            break;
        }
    }

    if !already_running && let Err(close_err) = sys.close_hwinfo().await {
        // A close failure must never mask an already-accumulated result --
        // log and return `checks` untouched either way, mirroring
        // `collect_thermal_sample`'s own close-failure handling.
        tracing::warn!(
            error = %close_err,
            "failed to close HWiNFO after a thermal cooldown wait"
        );
        let _ = events
            .send(EngineEvent::LogLine {
                text: format!("Failed to close HWiNFO after a thermal cooldown wait: {close_err}"),
            })
            .await;
    }

    if aborted {
        return Err(Error::aborted("operator requested Abort".into()));
    }

    // Fixed-time fallback for the sensor-read-failure exit only, run after
    // HWiNFO is already closed above -- there's no reason to keep it running
    // for the extra fallback sleep once this call is done sampling it.
    if sensor_read_failed {
        fixed_time_break(events, seconds, abort).await?;
    }

    Ok(checks)
}
