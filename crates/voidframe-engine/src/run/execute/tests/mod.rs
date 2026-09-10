use super::*;
use crate::capture::runner::MockCaptureRunner;
use crate::model::module::{AffinityMode, Hive, RegType, RegistryPayload};
use crate::model::project::Baseline;
use crate::system::{AcDc, AppManifest, MockController, SteamStatus};
use tokio::io::AsyncWriteExt;

// The items below exist only so that this module's sibling test files' own
// `use super::*;` resolves the same set of names the old single `tests.rs`
// had in scope via its own `use super::*;` -- none of them are used by this
// file's own (non-test) code.
use super::launch::prepare_cs2_session;
use super::scenario::run_scenario_inner;
use super::steam::{graceful_kill_cs2, wait_for_process};
use crate::journal::Journal;
use crate::model::project::Scenario;
use crate::model::results::Verdict;
use crate::model::{AffinityCpuPayload, Module, Settings};
use crate::run::RunOutcome;
use crate::system::{MutationCtx, PowerPlan};
use std::path::Path;

mod abort;
mod launch_args;
mod preflight;
mod reboot;
mod scenario;
mod scoring;
mod steam;
mod thermal;

fn settings_with(warmup: u32, measure: u32, watchdog: u32) -> Settings {
    let mut s: Settings = serde_json::from_str("{}").unwrap();
    s.warmup_loops = warmup;
    s.measure_loops = measure;
    s.watchdog_seconds = watchdog;
    s
}

fn project_with(settings: Settings) -> Project {
    Project {
        schema_version: crate::model::SCHEMA_VERSION.to_string(),
        id: "p1".into(),
        name: "P".into(),
        description: "d".into(),
        created_at: "2026-09-01T00:00:00Z".into(),
        settings,
        baseline: Baseline {
            name: "b".into(),
            description: "d".into(),
        },
        scenarios: vec![],
    }
}

fn config_with(data_root: PathBuf, settings: Settings, console_log_path: PathBuf) -> RunConfig {
    RunConfig {
        project: project_with(settings),
        run_id: "r1".into(),
        dry_run: false,
        data_root,
        webview_root_pid: None,
        console_log_override: Some(console_log_path),
        mock_cs2_log: None,
        thermal_sample_override: None,
        inter_scenario_break_seconds: 0,
        hwinfo_path: None,
        shutdown_when_complete: false,
        post_boot_settle: Duration::from_secs(0),
        exe_path: PathBuf::from("voidframe.exe"),
    }
}

/// A `RunContext` with no run-start observations -- the default for every
/// test that doesn't specifically exercise the build-freeze re-check or the
/// launch-args-preservation fallback (see `RunContext`'s own doc comment for
/// what `execute()` populates these with for a real run).
fn context_for(config: &RunConfig) -> RunContext<'_> {
    RunContext {
        config,
        start_build_id: None,
        start_launch_args: String::new(),
        start_launch_args_raw: String::new(),
        start_power_plan: PowerPlan {
            guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            name: "Balanced".into(),
            active: true,
        },
        abort: tokio::sync::watch::channel(false).1,
        no_return: tokio::sync::watch::channel(false).0,
    }
}

/// A minimal `RunProgress` for the tests that drive `rollback()` directly.
/// Only two of its fields matter to that function: `project` (which
/// `ordered_scenarios` walks) and `cursor` (which scenario, if any, was in
/// flight when the run failed). `Stage::Done` is the "nothing in flight"
/// value -- see `rollback::in_flight_journal`.
fn progress_at(config: &RunConfig, index: u32, stage: Stage) -> RunProgress {
    RunProgress {
        schema_version: crate::model::SCHEMA_VERSION.to_string(),
        run_id: config.run_id.clone(),
        project: config.project.clone(),
        start_build_id: None,
        start_launch_args: String::new(),
        start_launch_args_raw: String::new(),
        start_power_plan: PowerPlan {
            guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            name: "Balanced".into(),
            active: true,
        },
        thermal_baseline: None,
        completed: vec![],
        unstable: vec![],
        cursor: Cursor { index, stage },
        reboot: None,
        shutdown_when_complete: false,
        skip_revert_once: false,
        abort_requested: false,
    }
}

fn write_signatures(data_root: &Path) {
    std::fs::write(
        data_root.join("signatures.json"),
        r#"{
  "map_loaded": "Loading map \"(?P<map>[^\"]+)\" \\(addon '(?P<addon>\\d+)'\\)",
  "benchmark_started": "\\[Server\\] BeginMatch",
  "benchmark_ended": "\\[Client\\] Disconnected from server:",
  "vprof_fps": "\\[VProf\\] FPS: Avg=(?P<avg>[\\d.]+), P1=(?P<p1>[\\d.]+)",
  "menu_ready": "\\[SteamNetSockets\\] SDR RelayNetworkStatus:"
}"#,
    )
    .unwrap();
}

fn scenario_with_modules(id: &str) -> Scenario {
    Scenario {
        id: id.into(),
        name: format!("Scenario {id}"),
        description: "d".into(),
        enabled: true,
        modules: vec![
            Module::Registry(RegistryPayload {
                hive: Hive::Hklm,
                subkey: "SYSTEM\\Control\\GraphicsDrivers".into(),
                value_name: "HwSchMode".into(),
                value_type: RegType::Dword,
                value: serde_json::json!(2),
                requires_reboot: false,
            }),
            Module::Powercfg {
                sub: "sub_processor".into(),
                setting: "IDLEDISABLE".into(),
                value: 1,
            },
            Module::AffinityCpu(AffinityCpuPayload {
                mode: AffinityMode::ExcludeCore0,
                mask_hex: None,
            }),
        ],
    }
}

async fn append_line(path: &Path, line: &str) {
    let mut f = tokio::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .await
        .unwrap();
    f.write_all(line.as_bytes()).await.unwrap();
    f.flush().await.unwrap();
}

/// Appends one full iteration's worth of console.log lines (map load +
/// benchmark start, then — after a short delay simulating the actual
/// benchmark running — disconnect) for each of `iterations` iterations,
/// with an initial delay before the first write. Runs concurrently with
/// `run_scenario` itself: `LogTail::wait_for` seeks to EOF on open, so
/// (per `cs2::detection`'s own tests) these lines are only ever visible
/// if they land on disk *after* that seek — writing them all up front,
/// before calling `run_scenario`, would not exercise the real tailing
/// behavior at all.
async fn write_console_log_lines(path: PathBuf, iterations: usize) {
    tokio::time::sleep(Duration::from_millis(250)).await;
    // Once per CS2 session, before any map load — exercises
    // `wait_for_menu_ready`'s real "found" path (a couple hundred ms,
    // not its multi-second fallback) the same way a real run's console.log
    // actually looks.
    append_line(
        &path,
        "[SteamNetSockets] SDR RelayNetworkStatus:  avail=OK  config=OK  anyrelay=OK\n",
    )
    .await;
    for _ in 0..iterations {
        append_line(&path, "Loading map \"de_dust2\" (addon '3240880604')\n").await;
        append_line(&path, "[Server] BeginMatch\n").await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        append_line(
            &path,
            "[Client] Disconnected from server: NETWORK_DISCONNECT_DISCONNECT_BY_USER\n",
        )
        .await;
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
}

/// An operator Abort mid-run whose own ROLLBACK also fails must not come
/// back looking like a clean abort -- `spawn_run`'s `e.is_aborted()` check
/// is what decides whether this run's journal files get pruned, and a
/// failed ROLLBACK is exactly the case where they must survive for manual
/// rollback (High-severity finding fixed by this task). Drives `execute()`
/// end-to-end: a control channel with `Pause` pre-queued (so the warmup
/// iteration's `honor_pause` checkpoint blocks) followed by `Abort` (so it
/// unblocks with an aborted error) makes `body` fail with `is_aborted() ==
/// true`. `MockController::fail_next_write` (already used elsewhere for
/// exactly this "make the next mutation write fail" purpose) is the
/// mechanism that fails ROLLBACK's own `set_active_power_plan` call -- but
/// it is a single one-shot flag shared by every mutating call on the mock,
/// `launch_cs2` included, so it can't simply be armed up front (baseline's
/// own CS2 launch would consume it first and never even reach the abort
/// checkpoint below). The `armer` task instead waits for
/// `launch_cs2_calls()` to confirm that launch already happened before
/// arming it, so it stays fresh for ROLLBACK -- the next (and only
/// remaining) mutating call this run makes.
#[tokio::test]
async fn execute_returns_a_non_aborted_error_when_rollback_also_fails_after_an_abort() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    write_signatures(dir.path());
    let settings = settings_with(1, 0, 5); // 1 warmup iteration, 0 measure
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let mock = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 8181);
    // NOT armed yet: `MockController::launch_cs2` also consumes
    // `fail_next_write` (it's a shared one-shot flag across every mutating
    // call, launch_cs2 included), and baseline's own CS2 launch must
    // succeed for this run to ever reach the abort checkpoint below. Armed
    // instead by the `armer` task, once baseline's `launch_cs2` call is
    // confirmed to have already happened -- see its own comment.
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));

    let mut config = config_with(
        dir.path().to_path_buf(),
        settings,
        dir.path().join("console.log"),
    );
    config.mock_cs2_log = Some(Duration::from_millis(0));

    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(4);
    // Both buffered before `execute()` ever starts -- `honor_pause` only
    // ever treats a subsequent `Abort` as meaningful while genuinely
    // paused (see its own doc comment), so `Pause` must come first.
    control_tx.send(ControlMsg::Pause).await.unwrap();
    control_tx.send(ControlMsg::Abort).await.unwrap();
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

    let exec = execute(
        sys,
        capture,
        RunStart::Fresh(config),
        events_tx,
        control_rx,
        abort_rx,
        shutdown_toggle_rx,
    );
    // Waits until baseline's own `launch_cs2` call has already happened
    // (confirmed via the mock's own call counter, not a real sleep -- safe
    // under `tokio::time::pause()`) before arming `fail_next_write`, so it
    // is still fresh -- and therefore consumed by ROLLBACK's own
    // `set_active_power_plan` call, the next (and only remaining) mutating
    // call this run makes -- rather than by baseline's own CS2 launch.
    let armer = async {
        for _ in 0..100_000u32 {
            if mock.launch_cs2_calls() > 0 {
                mock.fail_next_write("disk full");
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("armer never saw launch_cs2");
    };
    let (result, ()) = tokio::join!(exec, armer);
    drop(events_rx);

    let err =
        result.expect_err("an aborted run whose own rollback also failed must still be an Err");
    assert!(
        !err.is_aborted(),
        "must not look like a clean abort -- spawn_run's is_aborted() check decides whether \
         journal files get pruned: {err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("rollback also failed"),
        "expected the combined error to mention the rollback failure, got: {msg}"
    );
    assert!(
        msg.contains("disk full"),
        "expected the combined error to include the underlying rollback error, got: {msg}"
    );
}

/// **The critical safety-bug regression this fix exists for**: an operator
/// Abort whose own ROLLBACK *succeeds* must never trigger D8's auto-shutdown
/// branch, even with `shutdown_when_complete: true` live at the time of the
/// abort. Before this fix, that branch only asked "did ROLLBACK succeed?" --
/// never "was this actually an operator Abort?" -- so clicking Abort mid-run
/// and having its rollback succeed (the common case) shut the machine down
/// anyway, unattended, while the operator was still right there. Same setup
/// as `execute_returns_a_non_aborted_error_when_rollback_also_fails_after_an_abort`
/// (a control channel with `Pause` then `Abort` pre-queued, so the warmup
/// iteration's `honor_pause` checkpoint unblocks with an aborted error) but
/// with NO `fail_next_write` armed, so ROLLBACK's own `set_active_power_plan`
/// call succeeds against a clean mock -- the exact "abort + clean rollback"
/// combination D8's `else if` branch must now exclude.
#[tokio::test]
async fn execute_never_shuts_down_after_an_operator_abort_even_with_a_successful_rollback() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    write_signatures(dir.path());
    let settings = settings_with(1, 0, 5); // 1 warmup iteration, 0 measure
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let mock = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 8181);
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));

    let mut config = config_with(
        dir.path().to_path_buf(),
        settings,
        dir.path().join("console.log"),
    );
    config.mock_cs2_log = Some(Duration::from_millis(0));
    config.shutdown_when_complete = true;

    let (events_tx, events_rx) = mpsc::channel(64);
    let (control_tx, control_rx) = mpsc::channel(4);
    // Same ordering requirement as the sibling test above: `honor_pause`
    // only ever treats a subsequent `Abort` as meaningful while genuinely
    // paused, so `Pause` must come first.
    control_tx.send(ControlMsg::Pause).await.unwrap();
    control_tx.send(ControlMsg::Abort).await.unwrap();
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    // Matches `config.shutdown_when_complete` -- see `reboot.rs`'s own `run`
    // helper doc comment on why a mismatched initial value here would be
    // misleading (not needed for correctness in this single-shot test, but
    // keeps this test's setup honest about what a real caller would do).
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(true);

    let result = execute(
        sys,
        capture,
        RunStart::Fresh(config),
        events_tx,
        control_rx,
        abort_rx,
        shutdown_toggle_rx,
    )
    .await;
    drop(events_rx);

    let err = result.expect_err("an aborted run must still return Err");
    assert!(
        err.is_aborted(),
        "expected a clean aborted error, got: {err}"
    );
    assert_eq!(
        mock.shutdown_calls(),
        0,
        "an operator Abort must never trigger D8's auto-shutdown, even with \
         shutdown_when_complete true and a successful rollback"
    );
}

/// The success-path counterpart to `execute_never_shuts_down_after_an_operator_
/// abort_even_with_a_successful_rollback` above: that test drives an Abort
/// through `scenario_loop`'s `Err` exit (D8's own `else if` branch in
/// `execute`'s `Err(body_err)` match arm). This one instead lands the run at
/// `Ok(LoopExit::Done)` -- the success path, reaching `finish_run` the way a
/// clean run genuinely does -- because `maybe_break` (the loop's one abort
/// checkpoint) is deliberately never reached after the last scenario, and
/// nothing else in `Stage::Revert`/`Stage::Done`/`finish_run` itself reads
/// `ctx.abort` at all. Hand-builds a `RunProgress` already at `Stage::Done`
/// with one completed baseline result (mirroring exactly what a real run's
/// last `Stage::Revert` -> `Stage::Done` transition saves to disk, per
/// `begin_resume_skips_the_settle_wait_on_the_final_revert_only_resume`'s own
/// doc comment on why `Stage::Done` uniquely identifies that resume) and
/// calls `finish_run` with `abort` already `true`. If `finish_run`'s
/// shutdown condition doesn't also exclude an aborted run, this reaches
/// `sys.shutdown()` despite the abort.
///
/// Calls `finish_run` directly rather than through `execute()` (2026-09-07
/// instant-abort spec): `execute()` now races the whole body against Abort,
/// so an Abort observed *before* `finish_run` starts drops the body and
/// never reaches this guard at all. The case the guard is actually for is an
/// Abort landing *during* `finish_run`'s own scoring/ROLLBACK/REPORT work --
/// `finish_run` engages the `no_return` shield on its first line, so the
/// race defers that Abort and lets `finish_run` run to exactly this check.
/// Reproducing that window through `execute()` would mean arming the abort
/// inside a stretch that emits no event and never yields, so this drives
/// `finish_run` with the same state the shield hands it instead.
#[tokio::test]
async fn finish_run_never_shuts_down_after_an_operator_abort_reaches_the_success_path() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let mock = MockController::new().with_launch_options(&desired_args);
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());

    let config = config_with(
        dir.path().to_path_buf(),
        settings_with(0, 3, 5),
        dir.path().join("console.log"),
    );

    let baseline_result = ScenarioResult {
        scenario_id: "baseline".into(),
        name: "baseline".into(),
        is_baseline: true,
        aggregated: crate::mock_harness::mock_metrics(),
        per_iteration: vec![crate::mock_harness::mock_metrics()],
        metric_deltas: vec![],
        wcps: 0.0,
        verdict: Verdict::ConfirmedSame,
        script_reverted_unverified: false,
    };
    let progress = RunProgress {
        schema_version: crate::model::SCHEMA_VERSION.to_string(),
        run_id: config.run_id.clone(),
        project: config.project.clone(),
        start_build_id: None,
        start_launch_args: String::new(),
        start_launch_args_raw: String::new(),
        start_power_plan: PowerPlan {
            guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            name: "Balanced".into(),
            active: true,
        },
        thermal_baseline: None,
        completed: vec![baseline_result],
        unstable: vec![],
        cursor: Cursor {
            index: 0,
            stage: Stage::Done,
        },
        reboot: Some(PendingReboot {
            reason: RebootReason::RevertOnly,
            scenario_id: "baseline".into(),
            initiated_at: "2026-09-06T00:00:00Z".into(),
            boot_count: 0,
        }),
        shutdown_when_complete: true,
        skip_revert_once: false,
        abort_requested: false,
    };

    let (events_tx, events_rx) = mpsc::channel(64);
    // `true` for this whole call, not toggled via an armer -- the point is
    // that NOTHING inside `finish_run` up to its shutdown decision reads
    // this receiver, so its value on entry is exactly as dangerous as a
    // value flipped moments before that check.
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(true);

    let ctx = RunContext {
        config: &config,
        start_build_id: progress.start_build_id.clone(),
        start_launch_args: progress.start_launch_args.clone(),
        start_launch_args_raw: progress.start_launch_args_raw.clone(),
        start_power_plan: progress.start_power_plan.clone(),
        abort: abort_rx,
        // Already engaged, exactly as the real `finish_run` sets it on its
        // own first line -- see this test's doc comment.
        no_return: tokio::sync::watch::channel(true).0,
    };
    let mut progress = progress;
    let result = finish_run(sys.as_ref(), &ctx, &mut progress, false, vec![], &events_tx).await;
    drop(events_rx);

    assert!(
        matches!(result.unwrap(), RunOutcome::Complete(_)),
        "an aborted run reaching finish_run's success path must still complete normally, not \
         shut down"
    );
    assert_eq!(
        mock.shutdown_calls(),
        0,
        "an operator Abort observed anywhere up through finish_run's own work must never let \
         the success path's D3 auto-shutdown fire, even with shutdown_when_complete true"
    );
}

/// Regression test for Important 3 in
/// docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes/final-fix-brief.md: the best-effort
/// launch-options restore must still run even when ROLLBACK's own
/// power-plan restore fails first (it used to sit after that `?`, so a
/// failure there skipped the restore entirely). Drives a full `execute()`
/// whose only scenario carries a `Module::LaunchArgs` (so ROLLBACK's own
/// launch-options restore has real work to do), arming `fail_next_write` --
/// a one-shot flag shared by `set_active_power_plan`/`write_registry`/
/// `write_powercfg`/`launch_cs2`, but NOT `write_cs2_launch_options` -- only
/// once both `launch_cs2` calls this run makes (baseline's own, then the
/// scenario's) have already happened, mirroring the `armer` technique
/// above. That leaves it fresh for ROLLBACK's own `set_active_power_plan`
/// call, the only other mutating call this run makes.
#[tokio::test]
async fn rollback_still_restores_launch_options_when_the_power_plan_restore_fails() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 3, 5);
    let start_args = crate::cs2::keybind_cfg::reconcile("");
    let mock = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 9701)
        .with_process_on_deelevate("steam.exe", 9702)
        .with_process("steamwebhelper.exe", 9799)
        .with_process_on_launch("cs2.exe", 9800)
        .with_launch_options(&start_args);
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![
        crate::mock_harness::mock_metrics(),
    ]));

    let mut config = config_with(
        dir.path().to_path_buf(),
        settings,
        dir.path().join("console.log"),
    );
    config.mock_cs2_log = Some(Duration::from_millis(0));
    config.project.scenarios.push(Scenario {
        id: "sc-launchargs-rollback-power-fail".into(),
        name: "LaunchArgsRollbackPowerFail".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![Module::LaunchArgs {
            args: "-novid -high".into(),
        }],
    });

    let (events_tx, events_rx) = mpsc::channel(256);
    let (_control_tx, control_rx) = mpsc::channel(4);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

    let exec = execute(
        sys,
        capture,
        RunStart::Fresh(config),
        events_tx,
        control_rx,
        abort_rx,
        shutdown_toggle_rx,
    );
    // Polls via a short real `sleep` rather than `yield_now` in a bare loop:
    // unlike the `execute_returns_a_non_aborted_error_when_rollback_also_fails_after_an_abort`
    // armer above (whose single `launch_cs2_calls() == 0` condition is
    // already satisfied before any timer-driven wait happens), this one
    // must survive real elapsed virtual time (the whole scenario lifecycle
    // between baseline's and the scenario's own `launch_cs2` calls) --
    // a `yield_now`-only loop stays perpetually ready and starves
    // `tokio::time::pause()`'s auto-advance, which only fires once the
    // runtime's ready queue is genuinely empty.
    let armer = async {
        for _ in 0..100_000u32 {
            if mock.launch_cs2_calls() >= 2 {
                mock.fail_next_write("disk full");
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("armer never saw both launch_cs2 calls");
    };
    let (result, ()) = tokio::join!(exec, armer);
    drop(events_rx);

    let err = result.expect_err("ROLLBACK's power-plan restore was armed to fail");
    assert!(
        err.to_string().contains("disk full"),
        "expected the power-plan restore's own failure to surface, got: {err}"
    );
    assert_eq!(
        mock.read_cs2_launch_options().await.unwrap(),
        start_args,
        "the launch-options restore must still run and succeed even though the power-plan \
         restore failed first"
    );
}

#[tokio::test]
async fn resolve_real_video_txt_path_returns_the_controllers_seeded_path() {
    let mock = MockController::new()
        .with_cs2_video_config_path(r"D:\Steam\userdata\1\730\local\cfg\cs2_video.txt");
    let sys: Arc<dyn SystemController> = Arc::new(mock);

    let path = resolve_real_video_txt_path(sys.as_ref()).await;

    assert_eq!(
        path,
        Some(PathBuf::from(
            r"D:\Steam\userdata\1\730\local\cfg\cs2_video.txt"
        )),
        "must resolve through SystemController::find_cs2_video_config_path, not a direct \
         system::windows reach-around"
    );
}

/// Spec section 6 / D6: a scenario whose modules include `custom_script`
/// gets its result flagged `script_reverted_unverified` once a real revert
/// completes, because VOIDFRAME ran the revert script but cannot verify what
/// it actually did to the system. Drives a full `execute()` run (not a
/// direct call into the flag-setting logic) so this exercises the real
/// `Stage::Measure` -> `Stage::Revert` sequencing: `scenario_result` builds
/// the `ScenarioResult` during `Stage::Measure`, before the scenario's own
/// revert has run at all, so the flag can only be patched in afterward.
#[tokio::test]
async fn a_custom_script_scenarios_result_is_marked_script_reverted_unverified_after_a_real_revert()
{
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    write_signatures(dir.path());
    let scripts_dir = dir.path().join("projects").join("proj1").join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();
    std::fs::write(scripts_dir.join("apply.bat"), "echo apply").unwrap();
    std::fs::write(scripts_dir.join("revert.bat"), "echo revert").unwrap();

    let settings = settings_with(0, 3, 5);
    let start_args = crate::cs2::keybind_cfg::reconcile("");
    let mock = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 9701)
        .with_process_on_deelevate("steam.exe", 9702)
        .with_process("steamwebhelper.exe", 9799)
        .with_process_on_launch("cs2.exe", 9800)
        .with_launch_options(&start_args);
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![
        crate::mock_harness::mock_metrics(),
    ]));

    let mut config = config_with(
        dir.path().to_path_buf(),
        settings,
        dir.path().join("console.log"),
    );
    config.project.id = "proj1".into();
    config.mock_cs2_log = Some(Duration::from_millis(0));
    config.project.scenarios.push(Scenario {
        id: "sc-custom-script".into(),
        name: "CustomScript".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![Module::CustomScript(
            crate::model::module::CustomScriptPayload {
                apply_script: "apply.bat".into(),
                revert_script: "revert.bat".into(),
                requires_reboot: false,
                description: "d".into(),
            },
        )],
    });

    let (events_tx, events_rx) = mpsc::channel(256);
    let (_control_tx, control_rx) = mpsc::channel(4);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

    let outcome = execute(
        sys,
        capture,
        RunStart::Fresh(config),
        events_tx,
        control_rx,
        abort_rx,
        shutdown_toggle_rx,
    )
    .await
    .unwrap();
    drop(events_rx);

    let result = match outcome {
        RunOutcome::Complete(r) => r,
        other => panic!("expected RunOutcome::Complete, got {other:?}"),
    };

    let scenario_result = result
        .scenarios
        .iter()
        .find(|r| r.scenario_id == "sc-custom-script")
        .expect("scenario result should exist");
    assert!(scenario_result.script_reverted_unverified);

    // The baseline scenario has no custom_script module -- must stay false.
    assert!(!result.baseline.script_reverted_unverified);
}
