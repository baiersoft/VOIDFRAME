//! The whole-body abort race (2026-09-07 instant-abort spec): an operator
//! Abort observed on the watch must cut ANY in-flight wait within one poll,
//! with cleanup running from persisted state afterward.
//!
//! Real (unpaused) time throughout, deliberately: `tokio::time::pause()`
//! would make "the wait was cut rather than waited out" unfalsifiable, since
//! a paused clock skips the wait either way.

use super::super::{RunConfig, RunOutcome, RunStart, execute, finalize_aborted_run};
use super::write_signatures;
use crate::capture::runner::{CaptureRunner, MockCaptureRunner};
use crate::mock_harness;
use crate::model::progress::{Cursor, PendingReboot, RunProgress, Stage};
use crate::model::results::{ScenarioResult, Verdict};
use crate::run::phase::RebootReason;
use crate::run::{EngineEvent, Phase};
use crate::system::{PowerPlan, SystemController};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};

fn config_with(dir: &std::path::Path, thermal_override: Option<(Duration, u32)>) -> RunConfig {
    write_signatures(dir);
    RunConfig {
        project: serde_json::from_value(serde_json::json!({
            "schema_version": crate::model::SCHEMA_VERSION, "id": "p", "name": "P",
            "description": "d", "created_at": "2026-09-07T00:00:00Z", "settings": {},
            "baseline": {"name": "Stock", "description": "d"}, "scenarios": []
        }))
        .unwrap(),
        run_id: "r-abort".into(),
        dry_run: false,
        data_root: dir.to_path_buf(),
        webview_root_pid: None,
        console_log_override: None,
        mock_cs2_log: Some(Duration::from_millis(50)),
        thermal_sample_override: thermal_override,
        inter_scenario_break_seconds: 0,
        hwinfo_path: Some(PathBuf::from("mock-hwinfo.exe")),
        shutdown_when_complete: false,
        post_boot_settle: Duration::from_secs(0),
        exe_path: PathBuf::from("voidframe.exe"),
    }
}

/// The reported bug, as a regression test: the 30s (here: 2h) thermal
/// baseline must not delay an Abort. Real (unpaused) time on purpose -- the
/// whole point is that the wait is cut, not waited out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_cuts_the_thermal_baseline_instead_of_waiting_it_out() {
    let dir = tempfile::tempdir().unwrap();
    let mock = Arc::new(mock_harness::mock_ready_controller());
    let sys: Arc<dyn SystemController> = mock.clone();
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));
    // 2 samples, 2h apart: the baseline pends effectively forever.
    let config = config_with(dir.path(), Some((Duration::from_secs(7200), 2)));

    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, control_rx) = mpsc::channel(16);
    let (abort_tx, abort_rx) = watch::channel(false);
    let (_toggle_tx, toggle_rx) = watch::channel(false);
    let handle = tokio::spawn(execute(
        sys,
        capture,
        RunStart::Fresh(config),
        events_tx,
        control_rx,
        abort_rx,
        toggle_rx,
    ));

    // Wait until the run is genuinely inside the thermal baseline.
    loop {
        if let EngineEvent::PhaseChanged {
            phase: Phase::ThermalBaseline,
        } = events_rx
            .recv()
            .await
            .expect("run ended before ThermalBaseline")
        {
            break;
        }
    }
    abort_tx.send(true).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("abort must cut the baseline within seconds, not hours")
        .unwrap();
    assert!(result.unwrap_err().is_aborted());

    let mut saw_aborting = false;
    while let Ok(ev) = events_rx.try_recv() {
        if matches!(
            ev,
            EngineEvent::PhaseChanged {
                phase: Phase::Aborting
            }
        ) {
            saw_aborting = true;
        }
    }
    assert!(
        saw_aborting,
        "finalize must announce Phase::Aborting immediately"
    );
    assert_eq!(
        mock.close_hwinfo_calls(),
        1,
        "finalize must close the HWiNFO instance the dropped body started"
    );
}

/// Abort after `progress.json` exists must run the full from-disk cleanup:
/// `Phase::Aborting`, then the Rollback phase `handle_body_failure` drives
/// off the reloaded progress.
///
/// Aborts inside `wait_for_process`'s CS2-discovery poll (up to 60s),
/// deliberately: since Task 6 removed the old per-wait `abortable` wrappers,
/// the whole-body race is now the *only* thing that can cut any wait in the
/// run body, this one included -- picked here mainly because it makes for a
/// direct, deterministic test of that race.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_mid_scenario_runs_rollback_from_persisted_state() {
    let dir = tempfile::tempdir().unwrap();
    let mock = crate::system::MockController::new()
        .with_launch_options(&crate::cs2::keybind_cfg::reconcile(""))
        // cs2.exe never becomes discoverable, so the run parks in
        // `wait_for_process`'s poll loop until something cuts it.
        .with_process_visible_after("cs2.exe", 999_999, u32::MAX);
    let sys: Arc<dyn SystemController> = Arc::new(mock);
    let capture: Arc<dyn CaptureRunner> =
        Arc::new(MockCaptureRunner::new(vec![mock_harness::mock_metrics()]));
    // No hwinfo: skip the thermal baseline entirely so the run reaches the
    // baseline scenario fast.
    let mut config = config_with(dir.path(), None);
    config.hwinfo_path = None;

    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, control_rx) = mpsc::channel(16);
    let (abort_tx, abort_rx) = watch::channel(false);
    let (_toggle_tx, toggle_rx) = watch::channel(false);
    let handle = tokio::spawn(execute(
        sys,
        capture,
        RunStart::Fresh(config),
        events_tx,
        control_rx,
        abort_rx,
        toggle_rx,
    ));

    loop {
        if let EngineEvent::LogLine { text } = events_rx
            .recv()
            .await
            .expect("run ended before CS2 was launched")
            && text.starts_with("Launching CS2")
        {
            break;
        }
    }
    abort_tx.send(true).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("abort must cut the CS2-discovery wait promptly, not wait out its 60s budget")
        .unwrap();
    assert!(result.unwrap_err().is_aborted());
    assert!(
        crate::model::progress::RunProgress::load(&dir.path().join("runs").join("r-abort"))
            .unwrap()
            .abort_requested,
        "the from-disk finalize must persist abort_requested, the same durability guard \
         scenario_loop's own pre-reboot checks write"
    );

    let (mut saw_aborting, mut saw_rollback) = (false, false);
    while let Ok(ev) = events_rx.try_recv() {
        match ev {
            EngineEvent::PhaseChanged {
                phase: Phase::Aborting,
            } => saw_aborting = true,
            EngineEvent::PhaseChanged {
                phase: Phase::Rollback,
            } => saw_rollback = true,
            _ => {}
        }
    }
    assert!(
        saw_aborting && saw_rollback,
        "aborting={saw_aborting} rollback={saw_rollback}"
    );
}

/// Fix 1, driven through the FULL `execute()` race: once `scenario_loop`
/// reaches `Stage::Done` the run is genuinely over and an Abort is
/// meaningless -- the body must reach and complete `finish_run` regardless
/// of an already-pending Abort, and the run must end `Complete`, not as an
/// aborted `Err`.
///
/// Its sibling `finish_run_never_shuts_down_after_an_operator_abort_reaches_
/// the_success_path` (in this module's `mod.rs`) calls `finish_run` directly
/// because driving this through `execute()` used to be "a coin flip": the
/// `select!` polled its two branches in a randomized order, so on the wake
/// where the body would otherwise flow synchronously from
/// `LoopExit::Done` into `finish_run`'s own `no_return.send(true)`, the
/// abort arm could be polled first, see `no_return` still `false`, and drop
/// the body. Both halves of the fix close that: `Stage::Done` now engages
/// the shield as its own first action, and `biased;` makes the body always
/// win a wake it can make forward progress on.
///
/// Construction: a `RunStart::Resume` whose cursor is already at
/// `Stage::Done` with no `results.json` on disk -- so `scenario_loop`'s very
/// first loop iteration takes the `Stage::Done` arm -- with the abort watch
/// already `true` before `execute()` is ever polled. Every await between
/// `execute()`'s first poll and `finish_run`'s ROLLBACK is a `MockController`
/// call or a buffered channel send (all synchronously `Ready`), so the body
/// genuinely does reach the shield within its very first poll; the first
/// `Pending` it hits (`sweep_journals`' `tokio::fs::read_dir`) is already
/// past it.
///
/// That last sentence is a real dependency, stated so a future reader does
/// not misdiagnose it: this test assumes everything from `execute()`'s first
/// poll through the `Stage::Done` arm is synchronously `Ready` against
/// `MockController`. A change that introduces a genuine yield point upstream
/// would make it fail for a reason unrelated to any real regression -- that
/// is expected and acceptable, and the fix is to re-derive the construction,
/// never to weaken the test.
#[tokio::test]
async fn a_pending_abort_never_steals_the_run_from_stage_done() {
    let dir = tempfile::tempdir().unwrap();
    let mock = crate::system::MockController::new()
        .with_launch_options(&crate::cs2::keybind_cfg::reconcile(""));
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));
    let mut config = config_with(dir.path(), None);
    config.hwinfo_path = None;

    let baseline_result = ScenarioResult {
        scenario_id: "baseline".into(),
        name: "Stock".into(),
        is_baseline: true,
        aggregated: mock_harness::mock_metrics(),
        per_iteration: vec![mock_harness::mock_metrics(); 3],
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
        // Matches the mock's live value, so ROLLBACK's best-effort
        // launch-options restore short-circuits instead of driving the real
        // close-Steam/rewrite/relaunch dance (which, against a
        // fixed-`steam_status` mock, burns its whole 60s
        // `wait_until_steam_closed` budget for nothing this test is about).
        start_launch_args_raw: crate::cs2::keybind_cfg::reconcile(""),
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
            initiated_at: "2026-09-07T00:00:00Z".into(),
            boot_count: 0,
        }),
        shutdown_when_complete: false,
        skip_revert_once: false,
        abort_requested: false,
    };

    let (events_tx, mut events_rx) = mpsc::channel(256);
    let (_control_tx, control_rx) = mpsc::channel(16);
    // Already aborted before `execute()` is polled even once.
    let (_abort_tx, abort_rx) = watch::channel(true);
    let (_toggle_tx, toggle_rx) = watch::channel(false);

    let outcome = execute(
        sys,
        capture,
        RunStart::Resume { config, progress },
        events_tx,
        control_rx,
        abort_rx,
        toggle_rx,
    )
    .await
    .expect("a run that reached Stage::Done must finish, not come back as an aborted Err");
    assert!(
        matches!(outcome, RunOutcome::Complete(_)),
        "expected RunOutcome::Complete, got {outcome:?}"
    );

    // The Abort was genuinely live and genuinely raced -- it was *deferred*
    // by the shield, not simply never observed. Without this the assertion
    // above could pass for the wrong reason.
    let mut saw_deferral = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.contains("past the point of no return")
        {
            saw_deferral = true;
        }
    }
    assert!(
        saw_deferral,
        "the abort arm must have observed the pending Abort and deferred it behind the \
         Stage::Done shield"
    );
}

/// Fix 2, end to end: a scenario whose modules were FULLY applied and
/// confirmed before an Abort landed must still have those mutations taken
/// back off the machine.
///
/// `sweep_journals` alone cannot do this -- it only reverts journals holding
/// at least one *unconfirmed* record, and a scenario that applied cleanly
/// has none, so its journal was skipped entirely and its powercfg/registry/
/// power-plan writes stayed live. `rollback` now force-reverts the journal
/// of whichever scenario `progress.cursor` says was in flight
/// (`Stage::Apply` or `Stage::Measure`, i.e. strictly before its own
/// `revert_stage` could have run).
///
/// Enters via `RunStart::Resume` with the cursor already on the scenario
/// (`ordered_scenarios` prepends the baseline dummy, so the one enabled
/// scenario is index 1) at `Stage::Apply`, so the run does its real
/// `apply_stage` for that scenario and nothing else first. Driving a
/// baseline through this file's real (unpaused) clock instead would cost a
/// literal 3 x Dust2's 8s `settle_before_capture` = 24 s for coverage this
/// test does not need (the per-kind settle lives in
/// `run/execute/scenario.rs`; an AveYo config would make that 15 s).
///
/// The abort is armed on the scenario's own "Launching CS2" line -- by then
/// `apply_stage` has already returned, so its powercfg write is journaled
/// AND confirmed, which is exactly the case `sweep_journals` skips. The
/// pre-abort sanity assertion below is what keeps the final assertion from
/// being vacuous, and the forced-revert LogLine assertion is what proves the
/// value came back via the new path rather than an ordinary `revert_stage`.
///
/// Coverage note: the on-disk cursor this exercises end to end is
/// `Stage::Apply` only, despite `in_flight_journal` covering `Stage::Measure`
/// too. `RunStart::Resume` reads `progress.json`, and the `Apply -> Measure`
/// step is an in-memory `progress.cursor.stage` assignment `scenario_loop`
/// never saves (the next save is at the end of `Stage::Measure` itself), so
/// a resumed run always lands on `Apply`. `Stage::Measure` being in flight
/// is covered by the unit test `rollback::tests::stage_measure_is_in_flight`
/// instead -- do not read this test as end-to-end proof of both stages.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_reverts_a_fully_applied_in_flight_scenario() {
    const SUB: &str = "sub_processor";
    const SETTING: &str = "IDLEDISABLE";

    let dir = tempfile::tempdir().unwrap();
    let mock = Arc::new(
        mock_harness::mock_ready_controller()
            .with_graceful_quit_working()
            .with_powercfg(SUB, SETTING, crate::system::AcDc { ac: 0, dc: 0 }),
    );
    let sys: Arc<dyn SystemController> = mock.clone();
    let capture: Arc<dyn CaptureRunner> =
        Arc::new(MockCaptureRunner::new(vec![mock_harness::mock_metrics()]));
    let mut config = config_with(dir.path(), None);
    config.hwinfo_path = None;
    config.mock_cs2_log = Some(Duration::from_millis(50));
    config.project.settings.warmup_loops = 0;
    config
        .project
        .scenarios
        .push(crate::model::project::Scenario {
            id: "sc-powercfg".into(),
            name: "Powercfg".into(),
            description: "d".into(),
            enabled: true,
            modules: vec![crate::model::Module::Powercfg {
                sub: SUB.into(),
                setting: SETTING.into(),
                value: 1,
            }],
        });

    let baseline_result = ScenarioResult {
        scenario_id: "baseline".into(),
        name: "Stock".into(),
        is_baseline: true,
        aggregated: mock_harness::mock_metrics(),
        per_iteration: vec![mock_harness::mock_metrics(); 3],
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
        // Matches the mock's live value -- see the sibling Stage::Done test.
        start_launch_args_raw: crate::cs2::keybind_cfg::reconcile(""),
        start_power_plan: PowerPlan {
            guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            name: "Balanced".into(),
            active: true,
        },
        thermal_baseline: None,
        completed: vec![baseline_result],
        unstable: vec![],
        cursor: Cursor {
            index: 1,
            stage: Stage::Apply,
        },
        reboot: Some(PendingReboot {
            reason: RebootReason::ApplyNext,
            scenario_id: "sc-powercfg".into(),
            initiated_at: "2026-09-07T00:00:00Z".into(),
            boot_count: 0,
        }),
        shutdown_when_complete: false,
        skip_revert_once: false,
        abort_requested: false,
    };

    let (events_tx, mut events_rx) = mpsc::channel(512);
    let (_control_tx, control_rx) = mpsc::channel(16);
    let (abort_tx, abort_rx) = watch::channel(false);
    let (_toggle_tx, toggle_rx) = watch::channel(false);
    let handle = tokio::spawn(execute(
        sys,
        capture,
        RunStart::Resume { config, progress },
        events_tx,
        control_rx,
        abort_rx,
        toggle_rx,
    ));

    loop {
        if let EngineEvent::LogLine { text } = events_rx
            .recv()
            .await
            .expect("run ended before the scenario launched CS2")
            && text.starts_with("Launching CS2 for scenario 'Powercfg'")
        {
            break;
        }
    }
    assert_eq!(
        mock.read_powercfg(SUB, SETTING).await.unwrap().ac,
        1,
        "sanity: the scenario's mutation must genuinely be live when the abort lands, or the \
         assertion at the end of this test is vacuous"
    );
    abort_tx.send(true).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(20), handle)
        .await
        .expect("abort must cut the scenario's measurement promptly")
        .unwrap();
    assert!(result.unwrap_err().is_aborted());

    assert_eq!(
        mock.read_powercfg(SUB, SETTING).await.unwrap().ac,
        0,
        "an aborted in-flight scenario's fully-applied mutations must be reverted, not left \
         live on the machine"
    );
    let mut saw_forced_revert = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.contains("never reverted its own modules")
        {
            saw_forced_revert = true;
        }
    }
    assert!(
        saw_forced_revert,
        "the value must have come back via ROLLBACK's forced revert of the in-flight scenario, \
         not via an ordinary revert_stage that happened to win the race"
    );
}

/// Fix C1, end to end: `Transition::ApplyNextThenReboot` is the one place in
/// `scenario_loop` that applies a scenario OTHER than the one
/// `progress.cursor.index` names -- it fully applies and confirms scenario
/// `i + 1`'s modules while the cursor still reads `{i, Stage::Revert}`, then
/// checks `ctx.abort` before handing off to `reboot_sequence`. Until the
/// cursor was advanced to `{i + 1, Stage::Measure}` *before* that check,
/// `rollback`'s `in_flight_journal` (correctly, for a real `Stage::Revert`)
/// returned `None` and scenario `i + 1`'s journal -- every record confirmed,
/// so invisible to `sweep_journals` too -- was skipped entirely, leaving its
/// registry write live on the machine with nothing left in the run to take
/// it back off.
///
/// Construction: a `RunStart::Resume` parked at `{1, Stage::Revert}` over a
/// two-scenario project whose SECOND scenario is a reboot scenario, with the
/// abort watch already `true` before `execute()` is ever polled. Scenario 1
/// has no modules at all, so its own `revert_stage` is a no-op over a journal
/// that was never created, and the loop flows straight into
/// `ApplyNextThenReboot` -> `apply_stage(sc-hags)` -> the abort check.
///
/// Like its `Stage::Done` sibling above, this depends on every await from
/// `execute()`'s first poll through that abort check being synchronously
/// `Ready` against `MockController` (no CS2 launch or measurement is on this
/// path), so the biased `select!`'s body arm never yields to the abort arm
/// and the Abort is observed by the loop's own check rather than by dropping
/// the body. A future change that introduces a genuine yield point in that
/// stretch would make this test fail for a reason unrelated to a real
/// regression; that is expected, not something to "fix" by weakening it.
#[tokio::test]
async fn abort_reverts_the_next_scenario_apply_next_then_reboot_already_applied() {
    const HAGS_SUBKEY: &str = "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers";
    let hags_key = crate::system::RegKey {
        hive: crate::model::module::Hive::Hklm,
        subkey: HAGS_SUBKEY.into(),
        value_name: "HwSchMode".into(),
    };

    let dir = tempfile::tempdir().unwrap();
    let mock = crate::system::MockController::new()
        .with_launch_options(&crate::cs2::keybind_cfg::reconcile(""))
        .with_registry(&hags_key, crate::system::RegValue::Dword(1));
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));
    let mut config = config_with(dir.path(), None);
    config.hwinfo_path = None;
    config.project.scenarios = vec![
        crate::model::project::Scenario {
            id: "sc-plain".into(),
            name: "Plain".into(),
            description: "d".into(),
            enabled: true,
            modules: vec![],
        },
        crate::model::project::Scenario {
            id: "sc-hags".into(),
            name: "HAGS".into(),
            description: "d".into(),
            enabled: true,
            modules: vec![crate::model::Module::Registry(
                crate::model::module::RegistryPayload {
                    hive: crate::model::module::Hive::Hklm,
                    subkey: HAGS_SUBKEY.into(),
                    value_name: "HwSchMode".into(),
                    value_type: crate::model::module::RegType::Dword,
                    value: serde_json::json!(2),
                    requires_reboot: true,
                },
            )],
        },
    ];

    let baseline_result = ScenarioResult {
        scenario_id: "baseline".into(),
        name: "Stock".into(),
        is_baseline: true,
        aggregated: mock_harness::mock_metrics(),
        per_iteration: vec![mock_harness::mock_metrics(); 3],
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
        // Matches the mock's live value -- see the sibling Stage::Done test.
        start_launch_args_raw: crate::cs2::keybind_cfg::reconcile(""),
        start_power_plan: PowerPlan {
            guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            name: "Balanced".into(),
            active: true,
        },
        thermal_baseline: None,
        completed: vec![baseline_result],
        unstable: vec![],
        cursor: Cursor {
            index: 1,
            stage: Stage::Revert,
        },
        reboot: Some(PendingReboot {
            reason: RebootReason::ApplyNext,
            scenario_id: "sc-plain".into(),
            initiated_at: "2026-09-07T00:00:00Z".into(),
            boot_count: 0,
        }),
        shutdown_when_complete: false,
        skip_revert_once: false,
        abort_requested: false,
    };

    let (events_tx, mut events_rx) = mpsc::channel(512);
    let (_control_tx, control_rx) = mpsc::channel(16);
    // Already aborted before `execute()` is polled even once.
    let (_abort_tx, abort_rx) = watch::channel(true);
    let (_toggle_tx, toggle_rx) = watch::channel(false);

    let err = execute(
        sys,
        capture,
        RunStart::Resume { config, progress },
        events_tx,
        control_rx,
        abort_rx,
        toggle_rx,
    )
    .await
    .expect_err("the abort observed before the reboot must stop the run");
    assert!(err.is_aborted(), "{err}");

    assert_eq!(
        mock.read_registry(&hags_key).await.unwrap(),
        crate::system::RegValue::Dword(1),
        "the next scenario that ApplyNextThenReboot already applied must be reverted at \
         ROLLBACK, not left live on the machine"
    );
    let mut saw_forced_revert = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.contains("never reverted its own modules")
        {
            saw_forced_revert = true;
        }
    }
    assert!(
        saw_forced_revert,
        "the value must have come back via ROLLBACK's forced revert of the in-flight scenario \
         (the cursor having been advanced to it), not via the unconfirmed-record sweep"
    );
}

/// The §2.2 "quiet window" suspends the app's own WebView2 process tree
/// around each capture. `run_one_iteration` resumes it on every in-body path
/// including the failing ones, but a body dropped by the abort race never
/// reaches that resume -- which would leave the operator's own UI frozen
/// with nothing short of Task Manager to recover it. `finalize_aborted_run`
/// therefore resumes it too.
///
/// Driven directly rather than through `execute()` to isolate
/// `finalize_aborted_run`'s own resume logic from the rest of a real run.
/// The end-to-end test below asserts the same user-visible property (nothing
/// is left suspended) via a full `execute()` run instead; this one pins the
/// new code path itself.
#[tokio::test]
async fn finalize_resumes_a_webview_tree_the_dropped_body_left_suspended() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config_with(dir.path(), None);
    config.webview_root_pid = Some(4242);
    let mock = crate::system::MockController::new();

    mock.suspend_process_tree(4242).await.unwrap();
    assert_eq!(
        mock.suspended_pids(),
        vec![4242],
        "sanity: the quiet window's suspend is what this cleanup has to undo"
    );

    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_abort_tx, abort_rx) = watch::channel(true);
    let (_toggle_tx, toggle_rx) = watch::channel(false);
    // No progress.json on disk: proves the resume is not gated on the
    // from-disk branch -- the webview can be suspended before a run ever
    // saves progress.
    let err = finalize_aborted_run(
        &mock,
        &config,
        abort_rx,
        &toggle_rx,
        &events_tx,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .await
    .unwrap_err();

    assert!(err.is_aborted(), "{err}");
    assert!(
        mock.suspended_pids().is_empty(),
        "abort cleanup must resume the WebView2 tree, or the app's own UI stays frozen"
    );
}

/// End-to-end counterpart: an Abort landing inside the capture window --
/// while the WebView2 tree is genuinely suspended by the run itself -- must
/// never leave it suspended, whichever cleanup path handles the abort.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_inside_the_capture_window_never_leaves_the_webview_suspended() {
    let dir = tempfile::tempdir().unwrap();
    // `with_graceful_quit_working`: CS2 *is* running when the abort lands
    // here, so `finalize_aborted_run` reaches `graceful_kill_cs2`. Without
    // this the mock's cs2.exe never goes away and that call burns its full
    // 10s `GRACEFUL_QUIT_TIMEOUT` poll -- a real code path, but not the one
    // under test, and it would make this test slow and its timeout margin
    // thin for no added coverage.
    let mock = Arc::new(mock_harness::mock_ready_controller().with_graceful_quit_working());
    let sys: Arc<dyn SystemController> = mock.clone();
    let capture: Arc<dyn CaptureRunner> =
        Arc::new(MockCaptureRunner::new(vec![mock_harness::mock_metrics()]));
    let mut config = config_with(dir.path(), None);
    config.hwinfo_path = None;
    config.webview_root_pid = Some(4242);

    let (events_tx, mut events_rx) = mpsc::channel(256);
    let (_control_tx, control_rx) = mpsc::channel(16);
    let (abort_tx, abort_rx) = watch::channel(false);
    let (_toggle_tx, toggle_rx) = watch::channel(false);
    let handle = tokio::spawn(execute(
        sys,
        capture,
        RunStart::Fresh(config),
        events_tx,
        control_rx,
        abort_rx,
        toggle_rx,
    ));

    // `CapturePending` is emitted immediately before the suspend, and only
    // for a measure iteration (warmup iterations run with no capture at all,
    // so nothing is suspended during them).
    loop {
        if let EngineEvent::CapturePending = events_rx
            .recv()
            .await
            .expect("run ended before the capture window opened")
        {
            break;
        }
    }
    // The suspend is the very next await after that event; give it the one
    // poll it needs before checking. Bounded so a regression that never
    // suspends fails here with a readable message instead of busy-spinning a
    // CI runner forever (real time in this file, so this is a real 10s).
    tokio::time::timeout(Duration::from_secs(10), async {
        while mock.suspended_pids().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the run must reach the capture window and suspend the webview tree");
    assert_eq!(
        mock.suspended_pids(),
        vec![4242],
        "sanity: the abort below must land while the tree is genuinely suspended, or the \
         assertion at the end of this test is vacuous"
    );
    abort_tx.send(true).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(20), handle)
        .await
        .expect("abort must cut the pre-capture settle promptly")
        .unwrap();
    assert!(result.unwrap_err().is_aborted());
    assert!(
        mock.suspended_pids().is_empty(),
        "no path out of an aborted run may leave the WebView2 tree suspended, got {:?}",
        mock.suspended_pids()
    );
}

/// C1, widened (2026-09-10 review): the in-memory cursor catch-up above only
/// covers an Abort *observed by the loop's own check* after `apply_stage`
/// returns. An Abort that drops the body while `apply_stage(next_scenario)`
/// is still running -- here, parked inside the second module's
/// `read_registry`, after the first module was journaled, written and
/// confirmed -- reaches `finalize_aborted_run`, which reloads the cursor
/// FROM DISK. Until the cursor was persisted as `{i + 1, Stage::Apply}`
/// before the apply began, disk still read `{i, Stage::Revert}`: not in
/// flight for `rollback`, and invisible to `sweep_journals` because the one
/// record in the journal is confirmed -- so the first module's registry
/// write stayed live after an "aborted, rolled back" run.
///
/// Real time and a multi-thread runtime on purpose: the armer polls the
/// mock until the first module's write has landed, then flips `abort` while
/// the body is asleep in the second module's delayed `read_registry`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn abort_dropped_mid_apply_next_then_reboot_still_reverts_the_confirmed_module() {
    const HAGS_SUBKEY: &str = r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers";
    let reg = |value_name: &str| crate::system::RegKey {
        hive: crate::model::module::Hive::Hklm,
        subkey: HAGS_SUBKEY.into(),
        value_name: value_name.into(),
    };
    let module = |value_name: &str| {
        crate::model::Module::Registry(crate::model::module::RegistryPayload {
            hive: crate::model::module::Hive::Hklm,
            subkey: HAGS_SUBKEY.into(),
            value_name: value_name.into(),
            value_type: crate::model::module::RegType::Dword,
            value: serde_json::json!(2),
            requires_reboot: true,
        })
    };

    let dir = tempfile::tempdir().unwrap();
    let mock = crate::system::MockController::new()
        .with_launch_options(&crate::cs2::keybind_cfg::reconcile(""))
        .with_registry(&reg("HwSchMode"), crate::system::RegValue::Dword(1))
        .with_registry(&reg("TdrLevel"), crate::system::RegValue::Dword(1))
        .with_read_registry_delay(Duration::from_millis(300));
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));
    let mut config = config_with(dir.path(), None);
    config.hwinfo_path = None;
    config.project.scenarios = vec![
        crate::model::project::Scenario {
            id: "sc-plain".into(),
            name: "Plain".into(),
            description: "d".into(),
            enabled: true,
            modules: vec![],
        },
        crate::model::project::Scenario {
            id: "sc-hags".into(),
            name: "HAGS".into(),
            description: "d".into(),
            enabled: true,
            modules: vec![module("HwSchMode"), module("TdrLevel")],
        },
    ];
    let run_dir = dir.path().join("runs").join(&config.run_id);

    let baseline_result = ScenarioResult {
        scenario_id: "baseline".into(),
        name: "Stock".into(),
        is_baseline: true,
        aggregated: mock_harness::mock_metrics(),
        per_iteration: vec![mock_harness::mock_metrics(); 3],
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
        start_launch_args_raw: crate::cs2::keybind_cfg::reconcile(""),
        start_power_plan: PowerPlan {
            guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            name: "Balanced".into(),
            active: true,
        },
        thermal_baseline: None,
        completed: vec![baseline_result],
        unstable: vec![],
        cursor: Cursor {
            index: 1,
            stage: Stage::Revert,
        },
        reboot: Some(PendingReboot {
            reason: RebootReason::ApplyNext,
            scenario_id: "sc-plain".into(),
            initiated_at: "2026-09-10T00:00:00Z".into(),
            boot_count: 0,
        }),
        shutdown_when_complete: false,
        skip_revert_once: false,
        abort_requested: false,
    };
    progress.save(&run_dir).unwrap();

    let (events_tx, mut events_rx) = mpsc::channel(512);
    let (_control_tx, control_rx) = mpsc::channel(16);
    let (abort_tx, abort_rx) = watch::channel(false);
    let (_toggle_tx, toggle_rx) = watch::channel(false);

    let exec = execute(
        sys,
        capture,
        RunStart::Resume { config, progress },
        events_tx,
        control_rx,
        abort_rx,
        toggle_rx,
    );
    let armer = async {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        // `snapshot()`, not `read_registry`: the latter carries the same
        // artificial delay the run is being parked on, and polling through
        // it would let the run finish before this armer ever fires.
        while !mock
            .snapshot()
            .registry
            .values()
            .any(|v| *v == crate::system::RegValue::Dword(2))
        {
            assert!(
                std::time::Instant::now() < deadline,
                "armer never saw the first module's write land"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // First module confirmed; the body is now inside the second
        // module's delayed `read_registry`, before its journal line exists.
        abort_tx.send(true).unwrap();
    };
    let (result, ()) = tokio::join!(exec, armer);
    let err = result.expect_err("the abort must stop the run");
    assert!(err.is_aborted(), "{err}");

    assert_eq!(
        mock.read_registry(&reg("HwSchMode")).await.unwrap(),
        crate::system::RegValue::Dword(1),
        "a module confirmed before the abort dropped the body mid-apply must be reverted at \
         ROLLBACK -- the on-disk cursor has to name the scenario being applied"
    );
    assert_eq!(
        mock.read_registry(&reg("TdrLevel")).await.unwrap(),
        crate::system::RegValue::Dword(1),
        "sanity: the second module must never have been applied at all"
    );
    let mut saw_forced_revert = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.contains("never reverted its own modules")
        {
            saw_forced_revert = true;
        }
    }
    assert!(
        saw_forced_revert,
        "the value must have come back via ROLLBACK's forced revert of the in-flight scenario"
    );
}
