//! `start_run` / `send_control` — the real Tauri bridge into
//! `voidframe_engine::run::execute`. This is the UI-facing analog of
//! `voidframe-cli`'s `cmd_run.rs`: same spawn/forward/control-channel
//! wiring, except events go to the frontend via `app.emit("vf:event", ..)`
//! instead of `println!`, and `EngineEvent::OperatorPrompt` is forwarded
//! like every other event rather than auto-acknowledged — a real UI modal
//! (`LiveMonitor`, from the frontend-integration plans) answers it by calling
//! `send_control(ControlMsg::OperatorAcknowledged)`.

use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tokio::sync::{Mutex as AsyncMutex, mpsc};
use voidframe_engine::capture::runner::{CaptureRunner, RealCaptureRunner};
use voidframe_engine::model::project::Project;
use voidframe_engine::run::execute::RunConfig;
use voidframe_engine::run::{ControlMsg, EngineEvent, Phase};

use crate::state::ActiveRun;

/// Refuses to start a new run while a previous one's crash-recovery
/// `RunState` is still on disk, unresolved. Split out from `start_run`'s
/// body so it's unit-testable against a real `StoreHandle` (via
/// `voidframe_engine::store::spawn_store` + a tempdir, the same convention
/// `commands::run_store`'s own `phase_changed_patches_current_scenario_not_just_phase`
/// uses) without needing the rest of `start_run`'s `AppHandle`/`State`
/// plumbing.
async fn refuse_if_unresolved_run_state(
    store: &voidframe_engine::store::StoreHandle,
) -> Result<(), String> {
    match store.snapshot().await.map_err(|e| e.to_string())? {
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
/// spawning the engine task: loads the project, reads `config.json`,
/// re-validates the PresentMon/HWiNFO paths, and builds the `RunConfig` and
/// `SystemController`/`CaptureRunner` the run will actually use. Split out
/// of `start_run_after_reservation` so the ordering (project load fails
/// before `state.sys` is ever touched) is exercised directly by
/// `a_failed_preparation_releases_the_reservation` without needing a real
/// project on disk.
async fn prepare_run(
    state: &crate::state::AppState,
    project_id: &str,
    run_id: &str,
    dry_run: bool,
) -> Result<PreparedRun, String> {
    // `Project::load`'s own `Display` is a bare io/serde message (e.g. on
    // Windows, a missing file surfaces only as "io: The system cannot find
    // the file specified.") with no path or project id in it -- `project_id`
    // is prefixed here so the operator (and
    // `a_failed_preparation_releases_the_reservation`, which asserts on this
    // exact text) can tell which project failed to load.
    let project = Project::load(
        &state
            .data_root
            .projects_dir()
            .join(format!("{project_id}.json")),
    )
    .map_err(|e| format!("{project_id}: {e}"))?;

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
    // still pays the real, capture-independent `SETTLE_BEFORE_CAPTURE`
    // delay in `run/execute/scenario.rs` -- deliberately left untouched
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
        let thermal_sample_override = if realistic {
            None
        } else {
            Some((std::time::Duration::from_millis(5), 2))
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

        prepare_run(state, project_id, run_id, dry_run).await
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

#[specta::specta]
#[tauri::command]
pub async fn start_run(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::state::AppState>,
    project_id: String,
    dry_run: bool,
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
    } = start_run_after_reservation(&state, &project_id, &run_id, dry_run).await?;

    let store = state.store.clone();
    let app_for_events = app.clone();

    // The forwarder does its own work -- spawn the run, forward events,
    // await its join handle -- and never touches `active_run` itself. If
    // its OWN code panics (not `execute()`, which is already isolated by
    // `spawn_run`'s own `JoinHandle`) before finishing, cleanup must not
    // depend on it reaching a particular line of its own body: dropping a
    // `JoinHandle` does NOT abort the still-running engine task, and a
    // panic here would otherwise skip a same-task cleanup line entirely.
    let mut run = voidframe_engine::run::spawn_run(sys, capture, config, control_rx);
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

        // `run.join.await` is `Result<Result<RunResults, engine::Error>,
        // JoinError>` -- BOTH layers matter. `spawn_run` already turns every
        // `Err` from `execute()` into exactly one `RunFailed`, sent through
        // the same `run.events` channel forwarded above -- so the frontend
        // and `update_store_from_event` (which clears the store on
        // `RunFailed`) have already seen it by the time this `match` runs.
        // Only a genuine panic (the outer `JoinError`) still needs its own
        // `RunFailed`/`store.clear()` here, since `spawn_run` never gets the
        // chance to emit one for a task that panicked instead of returning.
        match run.join.await {
            Ok(Ok(_)) => {
                // `execute()` emits `RunComplete` itself on the success
                // path, which `update_store_from_event` has already turned
                // into a `store.clear()`. Nothing to do.
            }
            Ok(Err(e)) => {
                // RunFailed was already emitted by spawn_run and cleared the
                // store via the forward loop above.
                log::error!("engine run failed: {e}");
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
            }
        }
    });

    // Decoupled from the forwarder's own body on purpose: this task only
    // awaits the forwarder's `JoinHandle`, which resolves to `Ok(())` on
    // normal completion OR `Err(JoinError)` if the forwarder itself
    // panicked -- either way that's the signal cleanup needs. This
    // guarantees `active_run` is cleared on every path: engine panic
    // (already handled inside the forwarder, which then finishes
    // normally), forwarder panic (caught here as `Err(JoinError)`), and
    // ordinary completion.
    tokio::spawn(async move {
        let _ = forwarder.await;
        if let Some(app_state) = app.try_state::<crate::state::AppState>() {
            *app_state.active_run.lock().await = None;
            record_last_known_build_id(&app_state).await;
        }
    });

    Ok(run_id)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn send_control_forwards_to_the_active_runs_channel() {
        let (tx, mut rx) = mpsc::channel(4);
        let active = tokio::sync::Mutex::new(Some(crate::state::ActiveRun {
            run_id: "r1".into(),
            control_tx: tx,
        }));
        send_control_impl(&active, voidframe_engine::run::ControlMsg::Pause)
            .await
            .unwrap();
        assert!(matches!(
            rx.recv().await,
            Some(voidframe_engine::run::ControlMsg::Pause)
        ));
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

        let active = Arc::new(tokio::sync::Mutex::new(Some(crate::state::ActiveRun {
            run_id: "r1".into(),
            control_tx: tx,
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
        *state.active_run.lock().await = Some(crate::state::ActiveRun {
            run_id: "r1".into(),
            control_tx,
        });

        let err = start_run_after_reservation(&state, "missing-project", "r1", false)
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
}
