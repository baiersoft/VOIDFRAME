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
//! (`honor_pause`, and `wait_for_abort_allowed` -- the abort arm `execute`
//! races the whole run body against) shared across the run loop's wait
//! points.
//! This file keeps the top-level phase sequencer (`execute`) plus scoring
//! (the run-start timestamp itself lives in
//! `crate::model::results::utc_timestamp_now`).
//!
//! Since M3 (`docs/superpowers/specs/2026-09-06-m3-autonomous-reboots-design.md`), BASELINE + SCENARIO* is a cursor-driven loop
//! (`scenario_loop`) over `[baseline] ++ enabled scenarios`, walking each
//! through `apply`/`measure`/`revert` and persisting `RunProgress` at every
//! boundary so a reboot scenario can stop the process (`RunOutcome::
//! RebootPending`) and a later `--resume` (`begin_resume`) can pick the
//! cursor back up. `reboot_sequence` is the reboot itself (recovery
//! scheduled tasks + `sys.reboot()`); `finish_run` is scoring/ROLLBACK/
//! REPORT plus the shutdown-or-final-reboot decision once the loop is
//! genuinely done.

mod boot;
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
use crate::model::Module;
use crate::model::progress::{Cursor, PendingReboot, RunProgress, Stage, UnstableScenario};
use crate::model::project::{Project, Scenario};
use crate::model::results::{Metrics, RunResults, ScenarioResult};
use crate::preflight::{self, PreflightFacts};
use crate::run::phase::RebootReason;
use crate::run::{ControlMsg, DetectionTier, EngineEvent, Phase, Transition, plan_transition};
use crate::system::{
    BootReport, DEADMAN_TASK_NAME, RESUME_TASK_NAME, SystemController, TaskPrincipal, TaskSpec,
    TaskTrigger,
};
pub use checks::{
    HwinfoCheckReport, HwinfoCheckStep, SteamLaunchCheckReport, SteamLaunchCheckStep, hwinfo_check,
    steam_launch_check,
};
use context::RunContext;
use rollback::rollback;
#[cfg(test)]
use scenario::run_scenario;
use scenario::{apply_stage, measure_stage, revert_stage, scenario_result};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use steam::ensure_steam_running;
use tokio::sync::mpsc;

use std::time::Duration;

/// `Clone` so `execute()` can keep a copy of the whole config for
/// `finalize_aborted_run`: on the abort path the run body future is dropped
/// at an arbitrary await point, taking every local it borrowed (`config`
/// included) with it, so the cleanup arm needs its own owned copy captured
/// before the body ever starts.
#[derive(Clone)]
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
    /// Spec §7 / D3: shut the machine down instead of the final revert
    /// reboot. Only the run's *initial* value -- `begin_fresh` seeds
    /// `RunProgress::shutdown_when_complete` from this once, at run start,
    /// and every actual decision point reads the live `progress` field from
    /// then on (it can be toggled mid-run via the dedicated
    /// `shutdown_toggle` channel `execute()` also takes -- see
    /// `control::drain_shutdown_toggle`). Do not read this field directly
    /// past `begin_fresh`.
    pub shutdown_when_complete: bool,
    /// Spec D5: wait this long after `--resume` before touching anything.
    pub post_boot_settle: Duration,
    /// The executable the resume/deadman tasks launch (normally
    /// `std::env::current_exe()`); explicit so tests never register a task
    /// pointing at the test binary.
    pub exe_path: PathBuf,
}

/// How a run enters `execute()`: freshly, at PREFLIGHT, or resuming
/// mid-scenario after a VOIDFRAME-initiated reboot (spec §3.2), with the
/// on-disk `RunProgress` this process's own cursor picks up from.
#[expect(
    clippy::large_enum_variant,
    reason = "RunStart is built once per process launch (Fresh at start_run, Resume at \
              --resume), never in a hot loop; boxing RunProgress here would deviate from the \
              plain two-field shape the M3 spec's RunStart interface specifies for no runtime \
              benefit"
)]
pub enum RunStart {
    Fresh(RunConfig),
    Resume {
        config: RunConfig,
        progress: RunProgress,
    },
}

impl RunStart {
    pub fn config(&self) -> &RunConfig {
        match self {
            RunStart::Fresh(c) | RunStart::Resume { config: c, .. } => c,
        }
    }
}

/// How a run's own `execute()` call ends -- not always with a finished
/// report, per spec §3: a reboot scenario stops the process here rather
/// than finishing, and a resumed process picks back up from `RunProgress`.
#[derive(Debug)]
pub enum RunOutcome {
    Complete(RunResults),
    /// `sys.reboot()` was called; the process is about to die.
    RebootPending(RebootReason),
    /// Results are on disk; `sys.shutdown()` was called with a 60 s countdown.
    ShutdownRequested(RunResults),
}

/// How [`scenario_loop`] ends -- an internal detail of `execute()`'s own
/// dispatch, never surfaced past this module (see [`RunOutcome`] for the
/// public shape).
enum LoopExit {
    Reboot(RebootReason),
    /// The scenario loop itself is done; whether the machine still needs a
    /// reboot to make the very last revert take effect is decided by
    /// [`finish_run`], after scoring/ROLLBACK/REPORT.
    Done {
        final_revert_needs_reboot: bool,
    },
    /// Resumed with `cursor.stage == Done` and `results.json` already on
    /// disk -- the previous process's own `finish_run` already ran; this
    /// resume has nothing left to do but tidy up.
    AlreadyFinished(Box<RunResults>),
}

fn run_dir(config: &RunConfig) -> PathBuf {
    config.data_root.join("runs").join(&config.run_id)
}

/// Resolves the real, on-disk `cs2_video.txt` path for the currently active
/// Steam account's CS2 install, for `VOIDFRAME_RESTORE.bat`'s
/// `Op::Cs2ConfigApply` line -- `None` when it can't be determined (Steam
/// not installed, no CS2 userdata yet), which is fine for a run with no
/// `cs2_config` module at all and must never fail `write_all` for every
/// other run. Goes through `SystemController::find_cs2_video_config_path`
/// (not a direct `system::windows` reach-around) so this resolution is
/// mockable/testable like every other cross-boundary call -- previously a
/// deliberate, parked exception to that rule; now closed.
async fn resolve_real_video_txt_path(sys: &dyn SystemController) -> Option<PathBuf> {
    sys.find_cs2_video_config_path().await.ok()
}

async fn emit(events: &mpsc::Sender<EngineEvent>, ev: EngineEvent) {
    let _ = events.send(ev).await;
}

/// ROLLBACK-adjacent cleanup that must happen on every exit except a
/// pending reboot (a reboot is about to kill this process anyway, and
/// `reboot_sequence` re-registers the tasks it needs on the far side of
/// it).
///
/// `remove_restore_script`: only when ROLLBACK actually confirmed the
/// machine is back to its run-start state. After a *failed* rollback the
/// journal is deliberately left in place for manual rollback, and the
/// standalone `VOIDFRAME_RESTORE.bat` (docs/07 §5's out-of-app safety net
/// for "a machine VOIDFRAME can no longer launch on") is that journal's
/// human-runnable form -- deleting it there would strip the one recovery
/// path that doesn't need this app to start. Mirrors the Tauri shell's
/// `cleanup_after_clean_revert`, which gates the same removal on
/// `revert_was_clean`.
async fn teardown(
    sys: &dyn SystemController,
    ctx: &RunContext<'_>,
    events: &mpsc::Sender<EngineEvent>,
    remove_restore_script: bool,
) {
    if let Err(e) = sys.inhibit_sleep(false).await {
        emit(
            events,
            EngineEvent::LogLine {
                text: format!("Failed to release sleep inhibition: {e}"),
            },
        )
        .await;
    }
    for name in [RESUME_TASK_NAME, DEADMAN_TASK_NAME] {
        if let Err(e) = sys.deregister_task(name).await {
            emit(
                events,
                EngineEvent::LogLine {
                    text: format!("Failed to remove scheduled task {name}: {e}"),
                },
            )
            .await;
        }
    }
    if remove_restore_script {
        crate::journal::restore_script::remove_all(&ctx.config.data_root);
    }
}

/// The name of the scenario ROLLBACK's forced in-flight revert just put
/// back, if that scenario's modules need a reboot to take effect -- the
/// registry write is reverted on disk, but the running kernel/driver still
/// has the applied value until the machine reboots, and nothing on the
/// failure path schedules one. `None` when nothing was in flight or it
/// didn't need a reboot. Same cursor rule as `rollback::in_flight_journal`.
fn in_flight_scenario_needing_reboot(progress: &RunProgress) -> Option<String> {
    if !matches!(progress.cursor.stage, Stage::Apply | Stage::Measure) {
        return None;
    }
    ordered_scenarios(&progress.project)
        .get(progress.cursor.index as usize)
        .filter(|s| s.requires_reboot())
        .map(|s| s.name.clone())
}

/// ROLLBACK-then-teardown handling shared by every way a run's own body can
/// fail: `scenario_loop`'s `Err` exit (the original call site this was
/// extracted from) and, since the fix for the M3 safety review's fifth-round
/// finding, a `begin_resume` failure too (an Abort during the post-boot
/// settle wait, or Layer 2's `abort_requested` check) -- before that fix,
/// `execute()`'s `begin_resume(...).await?` propagated straight out, skipping
/// ROLLBACK and `teardown()` entirely and leaving `RESUME_TASK_NAME`/
/// `DEADMAN_TASK_NAME` registered (so `voidframe.exe --resume` kept
/// auto-launching at every future logon) and the run-start power plan
/// unrestored. `ctx`/`progress` here are whatever the caller could build for
/// the failure at hand. On the `begin_resume` failure path (M3 safety review,
/// round 6), `progress` is a fresh `RunProgress::load` of whatever
/// `begin_resume` itself already wrote to disk before failing (e.g. the
/// `boot_count` increment near its start), not the pre-`begin_resume` clone
/// -- that clone is stale relative to disk by the time this function runs,
/// and this function's own `drain_shutdown_toggle` call can write `progress`
/// back to disk wholesale, which would otherwise clobber `begin_resume`'s
/// already-durable writes with stale values (see that call site's own
/// comment).
async fn handle_body_failure(
    sys: &dyn SystemController,
    ctx: &RunContext<'_>,
    progress: &mut RunProgress,
    shutdown_toggle: &tokio::sync::watch::Receiver<bool>,
    events: &mpsc::Sender<EngineEvent>,
    body_err: Error,
) -> Result<RunOutcome> {
    // From here on the run is cleaning up / finalizing; the top-level abort
    // race must no longer be able to drop this work.
    let _ = ctx.no_return.send(true);
    // ROLLBACK (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1): every *completed* scenario's own
    // apply/revert above already reverted its journal, success or failure --
    // so this phase's real job is narrower than its name suggests:
    // belt-and-braces restoration of the run-start active power plan, a
    // forced revert of the one scenario that was still in flight when this
    // run failed (it never reached its own `revert_stage`), and a defensive
    // sweep for any journal file this run left with unconfirmed entries.
    // Always runs, regardless of `body`'s outcome. `progress` is what tells
    // `rollback` which scenario was in flight -- on the abort path it is the
    // reload from disk `finalize_aborted_run` did, everywhere else the live
    // in-memory cursor.
    emit(
        events,
        EngineEvent::PhaseChanged {
            phase: Phase::Rollback,
        },
    )
    .await;
    let rollback_result = rollback(sys, ctx, progress, events).await;
    teardown(sys, ctx, events, rollback_result.is_ok()).await;
    if rollback_result.is_ok()
        && let Some(name) = in_flight_scenario_needing_reboot(progress)
    {
        emit(
            events,
            EngineEvent::LogLine {
                text: format!(
                    "Scenario '{name}' was reverted at ROLLBACK, but its changes need a reboot \
                     to take effect -- reboot the machine before relying on it being back to \
                     stock."
                ),
            },
        )
        .await;
    }
    // Finding 1+2 fix: `scenario_loop`'s own `Err` exit (e.g. a
    // `measure_stage` failure propagated via `?`) is not one of the drain
    // sites inside the loop -- a toggle sent moments before that failure
    // would otherwise leave `progress.shutdown_when_complete` stale for the
    // `else if` read just below. Drained here, before that read, regardless
    // of whether rollback itself succeeded.
    if let Err(e) = control::drain_shutdown_toggle(shutdown_toggle, progress, &run_dir(ctx.config))
    {
        tracing::error!(
            error = ?e,
            "failed to drain the shutdown toggle after a failed run body"
        );
    }
    if let Err(rollback_err) = &rollback_result {
        // Mirrors `run_scenario`'s own `inner_result`/`revert_report`
        // priority handling: `body`'s error is the more actionable one to
        // return (it's why this run is failing at all), but a rollback
        // failure that co-occurs with it is real and must not vanish with no
        // trace just because only one `Err` can be returned below.
        tracing::error!(
            body_error = ?body_err,
            rollback_error = ?rollback_err,
            "run body failed AND its own rollback also failed"
        );
        if body_err.is_aborted() {
            // Same rule as `run_scenario`'s identical guard: an operator
            // Abort whose own ROLLBACK also failed must not come back as
            // `is_aborted() == true` -- `spawn_run`'s `e.is_aborted()` check
            // decides whether this run's journal files get pruned, and a
            // failed ROLLBACK is exactly the case where they must survive
            // for manual rollback.
            return Err(Error::msg(format!(
                "{body_err}; this run's own rollback also failed ({rollback_err}) -- \
                 journal left in place for manual rollback"
            )));
        }
    } else if progress.shutdown_when_complete && !body_err.is_aborted() {
        // D8: the run's body failed, but ROLLBACK just confirmed the machine
        // is back to its run-start state -- same as `finish_run`'s
        // successful-run case, the machine is stock either way, so
        // auto-shutdown still fires. `progress.shutdown_when_complete` (not
        // `ctx.config`'s fixed start-time value) is the live,
        // possibly-toggled-mid-run read -- see `control::drain_shutdown_
        // toggle`. `body_err` remains this function's return value
        // regardless of whether scheduling the shutdown itself succeeds: it
        // is the reason this run failed and must not be replaced by a
        // shutdown-scheduling failure.
        //
        // `!body_err.is_aborted()` excludes an operator-requested Abort: it
        // propagates through `scenario_loop` (or `begin_resume`) as exactly
        // this kind of `Err`, and auto-shutdown exists for *unattended*
        // completion -- an explicit Abort is definitionally the opposite,
        // the operator is right there. Without this check a mid-run Abort
        // with a successful rollback would shut the machine down anyway,
        // which is the bug this guard fixes.
        if let Err(e) = sys
            .shutdown(
                60,
                &format!(
                    "VOIDFRAME: run {} failed and was rolled back, shutting down",
                    ctx.config.run_id
                ),
            )
            .await
        {
            tracing::error!(
                body_error = ?body_err,
                shutdown_error = ?e,
                "run body failed and rollback succeeded, but scheduling the \
                 auto-shutdown itself failed"
            );
        }
    }
    // A failing run leaves `progress.json` in place (crash-recovery and
    // Emergency Restore read it) -- unlike every other exit path in
    // `execute()`, this one never removes it.
    Err(body_err)
}

/// The run, raced against the operator-abort signal as a whole.
///
/// Before the 2026-09-07 instant-abort spec, Abort was only observed at the
/// individual wait points that happened to be wrapped in their own
/// per-wait abort race, so an Abort landing anywhere else -- inside the
/// 30-second thermal baseline, say -- was not acted on until that wait ran
/// to completion on its own. Racing the *whole* body against
/// [`control::wait_for_abort_allowed`] instead makes an Abort take effect
/// within one `select!` poll, wherever the run happens to be, by dropping
/// the body future outright.
///
/// Dropping the body is also why the abort arm cannot reuse anything the
/// body owned: [`finalize_aborted_run`] rebuilds what it needs from
/// persisted state instead (see its own doc comment), and the `no_return`
/// shield keeps the race off the stretches where dropping the body would be
/// unsafe (a reboot, the final cleanup, an in-flight uninterruptible
/// mutation).
pub async fn execute(
    sys: Arc<dyn SystemController>,
    capture: Arc<dyn CaptureRunner>,
    start: RunStart,
    events: mpsc::Sender<EngineEvent>,
    control: mpsc::Receiver<ControlMsg>,
    abort: tokio::sync::watch::Receiver<bool>,
    shutdown_toggle: tokio::sync::watch::Receiver<bool>,
) -> Result<RunOutcome> {
    // Everything finalize_aborted_run needs must be captured BEFORE `start`
    // moves into the body -- on the abort path the body future is dropped at
    // an arbitrary await point and none of its locals are reachable.
    let config_for_finalize = start.config().clone();
    let hwinfo_started = Arc::new(AtomicBool::new(false));
    let (no_return_tx, no_return_rx) = tokio::sync::watch::channel(false);

    let body = run_body(
        sys.clone(),
        capture,
        start,
        events.clone(),
        control,
        abort.clone(),
        shutdown_toggle.clone(),
        no_return_tx,
        hwinfo_started.clone(),
    );
    // `biased`: poll the branches in the order written, never randomized.
    // The body must always be given its chance to make forward progress
    // before the abort arm is checked, because several of the body's own
    // transitions reach a `no_return.send(true)` with zero yield points in
    // between (`Stage::Done` -> `finish_run`, and every `reboot_sequence`
    // call site). With `select!`'s default randomized poll order, an Abort
    // landing on exactly such a wake could win the race purely on poll
    // order and drop a body that was one synchronous step away from
    // shielding itself. `biased` does not weaken the race for genuinely
    // cancellable waits: the body returns `Pending` there on its own, and
    // the abort arm is then polled normally in that same wake.
    tokio::select! {
        biased;
        result = body => result,
        () = control::wait_for_abort_allowed(abort.clone(), no_return_rx, &events) => {
            finalize_aborted_run(
                sys.as_ref(),
                &config_for_finalize,
                abort,
                &shutdown_toggle,
                &events,
                &hwinfo_started,
            )
            .await
        }
    }
}

/// The run itself -- what `execute` was before the whole-body abort race was
/// wrapped around it (see [`execute`]).
#[expect(
    clippy::too_many_arguments,
    reason = "this is `execute`'s own 7-parameter public signature threaded through verbatim, \
              plus the two pieces of state the abort arm needs to survive this future being \
              dropped (the `no_return` shield and the shared hwinfo-started flag)"
)]
async fn run_body(
    sys: Arc<dyn SystemController>,
    capture: Arc<dyn CaptureRunner>,
    start: RunStart,
    events: mpsc::Sender<EngineEvent>,
    mut control: mpsc::Receiver<ControlMsg>,
    abort: tokio::sync::watch::Receiver<bool>,
    shutdown_toggle: tokio::sync::watch::Receiver<bool>,
    no_return: tokio::sync::watch::Sender<bool>,
    hwinfo_started: Arc<AtomicBool>,
) -> Result<RunOutcome> {
    let mut cooldown_checks: Vec<(String, Vec<thermal::ThermalReading>)> = Vec::new();
    let (config, mut progress) = match start {
        RunStart::Fresh(config) => {
            let progress = begin_fresh(sys.as_ref(), &config, &events, &hwinfo_started).await?;
            (config, progress)
        }
        RunStart::Resume { config, progress } => {
            // Finding fix (M3 safety review, round 5): `begin_resume` can
            // fail (an Abort during the post-boot settle wait -- reached on
            // every resume, up to `post_boot_settle` seconds -- or Layer 2's
            // `abort_requested` check) *before* handing back a valid
            // `RunProgress`, since it takes `progress` by value and only
            // returns a (possibly-mutated) one on success. Cloned here,
            // before the call, so `handle_body_failure` below still has
            // something to build a `RunContext` from on that failure path --
            // `start_build_id`/`start_launch_args`/`start_launch_args_raw`/
            // `start_power_plan` are all set once at the *original* run's
            // `begin_fresh` and never touched by `begin_resume`, so this
            // pre-call clone has them correct regardless of whether
            // `begin_resume` itself later fails.
            let progress_before = progress.clone();
            match begin_resume(
                sys.as_ref(),
                &config,
                progress,
                &events,
                &mut control,
                &shutdown_toggle,
                &abort,
                &no_return,
                &mut cooldown_checks,
                &hwinfo_started,
            )
            .await
            {
                Ok(progress) => (config, progress),
                Err(body_err) => {
                    let ctx = RunContext {
                        config: &config,
                        start_build_id: progress_before.start_build_id.clone(),
                        start_launch_args: progress_before.start_launch_args.clone(),
                        start_launch_args_raw: progress_before.start_launch_args_raw.clone(),
                        start_power_plan: progress_before.start_power_plan.clone(),
                        abort: abort.clone(),
                        no_return: no_return.clone(),
                    };
                    // Finding fix (M3 safety review, round 6): `progress_before`
                    // is stale the instant `begin_resume` writes anything to
                    // disk before failing (its own `progress.save(...)` calls,
                    // e.g. the `boot_count` increment near its start, or
                    // clearing `progress.reboot` after a bad-boot revert) --
                    // `handle_body_failure` below can call
                    // `drain_shutdown_toggle`, which writes the *entire*
                    // `progress` value back to `progress.json` if the shutdown
                    // toggle changed, which would silently overwrite
                    // `begin_resume`'s already-durable writes with the stale
                    // clone's values. Reload from disk instead, picking up
                    // whatever `begin_resume` itself last wrote; if it failed
                    // before writing anything (e.g. the Layer 2
                    // `abort_requested` check, which returns before any write),
                    // this reload just returns the same state
                    // `progress_before` already had. Falls back to
                    // `progress_before` only if the reload itself errors (e.g.
                    // the file was removed from under us) -- `progress_before`
                    // is still a valid, if possibly-stale, `RunProgress` to
                    // hand `handle_body_failure` in that unlikely case, better
                    // than failing this already-failing path a second way.
                    let mut progress = match RunProgress::load(&run_dir(&config)) {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::error!(
                                error = ?e,
                                "failed to reload progress.json after a failed resume; \
                                 falling back to the pre-resume snapshot"
                            );
                            progress_before
                        }
                    };
                    return handle_body_failure(
                        sys.as_ref(),
                        &ctx,
                        &mut progress,
                        &shutdown_toggle,
                        &events,
                        body_err,
                    )
                    .await;
                }
            }
        }
    };
    let ctx = RunContext {
        config: &config,
        start_build_id: progress.start_build_id.clone(),
        start_launch_args: progress.start_launch_args.clone(),
        start_launch_args_raw: progress.start_launch_args_raw.clone(),
        start_power_plan: progress.start_power_plan.clone(),
        abort: abort.clone(),
        no_return: no_return.clone(),
    };
    let body = scenario_loop(
        sys.as_ref(),
        capture.as_ref(),
        &ctx,
        &mut progress,
        &events,
        &mut control,
        &shutdown_toggle,
        &mut cooldown_checks,
        &hwinfo_started,
    )
    .await;
    match body {
        Ok(LoopExit::Reboot(reason)) => Ok(RunOutcome::RebootPending(reason)),
        Ok(LoopExit::AlreadyFinished(results)) => {
            // Same rule as `finish_run`/`handle_body_failure`: this arm is
            // finalizing too, and the abort race must not be able to drop it
            // between the teardown and the `progress.json` removal.
            let _ = ctx.no_return.send(true);
            teardown(sys.as_ref(), &ctx, &events, true).await;
            let _ = std::fs::remove_file(RunProgress::path(&run_dir(&config)));
            emit(
                &events,
                EngineEvent::RunComplete {
                    run_id: config.run_id.clone(),
                },
            )
            .await;
            Ok(RunOutcome::Complete(*results))
        }
        Ok(LoopExit::Done {
            final_revert_needs_reboot,
        }) => {
            finish_run(
                sys.as_ref(),
                &ctx,
                &mut progress,
                final_revert_needs_reboot,
                cooldown_checks,
                &events,
            )
            .await
        }
        Err(body_err) => {
            handle_body_failure(
                sys.as_ref(),
                &ctx,
                &mut progress,
                &shutdown_toggle,
                &events,
                body_err,
            )
            .await
        }
    }
}

/// The abort path's cleanup. The body future was just dropped at an
/// arbitrary await point, so nothing in-scope survived -- everything here
/// works from persisted state (`progress.json`, journals) plus live process
/// inspection, the same inputs crash recovery already trusts. Never raced
/// against anything: this IS the cleanup the race exists to hand off to.
async fn finalize_aborted_run(
    sys: &dyn SystemController,
    config: &RunConfig,
    abort: tokio::sync::watch::Receiver<bool>,
    shutdown_toggle: &tokio::sync::watch::Receiver<bool>,
    events: &mpsc::Sender<EngineEvent>,
    hwinfo_started: &AtomicBool,
) -> Result<RunOutcome> {
    emit(
        events,
        EngineEvent::PhaseChanged {
            phase: Phase::Aborting,
        },
    )
    .await;

    // The §2.2 "quiet window" suspends the app's own WebView2 process tree
    // around each capture (`scenario.rs`'s `run_one_iteration`), and resumes
    // it on every in-body path including the failing ones -- but a body
    // dropped mid-capture never reaches that resume, which would leave the
    // operator's own UI frozen with no way back short of Task Manager. Run
    // unconditionally and before anything else here: it is independent of
    // the CS2/HWiNFO cleanup below, and unlike them it is not safe to gate
    // on `progress.json` existing. Best-effort -- a pid that was never
    // suspended (or is already gone) is a harmless no-op.
    if let Some(pid) = config.webview_root_pid {
        let _ = sys.resume_process_tree(pid).await;
    }

    // The dropped body may have left CS2 running (PresentMon is covered by
    // its own kill_on_drop(true) -- see capture/presentmon.rs). Checked by
    // name first so an abort before any launch doesn't pay
    // graceful_kill_cs2's quit-then-poll dance for a game that isn't there.
    match sys.find_process("cs2.exe").await {
        Ok(Some(_)) => {
            if let Err(e) = steam::graceful_kill_cs2(sys).await {
                emit(
                    events,
                    EngineEvent::LogLine {
                        text: format!("Failed to close CS2 during abort cleanup: {e}"),
                    },
                )
                .await;
            }
        }
        Ok(None) => {}
        Err(e) => {
            emit(
                events,
                EngineEvent::LogLine {
                    text: format!("Could not check for a running CS2 during abort cleanup: {e}"),
                },
            )
            .await;
        }
    }
    // Only closed when the dropped body had started it itself -- the same
    // "only touch what we started" rule the thermal code follows. A detached
    // spawn_blocking readiness poll stranded by the drop fails out once the
    // process is gone.
    if hwinfo_started.load(Ordering::SeqCst)
        && let Err(e) = sys.close_hwinfo().await
    {
        emit(
            events,
            EngineEvent::LogLine {
                text: format!("Failed to close HWiNFO during abort cleanup: {e}"),
            },
        )
        .await;
    }

    let abort_err = Error::aborted("operator requested Abort".into());
    match RunProgress::load(&run_dir(config)) {
        Ok(mut progress) => {
            // Layer 2's durability guard (M3 safety review, round 3),
            // applied from disk: the body was dropped before it could reach
            // whichever of `scenario_loop`'s/`finish_run`'s own
            // `abort_requested` writes it was heading for, so this is the
            // abort path's equivalent of them. Without it, a `--resume`
            // fired by a scheduled task that this cleanup's own `teardown`
            // hasn't deregistered yet would carry on as if the operator
            // never clicked Abort. Best-effort: a failure to persist it must
            // not stop the rollback/teardown work below, which is what
            // actually puts the machine back.
            progress.abort_requested = true;
            if let Err(e) = progress.save(&run_dir(config)) {
                tracing::error!(error = ?e, "failed to persist abort_requested during abort cleanup");
            }
            // From here the existing failure machinery does the real work:
            // Rollback phase event, run-start power-plan and launch-options
            // restore, the defensive journal sweep, teardown, shutdown-toggle
            // drain. no_return is a fresh already-true channel: this cleanup
            // itself must never be raced, and handle_body_failure's own send
            // is then a no-op.
            //
            // Scope of that journal work, precisely, in two parts.
            //
            // 1. The scenario that was in flight when the Abort landed --
            //    `progress.cursor` at `Stage::Apply` or `Stage::Measure`,
            //    i.e. strictly before its own `revert_stage` was ever
            //    invoked -- is force-reverted UNCONDITIONALLY by
            //    `rollback`'s `force_revert_in_flight_scenario`, whether or
            //    not any of its records is unconfirmed. This is what keeps a
            //    cleanly-and-fully-applied scenario's registry/powercfg/
            //    power-plan mutations from staying live on the machine after
            //    an Abort. It is not abort-specific: `handle_body_failure`
            //    is equally `scenario_loop`'s own `Err` exit's handler, so
            //    an ordinary `measure_stage` failure (which `?`s straight
            //    past `revert_stage` too) gets the same treatment. It is
            //    exactly ONE scenario -- the one the cursor names -- not
            //    "every scenario except the current index"; the earlier
            //    indices each finished their own apply/measure/revert cycle
            //    before the loop moved on. `scenario_loop`'s
            //    `Transition::ApplyNextThenReboot` arm is what keeps that
            //    true in the one place it applies a scenario the cursor
            //    doesn't yet name: it persists the cursor as
            //    `{i + 1, Stage::Apply}` before that apply starts, so the
            //    reload below sees it even when the body is dropped
            //    mid-apply (C1, 2026-09-07 follow-up review; widened
            //    2026-09-10).
            // 2. `sweep_journals` then still reverts any OTHER journal with
            //    records marked *unconfirmed* (`applied: false`) -- an apply
            //    or revert interrupted between its journal record and its
            //    `mark_applied`, which is the window a dropped
            //    `apply_module` leaves behind.
            //
            // Still deliberately NOT covered: an interruption landing during
            // `revert_stage` itself (`Stage::Revert`). A revert does not
            // clear a record's `applied` flag, so a partially-reverted
            // journal is indistinguishable from an untouched one, and a
            // second reverse replay would re-attempt already-successful
            // reverts -- `Op::PowerPlanCreate`'s revert calls
            // `delete_power_plan`, which has nothing left to delete on a
            // second pass (`Op::PowerPlanDelete`'s own revert is an
            // unconditional `Ok(())`). Such a failure is collected into
            // `RevertReport::verify_failures` and logged rather than
            // hard-failing ROLLBACK, so the cost is an unreliable
            // "was it restored?" signal, not a broken rollback -- the
            // scoping decision rests on the ambiguity itself. See
            // `in_flight_journal`'s own doc comment. An operator Abort can
            // no longer open that window at all (`revert_stage` shields its
            // entire body, including the progress LogLine before the replay
            // -- I1, 2026-09-07 follow-up review, widened 2026-09-08); what
            // remains is crash-equivalent, and the journals survive
            // `prune_run_dir`, so Emergency Restore can still repair the
            // machine.
            let ctx = RunContext {
                config,
                start_build_id: progress.start_build_id.clone(),
                start_launch_args: progress.start_launch_args.clone(),
                start_launch_args_raw: progress.start_launch_args_raw.clone(),
                start_power_plan: progress.start_power_plan.clone(),
                abort,
                no_return: tokio::sync::watch::channel(true).0,
            };
            handle_body_failure(sys, &ctx, &mut progress, shutdown_toggle, events, abort_err).await
        }
        Err(e) => {
            // Usually: aborted before begin_fresh ever saved progress.json
            // (preflight/snapshot/thermal baseline) -- nothing journaled,
            // nothing applied, no scheduled tasks registered. But a
            // `progress.json` that exists and cannot be parsed lands here
            // too, and for that run everything above may well be live: say
            // so loudly, and still take the two cleanups that need no
            // progress to be correct (releasing sleep inhibition, and
            // deregistering the recovery tasks -- a no-op when none were
            // registered). The journals stay for Emergency Restore.
            tracing::warn!(
                error = ?e,
                "no usable progress.json during abort cleanup -- skipping rollback; if the \
                 file exists but is corrupt, use Emergency Restore"
            );
            let _ = sys.inhibit_sleep(false).await;
            for name in [RESUME_TASK_NAME, DEADMAN_TASK_NAME] {
                let _ = sys.deregister_task(name).await;
            }
            Err(abort_err)
        }
    }
}

/// PREFLIGHT -> SNAPSHOT -> THERMAL BASELINE, then builds and saves this
/// run's first [`RunProgress`] (spec §3.2's "Fresh" path).
async fn begin_fresh(
    sys: &dyn SystemController,
    config: &RunConfig,
    events: &mpsc::Sender<EngineEvent>,
    hwinfo_started: &AtomicBool,
) -> Result<RunProgress> {
    // PREFLIGHT
    emit(
        events,
        EngineEvent::PhaseChanged {
            phase: Phase::Preflight,
        },
    )
    .await;
    // The capture window is a run-readiness fact, not a load-time one (see
    // `Settings::validate_capture_window`): checked here so every front end
    // -- the CLI's hand-written project JSON included, which never goes
    // through the UI's `validate_scenario` -- refuses an AveYo capture
    // longer than its benchmark before anything is applied.
    config
        .project
        .settings
        .validate_capture_window()
        .map_err(|e| Error::preflight(e.to_string()))?;
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
        emit(
            events,
            EngineEvent::LogLine {
                text: "Steam is not running -- attempting to start it automatically...".into(),
            },
        )
        .await;
    }
    if let Err(e) = ensure_steam_running(sys).await {
        tracing::warn!("pre-flight auto-launch of Steam failed: {e}");
        emit(
            events,
            EngineEvent::LogLine {
                text: format!("Pre-flight auto-launch of Steam failed: {e}"),
            },
        )
        .await;
    }
    let facts: PreflightFacts = preflight::gather_facts(
        sys,
        None,
        2_000_000_000,
        &config.project.scenarios,
        config.project.settings.benchmark_kind,
    )
    .await?;
    let report = preflight::evaluate(&facts);
    if report.is_blocked() {
        let reason = report
            .blocking()
            .iter()
            .map(|c| c.detail.clone())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(Error::preflight(reason));
    }

    // SNAPSHOT (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1: read + store every value the enabled scenarios'
    // modules will touch, so the Desktop .bat (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §6.3) and the run's own
    // audit trail have a true before-state).
    emit(
        events,
        EngineEvent::PhaseChanged {
            phase: Phase::Snapshot,
        },
    )
    .await;
    let raw_launch_args = sys.read_cs2_launch_options().await?;
    emit(
        events,
        EngineEvent::LogLine {
            text: format!(
                "Current CS2 launch options: \"{}\"",
                crate::cs2::keybind_cfg::redact(&raw_launch_args)
            ),
        },
    )
    .await;
    let start_build_id = sys.app_manifest(730).await?.and_then(|m| m.build_id);
    let start_launch_args = crate::cs2::keybind_cfg::strip_reserved(&raw_launch_args);
    let start_launch_args_raw = raw_launch_args;
    let start_power_plan = sys.active_power_plan().await?;

    // THERMAL BASELINE (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2)
    let thermal_baseline = collect_thermal_baseline(sys, config, events, hwinfo_started).await;

    let progress = RunProgress {
        schema_version: crate::model::SCHEMA_VERSION.to_string(),
        run_id: config.run_id.clone(),
        project: config.project.clone(),
        start_build_id,
        start_launch_args,
        start_launch_args_raw,
        start_power_plan,
        thermal_baseline,
        completed: vec![],
        unstable: vec![],
        cursor: Cursor {
            index: 0,
            stage: Stage::Apply,
        },
        reboot: None,
        shutdown_when_complete: config.shutdown_when_complete,
        skip_revert_once: false,
        abort_requested: false,
    };
    progress.save(&run_dir(config))?;
    let real_video_txt_path = resolve_real_video_txt_path(sys).await;
    if let Err(e) = crate::journal::restore_script::write_all(
        &run_dir(config),
        &config.data_root,
        &config.run_id,
        &config
            .data_root
            .join("projects")
            .join(&config.project.id)
            .join("scripts"),
        real_video_txt_path.as_deref(),
    ) {
        emit(
            events,
            EngineEvent::LogLine {
                text: format!("Could not write VOIDFRAME_RESTORE.bat: {e}"),
            },
        )
        .await;
    }
    if let Err(e) = sys.inhibit_sleep(true).await {
        emit(
            events,
            EngineEvent::LogLine {
                text: format!("Could not inhibit sleep: {e}"),
            },
        )
        .await;
    }
    Ok(progress)
}

/// Only attempted when this run has an `hwinfo_path` configured at all, and
/// only when HWiNFO isn't already running externally -- VOIDFRAME only ever
/// drives HWiNFO's own lifecycle when it's the one that started it (the
/// same "only touch what we started" rule `close_steam_window` follows), so
/// an already-running instance means thermal mode falls back to the plain
/// fixed-time `maybe_break` for the whole run instead. Used by
/// [`begin_fresh`] for the run's one pre-run baseline -- `begin_resume`
/// deliberately never calls this: re-sampling here would record whatever
/// the machine happens to read right after a reboot (still hot from POST/
/// Windows startup) as if it were the run's true idle baseline, corrupting
/// every later cooldown comparison. `begin_resume` waits out a
/// [`maybe_break`] cooldown against the *existing* baseline instead.
async fn collect_thermal_baseline(
    sys: &dyn SystemController,
    config: &RunConfig,
    events: &mpsc::Sender<EngineEvent>,
    hwinfo_started: &AtomicBool,
) -> Option<thermal::ThermalReading> {
    let hwinfo_path = config.hwinfo_path.as_ref()?;
    let (sample_interval, sample_count) = config.thermal_sample_override.unwrap_or((
        thermal::THERMAL_SAMPLE_INTERVAL,
        thermal::THERMAL_SAMPLE_COUNT,
    ));
    match sys.hwinfo_already_running().await {
        Ok(true) => {
            emit(
                events,
                EngineEvent::LogLine {
                    text: "HWiNFO is already running -- thermal cooldown will use the plain \
                           fixed-time break instead (VOIDFRAME only drives HWiNFO's lifecycle \
                           when it's the one that started it)."
                        .into(),
                },
            )
            .await;
            None
        }
        Ok(false) => {
            emit(
                events,
                EngineEvent::PhaseChanged {
                    phase: Phase::ThermalBaseline,
                },
            )
            .await;
            match thermal::collect_thermal_sample(
                sys,
                events,
                hwinfo_path,
                sample_interval,
                sample_count,
                hwinfo_started,
            )
            .await
            {
                Ok(reading) => Some(reading),
                Err(e) => {
                    emit(
                        events,
                        EngineEvent::LogLine {
                            text: format!(
                                "Thermal baseline collection failed ({e}) -- falling back to \
                                 the plain fixed-time break."
                            ),
                        },
                    )
                    .await;
                    None
                }
            }
        }
        Err(e) => {
            emit(
                events,
                EngineEvent::LogLine {
                    text: format!(
                        "Could not check whether HWiNFO is already running ({e}) -- falling \
                         back to the plain fixed-time break."
                    ),
                },
            )
            .await;
            None
        }
    }
}

/// Spec §3.2's "Resume" path: settles, classifies the boot that got us
/// here, and -- if it wasn't clean and the cursor was mid-measurement --
/// reverts the scenario that was applied when this process died and marks
/// it unstable, before handing back to [`scenario_loop`].
#[expect(
    clippy::too_many_arguments,
    reason = "each parameter is a distinct piece of state this resume path genuinely needs \
              (system, config, live progress, the event/control/shutdown-toggle channels, the \
              abort/no-return signals, the cooldown-check accumulator, and the shared \
              hwinfo-started flag); bundling them into a struct here would just move the same \
              fields one layer over for no clarity gain"
)]
async fn begin_resume(
    sys: &dyn SystemController,
    config: &RunConfig,
    mut progress: RunProgress,
    events: &mpsc::Sender<EngineEvent>,
    control: &mut mpsc::Receiver<ControlMsg>,
    shutdown_toggle: &tokio::sync::watch::Receiver<bool>,
    abort: &tokio::sync::watch::Receiver<bool>,
    no_return: &tokio::sync::watch::Sender<bool>,
    cooldown_checks: &mut Vec<(String, Vec<thermal::ThermalReading>)>,
    hwinfo_started: &AtomicBool,
) -> Result<RunProgress> {
    emit(
        events,
        EngineEvent::PhaseChanged {
            phase: Phase::BootResume,
        },
    )
    .await;
    if let Err(e) = sys.inhibit_sleep(true).await {
        emit(
            events,
            EngineEvent::LogLine {
                text: format!("Could not inhibit sleep: {e}"),
            },
        )
        .await;
    }

    // Layer 2 (M3 safety review, round 3): durability guard for
    // `progress.abort_requested`, set by `scenario_loop`'s own Layer-1 check
    // immediately before it would otherwise call `reboot_sequence`. In
    // practice Layer 1 already stops the reboot from firing at all, so this
    // should essentially never actually read `true` here -- but if some
    // future reboot-triggering path were ever added without Layer 1's own
    // check, this is the belt-and-braces guarantee that a resumed run still
    // refuses to continue rather than running the rest of the benchmark (and
    // its own end-of-run shutdown decision) as if the operator never clicked
    // Abort. The settle wait and boot classification below are both
    // pointless for a run that's about to be treated as aborted, so this
    // returns before either.
    if progress.abort_requested {
        return Err(Error::aborted(
            "operator requested Abort before the reboot that preceded this resume".into(),
        ));
    }

    let pending = progress.reboot.clone().ok_or_else(|| {
        Error::msg("resume requested but progress.json records no pending reboot".into())
    })?;
    // Recorded now, before the settle wait, so a second reboot during that
    // wait (spec §4's UnexpectedExtraReboot) is visible to whichever resume
    // observes it next -- `classify_boot` below still uses the
    // pre-increment `pending.boot_count`, exactly as its own doc comment
    // requires ("as read from disk *before* this resume increments it").
    progress.reboot = Some(PendingReboot {
        boot_count: pending.boot_count + 1,
        ..pending.clone()
    });
    progress.save(&run_dir(config))?;

    // The final resume of a run exists purely to confirm the last revert
    // took effect -- no CS2 launch or measurement is left to do, so there is
    // nothing left that post-boot flakiness could disrupt. `finish_run`'s
    // call site is the only place that constructs a `next_cursor` with
    // `stage: Stage::Done` before a reboot, so by the time this resume loads
    // that `RunProgress` back, `Stage::Done` precisely identifies that case.
    if progress.cursor.stage == Stage::Done {
        emit(
            events,
            EngineEvent::LogLine {
                text: "Final revert-only resume -- skipping the post-boot settle wait, nothing \
                       left to measure."
                    .into(),
            },
        )
        .await;
    } else {
        settle_wait(config.post_boot_settle, events).await?;
    }

    let since = crate::model::results::parse_utc_timestamp(&pending.initiated_at)
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let report = match sys.boot_report(since).await {
        Ok(r) => r,
        Err(e) => {
            emit(
                events,
                EngineEvent::LogLine {
                    text: format!("Boot report unavailable ({e}); assuming a clean boot"),
                },
            )
            .await;
            BootReport::default()
        }
    };
    let class = boot::classify_boot(&report, pending.boot_count);
    emit(
        events,
        EngineEvent::LogLine {
            text: format!(
                "Boot classified as {class:?}: {}",
                boot::boot_class_reason(class)
            ),
        },
    )
    .await;

    if class != boot::BootClass::Clean && progress.cursor.stage == Stage::Measure {
        let scenarios = ordered_scenarios(&progress.project);
        let index = progress.cursor.index as usize;
        let scenario = scenarios.get(index).ok_or_else(|| {
            Error::msg(format!(
                "progress cursor index {index} out of range for {} scenarios",
                scenarios.len()
            ))
        })?;
        let revert_ctx = RunContext {
            config,
            start_build_id: progress.start_build_id.clone(),
            start_launch_args: progress.start_launch_args.clone(),
            start_launch_args_raw: progress.start_launch_args_raw.clone(),
            start_power_plan: progress.start_power_plan.clone(),
            abort: abort.clone(),
            no_return: no_return.clone(),
        };
        if let Err(e) = revert_stage(sys, scenario, &revert_ctx, events).await {
            emit(
                events,
                EngineEvent::LogLine {
                    text: format!(
                        "Revert of unstable scenario '{}' failed: {e}",
                        scenario.name
                    ),
                },
            )
            .await;
        }
        progress.unstable.push(UnstableScenario {
            scenario_id: scenario.id.clone(),
            reason: boot::boot_class_reason(class).to_string(),
        });
        progress.cursor.stage = Stage::Revert;
        progress.skip_revert_once = true;
    }
    // Only now, with the settle wait and boot classification behind us: the
    // deadman (spec §5.2) is the safety net for exactly the window this
    // resume has just crossed -- a machine that reboots again, or a process
    // killed, during the settle wait must still get its stranded scenario
    // reverted at the next boot. Deregistering it at the top of this
    // function, before the settle wait, gave that window no cover at all.
    // The deadman's own heartbeat check keeps it from acting while this
    // process is alive.
    if let Err(e) = sys.deregister_task(DEADMAN_TASK_NAME).await {
        emit(
            events,
            EngineEvent::LogLine {
                text: format!("Failed to remove the deadman task: {e}"),
            },
        )
        .await;
    }
    progress.reboot = None;
    progress.save(&run_dir(config))?;

    // A reboot heats the machine up again (POST, driver init, Windows
    // startup) same as any other activity a cooldown break waits out
    // between scenarios -- so a resume that still has measuring left to do
    // and thermal mode on waits for the *existing* baseline to be reached
    // again via the same `maybe_break` cooldown helper every inter-scenario
    // break uses, rather than treating the reboot as license to establish a
    // brand new baseline (see `collect_thermal_baseline`'s own doc comment
    // for why that would corrupt the run). Gated on `hwinfo_path` too, not
    // just `thermal_baseline` -- `maybe_break` itself runs its own
    // abort/pause checkpoint before it even looks at either, so a resume
    // with thermal mode off must not call it at all (every non-thermal
    // resume up to now has had no checkpoint here, and this is not the
    // place to add one). The final revert-only resume (`Stage::Done`, see
    // the settle-wait skip above) has no measurement left to protect and
    // nothing following it to cool down for, so it skips this entirely and
    // heads straight to `scenario_loop`'s `Stage::Done` handling (either
    // the already-finished short-circuit or `finish_run`).
    if progress.cursor.stage != Stage::Done
        && progress.thermal_baseline.is_some()
        && config.hwinfo_path.is_some()
    {
        let (sample_interval, sample_count) = config.thermal_sample_override.unwrap_or((
            thermal::THERMAL_SAMPLE_INTERVAL,
            thermal::THERMAL_SAMPLE_COUNT,
        ));
        let checks = maybe_break(
            sys,
            events,
            control,
            shutdown_toggle,
            &mut progress,
            &run_dir(config),
            abort.clone(),
            0,
            config.hwinfo_path.as_deref(),
            sample_interval,
            sample_count,
            hwinfo_started,
        )
        .await?;
        if !checks.is_empty() {
            cooldown_checks.push(("post_reboot".into(), checks));
        }
    }

    Ok(progress)
}

/// Sleeps `total`, in <=30s slices, logging the remaining time before each
/// -- an operator watching the live event stream after `--resume` sees
/// this settle wait counting down rather than a long silence (spec D5).
async fn settle_wait(total: Duration, events: &mpsc::Sender<EngineEvent>) -> Result<()> {
    let mut remaining = total;
    while !remaining.is_zero() {
        emit(
            events,
            EngineEvent::LogLine {
                text: format!("Post-boot settle: {}s remaining", remaining.as_secs()),
            },
        )
        .await;
        let slice = remaining.min(Duration::from_secs(30));
        tokio::time::sleep(slice).await;
        remaining -= slice;
    }
    Ok(())
}

/// `[baseline dummy] ++ enabled scenarios`, the loop's fixed index space --
/// `progress.cursor.index` is an index into exactly this list, on both the
/// Fresh and Resume paths.
pub(crate) fn ordered_scenarios(project: &Project) -> Vec<Scenario> {
    let mut v = vec![Scenario {
        id: "baseline".into(),
        name: project.baseline.name.clone(),
        description: project.baseline.description.clone(),
        enabled: true,
        modules: vec![],
    }];
    v.extend(project.enabled_scenarios().cloned());
    v
}

/// The `PhaseChanged` a scenario's own `Apply` stage announces itself with
/// -- `Baseline` for index 0 (the dummy baseline `ordered_scenarios`
/// prepends), `Scenario { id }` for every real one.
fn scenario_phase(index: usize, scenario: &Scenario) -> Phase {
    if index == 0 {
        Phase::Baseline
    } else {
        Phase::Scenario {
            id: scenario.id.clone(),
        }
    }
}

/// The cursor-driven scenario loop (spec §3.2-§3.4): walks
/// `[baseline] ++ enabled scenarios` from `progress.cursor`, applying,
/// measuring, and reverting each in turn, and stops -- without finishing
/// the run itself -- the instant a reboot is needed (spec D2's rule,
/// [`plan_transition`]).
#[expect(
    clippy::too_many_arguments,
    reason = "each parameter is a distinct piece of state the loop genuinely needs (system, \
              capture, run context, live progress, the event/control/shutdown-toggle channels, \
              and the cooldown-check accumulator); bundling them into a struct here would just \
              move the same fields one layer over for no clarity gain"
)]
async fn scenario_loop(
    sys: &dyn SystemController,
    capture: &dyn CaptureRunner,
    ctx: &RunContext<'_>,
    progress: &mut RunProgress,
    events: &mpsc::Sender<EngineEvent>,
    control: &mut mpsc::Receiver<ControlMsg>,
    shutdown_toggle: &tokio::sync::watch::Receiver<bool>,
    cooldown_checks: &mut Vec<(String, Vec<thermal::ThermalReading>)>,
    hwinfo_started: &AtomicBool,
) -> Result<LoopExit> {
    let scenarios = ordered_scenarios(&progress.project);
    let run_dir_path = run_dir(ctx.config);
    // A dry run logs every mutation but applies none -- and a reboot is the
    // one step nothing here could follow up on: `DryRunController`'s
    // `reboot`/`register_task` are logging no-ops that return `Ok`, so a dry
    // run that took a reboot path would return `RebootPending` and leave the
    // Tauri shell's `active_run` parked forever waiting for an OS reboot
    // that never comes (and a later `--resume` would continue it as a REAL
    // run). Reboot scenarios are therefore walked in-process like plain
    // ones under dry run; each skipped reboot is announced with a LogLine.
    let dry_run = ctx.config.dry_run;
    let needs_reboot = |s: &Scenario| s.requires_reboot() && !dry_run;

    loop {
        match progress.cursor.stage {
            Stage::Done => {
                // The run is genuinely over: whichever way this arm exits
                // (`AlreadyFinished`'s teardown, or `finish_run`'s
                // scoring/ROLLBACK/REPORT) is finalization, and an Abort is
                // meaningless from here. Engaged as this arm's very first
                // action, the same rule `reboot_sequence`/`finish_run`/
                // `handle_body_failure`/the `AlreadyFinished` arm all
                // follow.
                //
                // Defense-in-depth, not the mechanism that closes the race:
                // `biased;` on `execute()`'s top-level `select!` is what
                // actually does that. There is no yield point between this
                // arm being taken and `finish_run`'s own shield engagement
                // on its first line, so the body always runs that stretch
                // to completion within a single poll and the abort arm
                // never gets a chance to drop it there. This send is what
                // keeps that true if a future refactor ever DOES introduce
                // a yield point in between.
                let _ = ctx.no_return.send(true);
                let results_path = run_dir_path.join("results.json");
                if results_path.exists() {
                    let results = RunResults::load(&results_path)?;
                    return Ok(LoopExit::AlreadyFinished(Box::new(results)));
                }
                // Finding 1+2 fix: the loop is genuinely done here -- no
                // further inter-scenario checkpoint follows, so a toggle
                // sent during the last scenario's own measurement must be
                // drained before this exits, or `finish_run`'s D3/D8 read
                // of `progress.shutdown_when_complete` would silently miss
                // it.
                control::drain_shutdown_toggle(shutdown_toggle, progress, &run_dir_path)?;
                return Ok(LoopExit::Done {
                    final_revert_needs_reboot: false,
                });
            }
            Stage::Apply => {
                let index = progress.cursor.index as usize;
                let scenario = scenarios.get(index).ok_or_else(|| {
                    Error::msg(format!(
                        "progress cursor index {index} out of range for {} scenarios",
                        scenarios.len()
                    ))
                })?;
                emit(
                    events,
                    EngineEvent::PhaseChanged {
                        phase: scenario_phase(index, scenario),
                    },
                )
                .await;
                apply_stage(sys, scenario, ctx, events).await?;
                if dry_run && scenario.requires_reboot() {
                    emit(
                        events,
                        EngineEvent::LogLine {
                            text: format!(
                                "Dry run: scenario '{}' would reboot the machine here -- \
                                 continuing without a reboot",
                                scenario.name
                            ),
                        },
                    )
                    .await;
                }
                if needs_reboot(scenario) {
                    let scenario_id = scenario.id.clone();
                    let next_cursor = Cursor {
                        index: progress.cursor.index,
                        stage: Stage::Measure,
                    };
                    control::pre_reboot_checkpoint(
                        shutdown_toggle,
                        &ctx.abort,
                        progress,
                        &run_dir_path,
                    )?;
                    reboot_sequence(
                        sys,
                        ctx.config,
                        progress,
                        RebootReason::ApplyNext,
                        next_cursor,
                        scenario_id,
                        events,
                        &ctx.no_return,
                    )
                    .await?;
                    return Ok(LoopExit::Reboot(RebootReason::ApplyNext));
                }
                progress.cursor.stage = Stage::Measure;
            }
            Stage::Measure => {
                let index = progress.cursor.index as usize;
                let scenario = scenarios.get(index).ok_or_else(|| {
                    Error::msg(format!(
                        "progress cursor index {index} out of range for {} scenarios",
                        scenarios.len()
                    ))
                })?;
                let is_baseline = index == 0;
                let per_iteration =
                    measure_stage(sys, capture, scenario, ctx, events, control).await?;
                let result = scenario_result(scenario, is_baseline, per_iteration)?;
                emit(
                    events,
                    EngineEvent::ScenarioComplete {
                        result: result.clone(),
                    },
                )
                .await;
                progress.completed.push(result);
                progress.cursor.stage = Stage::Revert;
                progress.save(&run_dir_path)?;
            }
            Stage::Revert => {
                let index = progress.cursor.index as usize;
                let scenario = scenarios.get(index).ok_or_else(|| {
                    Error::msg(format!(
                        "progress cursor index {index} out of range for {} scenarios",
                        scenarios.len()
                    ))
                })?;
                if progress.skip_revert_once {
                    progress.skip_revert_once = false;
                } else {
                    revert_stage(sys, scenario, ctx, events).await?;
                    // Spec section 6 / D6: a custom_script revert that
                    // reached this point ran, but its effect was never
                    // verified -- patch the already-recorded result (it
                    // was built at Stage::Measure, before this revert ran
                    // at all) rather than guessing at construction time.
                    if scenario
                        .modules
                        .iter()
                        .any(|m| matches!(m, Module::CustomScript(_)))
                        && let Some(result) = progress
                            .completed
                            .iter_mut()
                            .find(|r| r.scenario_id == scenario.id)
                    {
                        result.script_reverted_unverified = true;
                    }
                }
                let next = scenarios.get(index + 1);
                match plan_transition(needs_reboot(scenario), next.map(needs_reboot)) {
                    Transition::None => {
                        if next.is_some() {
                            let (sample_interval, sample_count) =
                                ctx.config.thermal_sample_override.unwrap_or((
                                    thermal::THERMAL_SAMPLE_INTERVAL,
                                    thermal::THERMAL_SAMPLE_COUNT,
                                ));
                            let checks = maybe_break(
                                sys,
                                events,
                                control,
                                shutdown_toggle,
                                progress,
                                &run_dir_path,
                                ctx.abort.clone(),
                                ctx.config.inter_scenario_break_seconds,
                                ctx.config.hwinfo_path.as_deref(),
                                sample_interval,
                                sample_count,
                                hwinfo_started,
                            )
                            .await?;
                            if !checks.is_empty() {
                                cooldown_checks.push((scenario.id.clone(), checks));
                            }
                        }
                        progress.cursor = match next {
                            Some(_) => Cursor {
                                index: progress.cursor.index + 1,
                                stage: Stage::Apply,
                            },
                            None => Cursor {
                                index: progress.cursor.index,
                                stage: Stage::Done,
                            },
                        };
                        progress.save(&run_dir_path)?;
                    }
                    Transition::ApplyNextThenReboot => {
                        // `next` is guaranteed `Some` -- `plan_transition`
                        // only returns this variant when `next_needs_reboot`
                        // is `Some(true)`.
                        let next_scenario =
                            next.expect("ApplyNextThenReboot implies a next scenario");
                        // Whether this one reboot is *also* what makes the
                        // just-reverted scenario's own revert take effect
                        // (spec D2): only when that scenario itself needed
                        // a reboot to apply -- a scenario that never needed
                        // one (e.g. baseline) has nothing here for the
                        // reboot to be "reverting". Decided before the
                        // cursor moves off `scenario` below.
                        let reason = if needs_reboot(scenario) {
                            RebootReason::RevertAndApplyNext
                        } else {
                            RebootReason::ApplyNext
                        };
                        // C1 (2026-09-07 follow-up review, widened
                        // 2026-09-10): this is the one arm in this loop that
                        // applies a scenario OTHER than the one
                        // `progress.cursor.index` names, so the cursor has
                        // to name `next_scenario` -- on disk, not just in
                        // memory -- BEFORE its apply starts, exactly as the
                        // `Stage::Apply` arm's own cursor is already saved
                        // as `{i, Apply}` before it calls `apply_stage`.
                        // `rollback`'s `in_flight_journal` reads this cursor
                        // to decide which scenario's journal to
                        // force-revert, and a `Stage::Revert` cursor is
                        // deliberately never treated as in flight. The
                        // original C1 fix advanced the in-memory cursor only
                        // after `apply_stage` returned, which still left the
                        // abort race (`finalize_aborted_run` reloads the
                        // cursor from disk) able to drop the body mid-apply
                        // with `{i, Revert}` on disk: a module already
                        // confirmed by then was invisible to both the forced
                        // revert and `sweep_journals`, and stayed live on the
                        // machine after an "aborted, rolled back" run.
                        progress.cursor = Cursor {
                            index: progress.cursor.index + 1,
                            stage: Stage::Apply,
                        };
                        progress.save(&run_dir_path)?;
                        apply_stage(sys, next_scenario, ctx, events).await?;
                        let scenario_id = next_scenario.id.clone();
                        let next_cursor = Cursor {
                            index: progress.cursor.index,
                            stage: Stage::Measure,
                        };
                        // Confirmed applied: catch the in-memory cursor up
                        // before the abort check below, so an Abort observed
                        // there still force-reverts `next_scenario` at
                        // ROLLBACK. A no-op on the non-abort path:
                        // `reboot_sequence` assigns this same `next_cursor`
                        // (its own first act after engaging the shield) and
                        // reads nothing off `progress.cursor` before doing
                        // so, so the value that reaches disk is identical
                        // either way.
                        progress.cursor = next_cursor.clone();
                        control::pre_reboot_checkpoint(
                            shutdown_toggle,
                            &ctx.abort,
                            progress,
                            &run_dir_path,
                        )?;
                        reboot_sequence(
                            sys,
                            ctx.config,
                            progress,
                            reason,
                            next_cursor,
                            scenario_id,
                            events,
                            &ctx.no_return,
                        )
                        .await?;
                        return Ok(LoopExit::Reboot(reason));
                    }
                    Transition::RebootThenContinue => {
                        if next.is_none() {
                            // The very last scenario's own revert needs a
                            // reboot to take effect, and nothing follows it
                            // -- whether that reboot happens now or is
                            // replaced by a shutdown is `finish_run`'s call,
                            // made after scoring/ROLLBACK/REPORT. Finding
                            // 1+2 fix -- see the identical comment on the
                            // `Stage::Done` exit above.
                            control::drain_shutdown_toggle(
                                shutdown_toggle,
                                progress,
                                &run_dir_path,
                            )?;
                            return Ok(LoopExit::Done {
                                final_revert_needs_reboot: true,
                            });
                        }
                        let scenario_id = scenario.id.clone();
                        let next_cursor = Cursor {
                            index: progress.cursor.index + 1,
                            stage: Stage::Apply,
                        };
                        control::pre_reboot_checkpoint(
                            shutdown_toggle,
                            &ctx.abort,
                            progress,
                            &run_dir_path,
                        )?;
                        reboot_sequence(
                            sys,
                            ctx.config,
                            progress,
                            RebootReason::RevertOnly,
                            next_cursor,
                            scenario_id,
                            events,
                            &ctx.no_return,
                        )
                        .await?;
                        return Ok(LoopExit::Reboot(RebootReason::RevertOnly));
                    }
                }
            }
        }
    }
}

/// Persists the pending-reboot cursor, writes the recovery `.bat`,
/// registers both scheduled tasks, and calls `sys.reboot()` (spec §5.1).
/// The process is expected to die shortly after this returns.
#[expect(
    clippy::too_many_arguments,
    reason = "the pre-existing 7 parameters are each a distinct piece of state this sequence \
              genuinely needs; `no_return` is the 8th only because the abort race has to be \
              shut off before any of them are touched"
)]
async fn reboot_sequence(
    sys: &dyn SystemController,
    config: &RunConfig,
    progress: &mut RunProgress,
    reason: RebootReason,
    next_cursor: Cursor,
    scenario_id: String,
    events: &mpsc::Sender<EngineEvent>,
    no_return: &tokio::sync::watch::Sender<bool>,
) -> Result<()> {
    // From here on the run is committing to a reboot; the top-level abort
    // race must no longer be able to drop this work half-done (scheduled
    // tasks registered but `progress.json` not written, or vice versa).
    let _ = no_return.send(true);
    progress.cursor = next_cursor;
    progress.reboot = Some(PendingReboot {
        reason,
        scenario_id,
        initiated_at: crate::model::results::utc_timestamp_now(),
        boot_count: 0,
    });
    progress.save(&run_dir(config))?;
    let real_video_txt_path = resolve_real_video_txt_path(sys).await;
    if let Err(e) = crate::journal::restore_script::write_all(
        &run_dir(config),
        &config.data_root,
        &config.run_id,
        &config
            .data_root
            .join("projects")
            .join(&config.project.id)
            .join("scripts"),
        real_video_txt_path.as_deref(),
    ) {
        emit(
            events,
            EngineEvent::LogLine {
                text: format!("Could not write VOIDFRAME_RESTORE.bat: {e}"),
            },
        )
        .await;
    }
    sys.register_task(&TaskSpec {
        name: RESUME_TASK_NAME.to_string(),
        description: format!("VOIDFRAME run {}", config.run_id),
        trigger: TaskTrigger::AtLogonOfCurrentUser,
        principal: TaskPrincipal::CurrentUserHighest,
        exe: config.exe_path.clone(),
        args: "--resume".to_string(),
    })
    .await?;
    sys.register_task(&TaskSpec {
        name: DEADMAN_TASK_NAME.to_string(),
        description: format!("VOIDFRAME run {}", config.run_id),
        trigger: TaskTrigger::AtBootDelayed { minutes: 10 },
        principal: TaskPrincipal::LocalSystem,
        exe: config.exe_path.clone(),
        args: format!("--recover --data-root \"{}\"", config.data_root.display()),
    })
    .await?;
    emit(
        events,
        EngineEvent::PhaseChanged {
            phase: Phase::RebootPending { reason },
        },
    )
    .await;
    sys.reboot(
        10,
        &format!("VOIDFRAME: rebooting to continue run {}", config.run_id),
    )
    .await?;
    Ok(())
}

/// Scoring -> ROLLBACK -> REPORT, then the shutdown/final-reboot/complete
/// decision (spec §3.5, D3, D8). Only reached once [`scenario_loop`] itself
/// is genuinely done with every scenario -- a mid-run failure takes
/// `execute()`'s own error-path branch instead, never this function.
async fn finish_run(
    sys: &dyn SystemController,
    ctx: &RunContext<'_>,
    progress: &mut RunProgress,
    final_revert_needs_reboot: bool,
    cooldown_checks: Vec<(String, Vec<thermal::ThermalReading>)>,
    events: &mpsc::Sender<EngineEvent>,
) -> Result<RunOutcome> {
    // From here on the run is cleaning up / finalizing; the top-level abort
    // race must no longer be able to drop this work.
    let _ = ctx.no_return.send(true);
    // Scoring (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §8): now that every scenario's `per_iteration` data
    // is in, compute each non-baseline scenario's
    // `metric_deltas`/`wcps`/`verdict` against the baseline's per-iteration
    // distribution.
    let baseline =
        progress.completed.first().cloned().ok_or_else(|| {
            Error::msg("no baseline result recorded -- cannot score this run".into())
        })?;
    let mut scenario_results: Vec<ScenarioResult> = progress
        .completed
        .get(1..)
        .map(<[ScenarioResult]>::to_vec)
        .unwrap_or_default();
    score_scenarios(&baseline.per_iteration, &mut scenario_results)?;
    for result in &scenario_results {
        emit(
            events,
            EngineEvent::ScenarioScored {
                result: result.clone(),
            },
        )
        .await;
    }

    // ROLLBACK: belt-and-braces restoration of the run-start active power
    // plan, plus a defensive sweep for any journal file this run left with
    // unconfirmed entries. A failure here fails the whole run, matching
    // every earlier phase's own contract -- REPORT never runs against a
    // machine ROLLBACK couldn't confirm was actually restored.
    //
    // `rollback`'s forced revert of an in-flight scenario is inert on this
    // path by construction: reaching `finish_run` at all means the loop is
    // done, so `progress.cursor.stage` is either `Done` (the ordinary exit)
    // or `Revert` (the last scenario's revert already ran, and only its
    // confirmation reboot is outstanding) -- neither of which
    // `in_flight_journal` treats as in flight.
    emit(
        events,
        EngineEvent::PhaseChanged {
            phase: Phase::Rollback,
        },
    )
    .await;
    let rollback_result = rollback(sys, ctx, progress, events).await;
    teardown(sys, ctx, events, rollback_result.is_ok()).await;
    rollback_result?;

    // REPORT
    emit(
        events,
        EngineEvent::PhaseChanged {
            phase: Phase::Report,
        },
    )
    .await;
    let results = RunResults {
        schema_version: crate::model::SCHEMA_VERSION.to_string(),
        run_id: ctx.config.run_id.clone(),
        project_id: ctx.config.project.id.clone(),
        completed_at: crate::model::results::utc_timestamp_now(),
        detection_tier: DetectionTier::LogTail,
        baseline,
        scenarios: scenario_results,
        unstable: progress.unstable.clone(),
    };
    let dir = run_dir(ctx.config);
    results.save(&dir.join("results.json"))?;
    // Best-effort (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2/A5): only written when thermal mode was
    // actually used this run, and a write failure here is a warning, never
    // a run failure -- `results.json` above is the run's real record, this
    // file is diagnostic.
    if progress.thermal_baseline.is_some() {
        let thermal_log = crate::model::thermal::ThermalLog {
            baseline: progress.thermal_baseline.clone(),
            cooldown_checks,
        };
        if let Err(e) = thermal_log.write(&dir) {
            tracing::warn!(error = %e, "failed to write thermal.json for this run");
            emit(
                events,
                EngineEvent::LogLine {
                    text: format!("Failed to write thermal.json for this run: {e}"),
                },
            )
            .await;
        }
    }

    if progress.shutdown_when_complete && !*ctx.abort.borrow() {
        // D3/D8: replaces the final revert reboot entirely -- the next boot
        // is stock, results are on disk, and ROLLBACK just confirmed the
        // machine is back to its run-start state (a failed ROLLBACK already
        // returned `Err` above, so this is never reached after one).
        // `progress.shutdown_when_complete` (not `ctx.config`'s fixed
        // start-time value) is the live, possibly-toggled-mid-run read --
        // see `control::drain_shutdown_toggle`.
        //
        // `!*ctx.abort.borrow()` excludes an operator Abort observed at any
        // point up through here: unlike a mid-run failure (which propagates
        // through `scenario_loop` as an `Err` and takes `execute()`'s own
        // D8-guarded branch instead), an Abort landing during the final
        // scenario's own `Stage::Revert` or during this function's own
        // scoring/ROLLBACK/teardown/REPORT work reaches this exact success
        // path with no `Err` and no earlier abort check in between --
        // `maybe_break` (the loop's one abort checkpoint) is deliberately
        // never reached after the last scenario, so this read is the only
        // place left able to catch it. Same rule as the `Err(body_err)` D8
        // branch above: auto-shutdown exists for *unattended* completion,
        // and an explicit Abort is definitionally the opposite.
        //
        // A shutdown-scheduling failure is NOT a run failure: `results.json`
        // is already saved and ROLLBACK already succeeded, so `?`-ing it
        // here would emit `RunFailed` for a finished run and -- worse --
        // skip the `final_revert_needs_reboot` reboot below that the last
        // scenario's revert still depends on. Logged, then fall through to
        // the same reboot-or-complete decision a run without D3 makes.
        match sys
            .shutdown(
                60,
                &format!(
                    "VOIDFRAME: run {} complete, shutting down",
                    ctx.config.run_id
                ),
            )
            .await
        {
            Ok(()) => {
                let _ = std::fs::remove_file(RunProgress::path(&dir));
                emit(
                    events,
                    EngineEvent::RunComplete {
                        run_id: ctx.config.run_id.clone(),
                    },
                )
                .await;
                return Ok(RunOutcome::ShutdownRequested(results));
            }
            Err(e) => {
                tracing::error!(error = ?e, "run complete, but scheduling the auto-shutdown failed");
                emit(
                    events,
                    EngineEvent::LogLine {
                        text: format!(
                            "Could not schedule the shutdown ({e}) -- finishing the run without \
                             it"
                        ),
                    },
                )
                .await;
            }
        }
    }
    if final_revert_needs_reboot {
        let scenario_id = ordered_scenarios(&progress.project)
            .get(progress.cursor.index as usize)
            .map(|s| s.id.clone())
            .unwrap_or_default();
        let next_cursor = Cursor {
            index: progress.cursor.index,
            stage: Stage::Done,
        };
        // M3 safety review, round 4: an Abort observed anywhere up through
        // here -- during the final scenario's own `Stage::Revert` or during
        // this function's own scoring/ROLLBACK/teardown/REPORT work -- must
        // stop the run instead of rebooting unattended. The D3 shutdown
        // block just above deliberately excludes an aborted run via its own
        // `!*ctx.abort.borrow()` check, but not returning from that block is
        // exactly what an aborted run does, so without this check the abort
        // would fall straight through into this reboot with nothing to
        // catch it. Same pattern as `scenario_loop`'s three
        // `reboot_sequence` call sites: `progress.abort_requested` is
        // persisted so `begin_resume`'s Layer-2 guard still catches this
        // even if some future path reached a reboot without this check.
        if *ctx.abort.borrow() {
            progress.abort_requested = true;
            progress.save(&dir)?;
            // Minor finding (M3 safety review, round 5): this branch returns
            // with only "aborted" to go on -- the last scenario's own revert
            // was already written to disk/registry by `Stage::Revert` above,
            // but a reboot is the only thing that makes it live, and this
            // return is exactly what skips that reboot. Without this line
            // the operator has no way to tell, from the event stream alone,
            // that the machine is not actually back to its run-start state
            // yet.
            emit(
                events,
                EngineEvent::LogLine {
                    text: format!(
                        "Run {} aborted before the final revert-confirmation reboot -- the last \
                         scenario's revert is saved but will not take effect until the machine \
                         is rebooted manually.",
                        ctx.config.run_id
                    ),
                },
            )
            .await;
            return Err(Error::aborted("operator requested Abort".into()));
        }
        reboot_sequence(
            sys,
            ctx.config,
            progress,
            RebootReason::RevertOnly,
            next_cursor,
            scenario_id,
            events,
            &ctx.no_return,
        )
        .await?;
        return Ok(RunOutcome::RebootPending(RebootReason::RevertOnly));
    }
    let _ = std::fs::remove_file(RunProgress::path(&dir));
    emit(
        events,
        EngineEvent::RunComplete {
            run_id: ctx.config.run_id.clone(),
        },
    )
    .await;
    Ok(RunOutcome::Complete(results))
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
        Margin, WcpsV3Weights, compute_wcps_v3, evaluate_verdict, load_calibrated_thresholds,
    };

    const THROUGHPUT_NAMES: [&str; 4] = ["avg_fps", "p1_fps", "p01_fps", "adaptive_frame_time_cv"];
    const PACING_NAMES: [&str; 2] = ["stutter_count_pct", "mean_abs_animation_error_ms"];
    // Same judgment call `stats::v3::verdict.rs`'s own tests use -- these 4
    // throughput margins were never added to the calibrated JSON cache (see
    // `evaluate_verdict`'s own doc comment for why), so they stay a
    // caller-supplied constant here too.
    let throughput_margins: std::collections::BTreeMap<String, Margin> = [
        ("avg_fps", 3.0),
        ("p1_fps", 3.0),
        ("p01_fps", 3.0),
        ("adaptive_frame_time_cv", 10.0),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Margin::RelativePct(v)))
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
/// scenario's except the last (nothing follows it to cool down for), while
/// CS2 is genuinely closed (`run_scenario_inner` always kills it before
/// returning). See `RunConfig::inter_scenario_break_seconds` for why this
/// exists.
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
/// Also drains this run's live `shutdown_when_complete` toggle
/// (`control::drain_shutdown_toggle`) before checking for a pending `Pause`
/// -- an inter-scenario break is the between-scenarios checkpoint cadence
/// the toggle is designed for (see `RunConfig::shutdown_when_complete`'s own
/// doc comment).
///
/// Checks for a pending `Pause` before doing anything else -- the run's own
/// wait through this break (which can be as long as
/// `THERMAL_COOLDOWN_MAX_WAIT`) would otherwise be invisible to the operator
/// until the next scenario's first iteration. `Err` (`honor_pause` seeing an
/// `Abort` -- whether at the head of the channel while not paused, or while
/// already paused -- or `abort` already set) propagates straight out --
/// `execute()`'s caller still runs ROLLBACK regardless.
#[expect(
    clippy::too_many_arguments,
    reason = "each parameter is a distinct piece of state this checkpoint genuinely needs \
              (system, the event/control/shutdown-toggle channels, live progress, the abort \
              signal, the break duration, and the thermal-mode inputs); bundling them into a \
              struct here would just move the same fields one layer over for no clarity gain"
)]
async fn maybe_break(
    sys: &dyn SystemController,
    events: &mpsc::Sender<EngineEvent>,
    control: &mut mpsc::Receiver<ControlMsg>,
    shutdown_toggle: &tokio::sync::watch::Receiver<bool>,
    progress: &mut RunProgress,
    run_dir: &std::path::Path,
    abort: tokio::sync::watch::Receiver<bool>,
    seconds: u32,
    hwinfo_path: Option<&std::path::Path>,
    sample_interval: Duration,
    sample_count: u32,
    hwinfo_started: &AtomicBool,
) -> Result<Vec<thermal::ThermalReading>> {
    control::drain_shutdown_toggle(shutdown_toggle, progress, run_dir)?;
    control::honor_pause(control).await?;
    if *abort.borrow() {
        return Err(Error::aborted("operator requested Abort".into()));
    }
    // Snapshotted rather than borrowed: `wait_for_thermal_cooldown` below
    // also needs `&mut progress` (for its own `drain_shutdown_toggle` calls),
    // and a live `&ThermalReading` borrow of `progress.thermal_baseline`
    // held across that call would conflict with it.
    let thermal_baseline = progress.thermal_baseline.clone();
    if let (Some(baseline), Some(hwinfo_path)) = (thermal_baseline, hwinfo_path) {
        emit(
            events,
            EngineEvent::PhaseChanged {
                phase: Phase::ThermalCooldown,
            },
        )
        .await;
        return wait_for_thermal_cooldown(
            sys,
            events,
            control,
            shutdown_toggle,
            progress,
            run_dir,
            abort,
            &baseline,
            hwinfo_path,
            seconds,
            sample_interval,
            sample_count,
            hwinfo_started,
        )
        .await;
    }
    fixed_time_break(events, seconds).await?;
    Ok(Vec::new())
}

/// Sleeps `seconds`, emitting a `LogLine` first -- a no-op when `seconds` is
/// `0`. The plain fixed-time break itself, shared by `maybe_break`'s own
/// no-thermal-baseline path and by [`wait_for_thermal_cooldown`]'s
/// failure-to-even-start-cooling exit paths (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A5: every thermal-mode
/// failure condition falls back to "Fixed-time break, unchanged"). An
/// operator Abort during this break takes effect immediately, via the
/// top-level race in [`execute`] dropping the whole run body rather than any
/// racing done here.
async fn fixed_time_break(events: &mpsc::Sender<EngineEvent>, seconds: u32) -> Result<()> {
    if seconds == 0 {
        return Ok(());
    }
    let _ = events
        .send(EngineEvent::LogLine {
            text: format!("Cooldown break: sleeping {seconds}s (CS2 closed)"),
        })
        .await;
    tokio::time::sleep(Duration::from_secs(seconds as u64)).await;
    Ok(())
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
/// via `model::thermal::ThermalLog`. Also drains the live
/// `shutdown_when_complete` toggle once per loop iteration, alongside
/// `Pause` (`control::drain_shutdown_toggle`) -- this wait can run up to
/// `THERMAL_COOLDOWN_MAX_WAIT` (10 minutes), so it gets its own checkpoint
/// rather than making the operator wait for the next inter-scenario break to
/// have a toggle observed. Checks for `Pause` and `abort` once per loop
/// iteration (before each `sample_loop` call) as its own checkpoint; an
/// operator Abort landing anywhere else in this wait, including mid-sample or
/// mid-check-gap-sleep, is still caught immediately by the top-level race in
/// [`execute`] dropping the whole run body, not by any racing done here.
#[expect(
    clippy::too_many_arguments,
    reason = "each parameter is a distinct piece of state this checkpoint genuinely needs \
              (system, the event/control/shutdown-toggle channels, live progress, the abort \
              signal, the thermal baseline/hwinfo inputs, and the fallback break duration); \
              bundling them into a struct here would just move the same fields one layer over \
              for no clarity gain"
)]
async fn wait_for_thermal_cooldown(
    sys: &dyn SystemController,
    events: &mpsc::Sender<EngineEvent>,
    control: &mut mpsc::Receiver<ControlMsg>,
    shutdown_toggle: &tokio::sync::watch::Receiver<bool>,
    progress: &mut RunProgress,
    run_dir: &std::path::Path,
    abort: tokio::sync::watch::Receiver<bool>,
    baseline: &thermal::ThermalReading,
    hwinfo_path: &std::path::Path,
    seconds: u32,
    sample_interval: Duration,
    sample_count: u32,
    hwinfo_started: &AtomicBool,
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
            fixed_time_break(events, seconds).await?;
            return Ok(checks);
        }
    };
    if !already_running {
        // This wait is now the owner of the HWiNFO instance it is about to
        // start -- recorded outside this future's own stack so the abort
        // path's `finalize_aborted_run` can still close it after this future
        // is dropped mid-wait (see `thermal::collect_thermal_sample`'s own
        // `hwinfo_started` doc). Stored BEFORE the await, not after: the real
        // `start_hwinfo` spawns HWiNFO *and* polls it ready for up to 15s
        // inside one `spawn_blocking`, so a body dropped while suspended
        // there would otherwise leave HWiNFO running with the flag still
        // `false` and nothing to close it. A start that then fails only costs
        // a `close_hwinfo` that is a harmless no-op-by-process-name.
        hwinfo_started.store(true, Ordering::SeqCst);
        if let Err(e) = sys.start_hwinfo(hwinfo_path).await {
            let _ = events
                .send(EngineEvent::LogLine {
                    text: format!(
                        "Thermal cooldown: failed to start HWiNFO ({e}), falling back to the \
                         fixed-time break"
                    ),
                })
                .await;
            fixed_time_break(events, seconds).await?;
            return Ok(checks);
        }
    }

    let deadline = tokio::time::Instant::now() + THERMAL_COOLDOWN_MAX_WAIT;
    // Set on the sensor-read-failure exit only -- distinguishes it from the
    // threshold-met/cap-reached exits below, which share this same `break`
    // but must NOT get the fixed-time fallback (see this function's own doc
    // comment on why only the three genuinely-failed-to-cool paths do).
    let mut sensor_read_failed = false;
    // Set on the pre-sample `honor_pause`/`*abort.borrow()` check below --
    // shares the same close-hwinfo-then-return-Err tail as the other exit
    // paths' own cleanup, so it's checked once, after the loop, alongside
    // `sensor_read_failed`. An Abort landing anywhere else in this wait is
    // instead caught by the top-level race in [`execute`] dropping the whole
    // run body.
    let mut aborted = false;
    // Set if `drain_shutdown_toggle` itself fails (a `progress.json` write
    // error) -- shares the same close-hwinfo-then-return-Err tail as
    // `aborted`, but is a distinct failure kind, so it gets its own flag
    // rather than being folded into `aborted`.
    let mut checkpoint_err = None;
    loop {
        if let Err(e) = control::drain_shutdown_toggle(shutdown_toggle, progress, run_dir) {
            checkpoint_err = Some(e);
            break;
        }
        if control::honor_pause(control).await.is_err() || *abort.borrow() {
            aborted = true;
            break;
        }
        // `sample_loop` alone can take `sample_count * sample_interval` (~30s
        // at the real production cadence) with no internal abort check of its
        // own; an Abort clicked mid-sample is still caught immediately by the
        // top-level race in [`execute`] dropping the whole run body.
        let sample = match thermal::sample_loop(sys, events, sample_interval, sample_count).await {
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
        tokio::time::sleep(THERMAL_COOLDOWN_CHECK_GAP).await;
    }

    if !already_running {
        if let Err(close_err) = sys.close_hwinfo().await {
            // A close failure must never mask an already-accumulated result
            // -- log and return `checks` untouched either way, mirroring
            // `collect_thermal_sample`'s own close-failure handling.
            tracing::warn!(
                error = %close_err,
                "failed to close HWiNFO after a thermal cooldown wait"
            );
            let _ = events
                .send(EngineEvent::LogLine {
                    text: format!(
                        "Failed to close HWiNFO after a thermal cooldown wait: {close_err}"
                    ),
                })
                .await;
        }
        // Cleared even when the close failed -- see the identical comment in
        // `thermal::collect_thermal_sample`.
        hwinfo_started.store(false, Ordering::SeqCst);
    }

    if let Some(e) = checkpoint_err {
        return Err(e);
    }
    if aborted {
        return Err(Error::aborted("operator requested Abort".into()));
    }

    // Fixed-time fallback for the sensor-read-failure exit only, run after
    // HWiNFO is already closed above -- there's no reason to keep it running
    // for the extra fallback sleep once this call is done sampling it.
    if sensor_read_failed {
        fixed_time_break(events, seconds).await?;
    }

    Ok(checks)
}
