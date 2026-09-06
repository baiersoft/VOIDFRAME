//! The per-scenario CS2 lifecycle (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1/§7.2/§7.3): `run_scenario`
//! applies a scenario's modules, launches CS2, runs warmup + measure
//! iterations, kills CS2, and reverts — success or failure. `run_scenario`
//! is called once per baseline and once per enabled scenario by
//! `super::execute`.

use super::RunContext;
use super::control::{abortable, honor_pause};
use super::launch::{is_ready_marker, prepare_cs2_session, wait_for_menu_ready};
use super::steam::{graceful_kill_cs2, kill_process_tree, wait_for_process};
use crate::capture::runner::CaptureRunner;
use crate::cs2::detection::{Cs2LogDetector, DetectionEvent, RealLogTail, Signatures};
use crate::error::{Error, Result};
use crate::journal::Journal;
use crate::model::project::Scenario;
use crate::model::results::{Metrics, ScenarioResult};
use crate::model::{AffinityCpuPayload, Module, Settings};
use crate::run::{ControlMsg, EngineEvent, IterationKind};
use crate::system::{Cs2LaunchSpec, SystemController};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

/// Opens the CS2-readiness detection channel `run_scenario_inner`/
/// `run_one_iteration` wait on: a real `RealLogTail` tailing `console.log`,
/// or (`RunConfig::mock_cs2_log`, the E2E mock-run mode) a `MockLogTail`
/// that never touches a real file — `console_log_path`/`sigs` are ignored
/// in that case, since no real CS2 process exists to have written either.
async fn open_log_tail(
    path: &Path,
    sigs: &Signatures,
    mock: Option<Duration>,
) -> Result<Box<dyn Cs2LogDetector>> {
    if let Some(event_delay) = mock {
        Ok(Box::new(crate::mock_harness::MockLogTail::new(event_delay)))
    } else {
        Ok(Box::new(RealLogTail::open(path, sigs).await?))
    }
}

/// Runs one scenario (baseline included — `is_baseline` distinguishes it
/// for reporting only, the mechanics are identical since baseline's
/// `apply_scenario` is a no-op over zero modules) end to end: CS2 launch,
/// warmup + measure iterations, kill, and returns the aggregated
/// `ScenarioResult`.
pub(super) async fn run_scenario(
    sys: &dyn SystemController,
    capture: &dyn CaptureRunner,
    scenario: &Scenario,
    is_baseline: bool,
    ctx: &RunContext<'_>,
    events: &mpsc::Sender<EngineEvent>,
    control: &mut mpsc::Receiver<ControlMsg>,
) -> Result<ScenarioResult> {
    // --- APPLY_MODULES (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1) ---
    // Real registry/powercfg/power_plan mutations apply now, while CS2 is
    // closed; affinity_cpu and launch_args are no-ops here (they apply at
    // CS2 launch, below) — this exactly matches mutation::apply_module's
    // EXISTING behavior
    // (`docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md`),
    // no change needed to that function.
    let journal_path = ctx
        .config
        .data_root
        .join("runs")
        .join(&ctx.config.run_id)
        .join(format!("journal-{}.jsonl", scenario.id));
    let mut journal = Journal::open(&journal_path)?;
    events
        .send(EngineEvent::LogLine {
            text: format!("Applying modules for scenario '{}'", scenario.name),
        })
        .await
        .ok();
    if let Err(e) =
        crate::mutation::apply_scenario(scenario, &ctx.config.run_id, sys, &mut journal).await
    {
        // docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §10: "Mutation fails mid-APPLY_MODULES -> Stop the scenario,
        // mark it errored, reverse-replay this scenario's entries, continue
        // to next scenario."
        drop(journal);
        let _ = crate::mutation::revert_scenario(&scenario.id, &journal_path, sys).await;
        return Err(e);
    }
    drop(journal); // flush/close before revert_scenario re-opens it for replay

    // Everything from here on (the build-freeze re-check included — see
    // `run_scenario_inner`'s own doc comment for why it moved there) needs
    // uniform cleanup on failure: if CS2 has been launched, it must be
    // killed; either way this scenario's journal must be reverted. The
    // plan's own illustrative sketch only wires that cleanup for the
    // APPLY_MODULES failure above — anything failing after that (the
    // build-freeze check, an operator Abort, an iteration failure
    // including a watchdog hard failure) would otherwise skip both the
    // kill and the revert entirely, leaving CS2 orphaned and this
    // scenario's mutations un-reverted. A plain `?` chain can't recover
    // control to run that cleanup, so this delegates to
    // `run_scenario_inner`, threading through an out-param for "what pid,
    // if any, is still alive" so the caller always knows what (if
    // anything) still needs killing.
    let mut launched_pid: Option<u32> = None;
    let inner_result = run_scenario_inner(
        sys,
        capture,
        scenario,
        ctx,
        events,
        control,
        &mut launched_pid,
    )
    .await;

    if inner_result.is_err()
        && let Some(pid) = launched_pid
    {
        let _ = kill_process_tree(sys, pid).await;
    }

    events
        .send(EngineEvent::LogLine {
            text: format!("Reverting modules for scenario '{}'", scenario.name),
        })
        .await
        .ok();
    let revert_report = crate::mutation::revert_scenario(&scenario.id, &journal_path, sys).await;

    if inner_result.is_err() && revert_report.is_err() {
        // `?` below can only propagate ONE error — the scenario's own
        // failure takes priority since it's the more actionable one for
        // the caller (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §10: continue to the next scenario). Without
        // this, a revert failure that co-occurs with a scenario failure
        // would vanish with no trace at all — the revert error is real
        // (revert is still attempted either way) and worth knowing about
        // even though it can't also be the returned `Err`.
        tracing::error!(
            scenario = %scenario.id,
            scenario_error = ?inner_result.as_ref().err(),
            revert_error = ?revert_report.as_ref().err(),
            "scenario failed AND its journal revert also failed"
        );
    }
    if let (Err(inner_err), Err(revert_err)) = (&inner_result, &revert_report)
        && inner_err.is_aborted()
    {
        // An operator Abort whose own journal revert also failed must not
        // come back as `is_aborted() == true` -- `spawn_run`'s
        // `e.is_aborted()` check is what decides whether this run's
        // `journal-*.jsonl` gets pruned, and a failed revert is exactly the
        // case where that journal must survive for manual rollback.
        return Err(Error::msg(format!(
            "{inner_err}; this scenario's journal revert also failed ({revert_err}) -- \
             journal left in place for manual rollback"
        )));
    }

    let per_iteration_measure = inner_result?;
    let revert_report = revert_report?;
    if !revert_report.verify_failures.is_empty() {
        // docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §6.2: never swallowed — recorded, but scenario still
        // produces a result (the run continues; the header banner /
        // results.json surfacing happens at the run/report level, not
        // aborted here).
        tracing::warn!(
            scenario = %scenario.id,
            failures = ?revert_report.verify_failures,
            "rollback verify mismatch"
        );
        events
            .send(EngineEvent::LogLine {
                text: format!(
                    "Rollback verify mismatch for scenario '{}': {:?}",
                    scenario.name, revert_report.verify_failures
                ),
            })
            .await
            .ok();
    }

    // --- Aggregate + score (§8) ---
    let aggregated = crate::stats::aggregate_iterations(&per_iteration_measure)?;
    Ok(ScenarioResult {
        scenario_id: scenario.id.clone(),
        name: scenario.name.clone(),
        is_baseline,
        aggregated,
        per_iteration: per_iteration_measure,
        metric_deltas: vec![], // filled in at the run/report level once baseline exists to compare against
        wcps: 0.0,             // same
        verdict: crate::model::results::Verdict::ConfirmedSame, // placeholder, filled in by score_scenarios (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §8) -- see that function's own doc comment for why the baseline's own entry keeps this value unchanged
    })
}

/// Everything from the build-freeze re-check through "CS2 killed, detection
/// channel closed" — the part of a scenario that can leave real mutations
/// un-reverted or CS2 running if it fails partway. `launched_pid` is set
/// the moment CS2 is confirmed running, kept in sync with the watchdog
/// retry's relaunch, and cleared back to `None` once this function kills
/// CS2 itself on the success path — so the caller only needs to act on it
/// when this function returns `Err`.
///
/// The build-freeze check specifically lives here (not in `run_scenario`,
/// where it originally landed) *because* this is the function whose result
/// flows through `run_scenario`'s unconditional kill+revert cleanup: an
/// earlier version had it sit between APPLY_MODULES and this call, where a
/// failed `sys.app_manifest` read (or an operator `Abort` mid-prompt) would
/// return `Err` straight out of `run_scenario` via `?` with real
/// registry/powercfg mutations left applied and zero cleanup attempt. It
/// needs no CS2 state, so it runs first, before Steam-readiness.
pub(super) async fn run_scenario_inner(
    sys: &dyn SystemController,
    capture: &dyn CaptureRunner,
    scenario: &Scenario,
    ctx: &RunContext<'_>,
    events: &mpsc::Sender<EngineEvent>,
    control: &mut mpsc::Receiver<ControlMsg>,
    launched_pid: &mut Option<u32>,
) -> Result<Vec<Metrics>> {
    let session = prepare_cs2_session(sys, scenario, ctx, events, control).await?;
    *launched_pid = Some(session.pid);
    let mut cs2_pid = session.pid;

    let affinity_module = scenario.modules.iter().find_map(|m| match m {
        Module::AffinityCpu(p) => Some(p),
        _ => None,
    });

    // --- Connect detection channel (§7.2 step 4) ---
    // `console_log_path` and `sigs` were both already resolved/loaded
    // above (before launch), so this just opens the tail.
    let mut tail = open_log_tail(
        &session.console_log_path,
        &session.sigs,
        ctx.config.mock_cs2_log,
    )
    .await?;

    // --- Iterations (§7.3) ---
    let watchdog = Duration::from_secs(ctx.config.project.settings.watchdog_seconds as u64);

    // Confirmed live: the Win32 "window is visible" signal alone (already
    // waited for inside `reissue_map` itself) fires several seconds before
    // CS2 is actually processing keyboard input — the game still shows its
    // own boot/intro sequence after the window appears. Wait for a real,
    // log-observed readiness signal before ever sending simulated input,
    // rather than guessing a fixed delay (see `wait_for_menu_ready`'s own
    // doc comment). Reuses this scenario's own `watchdog` duration as the
    // search budget — real runs get a generous, user-configured timeout for
    // free; tests using a short `watchdog_seconds` stay fast automatically.
    abortable(ctx.abort.clone(), async {
        wait_for_menu_ready(&mut *tail, watchdog, events).await;
        Ok(())
    })
    .await?;

    // Applied here, not right after process discovery above: the process
    // existing is not the same as CS2 having actually finished loading, and
    // `wait_for_menu_ready` is the real "game is loaded and responsive"
    // signal this codebase already has -- affinity has no journal/rollback
    // dependency (process-scoped, dies with CS2), so there's no correctness
    // reason to apply it any earlier than immediately before the first
    // `reissue_map` call that follows.
    if let Some(p) = affinity_module {
        crate::mutation::affinity_cpu::apply_to_pid(p, cs2_pid, sys).await?;
    }

    let iter_ctx = IterationCtx {
        sys,
        settings: &ctx.config.project.settings,
        watchdog,
        console_log_path: &session.console_log_path,
        sigs: &session.sigs,
        mock_cs2_log: ctx.config.mock_cs2_log,
        abort: ctx.abort.clone(),
        affinity: affinity_module,
        webview_root_pid: ctx.config.webview_root_pid,
        events,
        data_root: &ctx.config.data_root,
        run_id: &ctx.config.run_id,
        scenario_id: &scenario.id,
    };
    let mut per_iteration_measure = Vec::new();

    for i in 0..ctx.config.project.settings.warmup_loops {
        events
            .send(EngineEvent::IterationStarted {
                kind: IterationKind::Warmup,
                index: i,
            })
            .await
            .ok();
        let iter_result =
            run_one_iteration(&iter_ctx, None, i, &mut tail, &mut cs2_pid, control).await;
        // Sync the (possibly watchdog-relaunched) pid back to the caller's
        // cleanup hook regardless of outcome, before propagating any error.
        *launched_pid = Some(cs2_pid);
        iter_result?;
        events
            .send(EngineEvent::LogLine {
                text: format!("warmup iteration {i} complete"),
            })
            .await
            .ok();
    }
    for i in 0..ctx.config.project.settings.measure_loops {
        events
            .send(EngineEvent::IterationStarted {
                kind: IterationKind::Measure,
                index: i,
            })
            .await
            .ok();
        let iter_result = run_one_iteration(
            &iter_ctx,
            Some(capture),
            i,
            &mut tail,
            &mut cs2_pid,
            control,
        )
        .await;
        *launched_pid = Some(cs2_pid);
        let metrics = iter_result?.expect("measure iteration always returns Some metrics");
        events
            .send(EngineEvent::IterationComplete {
                metrics: metrics.clone(),
            })
            .await
            .ok();
        events
            .send(EngineEvent::LogLine {
                text: format!("measure iteration {i} complete"),
            })
            .await
            .ok();
        per_iteration_measure.push(metrics);
    }

    // --- Close CS2, close detection channel (§7.3a) ---
    drop(tail);
    graceful_kill_cs2(sys).await?;
    *launched_pid = None; // already cleaned up — caller must not kill again

    Ok(per_iteration_measure)
}

/// Read-only context `run_one_iteration` needs, bundled so its own
/// parameter list stays well under clippy's `too_many_arguments`
/// threshold. `tail` and `cs2_pid` stay as separate `&mut` parameters on
/// the function itself since both may be replaced mid-iteration by the
/// watchdog retry (§7.3 step 6).
struct IterationCtx<'a> {
    sys: &'a dyn SystemController,
    settings: &'a Settings,
    watchdog: Duration,
    console_log_path: &'a Path,
    sigs: &'a Signatures,
    mock_cs2_log: Option<Duration>,
    abort: tokio::sync::watch::Receiver<bool>,
    affinity: Option<&'a AffinityCpuPayload>,
    webview_root_pid: Option<u32>,
    events: &'a mpsc::Sender<EngineEvent>,
    /// Needed only to build this scenario's own permanent capture directory
    /// (see [`capture_csv_path`]) — a measure iteration's raw PresentMon CSV
    /// is kept there rather than a leaked temp file.
    data_root: &'a Path,
    run_id: &'a str,
    scenario_id: &'a str,
}

/// Settle between the console closing and starting a capture. Domain
/// knowledge from this benchmark map specifically (not a generic engine
/// concern): after the loading screen ends and the console closes, there's
/// a black-screen countdown (~5s to 0), then the benchmark itself starts —
/// and its first few seconds have abnormal frame times (shader/asset
/// warm-up, not representative of steady-state performance). Capturing
/// through that window would pollute every iteration's metrics with a
/// transient that has nothing to do with whatever's actually being
/// benchmarked. Purely additive: this runs *before* `PmArgs.timed_seconds`
/// starts counting, not carved out of it, so `capture_seconds` still means
/// what the project config says it means.
const SETTLE_BEFORE_CAPTURE: Duration = Duration::from_secs(8);

/// One warmup (`capture: None`) or measure (`capture: Some`) iteration.
/// Returns `Ok(Some(metrics))` for a measure iteration, `Ok(None)` for
/// warmup.
///
/// Implements the watchdog retry (§7.3 step 6) for real: on a detection
/// timeout waiting for the map-loaded/benchmark-started marker, this kills
/// CS2, relaunches it fresh, reopens the detection channel (`console.log`
/// is truncated by a fresh CS2 launch — the previous `LogTail`'s file
/// position would otherwise sit past the new, shorter file's end and never
/// see new lines), reapplies this scenario's CPU-affinity module if any,
/// reissues the map command, and retries the wait exactly once before
/// surfacing a hard failure. `cs2_pid` is `&mut` because a successful
/// retry changes it — every subsequent iteration in the same scenario, and
/// the scenario's final kill, must use the *new* pid; it is kept
/// up to date even when this function ultimately returns `Err`, so the
/// caller's own cleanup always targets whatever is actually still alive.
async fn run_one_iteration(
    ctx: &IterationCtx<'_>,
    capture: Option<&dyn CaptureRunner>,
    iteration_index: u32,
    tail: &mut Box<dyn Cs2LogDetector>,
    cs2_pid: &mut u32,
    control: &mut mpsc::Receiver<ControlMsg>,
) -> Result<Option<Metrics>> {
    // Honour Pause between iterations (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1: "ControlMsg::Pause is
    // honoured only between iterations").
    honor_pause(control).await?;

    let map_cmd = format!("map_workshop {} de_dust2", ctx.settings.map_id);
    ctx.sys.reissue_map(&map_cmd).await?;
    ctx.events
        .send(EngineEvent::LogLine {
            text: "Waiting for map load".into(),
        })
        .await
        .ok();

    let mut ready = abortable(
        ctx.abort.clone(),
        tail.wait_for(ctx.watchdog, &mut is_ready_marker),
    )
    .await?;
    if ready.is_none() {
        // Diagnostics: distinguishes "nothing was ever read from
        // console.log during this wait" (a truncation/identity-detection
        // bug, or reissue_map's input never actually reaching CS2) from
        // "plenty was read, just never the map-load marker" (a regex
        // problem, or CS2 genuinely failing to load the map) — without
        // needing to reproduce a live failure to find out which.
        ctx.events
            .send(EngineEvent::LogLine {
                text: format!(
                    "Watchdog timeout waiting for map load — read {} line(s) in that time, \
                     last one: {:?} — relaunching CS2 and retrying once",
                    tail.lines_read(),
                    tail.last_line_seen().unwrap_or("<none>")
                ),
            })
            .await
            .ok();
        kill_process_tree(ctx.sys, *cs2_pid).await?;
        ctx.sys.launch_cs2(&Cs2LaunchSpec { app_id: 730 }).await?;
        let relaunched =
            wait_for_process(ctx.sys, "cs2.exe", Duration::from_secs(60), true).await?;
        *cs2_pid = relaunched.pid;
        *tail = open_log_tail(ctx.console_log_path, ctx.sigs, ctx.mock_cs2_log).await?;
        abortable(ctx.abort.clone(), async {
            wait_for_menu_ready(&mut **tail, ctx.watchdog, ctx.events).await;
            Ok(())
        })
        .await?;
        // Same reasoning as the initial-launch path: applied after the game
        // is actually loaded and responsive, not right after the relaunched
        // process is discovered.
        if let Some(affinity) = ctx.affinity {
            crate::mutation::affinity_cpu::apply_to_pid(affinity, *cs2_pid, ctx.sys).await?;
        }
        ctx.sys.reissue_map(&map_cmd).await?;
        ready = abortable(
            ctx.abort.clone(),
            tail.wait_for(ctx.watchdog, &mut is_ready_marker),
        )
        .await?;
        if ready.is_none() {
            // docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §10: a second watchdog failure -> RunFailed -> full
            // ROLLBACK. Returning Err here, propagated up through
            // `run_scenario_inner` and `run_scenario`, is what achieves
            // that — the caller's own kill+revert cleanup still runs on
            // this Err (see `run_scenario`'s doc comment).
            return Err(Error::msg(format!(
                "watchdog: no map-loaded/benchmark-started marker, even after a relaunch retry \
                 — read {} line(s) in that final wait, last one: {:?}",
                tail.lines_read(),
                tail.last_line_seen().unwrap_or("<none>")
            )));
        }
    }

    // `ready` is `Some` here — either from the initial wait, or the
    // relaunch retry above (any other path already returned `Err`). Close
    // the console now, via the real signal that the load actually
    // finished, rather than on a fixed delay guessed inside `reissue_map`
    // itself — a real cold map load's duration isn't knowable up front,
    // and guessing it short was confirmed live to leave the console open.
    ctx.sys.hide_console().await?;

    // `suspended` tracks whether the block below actually needs undoing —
    // not currently reachable in this plan (`webview_root_pid` is always
    // `None` everywhere in M1), but new code, and worth getting right now
    // rather than shipping a known latent bug for
    // `docs/superpowers/plans/2026-09-01-m1-phase-4a-tauri-backend-bridge.md`
    // (the first real `Some(webview_root_pid)` caller) to discover later.
    let suspended = capture.is_some() && ctx.webview_root_pid.is_some();
    if capture.is_some() {
        ctx.events.send(EngineEvent::CapturePending).await.ok();
        if let Some(webview_pid) = ctx.webview_root_pid {
            ctx.sys.suspend_process_tree(webview_pid).await?;
        }
    }

    // The capture + BenchmarkEnded wait can each fail; whatever was
    // suspended above must still be resumed on the way out either way — a
    // bare `?` here (as the previous version had) would skip the resume
    // entirely on either failure, leaving the webview tree suspended for
    // the rest of the run. Capturing the fallible part's `Result` and
    // resuming unconditionally afterward, before propagating it, mirrors
    // the same "always runs" pattern `run_scenario`'s own kill+revert
    // cleanup already uses for CS2.
    let body_result: Result<Option<Metrics>> = async {
        let metrics = if let Some(c) = capture {
            abortable(ctx.abort.clone(), async {
                tokio::time::sleep(SETTLE_BEFORE_CAPTURE).await;
                Ok(())
            })
            .await?;
            let output_file =
                capture_csv_path(ctx.data_root, ctx.run_id, ctx.scenario_id, iteration_index);
            // PresentMon does not create missing directories for its own
            // `--output_file` — this scenario's capture directory must
            // already exist before the process is spawned.
            if let Some(parent) = output_file.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let args = crate::capture::presentmon::PmArgs {
                process_name: "cs2.exe".into(),
                output_file,
                timed_seconds: ctx.settings.capture_seconds,
            };
            ctx.events.send(EngineEvent::RecordingStarted).await.ok();
            // `kill_on_drop(true)` on the real `PresentMonController`'s
            // `Command` (see its own doc comment) makes an abort here
            // actually terminate the real PresentMon process rather than
            // leaving it running detached.
            let capture_result = abortable(ctx.abort.clone(), c.capture(&args)).await;
            // Sent regardless of outcome — a listener bracketing the real
            // capture window (e.g. an audio cue) cares that recording
            // ended, whether or not it ended successfully. `?` below still
            // propagates the same error unchanged.
            ctx.events.send(EngineEvent::RecordingStopped).await.ok();
            Some(capture_result?)
        } else {
            None
        };

        let _ = abortable(
            ctx.abort.clone(),
            tail.wait_for(ctx.watchdog, &mut |ev| {
                matches!(ev, DetectionEvent::BenchmarkEnded)
            }),
        )
        .await?;

        Ok(metrics)
    }
    .await;

    if suspended && let Some(webview_pid) = ctx.webview_root_pid {
        let _ = ctx.sys.resume_process_tree(webview_pid).await;
    }
    if capture.is_some() {
        ctx.events.send(EngineEvent::CaptureResumed).await.ok();
    }

    body_result
}

/// The permanent on-disk location for one iteration's raw PresentMon CSV.
/// Every real capture is kept here — not discarded to a temp file, as this
/// used to do — so the raw recording is available to other tools and lets
/// VOIDFRAME's own aggregated metrics be independently validated against
/// it. `scenario_id` is `"baseline"` for the baseline's own dummy scenario
/// (`execute::execute`'s `baseline_dummy_scenario`), giving it the same
/// `scenario-<id>/` layout as every real scenario.
fn capture_csv_path(
    data_root: &Path,
    run_id: &str,
    scenario_id: &str,
    iteration_index: u32,
) -> PathBuf {
    data_root
        .join("runs")
        .join(run_id)
        .join(format!("scenario-{scenario_id}"))
        .join(format!("iter-{iteration_index:02}.csv"))
}

#[cfg(test)]
mod capture_path_tests {
    use super::*;

    #[test]
    fn builds_the_run_scenario_iteration_layout() {
        let data_root = Path::new(r"C:\data");
        let path = capture_csv_path(data_root, "r1", "s1", 3);
        assert_eq!(
            path,
            PathBuf::from(r"C:\data\runs\r1\scenario-s1\iter-03.csv")
        );
    }

    #[test]
    fn baseline_scenario_id_gets_its_own_directory() {
        let data_root = Path::new(r"C:\data");
        let path = capture_csv_path(data_root, "r1", "baseline", 0);
        assert_eq!(
            path,
            PathBuf::from(r"C:\data\runs\r1\scenario-baseline\iter-00.csv")
        );
    }

    #[test]
    fn iteration_index_is_zero_padded_to_two_digits() {
        let data_root = Path::new(r"C:\data");
        let path = capture_csv_path(data_root, "r1", "s1", 7);
        assert_eq!(path.file_name().unwrap(), "iter-07.csv");
    }
}
