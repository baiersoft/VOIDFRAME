//! `start_run` / `resume_run` / `send_control` — the real Tauri bridge into
//! `voidframe_engine::run::execute`. This is the UI-facing analog of
//! `voidframe-cli`'s `cmd_run.rs`: same spawn/forward/control-channel
//! wiring, except events go to the frontend via `app.emit("vf:event", ..)`
//! instead of `println!`, and `EngineEvent::OperatorPrompt` is forwarded
//! like every other event rather than auto-acknowledged — a real UI modal
//! (`LiveMonitor`, from the frontend-integration plans) answers it by calling
//! `send_control(ControlMsg::OperatorAcknowledged)`.
//!
//! `spawn_and_forward` is the one place both `start_run` (a fresh run) and
//! `resume_run` (spec §3.4, picking a `RebootPending` run back up after
//! `--resume`) hand off to the engine and forward its event stream — both
//! also start the heartbeat task here (spec §5.2). The one outcome the two
//! paths handle differently from every other exit is
//! `RunOutcome::RebootPending`: the OS reboot is about to kill this process,
//! so `active_run` and the crash-recovery store are deliberately left
//! exactly as they are (a later `--resume` reads them back) — see
//! `spawn_and_forward`'s own doc comment for how that's structured.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot, watch};
use voidframe_engine::capture::runner::{CaptureRunner, RealCaptureRunner};
use voidframe_engine::model::progress::RunProgress;
use voidframe_engine::model::project::Project;
use voidframe_engine::run::execute::RunConfig;
use voidframe_engine::run::{ControlMsg, EngineEvent, Phase, RunOutcome, RunStart};
use voidframe_engine::store::StoreHandle;
use voidframe_engine::system::SystemController;

use crate::state::ActiveRun;

/// Refuses to start a new run while a previous one's crash-recovery
/// `RunState` is still on disk, unresolved. Split out from `start_run`'s
/// body so it's unit-testable against a real `StoreHandle` (via
/// `voidframe_engine::store::spawn_store` + a tempdir, the same convention
/// `commands::run_store`'s own `phase_changed_patches_current_scenario_not_just_phase`
/// uses) without needing the rest of `start_run`'s `AppHandle`/`State`
/// plumbing.
async fn refuse_if_unresolved_run_state(store: &StoreHandle) -> Result<(), String> {
    match store.snapshot().await.map_err(|e| e.to_string())? {
        // A `RebootPending` snapshot isn't a crash -- it's a run
        // deliberately waiting for `resume_run` to pick it back up after the
        // OS reboots this process away (spec §3.3/§3.5); `BootResume` is
        // that same run interrupted again during its post-boot settle, which
        // `resume_run` also accepts. Emergency Restore remains the way out
        // for someone who wants to abandon it instead.
        Some(snapshot)
            if matches!(
                snapshot.phase,
                Phase::RebootPending { .. } | Phase::BootResume
            ) =>
        {
            Err(
                "a run is waiting to resume after a reboot -- use Resume on the dashboard, or \
                 Emergency Restore to abandon it"
                    .to_string(),
            )
        }
        Some(_) => Err(
            "a previous run's crash-recovery state is still unresolved -- use Emergency \
             Restore to roll it back before starting a new run"
                .to_string(),
        ),
        None => Ok(()),
    }
}

/// The toggle and the configured path both have to be set for thermal mode
/// to even be attempted -- a configured `hwinfo_path` with
/// `thermal_cooldown_enabled: false` must behave exactly like nothing
/// configured at all. Split out from `start_run`'s body so it's a plain,
/// synchronous, unit-testable mapping (same reasoning as
/// `system::select` above).
fn resolve_hwinfo_path(enabled: bool, configured: Option<String>) -> Option<PathBuf> {
    if enabled {
        configured.map(PathBuf::from)
    } else {
        None
    }
}

/// Re-validates `path` (the same TOCTOU-defense re-check `start_run` already
/// does for `presentmon_path` -- `config.json` is user-writable even though
/// this process runs elevated) and, unlike the PresentMon path, degrades to
/// `None` on failure instead of failing the run: thermal cooldown is a
/// nice-to-have, not a run-blocking requirement, so an invalid/stale
/// `hwinfo_path` must fall back to the plain fixed-time break rather than
/// aborting `start_run` entirely.
async fn validate_or_drop_hwinfo_path(path: PathBuf) -> Option<PathBuf> {
    let path_str = path.to_string_lossy().into_owned();
    let validation = match tokio::task::spawn_blocking(move || {
        crate::commands::config::validate_hwinfo_path(&path_str)
    })
    .await
    {
        Ok(result) => result,
        Err(e) => Err(super::join_error_to_string("validate_hwinfo_path", e)),
    };
    match validation {
        Ok(()) => Some(path),
        Err(e) => {
            log::warn!(
                "configured HWiNFO path failed re-validation, thermal cooldown disabled for \
                 this run: {e}"
            );
            None
        }
    }
}

/// Everything `start_run` needs from the fallible preparation chain (project
/// load, config read, PresentMon/HWiNFO re-validation, `RunConfig`
/// construction, controller selection) bundled up so `prepare_run` can be a
/// single function with a single return type instead of `start_run` juggling
/// each intermediate value inline.
struct PreparedRun {
    sys: Arc<dyn voidframe_engine::system::SystemController>,
    capture: Arc<dyn CaptureRunner>,
    config: RunConfig,
}

// Manual, field-less `Debug` impl: none of `SystemController`, `CaptureRunner`,
// or `RunConfig` implement `Debug`, but `a_failed_preparation_releases_the_
// reservation`'s `.unwrap_err()` on `Result<PreparedRun, String>` requires
// `PreparedRun: Debug` (it's only ever printed on a test failure, so there's
// nothing worth showing here).
impl std::fmt::Debug for PreparedRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedRun").finish_non_exhaustive()
    }
}

/// The fallible preparation chain between reserving `active_run` and
/// spawning the engine task: resolves the project (from disk, or from
/// `project_override` -- see below), reads `config.json`, re-validates the
/// PresentMon/HWiNFO paths, and builds the `RunConfig` and
/// `SystemController`/`CaptureRunner` the run will actually use. Split out
/// of `start_run_after_reservation` so the ordering (project load fails
/// before `state.sys` is ever touched) is exercised directly by
/// `a_failed_preparation_releases_the_reservation` without needing a real
/// project on disk.
///
/// `project_override`: `resume_run` passes `Some(progress.project.clone())`
/// so the resumed run replays against the exact project snapshot taken at
/// its own start (spec §3.1) rather than whatever `<project_id>.json`
/// happens to contain on disk now -- a project edited mid-run must not
/// change what a resumed run applies/measures. `start_run` always passes
/// `None`, the normal "load the current project" path.
async fn prepare_run(
    state: &crate::state::AppState,
    project_id: &str,
    run_id: &str,
    dry_run: bool,
    project_override: Option<Project>,
    shutdown_when_complete: bool,
) -> Result<PreparedRun, String> {
    let project = match project_override {
        // The override comes straight out of `progress.json`
        // (`%LOCALAPPDATA%`, writable by the unelevated user) and is never
        // passed through `Project::load`, so it gets the same two checks
        // `Project::load` gives a project read from disk: `id` is joined
        // into `projects\<id>\scripts\` and every `custom_script` there is
        // executed by this elevated process, and every scenario id is joined
        // into a journal path. Without this a crafted `progress.json` could
        // steer `--resume` into running an arbitrary script as
        // Administrator.
        Some(project) => {
            crate::commands::projects::validate_project_id(&project.id)?;
            project
                .validate_structure()
                .map_err(|e| format!("{}: {e}", project.id))?;
            project
        }
        // `Project::load`'s own `Display` is a bare io/serde message (e.g.
        // on Windows, a missing file surfaces only as "io: The system
        // cannot find the file specified.") with no path or project id in
        // it -- `project_id` is prefixed here so the operator (and
        // `a_failed_preparation_releases_the_reservation`, which asserts on
        // this exact text) can tell which project failed to load.
        None => Project::load(
            &state
                .data_root
                .projects_dir()
                .join(format!("{project_id}.json")),
        )
        .map_err(|e| format!("{project_id}: {e}"))?,
    };

    // Best-effort, tolerant of a poisoned config mutex (falls back to the
    // spec default rather than failing the whole run over a field that only
    // matters once this run reaches a reboot) -- unlike
    // `presentmon_path`/`hwinfo_path` below, a stale/default settle time is
    // never a safety issue, just a possibly-too-short or too-long wait.
    let post_boot_settle_seconds = state
        .config
        .lock()
        .map(|cfg| cfg.post_boot_settle_seconds)
        .unwrap_or(180);

    // Debug-build-only (the `mock-run` Cargo feature is never enabled by a
    // plain `cargo build`/`tauri build`/`tauri:build` release) AND only
    // activated when `VOIDFRAME_SIMULATE_RUN` is set -- an elevated,
    // system-mutating app must never run the real pipeline against a fake
    // backend just because it happens to be that build. Reuses the exact
    // same synthetic `SystemController`/`CaptureRunner` pair
    // `voidframe-cli`'s own `--controller mock` dev-harness flag already
    // exercises (`voidframe_engine::mock_harness`) -- no real CS2 launch,
    // no real PresentMon capture, so `run::execute`'s real state machine
    // and event stream (PhaseChanged/IterationComplete/ScenarioComplete/...)
    // drive the real UI in roughly a minute instead of ~20 (each iteration
    // still pays the real, capture-independent per-kind
    // `settle_before_capture` delay in `run/execute/scenario.rs` --
    // deliberately left untouched
    // rather than special-cased for this, so real-run timing behavior
    // isn't forked for a test-only path). PresentMon/HWiNFO path
    // validation below is skipped entirely: nothing here ever spawns
    // either, so a test environment need not have them configured at all.
    #[cfg(feature = "mock-run")]
    if std::env::var_os("VOIDFRAME_SIMULATE_RUN").is_some() {
        // `VOIDFRAME_MOCK_REALISTIC`: unset (the E2E suite's own default,
        // wdio.conf.ts never sets it) is the fast preset -- every mock wait
        // resolves in milliseconds so an automated test isn't paying real
        // wall-clock time for a foregone conclusion. Set it by hand before
        // a manual `tauri dev`/launch (`$env:VOIDFRAME_SIMULATE_RUN=1;
        // $env:VOIDFRAME_MOCK_REALISTIC=1`) to instead see roughly what a
        // real run looks like -- real thermal-sampling cadence, and a real
        // per-iteration capture-window sleep -- without needing real
        // CS2/Steam/HWiNFO installed.
        let realistic = std::env::var_os("VOIDFRAME_MOCK_REALISTIC").is_some();

        let sys: Arc<dyn voidframe_engine::system::SystemController> =
            Arc::new(voidframe_engine::mock_harness::mock_ready_controller());
        let mut capture_runner = voidframe_engine::capture::runner::MockCaptureRunner::new(vec![
            voidframe_engine::mock_harness::mock_metrics(),
        ]);
        if realistic {
            capture_runner = capture_runner.with_realistic_duration();
        }
        let capture: Arc<dyn CaptureRunner> = Arc::new(capture_runner);

        let mock_cs2_log_delay = if realistic {
            std::time::Duration::from_secs(2)
        } else {
            std::time::Duration::from_millis(150)
        };
        // `None` here means "use the real production thermal-sampling
        // cadence" (see `RunConfig::thermal_sample_override`'s own doc
        // comment) -- exactly what the realistic preset wants. The fast
        // preset overrides it: `MockController`'s HWiNFO reading is a
        // fixed value, so baseline and every cooldown check land on the
        // same reading and "already cool" is a foregone conclusion --
        // no reason to pay the real ~30s per check finding that out.
        // `VOIDFRAME_MOCK_THERMAL` (via `parse_mock_thermal`) provides an
        // E2E seam to pin the thermal baseline to a clickable window.
        let thermal_sample_override = match std::env::var("VOIDFRAME_MOCK_THERMAL")
            .ok()
            .as_deref()
            .and_then(parse_mock_thermal)
        {
            Some(pinned) => Some(pinned),
            None if realistic => None,
            None => Some((std::time::Duration::from_millis(5), 2)),
        };

        let config = RunConfig {
            project,
            run_id: run_id.to_string(),
            dry_run,
            data_root: state.data_root.path().to_path_buf(),
            webview_root_pid: Some(crate::webview_pid::current_process_id()),
            console_log_override: None,
            // The whole reason the `Cs2LogDetector` trait abstraction
            // exists (see `voidframe_engine::mock_harness::MockLogTail`'s
            // own doc comment): the mock `sys` above never launches a real
            // CS2, so a real `RealLogTail` would tail a `console.log` that's
            // never written to and every watchdog wait would genuinely
            // time out.
            mock_cs2_log: Some(mock_cs2_log_delay),
            thermal_sample_override,
            inter_scenario_break_seconds: 0,
            // Always `Some` now (never a real path -- `MockController`'s
            // HWiNFO methods never touch the filesystem) so both mock-run
            // presets exercise the real ThermalBaseline phase and cooldown
            // events instead of silently skipping them, matching what a
            // real run configured for thermal cooldown would do.
            hwinfo_path: Some(std::path::PathBuf::from("mock-hwinfo.exe")),
            shutdown_when_complete,
            post_boot_settle: Duration::from_secs(post_boot_settle_seconds.into()),
            exe_path: std::env::current_exe().map_err(|e| e.to_string())?,
        };
        return Ok(PreparedRun {
            sys,
            capture,
            config,
        });
    }

    // `.lock()` on a std Mutex returns `Err` if another thread panicked
    // while holding it. `.unwrap()`-ing that here would panic *after* the
    // `active_run` reservation was taken but *before* any task exists to
    // clear it, permanently leaking the reservation so every future
    // `start_run` fails with "a run is already active" until the app is
    // restarted. Surface it as an ordinary error, on the same rollback path
    // as every other failure in this window.
    //
    // The `map`/`map_err` pair (rather than matching on `lock()` directly)
    // is load-bearing: it drops the non-`Send` `MutexGuard` at the end of
    // this statement, so the guard is not held across the `.await`s below --
    // a `#[tauri::command]`'s future must be `Send`.
    let config_fields = state
        .config
        .lock()
        .map(|cfg| {
            (
                cfg.presentmon_path.clone(),
                cfg.hwinfo_path.clone(),
                cfg.thermal_cooldown_enabled,
            )
        })
        .map_err(|_| ());
    let (presentmon_path_str, hwinfo_path_configured, thermal_cooldown_enabled) = config_fields
        .map_err(|()| {
            "configuration state is poisoned (a previous operation panicked while holding it) \
             -- restart VOIDFRAME"
                .to_string()
        })?;

    // Re-validated here, not just at `save_config` time. This is defense in
    // depth against the exact escalation `validate_presentmon_path`
    // documents: `config.json` sits under `%LOCALAPPDATA%`, writable by the
    // unelevated user, and the file it names is about to be spawned as a
    // subprocess by this *elevated* process. Save-time validation alone
    // leaves a TOCTOU window -- the value (or the file it points at) can be
    // swapped between the last save and this spawn, including by a
    // `config.json` edited entirely outside the app.
    // `validate_presentmon_path` calls `std::fs::metadata` -- moved onto
    // `spawn_blocking` rather than run directly on this async command's own
    // task, matching every other blocking-fs call site in `commands/*.rs`
    // (see `crate::commands::join_error_to_string`'s doc comment).
    let presentmon_path_str_for_validation = presentmon_path_str.clone();
    let validation = match tokio::task::spawn_blocking(move || {
        crate::commands::config::validate_presentmon_path(&presentmon_path_str_for_validation)
    })
    .await
    {
        Ok(result) => result,
        Err(e) => Err(crate::commands::join_error_to_string(
            "validate_presentmon_path",
            e,
        )),
    };
    validation?;

    let presentmon_path: PathBuf = presentmon_path_str.into();
    let sys = voidframe_engine::system::select(dry_run, state.sys.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(RealCaptureRunner::new(presentmon_path));

    // Both the toggle and a configured path are required for thermal mode
    // to even be attempted; if so, the path is re-validated for the same
    // TOCTOU reason as `presentmon_path` above, but degrades to `None`
    // instead of failing the run -- thermal cooldown always falls back to
    // the plain fixed-time break rather than blocking a run.
    let hwinfo_path = match resolve_hwinfo_path(thermal_cooldown_enabled, hwinfo_path_configured) {
        Some(path) => validate_or_drop_hwinfo_path(path).await,
        None => None,
    };

    let config = RunConfig {
        project,
        run_id: run_id.to_string(),
        dry_run,
        data_root: state.data_root.path().to_path_buf(),
        webview_root_pid: Some(crate::webview_pid::current_process_id()),
        console_log_override: None,
        mock_cs2_log: None,
        thermal_sample_override: None,
        inter_scenario_break_seconds: 0,
        hwinfo_path,
        shutdown_when_complete,
        post_boot_settle: Duration::from_secs(post_boot_settle_seconds.into()),
        exe_path: std::env::current_exe().map_err(|e| e.to_string())?,
    };

    Ok(PreparedRun {
        sys,
        capture,
        config,
    })
}

/// Runs `refuse_if_unresolved_run_state`, initializes the crash-recovery
/// `RunState`, and then runs `prepare_run` -- the whole fallible chain
/// between `start_run`'s reservation and `spawn_run`. This is the single
/// release site: any `Err` from `prepare_run` (or from `set_state`) releases
/// the `active_run` reservation and clears the store it just wrote, so
/// `start_run` itself never repeats that cleanup pair.
///
/// `refuse_if_unresolved_run_state` is deliberately checked *before* the
/// `async` block below, not inside it: if a previous run's crash-recovery
/// state is still on disk, this call never wrote it and must not clear it --
/// only the reservation this call took is released. Everything from
/// `set_state` onward, by contrast, is state this call itself created, so a
/// failure there does clear the store. The two release sites below (the
/// refusal path and the `prepared.is_err()` branch) are deliberately
/// separate, not a shared cleanup path: the refusal path must leave a
/// previous run's crash-recovery `RunState` on disk, while every later
/// failure must clear the `RunState` this call itself just wrote -- do not
/// collapse them into one.
async fn start_run_after_reservation(
    state: &crate::state::AppState,
    project_id: &str,
    run_id: &str,
    dry_run: bool,
    shutdown_when_complete: bool,
) -> Result<PreparedRun, String> {
    // A `RunState` already on disk means the previous session ended without
    // a clean terminal event -- store/mod.rs's own doc comment calls this
    // exact condition the crash-recovery signal. Overwriting it
    // unconditionally would destroy that signal before the new run's own
    // PREFLIGHT phase even begins, leaving only `emergency_rollback` (which
    // scans disk, not `state.json`) able to reach the crashed run afterward.
    if let Err(e) = refuse_if_unresolved_run_state(&state.store).await {
        *state.active_run.lock().await = None;
        return Err(e);
    }

    let prepared = async {
        // Initialize the crash-recovery RunState eagerly, here, rather than
        // lazily from inside the event-forwarding loop. `execute()`'s first
        // three events are `PhaseChanged { phase: Phase::Preflight }`,
        // `PhaseChanged { phase: Phase::Snapshot }`, `PhaseChanged { phase:
        // Phase::Baseline }` -- all routed through `StoreHandle::patch`,
        // which is a no-op when no `RunState` exists yet. A lazy catch-all
        // init never sees any of those three variants, so all three phase
        // transitions were silently lost; a crash during
        // PREFLIGHT/SNAPSHOT/BASELINE left no `RunState` on disk at all for
        // `get_run_snapshot` to report, as if no run had ever started.
        state
            .store
            .set_state(voidframe_engine::store::RunState {
                schema_version: "1.1.0".into(),
                run_id: run_id.to_string(),
                project_id: project_id.to_string(),
                phase: Phase::Preflight,
                current_scenario: None,
                completed_scenarios: vec![],
                revision: 1,
            })
            .await
            .map_err(|e| e.to_string())?;

        prepare_run(
            state,
            project_id,
            run_id,
            dry_run,
            None,
            shutdown_when_complete,
        )
        .await
    }
    .await;

    if prepared.is_err() {
        // The single release site: whatever failed, the slot and the
        // crash-recovery state this call created must not outlive it.
        *state.active_run.lock().await = None;
        let _ = state.store.clear().await;
    }
    prepared
}

/// Sends `msg` down the currently active run's control channel. Split out
/// from the `#[tauri::command]` wrapper so it's testable against a plain
/// `AsyncMutex<Option<ActiveRun>>` without a real `tauri::State`.
pub(crate) async fn send_control_impl(
    active_run: &AsyncMutex<Option<ActiveRun>>,
    msg: ControlMsg,
) -> Result<(), String> {
    // `control_tx` is a cheaply-`Clone`-able `mpsc::Sender`, so it's cloned
    // out of the guard and the guard is dropped (block ends) BEFORE the
    // `.send().await` below -- the channel is bounded(16) (see `start_run`),
    // so under backpressure a send held across the lock would park every
    // other `active_run` lock attempt (a concurrent `start_run`/cleanup)
    // behind it for no reason: the lookup is the only part that actually
    // needs the lock.
    let control_tx = {
        let guard = active_run.lock().await;
        match guard.as_ref() {
            Some(active) => active.control_tx.clone(),
            None => return Err("no run is currently active".to_string()),
        }
    };
    control_tx
        .send(msg)
        .await
        .map_err(|_| "run has already ended".to_string())
}

#[specta::specta]
#[tauri::command]
pub async fn send_control(
    state: tauri::State<'_, crate::state::AppState>,
    msg: ControlMsg,
) -> Result<(), String> {
    send_control_impl(&state.active_run, msg).await
}

/// Sends `value` down the currently active run's live `shutdown_when_complete`
/// toggle channel -- a separate, dedicated channel from `control_tx` (see
/// `voidframe_engine::run::execute::control::drain_shutdown_toggle`'s own doc
/// comment for why this is never routed through `ControlMsg`). Split out
/// from the `#[tauri::command]` wrapper for the same reason as
/// `send_control_impl`.
///
/// `watch::Sender::send` is synchronous and never blocks -- unlike
/// `send_control_impl`'s `control_tx.send(msg).await`, there is no backpressure
/// to hold the lock across, so the guard can be dropped and the send made in
/// the same statement. It only errors when every receiver has been dropped
/// (the run has ended), which is mapped to the same "run has already ended"
/// message `send_control_impl` uses for its own analogous failure.
pub(crate) async fn set_shutdown_when_complete_impl(
    active_run: &AsyncMutex<Option<ActiveRun>>,
    value: bool,
) -> Result<(), String> {
    let guard = active_run.lock().await;
    match guard.as_ref() {
        Some(active) => active
            .shutdown_toggle_tx
            .send(value)
            .map_err(|_| "run has already ended".to_string()),
        None => Err("no run is currently active".to_string()),
    }
}

#[specta::specta]
#[tauri::command]
pub async fn set_shutdown_when_complete(
    state: tauri::State<'_, crate::state::AppState>,
    value: bool,
) -> Result<(), String> {
    set_shutdown_when_complete_impl(&state.active_run, value).await
}

/// Spawns the engine task, forwards its event stream to the frontend and the
/// crash-recovery store, and runs the heartbeat task (spec §5.2) alongside
/// it -- the one hand-off both `start_run` (fresh) and `resume_run` (after
/// `--resume`) share.
///
/// The forwarder does its own work -- spawn the run, forward events, await
/// its join handle -- and never touches `active_run` itself. If its OWN code
/// panics (not `execute()`, which is already isolated by `spawn_run`'s own
/// `JoinHandle`) before finishing, cleanup must not depend on it reaching a
/// particular line of its own body: dropping a `JoinHandle` does NOT abort
/// the still-running engine task, and a panic here would otherwise skip a
/// same-task cleanup line entirely. So a second, decoupled task only awaits
/// the forwarder's own `JoinHandle` -- which resolves on every path: engine
/// panic (already handled inside the forwarder, which then finishes
/// normally), forwarder panic (caught there as `Err(JoinError)`), or
/// ordinary completion.
///
/// `RunOutcome::RebootPending` is the one exception to "clear `active_run`
/// once the run stops": the process is about to be killed by the OS reboot,
/// and both `active_run` and the crash-recovery store must be left exactly
/// as they are for `resume_run` to read back after `--resume`. The forwarder
/// signals "go ahead and clean up" to the second task via a `oneshot` it
/// only sends on every OTHER outcome (including a genuine failure) --
/// dropping the sender without sending, as the `RebootPending` arm does, is
/// what tells the cleanup task to skip clearing `active_run` altogether.
#[expect(
    clippy::too_many_arguments,
    reason = "each parameter is a distinct piece of state spawn_run/execute() genuinely need; \
              bundling them into a struct here would just move the same eight fields one layer \
              over for no clarity gain"
)]
fn spawn_and_forward(
    app: tauri::AppHandle,
    store: StoreHandle,
    sys: Arc<dyn SystemController>,
    capture: Arc<dyn CaptureRunner>,
    start: RunStart,
    control_rx: mpsc::Receiver<ControlMsg>,
    shutdown_toggle_rx: watch::Receiver<bool>,
    run_dir: PathBuf,
) {
    let heartbeat_dir = run_dir.clone();
    let heartbeat = tokio::spawn(async move {
        loop {
            // `touch_heartbeat` is a synchronous file write -- kept off the
            // async runtime like every other blocking-fs call in
            // `commands/*.rs`. Best-effort either way: a missed beat only
            // shortens the deadman's grace, it never fails the run.
            let dir = heartbeat_dir.clone();
            let _ = tokio::task::spawn_blocking(move || {
                voidframe_engine::recover::touch_heartbeat(&dir)
            })
            .await;
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });

    let app_for_events = app.clone();
    let (cleanup_tx, cleanup_rx) = oneshot::channel::<()>();

    let mut run =
        voidframe_engine::run::spawn_run(sys, capture, start, control_rx, shutdown_toggle_rx);
    let forwarder = tokio::spawn(async move {
        while let Some(ev) = run.events.recv().await {
            let _ = app_for_events.emit("vf:event", &ev);
            // Best-effort: a store-update failure must never stop event
            // forwarding to the UI -- the UI seeing the real event stream
            // is what matters; `get_run_snapshot` losing one intermediate
            // update is a much smaller problem than the live monitor
            // silently freezing because a background actor hiccuped.
            let _ = super::run_store::update_store_from_event(&store, &ev).await;
        }

        // `run.join.await` is `Result<Result<RunOutcome, engine::Error>,
        // JoinError>` -- BOTH layers matter. `spawn_run` already turns every
        // `Err` from `execute()` into exactly one `RunFailed`, sent through
        // the same `run.events` channel forwarded above -- so the frontend
        // and `update_store_from_event` (which clears the store on
        // `RunFailed`) have already seen it by the time this `match` runs.
        // Only a genuine panic (the outer `JoinError`) still needs its own
        // `RunFailed`/`store.clear()` here, since `spawn_run` never gets the
        // chance to emit one for a task that panicked instead of returning.
        match run.join.await {
            Ok(Ok(RunOutcome::RebootPending(reason))) => {
                log::info!("engine leg ended with a pending reboot ({reason}); waiting for the OS");
                // Deliberately does NOT send on `cleanup_tx`: `active_run`
                // and the store must be left exactly as they are (see this
                // function's own doc comment). A watchdog covers the case
                // where `SystemController::reboot` silently never actually
                // reboots the machine.
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    log::error!(
                        "60s after entering RebootPending and the OS still has not rebooted \
                         this process away -- the reboot may have failed silently"
                    );
                });
            }
            Ok(Ok(RunOutcome::ShutdownRequested(_))) => {
                log::info!("run complete; shutdown requested");
                let _ = cleanup_tx.send(());
            }
            Ok(Ok(RunOutcome::Complete(_))) => {
                // `execute()` emits `RunComplete` itself on the success
                // path, which `update_store_from_event` has already turned
                // into a `store.clear()`. Nothing to do here besides
                // cleaning up `active_run`.
                let _ = cleanup_tx.send(());
            }
            Ok(Err(e)) => {
                // RunFailed was already emitted by spawn_run and cleared the
                // store via the forward loop above.
                log::error!("engine run failed: {e}");
                let _ = cleanup_tx.send(());
            }
            Err(join_err) => {
                log::error!("engine task join error (likely a panic): {join_err:?}");
                let _ = app_for_events.emit(
                    "vf:event",
                    &EngineEvent::RunFailed {
                        reason: format!("engine task panicked: {join_err}"),
                    },
                );
                let _ = store.clear().await;
                let _ = cleanup_tx.send(());
            }
        }
    });

    tokio::spawn(async move {
        let _ = forwarder.await;
        // The heartbeat only matters while the engine leg (or its own
        // forwarding) is still alive -- ends here regardless of outcome; on
        // `RebootPending` the process is about to die anyway.
        heartbeat.abort();
        if cleanup_rx.await.is_ok()
            && let Some(app_state) = app.try_state::<crate::state::AppState>()
        {
            *app_state.active_run.lock().await = None;
            record_last_known_build_id(&app_state).await;
        }
    });
}

#[specta::specta]
#[tauri::command]
pub async fn start_run(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    project_id: String,
    dry_run: bool,
    shutdown_when_complete: bool,
) -> Result<String, String> {
    // Validated first, before generating a run_id or touching active_run/
    // the store at all -- a rejected id fails fast without reserving
    // anything. Reuses the same path-traversal guard already proven
    // against `../../evil`-style ids in commands/projects.rs, so
    // `project_id` joined into a filesystem path below is never
    // caller-controlled in an unsafe way.
    crate::commands::projects::validate_project_id(&project_id)?;

    let run_id = uuid::Uuid::new_v4().to_string();
    let (control_tx, control_rx) = mpsc::channel::<ControlMsg>(16);
    // Seeded with this run's own starting value -- see
    // `voidframe_engine::run::execute::control::drain_shutdown_toggle`'s own
    // doc comment for why `watch` (always exactly the latest value, a
    // `.send()` that never blocks) is the right primitive for this toggle.
    let (shutdown_toggle_tx, shutdown_toggle_rx) = watch::channel::<bool>(shutdown_when_complete);

    // Atomic check-and-reserve: the lock is held continuously from the
    // emptiness check through the insert (no `.await` in between), closing
    // the TOCTOU window a separate check-then-act pair left open -- two
    // concurrent `start_run` calls could otherwise both observe `None`
    // during a released-lock check, both proceed past it, and race the
    // same SystemController/CS2 instance while silently overwriting each
    // other's `ActiveRun`.
    {
        let mut guard = state.active_run.lock().await;
        if guard.is_some() {
            return Err("a run is already active".into());
        }
        *guard = Some(ActiveRun {
            run_id: run_id.clone(),
            control_tx,
            shutdown_toggle_tx,
        });
    }

    // From here on, any failure before the forwarder is actually spawned
    // must roll back the reservation -- otherwise a failed start_run would
    // permanently block every future one. `start_run_after_reservation` is
    // the single release site for that entire window.
    let PreparedRun {
        sys,
        capture,
        config,
    } = start_run_after_reservation(
        &state,
        &project_id,
        &run_id,
        dry_run,
        shutdown_when_complete,
    )
    .await?;

    let run_dir = state.data_root.run_dir(&run_id);
    spawn_and_forward(
        app,
        state.store.clone(),
        sys,
        capture,
        RunStart::Fresh(config),
        control_rx,
        shutdown_toggle_rx,
        run_dir,
    );

    Ok(run_id)
}

/// Everything `resume_run` does that only needs `&AppState`: the store
/// snapshot/phase check, `RunProgress::load`, the atomic `active_run`
/// reservation, and `prepare_run` with the progress's own project as the
/// override (spec §3.4: "the progress's project snapshot is used, not the
/// file on disk"). Split out of `resume_run_impl` so it's unit-testable
/// without a live `tauri::AppHandle` -- the production `AppHandle` is
/// `Wry`-typed and cannot be constructed outside a real webview;
/// `tauri::test::mock_builder` builds a *different*, `MockRuntime`-typed
/// handle that a `Wry`-specific signature can't accept. Same reasoning as
/// every other `_impl` helper in this file (`send_control_impl`,
/// `start_run_after_reservation`, ...).
async fn resume_run_reserved(
    state: &crate::state::AppState,
) -> Result<
    (
        PreparedRun,
        RunProgress,
        mpsc::Receiver<ControlMsg>,
        watch::Receiver<bool>,
        PathBuf,
    ),
    String,
> {
    let snapshot = state
        .store
        .snapshot()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no run to resume".to_string())?;
    // `BootResume` too: `begin_resume` persists it (via `run_store`) the
    // moment a resume starts, before its ~180s settle wait -- a machine that
    // reboots again, or a process killed, inside that window leaves the
    // store at `BootResume` with `progress.json` still carrying the pending
    // reboot. Refusing that here made such a run unresumable for good
    // (`RESUME_TASK` re-firing every logon into this same error, and
    // `start_run` refusing as an unresolved crash). The engine decides what
    // the second resume means: a still-pending reboot classifies as an
    // extra reboot (spec §4), a cleared one fails cleanly through
    // `handle_body_failure`'s ROLLBACK.
    if !matches!(
        snapshot.phase,
        Phase::RebootPending { .. } | Phase::BootResume
    ) {
        return Err(format!(
            "run {} is not waiting for a reboot (phase {})",
            snapshot.run_id, snapshot.phase
        ));
    }
    // `run_id` is disk-sourced (`state/current.json`, writable by the
    // unelevated user) and gets joined into a filesystem path immediately
    // below -- same treatment `rollback_now`/`emergency_rollback` already
    // give it.
    crate::commands::projects::validate_project_id(&snapshot.run_id)?;
    let run_dir = state.data_root.run_dir(&snapshot.run_id);
    let progress = {
        let run_dir = run_dir.clone();
        tokio::task::spawn_blocking(move || RunProgress::load(&run_dir))
            .await
            .map_err(|e| crate::commands::join_error_to_string("resume_run progress load", e))?
            .map_err(|e| e.to_string())?
    };

    let (control_tx, control_rx) = mpsc::channel::<ControlMsg>(16);
    // Seeded with the resumed run's own on-disk value -- see `start_run`'s
    // matching comment on why `watch` and why it's seeded rather than
    // defaulted.
    let (shutdown_toggle_tx, shutdown_toggle_rx) =
        watch::channel::<bool>(progress.shutdown_when_complete);
    {
        let mut guard = state.active_run.lock().await;
        if guard.is_some() {
            return Err("a run is already active".into());
        }
        *guard = Some(ActiveRun {
            run_id: snapshot.run_id.clone(),
            control_tx,
            shutdown_toggle_tx,
        });
    }

    let prepared = prepare_run(
        state,
        &progress.project.id,
        &snapshot.run_id,
        false,
        Some(progress.project.clone()),
        progress.shutdown_when_complete,
    )
    .await;
    match prepared {
        Ok(p) => Ok((p, progress, control_rx, shutdown_toggle_rx, run_dir)),
        Err(e) => {
            *state.active_run.lock().await = None;
            Err(e)
        }
    }
}

/// The real production body of `resume_run`, taking only an `AppHandle` (not
/// a `tauri::State`) so it's callable both as the `#[tauri::command]` below
/// and from `lib.rs`'s `setup` hook on `--resume` startup, before any
/// command has ever been invoked.
pub(crate) async fn resume_run_impl(app: &tauri::AppHandle) -> Result<String, String> {
    let state = app.state::<crate::state::AppState>();
    let (prepared, progress, control_rx, shutdown_toggle_rx, run_dir) =
        resume_run_reserved(&state).await?;
    let PreparedRun {
        sys,
        capture,
        config,
    } = prepared;
    let run_id = config.run_id.clone();
    spawn_and_forward(
        app.clone(),
        state.store.clone(),
        sys,
        capture,
        RunStart::Resume { config, progress },
        control_rx,
        shutdown_toggle_rx,
        run_dir,
    );
    Ok(run_id)
}

#[specta::specta]
#[tauri::command]
pub async fn resume_run(app: tauri::AppHandle) -> Result<String, String> {
    resume_run_impl(&app).await
}

/// Split out from [`is_resume_launch`] so it's unit-testable on a plain
/// `bool`, without a real `tauri::State` -- same convention as every other
/// `_impl` helper in this file.
pub(crate) fn is_resume_launch_impl(resume_launch: bool) -> Result<bool, String> {
    Ok(resume_launch)
}

/// Whether this process was launched via `--resume` -- see
/// [`crate::state::AppState::resume_launch`]'s own doc comment for why the
/// frontend needs this exposed synchronously, separately from
/// `resume_run_impl`'s own completion.
#[specta::specta]
#[tauri::command]
pub async fn is_resume_launch(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<bool, String> {
    is_resume_launch_impl(state.resume_launch)
}

/// Records the CS2 build id this run ran against into `Config
/// .last_known_cs2_build_id`, in memory and on disk.
///
/// Without this the field was permanently `None` and docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §4.4/§7.5's
/// build-freeze pre-flight warning — `preflight` reads it and compares it
/// against the currently-installed build to say "CS2 updated since your last
/// run; earlier results may not be comparable" — was dead code that could
/// never fire.
///
/// Called after EVERY run, successful or not: what makes the next
/// pre-flight's comparison meaningful is "what build was installed the last
/// time this machine attempted a benchmark", and a run that failed halfway
/// still establishes that.
///
/// Reads the id back via `SystemController::app_manifest` (the same source
/// `execute()` itself uses to populate its own private `RunContext`'s
/// `start_build_id`) rather than plumbing `execute()`'s own captured copy
/// back out to this layer. `execute()` consumes its `RunConfig` by value and
/// its `RunResults` carries no build id, so surfacing the captured value
/// would mean a new field on a `schema_version`-checked persisted struct —
/// and it would still be unavailable on the failure paths, which is
/// precisely where recording it matters most. Re-reading here is a single
/// code path that covers both outcomes, and the two values can only differ
/// if Steam updated CS2 during the run, a case `execute()`'s own
/// per-scenario build-freeze check already prompts the operator about.
///
/// Entirely best-effort: `Ok(None)` (CS2 not installed / appmanifest
/// unreadable) leaves the previous value alone rather than erasing a
/// perfectly good one, and a failed `save` is not worth failing a finished
/// run over.
async fn record_last_known_build_id(state: &crate::state::AppState) {
    let build_id = match state.sys.app_manifest(730).await {
        Ok(Some(voidframe_engine::system::AppManifest {
            build_id: Some(id), ..
        })) => id,
        Ok(_) => return,
        Err(e) => {
            log::warn!("could not read the installed CS2 build id after the run: {e}");
            return;
        }
    };

    // Clone the updated config out from under the std Mutex and drop the
    // guard BEFORE touching the filesystem: a `MutexGuard` is not `Send`,
    // and this is an async fn.
    let to_save = {
        let Ok(mut cfg) = state.config.lock() else {
            log::warn!("config mutex poisoned; not recording the CS2 build id");
            return;
        };
        if cfg.last_known_cs2_build_id.as_deref() == Some(build_id.as_str()) {
            return; // unchanged -- no need to rewrite config.json
        }
        cfg.last_known_cs2_build_id = Some(build_id);
        cfg.clone()
    };

    if let Err(e) = to_save.save(&state.data_root.config_path()) {
        log::warn!("could not persist the CS2 build id to config.json: {e}");
    }
}

/// Parses `VOIDFRAME_MOCK_THERMAL` ("interval_ms,count") -- the E2E seam
/// that pins the mock run's thermal baseline to a clickable window (the
/// fast preset finishes in milliseconds; the realistic preset takes a real
/// 30s). Read only inside the `mock-run` + `VOIDFRAME_SIMULATE_RUN` gate
/// below; malformed input degrades to `None` (the preset default) rather
/// than failing a run over a test-only variable.
#[cfg(any(test, feature = "mock-run"))]
fn parse_mock_thermal(spec: &str) -> Option<(std::time::Duration, u32)> {
    let (ms, count) = spec.split_once(',')?;
    let ms: u64 = ms.trim().parse().ok()?;
    let count: u32 = count.trim().parse().ok()?;
    if count == 0 {
        return None;
    }
    Some((std::time::Duration::from_millis(ms), count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn send_control_forwards_to_the_active_runs_channel() {
        let (tx, mut rx) = mpsc::channel(4);
        let (shutdown_toggle_tx, _shutdown_toggle_rx) = watch::channel(false);
        let active = tokio::sync::Mutex::new(Some(crate::state::ActiveRun {
            run_id: "r1".into(),
            control_tx: tx,
            shutdown_toggle_tx,
        }));
        send_control_impl(&active, voidframe_engine::run::ControlMsg::Pause)
            .await
            .unwrap();
        assert!(matches!(
            rx.recv().await,
            Some(voidframe_engine::run::ControlMsg::Pause)
        ));
    }

    /// Mirrors `send_control_forwards_to_the_active_runs_channel` for the new
    /// live-toggle channel -- proves `set_shutdown_when_complete_impl` sends
    /// down `shutdown_toggle_tx`, not `control_tx`.
    #[tokio::test]
    async fn set_shutdown_when_complete_forwards_to_the_active_runs_toggle_channel() {
        let (control_tx, _control_rx) = mpsc::channel(4);
        let (shutdown_toggle_tx, shutdown_toggle_rx) = watch::channel(false);
        let active = tokio::sync::Mutex::new(Some(crate::state::ActiveRun {
            run_id: "r1".into(),
            control_tx,
            shutdown_toggle_tx,
        }));
        set_shutdown_when_complete_impl(&active, true)
            .await
            .unwrap();
        assert!(*shutdown_toggle_rx.borrow());
    }

    /// Mirrors `send_control_with_no_active_run_is_an_error_not_a_silent_noop`.
    #[tokio::test]
    async fn set_shutdown_when_complete_with_no_active_run_is_an_error_not_a_silent_noop() {
        let active: tokio::sync::Mutex<Option<crate::state::ActiveRun>> =
            tokio::sync::Mutex::new(None);
        assert!(
            set_shutdown_when_complete_impl(&active, true)
                .await
                .is_err()
        );
    }

    /// Regression proof for the audit's Medium finding: `send_control_impl`
    /// used to hold `active_run.lock().await`'s guard across the entire
    /// `control_tx.send(msg).await` call. Fills the channel to capacity so
    /// the send genuinely blocks (waiting on a receiver that never reads
    /// until this test says so), then races a second task for the same
    /// lock -- with the fix, that second task must acquire it promptly
    /// instead of waiting behind the in-flight send.
    ///
    /// Multi-threaded so the spawned send actually runs concurrently with
    /// this task rather than depending on single-threaded cooperative
    /// scheduling order; the sleep before racing for the lock is a
    /// generous, one-directional margin (a regression -- the lock still
    /// held during the send -- would make the timeout below fire, not the
    /// other way around, so a slow CI runner cannot turn this into a false
    /// pass).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_control_impl_releases_the_lock_before_the_send_completes() {
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(voidframe_engine::run::ControlMsg::Pause)
            .unwrap(); // fill the channel to capacity so the next send blocks

        let (shutdown_toggle_tx, _shutdown_toggle_rx) = watch::channel(false);
        let active = Arc::new(tokio::sync::Mutex::new(Some(crate::state::ActiveRun {
            run_id: "r1".into(),
            control_tx: tx,
            shutdown_toggle_tx,
        })));

        let active_for_send = active.clone();
        let send_task = tokio::spawn(async move {
            send_control_impl(&active_for_send, voidframe_engine::run::ControlMsg::Resume).await
        });

        // Give the spawned task time to reach and start blocking on the
        // full channel's `.send().await`.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let lock_attempt =
            tokio::time::timeout(std::time::Duration::from_millis(500), active.lock()).await;
        assert!(
            lock_attempt.is_ok(),
            "active_run's lock was still held while a send was in flight on a full channel"
        );
        drop(lock_attempt.unwrap());

        // Unblock the pending send and let the spawned task finish cleanly.
        rx.recv().await.unwrap();
        send_task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn send_control_with_no_active_run_is_an_error_not_a_silent_noop() {
        let active: tokio::sync::Mutex<Option<crate::state::ActiveRun>> =
            tokio::sync::Mutex::new(None);
        assert!(
            send_control_impl(&active, voidframe_engine::run::ControlMsg::Pause)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn start_run_rejects_a_traversal_project_id() {
        // `start_run` itself is a `#[tauri::command]` that needs a real
        // `AppHandle`/`State` this test can't construct -- so this proves
        // the exact function `start_run` now calls as its first line
        // (`crate::commands::projects::validate_project_id`) genuinely
        // rejects a traversal id, reusing the same evidence
        // `commands/projects.rs`'s own tests already establish for its
        // other callers (`get_project_rejects_a_traversal_id`, etc).
        assert!(crate::commands::projects::validate_project_id("../../evil").is_err());
    }

    #[tokio::test]
    async fn refuses_to_start_when_an_unresolved_run_state_is_already_on_disk() {
        // Regression for the audit's Medium finding: `start_run` used to
        // call `set_state` unconditionally, silently destroying a previous
        // crashed run's crash-recovery state before its own PREFLIGHT phase
        // even began.
        let dir = tempfile::tempdir().unwrap();
        let root = voidframe_engine::paths::DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let store = voidframe_engine::store::spawn_store(root);
        store
            .set_state(voidframe_engine::store::RunState {
                schema_version: "1.1.0".into(),
                run_id: "crashed-run".into(),
                project_id: "p1".into(),
                phase: Phase::Baseline,
                current_scenario: Some("baseline".into()),
                completed_scenarios: vec![],
                revision: 1,
            })
            .await
            .unwrap();

        let err = refuse_if_unresolved_run_state(&store).await.unwrap_err();
        assert!(err.contains("Emergency Restore"), "{err}");

        // The unresolved state itself must still be intact afterward -- the
        // whole point is that it was never overwritten.
        let snap = store.snapshot().await.unwrap().unwrap();
        assert_eq!(snap.run_id, "crashed-run");
    }

    #[tokio::test]
    async fn allows_starting_when_no_run_state_is_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let root = voidframe_engine::paths::DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let store = voidframe_engine::store::spawn_store(root);
        assert!(refuse_if_unresolved_run_state(&store).await.is_ok());
    }

    #[tokio::test]
    async fn refuses_to_start_with_a_distinct_message_when_a_run_is_waiting_to_resume() {
        // A `RebootPending` snapshot isn't a crash -- it's a run
        // deliberately waiting for `resume_run`, so the message must point
        // at Resume, not Emergency Restore alone.
        let dir = tempfile::tempdir().unwrap();
        let root = voidframe_engine::paths::DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let store = voidframe_engine::store::spawn_store(root);
        store
            .set_state(voidframe_engine::store::RunState {
                schema_version: "1.1.0".into(),
                run_id: "r1".into(),
                project_id: "p1".into(),
                phase: Phase::RebootPending {
                    reason: voidframe_engine::run::phase::RebootReason::ApplyNext,
                },
                current_scenario: Some("s1".into()),
                completed_scenarios: vec![],
                revision: 1,
            })
            .await
            .unwrap();

        let err = refuse_if_unresolved_run_state(&store).await.unwrap_err();
        assert!(err.contains("Resume"), "{err}");
        assert!(err.contains("Emergency Restore"), "{err}");
    }

    #[test]
    fn is_resume_launch_impl_echoes_the_flag_it_is_given() {
        assert_eq!(is_resume_launch_impl(true), Ok(true));
        assert_eq!(is_resume_launch_impl(false), Ok(false));
    }

    #[test]
    fn resolve_hwinfo_path_is_none_when_the_toggle_is_off_even_with_a_path_configured() {
        // The toggle and the path both have to be set for thermal mode to
        // even be attempted -- a configured path with the toggle off must
        // behave exactly like nothing configured at all.
        assert_eq!(
            resolve_hwinfo_path(false, Some(r"C:\HWiNFO64\HWiNFO64.exe".to_string())),
            None
        );
    }

    #[test]
    fn resolve_hwinfo_path_is_none_when_no_path_is_configured_even_with_the_toggle_on() {
        assert_eq!(resolve_hwinfo_path(true, None), None);
    }

    #[test]
    fn resolve_hwinfo_path_is_some_when_both_the_toggle_and_the_path_are_set() {
        assert_eq!(
            resolve_hwinfo_path(true, Some(r"C:\HWiNFO64\HWiNFO64.exe".to_string())),
            Some(PathBuf::from(r"C:\HWiNFO64\HWiNFO64.exe"))
        );
    }

    #[test]
    fn parse_mock_thermal_accepts_interval_ms_comma_count() {
        assert_eq!(
            parse_mock_thermal("250,40"),
            Some((std::time::Duration::from_millis(250), 40))
        );
        assert_eq!(
            parse_mock_thermal(" 5 , 2 "),
            Some((std::time::Duration::from_millis(5), 2))
        );
    }

    #[test]
    fn parse_mock_thermal_rejects_garbage_and_zero_count() {
        assert_eq!(parse_mock_thermal(""), None);
        assert_eq!(parse_mock_thermal("250"), None);
        assert_eq!(parse_mock_thermal("abc,3"), None);
        assert_eq!(parse_mock_thermal("250,0"), None);
    }

    #[tokio::test]
    async fn validate_or_drop_hwinfo_path_keeps_a_path_that_passes_validation() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("HWiNFO64.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let kept = validate_or_drop_hwinfo_path(exe.clone()).await;
        assert_eq!(kept, Some(exe));
    }

    #[tokio::test]
    async fn validate_or_drop_hwinfo_path_falls_back_to_none_instead_of_failing_the_run() {
        // Thermal cooldown is a nice-to-have, not a run-blocking
        // requirement -- a TOCTOU-invalidated path must degrade to the
        // plain fixed-time break, never abort `start_run`.
        let dropped =
            validate_or_drop_hwinfo_path(PathBuf::from(r"C:\definitely\not\here\evil.exe")).await;
        assert_eq!(dropped, None);
    }

    #[tokio::test]
    async fn a_failed_preparation_releases_the_reservation() {
        // prepare_run fails on a missing project file; the slot must be free afterwards.
        let dir = tempfile::tempdir().unwrap();
        let data_root =
            voidframe_engine::paths::DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let state = crate::state::AppState::with_data_root(data_root).unwrap();
        let (control_tx, _rx) = mpsc::channel(1);
        let (shutdown_toggle_tx, _shutdown_toggle_rx) = watch::channel(false);
        *state.active_run.lock().await = Some(crate::state::ActiveRun {
            run_id: "r1".into(),
            control_tx,
            shutdown_toggle_tx,
        });

        let err = start_run_after_reservation(&state, "missing-project", "r1", false, false)
            .await
            .unwrap_err();
        assert!(
            err.contains("missing-project") || err.to_lowercase().contains("not found"),
            "{err}"
        );
        assert!(
            state.active_run.lock().await.is_none(),
            "reservation must be released on failure"
        );
        assert!(
            state.store.snapshot().await.unwrap().is_none(),
            "store must be cleared on failure"
        );
    }

    #[test]
    fn start_run_rejects_a_presentmon_path_it_would_otherwise_spawn_elevated() {
        // `start_run` needs a real `AppHandle`/`State` this test can't
        // build, so this proves the exact guard it now runs before
        // constructing `RealCaptureRunner` -- the spawn-time half of the
        // save-time/spawn-time pair (config.json is user-writable, so the
        // save-time check alone leaves a TOCTOU window).
        assert!(
            crate::commands::config::validate_presentmon_path(r"\\attacker\share\evil.exe")
                .is_err()
        );
        assert!(crate::commands::config::validate_presentmon_path(r"C:\nope\missing.exe").is_err());
    }

    /// Builds a minimal but valid `Project` for `resume_run_reserved`'s own
    /// test -- field values themselves don't matter, only that `Project`
    /// deserializes/round-trips cleanly as the progress's `project_override`.
    #[cfg(feature = "mock-run")]
    fn minimal_project(id: &str) -> Project {
        Project {
            schema_version: voidframe_engine::model::SCHEMA_VERSION.to_string(),
            id: id.to_string(),
            name: "P".into(),
            description: "d".into(),
            created_at: "2026-09-06T00:00:00Z".into(),
            settings: serde_json::from_str("{}").unwrap(),
            baseline: voidframe_engine::model::project::Baseline {
                name: "Stock".into(),
                description: "d".into(),
            },
            scenarios: vec![],
        }
    }

    /// `resume_run_reserved` (spec §3.4): a `RebootPending` store snapshot
    /// plus a matching on-disk `RunProgress` must resume via the mock-run
    /// harness (`VOIDFRAME_SIMULATE_RUN`, so `prepare_run` never needs a
    /// real PresentMon/HWiNFO/project.json), reserving `active_run` and
    /// returning the progress's own run id -- `resume_run_impl` itself needs
    /// a real `Wry` `AppHandle` this test can't construct (see
    /// `resume_run_reserved`'s own doc comment), so this exercises the exact
    /// same fallible chain up to (not including) `spawn_and_forward`.
    #[cfg(feature = "mock-run")]
    #[tokio::test]
    async fn resume_run_reserved_resumes_a_reboot_pending_run_via_the_mock_harness() {
        // SAFETY: this test is the only one in the crate that reads
        // `VOIDFRAME_SIMULATE_RUN` (gated behind the `mock-run` feature,
        // never compiled into a plain `cargo test`), and every other test in
        // this module that reaches `prepare_run` without it either supplies
        // no override and fails at `Project::load` before the env var is
        // ever checked, or never calls `prepare_run` at all -- so a
        // concurrently-running sibling test cannot observe this value.
        unsafe {
            std::env::set_var("VOIDFRAME_SIMULATE_RUN", "1");
        }

        let dir = tempfile::tempdir().unwrap();
        let data_root =
            voidframe_engine::paths::DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let state = crate::state::AppState::with_data_root(data_root).unwrap();

        let run_dir = state.data_root.run_dir("r1");
        let progress = RunProgress {
            schema_version: voidframe_engine::model::SCHEMA_VERSION.to_string(),
            run_id: "r1".into(),
            project: minimal_project("p1"),
            start_build_id: None,
            start_launch_args: String::new(),
            start_launch_args_raw: String::new(),
            start_power_plan: voidframe_engine::system::PowerPlan {
                guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
                name: "Balanced".into(),
                active: true,
            },
            thermal_baseline: None,
            completed: vec![],
            unstable: vec![],
            cursor: voidframe_engine::model::progress::Cursor {
                index: 1,
                stage: voidframe_engine::model::progress::Stage::Apply,
            },
            reboot: Some(voidframe_engine::model::progress::PendingReboot {
                reason: voidframe_engine::run::phase::RebootReason::ApplyNext,
                scenario_id: "s1".into(),
                initiated_at: "2026-09-06T00:00:00Z".into(),
                boot_count: 0,
            }),
            shutdown_when_complete: false,
            skip_revert_once: false,
            abort_requested: false,
        };
        progress.save(&run_dir).unwrap();

        state
            .store
            .set_state(voidframe_engine::store::RunState {
                schema_version: "1.1.0".into(),
                run_id: "r1".into(),
                project_id: "p1".into(),
                phase: Phase::RebootPending {
                    reason: voidframe_engine::run::phase::RebootReason::ApplyNext,
                },
                current_scenario: Some("s1".into()),
                completed_scenarios: vec![],
                revision: 1,
            })
            .await
            .unwrap();

        let (prepared, loaded_progress, _control_rx, _shutdown_toggle_rx, returned_run_dir) =
            resume_run_reserved(&state).await.unwrap();

        assert_eq!(loaded_progress.run_id, "r1");
        assert_eq!(prepared.config.run_id, "r1");
        assert_eq!(returned_run_dir, run_dir);
        assert!(
            state.active_run.lock().await.is_some(),
            "active_run must be reserved for the resumed run"
        );

        // SAFETY: see the matching `set_var` comment above.
        unsafe {
            std::env::remove_var("VOIDFRAME_SIMULATE_RUN");
        }
    }

    /// The resume path's `project_override` comes straight out of
    /// `progress.json` (user-writable) and used to bypass both checks
    /// `Project::load` gives a project read from disk -- so a crafted
    /// `project.id` was joined verbatim into `projects\<id>\scripts\` and its
    /// scripts executed as Administrator on the next `--resume`.
    #[tokio::test]
    async fn prepare_run_rejects_a_project_override_with_a_traversal_id() {
        let dir = tempfile::tempdir().unwrap();
        let data_root =
            voidframe_engine::paths::DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let state = crate::state::AppState::with_data_root(data_root).unwrap();
        let project: Project = serde_json::from_value(serde_json::json!({
            "schema_version": voidframe_engine::model::SCHEMA_VERSION,
            "id": r"..\..\Users\Public\x", "name": "P", "description": "d",
            "created_at": "2026-09-10T00:00:00Z", "settings": {},
            "baseline": {"name": "Stock", "description": "d"},
            "scenarios": [{
                "id": "s1", "name": "S", "description": "d", "enabled": true,
                "modules": [{
                    "type": "custom_script", "apply_script": "apply.bat",
                    "revert_script": "revert.bat", "requires_reboot": false,
                    "description": "d"
                }]
            }]
        }))
        .unwrap();

        let err = prepare_run(&state, "ignored", "r1", false, Some(project), false)
            .await
            .unwrap_err();
        assert!(err.contains("invalid project id"), "{err}");
    }

    /// A scenario id that is not a safe path segment is rejected the same way
    /// -- it is joined into `journal-<id>.jsonl` by the run loop.
    #[tokio::test]
    async fn prepare_run_rejects_a_project_override_with_a_traversal_scenario_id() {
        let dir = tempfile::tempdir().unwrap();
        let data_root =
            voidframe_engine::paths::DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let state = crate::state::AppState::with_data_root(data_root).unwrap();
        let project: Project = serde_json::from_value(serde_json::json!({
            "schema_version": voidframe_engine::model::SCHEMA_VERSION,
            "id": "p1", "name": "P", "description": "d",
            "created_at": "2026-09-10T00:00:00Z", "settings": {},
            "baseline": {"name": "Stock", "description": "d"},
            "scenarios": [{
                "id": r"..\evil", "name": "S", "description": "d", "enabled": true,
                "modules": []
            }]
        }))
        .unwrap();

        let err = prepare_run(&state, "ignored", "r1", false, Some(project), false)
            .await
            .unwrap_err();
        assert!(err.contains("invalid id"), "{err}");
    }

    /// A resume interrupted during its post-boot settle leaves the store at
    /// `BootResume`; `resume_run_reserved` must let it through to the
    /// engine rather than refusing it forever. Without a `progress.json` on
    /// disk the next failure is the load, proving the phase gate was passed.
    #[tokio::test]
    async fn resume_run_reserved_accepts_a_boot_resume_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let data_root =
            voidframe_engine::paths::DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let state = crate::state::AppState::with_data_root(data_root).unwrap();
        state
            .store
            .set_state(voidframe_engine::store::RunState {
                schema_version: "1.1.0".into(),
                run_id: "r1".into(),
                project_id: "p1".into(),
                phase: Phase::BootResume,
                current_scenario: Some("s1".into()),
                completed_scenarios: vec![],
                revision: 1,
            })
            .await
            .unwrap();

        let err = resume_run_reserved(&state).await.unwrap_err();
        assert!(
            !err.contains("not waiting for a reboot"),
            "boot_resume must pass the phase gate: {err}"
        );
        assert!(
            state.active_run.lock().await.is_none(),
            "a failed reservation must not leave active_run set"
        );
    }

    /// `start_run` must point a `BootResume` snapshot at Resume, not at
    /// Emergency Restore -- the same message the `RebootPending` case gets.
    #[tokio::test]
    async fn start_run_refuses_a_boot_resume_snapshot_with_the_resume_hint() {
        let dir = tempfile::tempdir().unwrap();
        let root = voidframe_engine::paths::DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let store = voidframe_engine::store::spawn_store(root);
        store
            .set_state(voidframe_engine::store::RunState {
                schema_version: "1.1.0".into(),
                run_id: "r1".into(),
                project_id: "p1".into(),
                phase: Phase::BootResume,
                current_scenario: Some("s1".into()),
                completed_scenarios: vec![],
                revision: 1,
            })
            .await
            .unwrap();
        let err = refuse_if_unresolved_run_state(&store).await.unwrap_err();
        assert!(err.contains("use Resume on the dashboard"), "{err}");
    }
}
