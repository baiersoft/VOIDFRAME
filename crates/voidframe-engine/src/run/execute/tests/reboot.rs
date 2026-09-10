use super::*;
// Explicit, not just glob-imported (see `tests::thermal`'s own top-of-file
// comment for why): `tests`'s `mod thermal;` shadows the glob-imported
// `execute::thermal` this file also needs for `thermal::ThermalReading`.
use super::super::thermal;
use crate::model::progress::{Cursor, PendingReboot, RunProgress, Stage};
use crate::run::phase::RebootReason;
use crate::run::{RunOutcome, RunStart};
use crate::system::{
    BootReport, DEADMAN_TASK_NAME, MutationCtx, RESUME_TASK_NAME, RegKey, RegValue, TaskPrincipal,
    TaskSpec, TaskTrigger,
};

const HAGS_SUBKEY: &str = "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers";

fn hags_key() -> RegKey {
    RegKey {
        hive: Hive::Hklm,
        subkey: HAGS_SUBKEY.into(),
        value_name: "HwSchMode".into(),
    }
}

fn hags_scenario(id: &str) -> Scenario {
    Scenario {
        id: id.into(),
        name: format!("HAGS {id}"),
        description: "d".into(),
        enabled: true,
        modules: vec![Module::Registry(RegistryPayload {
            hive: Hive::Hklm,
            subkey: HAGS_SUBKEY.into(),
            value_name: "HwSchMode".into(),
            value_type: RegType::Dword,
            value: serde_json::json!(2),
            requires_reboot: true,
        })],
    }
}

fn plain_scenario(id: &str) -> Scenario {
    Scenario {
        id: id.into(),
        name: format!("Plain {id}"),
        description: "d".into(),
        enabled: true,
        modules: vec![Module::Powercfg {
            sub: "sub_processor".into(),
            setting: "IDLEDISABLE".into(),
            value: 1,
        }],
    }
}

fn ready_mock() -> MockController {
    let start_args = crate::cs2::keybind_cfg::reconcile("");
    MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 9001)
        .with_process_on_deelevate("steam.exe", 9002)
        .with_process("steamwebhelper.exe", 9099)
        .with_process_on_launch("cs2.exe", 9500)
        .with_launch_options(&start_args)
        .with_powercfg("sub_processor", "IDLEDISABLE", AcDc { ac: 0, dc: 0 })
        // AutoLogon configured the safe way (local account, no plain-text
        // password) so pre-flight's reboot-readiness checks (Task 11) pass
        // for these HAGS/reboot-scenario tests.
        .with_registry(
            &RegKey {
                hive: Hive::Hklm,
                subkey: r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon".into(),
                value_name: "AutoAdminLogon".into(),
            },
            RegValue::Sz("1".into()),
        )
        .with_registry(
            &RegKey {
                hive: Hive::Hklm,
                subkey: r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon".into(),
                value_name: "DefaultUserName".into(),
            },
            RegValue::Sz("bench".into()),
        )
}

fn capture_for(iterations: usize) -> Arc<dyn CaptureRunner> {
    Arc::new(MockCaptureRunner::new(
        (0..iterations)
            .map(|_| crate::mock_harness::mock_metrics())
            .collect(),
    ))
}

fn fresh_config(dir: &Path, run_id: &str, scenarios: Vec<Scenario>) -> RunConfig {
    write_signatures(dir);
    let mut config = config_with(
        dir.to_path_buf(),
        settings_with(0, 3, 5),
        dir.join("console.log"),
    );
    config.run_id = run_id.into();
    config.mock_cs2_log = Some(Duration::from_millis(0));
    config.project.scenarios = scenarios;
    config
}

async fn run(
    sys: Arc<dyn SystemController>,
    capture: Arc<dyn CaptureRunner>,
    start: RunStart,
) -> (Result<RunOutcome>, Vec<EngineEvent>) {
    // Seeded from whichever `shutdown_when_complete` `start` itself already
    // encodes -- `RunConfig.shutdown_when_complete` for a Fresh start, or
    // the resumed `RunProgress.shutdown_when_complete` for a Resume one
    // (mirrors production's `prepare_run`, which seeds a resumed run's own
    // `RunConfig.shutdown_when_complete` from `progress.shutdown_when_complete`
    // -- see `src-tauri/src/commands/run.rs`). A hardcoded `false` here would
    // make this throwaway channel's own initial value disagree with a
    // resumed run's on-disk `progress.json` whenever a previous call's own
    // toggle test left it `true`, and the drain sites `scenario_loop` now
    // has (findings 1+2) would spuriously flip it back the moment this
    // checkpoint is reached.
    let initial_shutdown_when_complete = match &start {
        RunStart::Fresh(config) => config.shutdown_when_complete,
        RunStart::Resume { progress, .. } => progress.shutdown_when_complete,
    };
    let (_shutdown_toggle_tx, shutdown_toggle_rx) =
        tokio::sync::watch::channel(initial_shutdown_when_complete);
    run_with_shutdown_toggle(sys, capture, start, shutdown_toggle_rx).await
}

/// Like `run`, but takes an already-built `shutdown_toggle` receiver instead
/// of a throwaway one -- lets a caller hold onto the matching sender and
/// flip it either before this starts (pre-set on the `watch::channel`'s own
/// initial value, or via `.send()` beforehand) or mid-run (a genuine
/// `.send()` call while `execute()`'s own future is polled alongside it, the
/// same interleaving technique `tee_flips_abort_watch_...` in `spawn.rs`
/// uses for `abort`).
async fn run_with_shutdown_toggle(
    sys: Arc<dyn SystemController>,
    capture: Arc<dyn CaptureRunner>,
    start: RunStart,
    shutdown_toggle_rx: tokio::sync::watch::Receiver<bool>,
) -> (Result<RunOutcome>, Vec<EngineEvent>) {
    let (events_tx, mut events_rx) = mpsc::channel(1024);
    let (_control_tx, control_rx) = mpsc::channel(4);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let result = execute(
        sys,
        capture,
        start,
        events_tx,
        control_rx,
        abort_rx,
        shutdown_toggle_rx,
    )
    .await;
    let mut events = Vec::new();
    while let Ok(ev) = events_rx.try_recv() {
        events.push(ev);
    }
    (result, events)
}

fn resume_start(dir: &Path, run_id: &str, scenarios: Vec<Scenario>) -> RunStart {
    let run_dir = dir.join("runs").join(run_id);
    RunStart::Resume {
        config: fresh_config(dir, run_id, scenarios),
        progress: RunProgress::load(&run_dir).unwrap(),
    }
}

#[tokio::test]
async fn a_hags_scenario_stops_at_reboot_pending_with_tasks_registered_and_progress_saved() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");

    let (result, events) = run(
        sys,
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", vec![hags_scenario("hags")])),
    )
    .await;

    assert!(matches!(
        result.unwrap(),
        RunOutcome::RebootPending(RebootReason::ApplyNext)
    ));
    assert_eq!(mock.reboot_calls(), 1);
    let tasks = mock.registered_tasks();
    let names: Vec<&str> = tasks.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&RESUME_TASK_NAME) && names.contains(&DEADMAN_TASK_NAME));
    assert!(
        tasks
            .iter()
            .any(|t| t.args.contains("--recover") && t.args.contains("--data-root"))
    );
    assert_eq!(mock.inhibit_sleep_calls(), vec![true]);
    let progress = RunProgress::load(&run_dir).unwrap();
    assert_eq!(
        (progress.cursor.index, progress.cursor.stage),
        (1, Stage::Measure)
    );
    assert_eq!(
        progress.reboot.as_ref().unwrap().reason,
        RebootReason::ApplyNext
    );
    assert_eq!(
        progress.completed.len(),
        1,
        "baseline result persisted before the reboot"
    );
    assert!(
        dir.path()
            .join("recovery")
            .join("VOIDFRAME_RESTORE.bat")
            .exists()
    );
    assert!(events.iter().any(|e| matches!(
        e,
        EngineEvent::PhaseChanged {
            phase: Phase::RebootPending { .. }
        }
    )));
    // HAGS stays applied: the reboot is what makes it effective.
    assert_eq!(
        mock.read_registry(&hags_key()).await.unwrap(),
        RegValue::Dword(2)
    );
}

#[tokio::test]
async fn resume_after_a_clean_boot_measures_reverts_and_ends_with_a_final_revert_reboot() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let (first, _) = run(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", vec![hags_scenario("hags")])),
    )
    .await;
    assert!(matches!(first.unwrap(), RunOutcome::RebootPending(_)));

    let (second, events) = run(
        sys,
        capture_for(3),
        resume_start(dir.path(), "r1", vec![hags_scenario("hags")]),
    )
    .await;

    assert!(matches!(
        second.unwrap(),
        RunOutcome::RebootPending(RebootReason::RevertOnly)
    ));
    assert_eq!(mock.reboot_calls(), 2);
    assert!(events.iter().any(|e| matches!(
        e,
        EngineEvent::PhaseChanged {
            phase: Phase::BootResume
        }
    )));
    assert!(events.iter().any(
        |e| matches!(e, EngineEvent::ScenarioComplete { result } if result.scenario_id == "hags")
    ));
    let progress = RunProgress::load(&run_dir).unwrap();
    assert_eq!(progress.cursor.stage, Stage::Done);
    assert_eq!(progress.completed.len(), 2);
    assert_eq!(
        progress.reboot.as_ref().unwrap().reason,
        RebootReason::RevertOnly
    );
    assert!(
        run_dir.join("results.json").exists(),
        "scoring + REPORT ran before the final reboot"
    );
    assert_eq!(
        mock.read_registry(&hags_key()).await.unwrap(),
        RegValue::Absent,
        "reverted"
    );
}

#[tokio::test]
async fn resume_after_the_final_revert_reboot_just_completes() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let scenarios = || vec![hags_scenario("hags")];
    let _ = run(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", scenarios())),
    )
    .await;
    let _ = run(
        sys.clone(),
        capture_for(3),
        resume_start(dir.path(), "r1", scenarios()),
    )
    .await;
    let (third, events) = run(
        sys,
        capture_for(0),
        resume_start(dir.path(), "r1", scenarios()),
    )
    .await;
    assert!(matches!(third.unwrap(), RunOutcome::Complete(_)));
    assert_eq!(mock.reboot_calls(), 2, "no further reboot");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, EngineEvent::RunComplete { .. }))
    );
    assert!(!run_dir.join("progress.json").exists());
    assert!(mock.registered_tasks().is_empty());
    assert_eq!(mock.inhibit_sleep_calls().last(), Some(&false));
}

#[tokio::test]
async fn two_reboot_scenarios_share_one_reboot_between_them() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let scenarios = || vec![hags_scenario("a"), hags_scenario("b")];
    let (first, _) = run(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", scenarios())),
    )
    .await;
    assert!(matches!(
        first.unwrap(),
        RunOutcome::RebootPending(RebootReason::ApplyNext)
    ));

    let (second, _) = run(
        sys,
        capture_for(3),
        resume_start(dir.path(), "r1", scenarios()),
    )
    .await;
    assert!(matches!(
        second.unwrap(),
        RunOutcome::RebootPending(RebootReason::RevertAndApplyNext)
    ));
    let progress = RunProgress::load(&run_dir).unwrap();
    assert_eq!(
        (progress.cursor.index, progress.cursor.stage),
        (2, Stage::Measure)
    );
    assert_eq!(mock.reboot_calls(), 2);
    assert_eq!(
        mock.read_registry(&hags_key()).await.unwrap(),
        RegValue::Dword(2),
        "b applied before the shared reboot"
    );
}

#[tokio::test]
async fn a_reboot_scenario_followed_by_a_plain_one_reboots_before_the_plain_one() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let scenarios = || vec![hags_scenario("a"), plain_scenario("p")];
    let _ = run(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", scenarios())),
    )
    .await;
    let (second, _) = run(
        sys.clone(),
        capture_for(3),
        resume_start(dir.path(), "r1", scenarios()),
    )
    .await;
    assert!(matches!(
        second.unwrap(),
        RunOutcome::RebootPending(RebootReason::RevertOnly)
    ));
    let progress = RunProgress::load(&run_dir).unwrap();
    assert_eq!(
        (progress.cursor.index, progress.cursor.stage),
        (2, Stage::Apply)
    );

    let (third, _) = run(
        sys,
        capture_for(3),
        resume_start(dir.path(), "r1", scenarios()),
    )
    .await;
    assert!(matches!(third.unwrap(), RunOutcome::Complete(_)));
    assert_eq!(mock.reboot_calls(), 2);
    assert!(
        !run_dir.join("progress.json").exists(),
        "progress removed on Complete"
    );
    assert!(
        mock.registered_tasks().is_empty(),
        "both tasks removed at ROLLBACK"
    );
    let results = crate::model::results::RunResults::load(&run_dir.join("results.json")).unwrap();
    assert_eq!(results.scenarios.len(), 2);
}

#[tokio::test]
async fn a_bugcheck_on_resume_marks_the_scenario_unstable_and_continues() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let scenarios = || vec![hags_scenario("a"), plain_scenario("p")];
    let _ = run(
        sys,
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", scenarios())),
    )
    .await;

    let crashed = mock.clone().with_boot_report(BootReport {
        kernel_power_41: true,
        bugcheck_1001: true,
        new_minidumps: vec![PathBuf::from("C:\\Windows\\Minidump\\x.dmp")],
        safeboot_option: None,
    });
    let sys2: Arc<dyn SystemController> = Arc::new(crashed);
    let (second, events) = run(
        sys2,
        capture_for(3),
        resume_start(dir.path(), "r1", scenarios()),
    )
    .await;

    assert!(matches!(
        second.unwrap(),
        RunOutcome::RebootPending(RebootReason::RevertOnly)
    ));
    let progress = RunProgress::load(&run_dir).unwrap();
    assert_eq!(progress.unstable.len(), 1);
    assert_eq!(progress.unstable[0].scenario_id, "a");
    assert!(progress.unstable[0].reason.contains("bugcheck"));
    assert!(!events.iter().any(
        |e| matches!(e, EngineEvent::ScenarioComplete { result } if result.scenario_id == "a")
    ));
    assert_eq!(
        (progress.cursor.index, progress.cursor.stage),
        (2, Stage::Apply)
    );
    assert_eq!(
        mock.read_registry(&hags_key()).await.unwrap(),
        RegValue::Absent,
        "a reverted without measuring"
    );
}

/// Spec §4's `UnexpectedExtraReboot`: a `PendingReboot.boot_count` of `1`
/// (i.e. the machine already rebooted once since VOIDFRAME's own reboot,
/// before this resume) must be treated the same as a bugcheck -- reverted
/// and marked unstable without ever measuring -- even against an
/// otherwise-clean `BootReport`.
#[tokio::test]
async fn a_second_reboot_before_resume_is_marked_unstable_as_an_unexpected_extra_reboot() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let scenarios = vec![hags_scenario("hags")];
    let _ = run(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", scenarios.clone())),
    )
    .await;

    // Hand-edit the saved progress to look like the machine already
    // rebooted once more before this resume ever ran.
    let mut progress = RunProgress::load(&run_dir).unwrap();
    progress.reboot.as_mut().unwrap().boot_count = 1;
    progress.save(&run_dir).unwrap();

    let progress = RunProgress::load(&run_dir).unwrap();
    let config = fresh_config(dir.path(), "r1", scenarios);
    let (second, events) = run(sys, capture_for(3), RunStart::Resume { config, progress }).await;

    assert!(matches!(
        second.unwrap(),
        RunOutcome::RebootPending(RebootReason::RevertOnly)
    ));
    let progress = RunProgress::load(&run_dir).unwrap();
    assert_eq!(progress.unstable.len(), 1);
    assert_eq!(progress.unstable[0].scenario_id, "hags");
    assert!(progress.unstable[0].reason.contains("rebooted again"));
    assert!(!events.iter().any(
        |e| matches!(e, EngineEvent::ScenarioComplete { result } if result.scenario_id == "hags")
    ));
}

#[tokio::test]
async fn shutdown_when_complete_replaces_the_final_revert_reboot() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let mut config = fresh_config(dir.path(), "r1", vec![hags_scenario("hags")]);
    config.shutdown_when_complete = true;
    let _ = run(sys.clone(), capture_for(3), RunStart::Fresh(config)).await;
    let mut config = fresh_config(dir.path(), "r1", vec![hags_scenario("hags")]);
    config.shutdown_when_complete = true;
    let progress = RunProgress::load(&run_dir).unwrap();
    let (second, _) = run(sys, capture_for(3), RunStart::Resume { config, progress }).await;
    assert!(matches!(second.unwrap(), RunOutcome::ShutdownRequested(_)));
    assert_eq!(
        mock.reboot_calls(),
        1,
        "no second reboot: the shutdown replaces it"
    );
    assert_eq!(mock.shutdown_calls(), 1);
    assert!(run_dir.join("results.json").exists());
    assert!(!run_dir.join("progress.json").exists());
}

/// The live-toggle counterpart to `shutdown_when_complete_replaces_the_final_
/// revert_reboot`: a run that starts with `shutdown_when_complete: false`
/// but has `true` set on its own `shutdown_toggle` channel before `execute()`
/// ever starts (drained at the run's first inter-scenario break, after
/// baseline reverts and before `plain_scenario` applies -- see
/// `control::drain_shutdown_toggle`) must still call `sys.shutdown()` by the
/// time it finishes.
#[tokio::test]
async fn shutdown_toggle_mid_run_to_true_causes_a_shutdown() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let mut config = fresh_config(dir.path(), "r1", vec![plain_scenario("p")]);
    config.shutdown_when_complete = false;
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(true);
    let (result, _events) = run_with_shutdown_toggle(
        sys,
        capture_for(3),
        RunStart::Fresh(config),
        shutdown_toggle_rx,
    )
    .await;
    assert!(matches!(result.unwrap(), RunOutcome::ShutdownRequested(_)));
    assert_eq!(mock.shutdown_calls(), 1);
}

/// The inverse of the test above: `shutdown_when_complete: true` at start,
/// toggled to `false` before the same checkpoint, must NOT shut down.
#[tokio::test]
async fn shutdown_toggle_mid_run_to_false_prevents_a_shutdown() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let mut config = fresh_config(dir.path(), "r1", vec![plain_scenario("p")]);
    config.shutdown_when_complete = true;
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);
    let (result, _events) = run_with_shutdown_toggle(
        sys,
        capture_for(3),
        RunStart::Fresh(config),
        shutdown_toggle_rx,
    )
    .await;
    assert!(matches!(result.unwrap(), RunOutcome::Complete(_)));
    assert_eq!(mock.shutdown_calls(), 0);
}

/// Unlike the two tests above (which set the toggle before `execute()` ever
/// starts polling), this sends on the toggle's sender genuinely mid-run --
/// after baseline's own `launch_cs2` call has already happened -- proving a
/// toggle observed while the run is already in progress (not just
/// pre-buffered) is picked up at the next checkpoint, the same interleaving
/// `tee_flips_abort_watch_even_with_a_full_inner_channel_and_preserves_order`
/// in `spawn.rs` uses for `abort`.
#[tokio::test]
async fn shutdown_toggle_sent_after_the_run_has_already_started_is_observed() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let mut config = fresh_config(dir.path(), "r1", vec![plain_scenario("p")]);
    config.shutdown_when_complete = false;
    let (shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);
    let exec = run_with_shutdown_toggle(
        sys,
        capture_for(3),
        RunStart::Fresh(config),
        shutdown_toggle_rx,
    );
    let armer = async {
        for _ in 0..100_000u32 {
            if mock.launch_cs2_calls() > 0 {
                shutdown_toggle_tx.send(true).unwrap();
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("armer never saw launch_cs2");
    };
    let ((result, _events), ()) = tokio::join!(exec, armer);
    assert!(matches!(result.unwrap(), RunOutcome::ShutdownRequested(_)));
    assert_eq!(mock.shutdown_calls(), 1);
}

/// Coverage for the finding-1+2 fix at one of the newly-added
/// `reboot_sequence` drain sites: a toggle flipped to `true` while
/// baseline's own MEASURE is still running -- the point `launch_cs2_calls()`
/// actually fires, before baseline's own Revert decides the transition into
/// the reboot-requiring `hags` scenario -- must survive `reboot_sequence`
/// (which persists `progress.json` and kills this process) and still be
/// observed once the resumed run eventually finishes. This exercises the
/// `Transition::ApplyNextThenReboot` drain site specifically (the site
/// guarding the reboot triggered when the NEXT scenario needs a reboot on
/// entry) -- not the `Stage::Apply` + `requires_reboot()` site (a different
/// site, for when the CURRENT scenario's own apply itself needs a reboot;
/// see `shutdown_toggle_survives_a_reboot_triggered_by_the_current_
/// scenarios_own_apply` below for that one) -- proving
/// `progress.shutdown_when_complete` was actually drained and saved before
/// the reboot, not merely held in the now-dead process's own in-memory
/// channel.
#[tokio::test]
async fn shutdown_toggle_sent_before_a_reboot_survives_the_reboot() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let mut config = fresh_config(dir.path(), "r1", vec![hags_scenario("hags")]);
    config.shutdown_when_complete = false;
    let (shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);
    let exec = run_with_shutdown_toggle(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(config),
        shutdown_toggle_rx,
    );
    let armer = async {
        for _ in 0..100_000u32 {
            if mock.launch_cs2_calls() > 0 {
                shutdown_toggle_tx.send(true).unwrap();
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("armer never saw launch_cs2");
    };
    let ((first, _), ()) = tokio::join!(exec, armer);
    assert!(matches!(
        first.unwrap(),
        RunOutcome::RebootPending(RebootReason::ApplyNext)
    ));
    let progress = RunProgress::load(&run_dir).unwrap();
    assert!(
        progress.shutdown_when_complete,
        "the toggle sent before the reboot must have been drained into progress.json"
    );

    let (second, _) = run(
        sys,
        capture_for(3),
        resume_start(dir.path(), "r1", vec![hags_scenario("hags")]),
    )
    .await;
    assert!(matches!(second.unwrap(), RunOutcome::ShutdownRequested(_)));
    assert_eq!(mock.shutdown_calls(), 1);
}

/// Finding 3's remaining gap: `scenario_loop`'s `Stage::Apply` +
/// `requires_reboot()` drain site (as opposed to the `ApplyNextThenReboot`
/// site the test above actually exercises) -- reached when a resume's own
/// cursor lands directly at `Stage::Apply` on a reboot-requiring scenario,
/// so `apply_stage` and the reboot both happen inline in that match arm
/// rather than via `Transition::ApplyNextThenReboot`'s own inline apply
/// during Revert handling. `progress_at_stage` hand-builds that cursor
/// state (its `cursor.index` of `1` lines up with `hags_scenario` being the
/// sole enabled scenario, right after the dummy baseline `ordered_scenarios`
/// prepends at index 0) the same way the `begin_resume` settle-wait tests
/// above already do, since no production flow persists `progress.json` at
/// exactly this cursor for a reboot-requiring scenario -- `plan_transition`
/// always routes that case through `ApplyNextThenReboot` instead, so this
/// `Stage::Apply` + `requires_reboot()` arm is a defensive guard with no
/// live transition into it today, not a path a real run currently takes.
/// It must still behave correctly if reached (removing it would silently
/// skip the reboot for whatever future path lands here), so this test
/// pins the guard's own drain-and-survive behavior rather than covering
/// a production-exercised path.
#[tokio::test]
async fn shutdown_toggle_survives_a_reboot_triggered_by_the_current_scenarios_own_apply() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let scenarios = vec![hags_scenario("hags")];
    let config = fresh_config(dir.path(), "r1", scenarios.clone());
    let progress = progress_at_stage(config.project.clone(), Stage::Apply);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(true);

    let (first, _) = run_with_shutdown_toggle(
        sys.clone(),
        capture_for(3),
        RunStart::Resume { config, progress },
        shutdown_toggle_rx,
    )
    .await;

    assert!(matches!(
        first.unwrap(),
        RunOutcome::RebootPending(RebootReason::ApplyNext)
    ));
    let saved = RunProgress::load(&run_dir).unwrap();
    assert_eq!(
        (saved.cursor.index, saved.cursor.stage),
        (1, Stage::Measure),
        "the Stage::Apply site's own reboot moves straight to Measure -- the scenario was \
         already applied, nothing left to redo on resume"
    );
    assert!(
        saved.shutdown_when_complete,
        "the toggle observed at the Stage::Apply + requires_reboot() drain site must survive \
         the reboot"
    );

    let (second, _) = run(
        sys,
        capture_for(3),
        resume_start(dir.path(), "r1", scenarios),
    )
    .await;
    assert!(matches!(second.unwrap(), RunOutcome::ShutdownRequested(_)));
    assert_eq!(mock.shutdown_calls(), 1);
}

/// The `mock.fail_next_write("disk full")` one-shot injected here lands on
/// `plain_scenario`'s own APPLY_MODULES `write_powercfg` call (the body
/// failure) -- its per-scenario error handling immediately runs its own
/// defensive `revert_scenario` against a journal that never got a confirmed
/// entry (a no-op), which does not re-consume the one-shot since that
/// record's `applied` flag never flipped to `true` in the first place. By
/// the time `execute()`'s own `rollback()` runs (`ctx.start_power_plan`
/// restore + journal sweep), the one-shot is long spent and both of
/// `rollback()`'s steps succeed against a clean mock -- so this run's own
/// ROLLBACK genuinely succeeds. Under D8 that makes shutdown firing the
/// spec-correct outcome, not a bug (confirmed empirically: this run's own
/// `tracing::error!` "rollback also failed" line -- checked with
/// `--nocapture` before the D8 fix landed -- never appears here).
#[tokio::test]
async fn shutdown_fires_after_a_failed_scenario_whose_own_rollback_recovers() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let mut config = fresh_config(dir.path(), "r1", vec![plain_scenario("p")]);
    config.shutdown_when_complete = true;
    let exec = run(sys, capture_for(3), RunStart::Fresh(config));
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
    let ((result, _events), ()) = tokio::join!(exec, armer);
    assert!(result.is_err(), "the run's body still fails");
    assert_eq!(
        mock.shutdown_calls(),
        1,
        "D8: shutdown fires once ROLLBACK itself has succeeded, even after a failed run"
    );
}

/// Unlike the test above, this forces `rollback()`'s own work -- not the
/// scenario's per-scenario revert -- to fail, deterministically rather than
/// via `fail_next_write`'s one-shot: deleting the run-start active power
/// plan out from under `ctx.start_power_plan.guid` makes `rollback()`'s
/// `set_active_power_plan` call return a real (non-one-shot) `Err` every
/// time, proving D8's "never when a revert failed" guarantee against a
/// genuinely failing ROLLBACK.
#[tokio::test]
async fn shutdown_is_not_requested_when_rollback_itself_fails() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let mut config = fresh_config(dir.path(), "r1", vec![plain_scenario("p")]);
    config.shutdown_when_complete = true;
    let exec = run(sys, capture_for(3), RunStart::Fresh(config));
    let armer = async {
        for _ in 0..100_000u32 {
            if mock.launch_cs2_calls() > 0 {
                mock.fail_next_write("disk full");
                mock.delete_power_plan(
                    "381b4222-f694-41f0-9685-ff5bb260df2e",
                    &MutationCtx {
                        run_id: "r1".into(),
                        scenario_id: "test-armer".into(),
                        step_index: 0,
                    },
                )
                .await
                .unwrap();
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("armer never saw launch_cs2");
    };
    let ((result, _events), ()) = tokio::join!(exec, armer);
    assert!(result.is_err());
    assert_eq!(
        mock.shutdown_calls(),
        0,
        "D8: never shut down after a genuinely failed ROLLBACK"
    );
}

/// A `RunProgress` at `stage`, otherwise minimal -- shared by the two
/// `begin_resume` settle-wait tests below. `reboot` is always populated:
/// `begin_resume` errors out immediately without it (`progress.reboot.clone()
/// .ok_or_else(...)`), regardless of which `Stage` the cursor is at.
fn progress_at_stage(project: Project, stage: Stage) -> RunProgress {
    RunProgress {
        schema_version: crate::model::SCHEMA_VERSION.to_string(),
        run_id: "r1".into(),
        project,
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
        cursor: Cursor { index: 1, stage },
        reboot: Some(PendingReboot {
            reason: RebootReason::RevertOnly,
            scenario_id: "hags".into(),
            initiated_at: "2026-09-06T00:00:00Z".into(),
            boot_count: 0,
        }),
        shutdown_when_complete: false,
        skip_revert_once: false,
        abort_requested: false,
    }
}

/// Confirmed live 2026-09-07 (item 8 of the M3 rig checklist): the FINAL
/// resume of a run -- the one that exists purely to confirm the last revert
/// took effect, with no CS2 launch or measurement left to do -- has nothing
/// left that post-boot flakiness could disrupt, so waiting the full
/// `post_boot_settle` (default 180s) before it is pointless.
/// `finish_run`'s call site is the only place that saves a `next_cursor`
/// with `stage: Stage::Done` before a reboot, so `Stage::Done` on the
/// just-loaded `RunProgress` precisely identifies this case.
#[tokio::test]
async fn begin_resume_skips_the_settle_wait_on_the_final_revert_only_resume() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock);
    let mut config = fresh_config(dir.path(), "r1", vec![]);
    config.post_boot_settle = Duration::from_secs(180);
    let progress = progress_at_stage(config.project.clone(), Stage::Done);

    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let mut cooldown_checks = Vec::new();

    let start = tokio::time::Instant::now();
    begin_resume(
        sys.as_ref(),
        &config,
        progress,
        &events_tx,
        &mut control_rx,
        &shutdown_toggle_rx,
        &abort_rx,
        &tokio::sync::watch::channel(false).0,
        &mut cooldown_checks,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .await
    .unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed.is_zero(),
        "a final revert-only resume must not wait the configured post_boot_settle duration, \
         elapsed {elapsed:?}"
    );

    drop(events_tx);
    let mut events = Vec::new();
    while let Ok(ev) = events_rx.try_recv() {
        events.push(ev);
    }
    assert!(
        events.iter().any(
            |e| matches!(e, EngineEvent::LogLine { text } if text.contains("skipping the post-boot settle wait"))
        ),
        "expected an explanatory LogLine for the skip, got: {events:?}"
    );
}

/// Regression guard for the fix above: a resume at any earlier `Stage`
/// (still mid-run, with a CS2 launch/measurement or revert still ahead of
/// it) must keep waiting the full configured `post_boot_settle` duration --
/// the skip is specific to `Stage::Done`, not every resume.
#[tokio::test]
async fn begin_resume_still_waits_the_full_settle_when_resuming_mid_run() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock);
    let mut config = fresh_config(dir.path(), "r1", vec![]);
    config.post_boot_settle = Duration::from_secs(180);
    let progress = progress_at_stage(config.project.clone(), Stage::Measure);

    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let mut cooldown_checks = Vec::new();

    let start = tokio::time::Instant::now();
    begin_resume(
        sys.as_ref(),
        &config,
        progress,
        &events_tx,
        &mut control_rx,
        &shutdown_toggle_rx,
        &abort_rx,
        &tokio::sync::watch::channel(false).0,
        &mut cooldown_checks,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .await
    .unwrap();
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_secs(180),
        "a mid-run resume must still wait the full configured post_boot_settle duration, \
         elapsed {elapsed:?}"
    );
}

/// Confirmed live on the M3 rig (2026-09-08): a mid-run resume with thermal
/// mode on used to call `collect_thermal_baseline` again, overwriting
/// `progress.thermal_baseline` with whatever the machine reads right after
/// a reboot (still hot from POST/Windows startup) and re-entering
/// `Phase::ThermalBaseline`. It must instead wait out a `maybe_break`
/// cooldown against the *existing* baseline -- same as any other
/// inter-scenario break -- and leave that baseline untouched.
#[tokio::test]
async fn begin_resume_waits_for_cooldown_instead_of_resampling_the_baseline_mid_run() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let hwinfo_path = std::path::PathBuf::from("C:\\fake\\hwinfo.exe");
    let baseline = thermal::ThermalReading {
        cpu_temp_celsius: 60.0,
        gpu_temp_celsius: None,
        sample_count: 30,
    };
    // One check, already within the +/-3C threshold -- the cooldown wait
    // ends after its first check instead of running the full 10-minute cap.
    let readings: Vec<(f64, Option<f64>)> =
        std::iter::repeat_n((61.0, None), thermal::THERMAL_SAMPLE_COUNT as usize).collect();
    let mock = ready_mock().with_hwinfo_readings(readings);
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let mut config = fresh_config(dir.path(), "r1", vec![]);
    config.post_boot_settle = Duration::from_secs(0);
    config.hwinfo_path = Some(hwinfo_path);
    let mut progress = progress_at_stage(config.project.clone(), Stage::Measure);
    progress.thermal_baseline = Some(baseline.clone());

    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let mut cooldown_checks = Vec::new();

    let result = begin_resume(
        sys.as_ref(),
        &config,
        progress,
        &events_tx,
        &mut control_rx,
        &shutdown_toggle_rx,
        &abort_rx,
        &tokio::sync::watch::channel(false).0,
        &mut cooldown_checks,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert_eq!(
        result.thermal_baseline,
        Some(baseline),
        "the pre-existing baseline must survive a mid-run resume unchanged"
    );
    assert_eq!(
        mock.start_hwinfo_calls(),
        1,
        "expected the cooldown wait to start HWiNFO once"
    );
    assert_eq!(
        cooldown_checks.len(),
        1,
        "the cooldown check taken during this resume must be recorded for thermal.json"
    );

    drop(events_tx);
    let mut events = Vec::new();
    while let Ok(ev) = events_rx.try_recv() {
        events.push(ev);
    }
    assert!(
        !events.iter().any(|e| matches!(
            e,
            EngineEvent::PhaseChanged {
                phase: Phase::ThermalBaseline
            }
        )),
        "a resume must never re-enter Phase::ThermalBaseline, got: {events:?}"
    );
}

/// The final revert-only resume (`Stage::Done`) has nothing left to measure
/// and nothing to cool down for -- it must skip thermal handling entirely
/// (neither a re-sampled baseline nor a cooldown wait) rather than delaying
/// the run's finish (or, when `results.json` is already on disk, the
/// already-finished short-circuit) behind either.
#[tokio::test]
async fn begin_resume_skips_thermal_handling_entirely_on_the_final_revert_only_resume() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let hwinfo_path = std::path::PathBuf::from("C:\\fake\\hwinfo.exe");
    let baseline = thermal::ThermalReading {
        cpu_temp_celsius: 60.0,
        gpu_temp_celsius: None,
        sample_count: 30,
    };
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let mut config = fresh_config(dir.path(), "r1", vec![]);
    config.post_boot_settle = Duration::from_secs(180);
    config.hwinfo_path = Some(hwinfo_path);
    let mut progress = progress_at_stage(config.project.clone(), Stage::Done);
    progress.thermal_baseline = Some(baseline.clone());

    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let mut cooldown_checks = Vec::new();

    let result = begin_resume(
        sys.as_ref(),
        &config,
        progress,
        &events_tx,
        &mut control_rx,
        &shutdown_toggle_rx,
        &abort_rx,
        &tokio::sync::watch::channel(false).0,
        &mut cooldown_checks,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .await
    .unwrap();

    assert_eq!(result.thermal_baseline, Some(baseline));
    assert_eq!(
        mock.start_hwinfo_calls(),
        0,
        "the final revert-only resume must never touch HWiNFO"
    );
    assert!(
        cooldown_checks.is_empty(),
        "the final revert-only resume has nothing to record a cooldown check against"
    );

    drop(events_tx);
    let mut events = Vec::new();
    while let Ok(ev) = events_rx.try_recv() {
        events.push(ev);
    }
    assert!(
        !events.iter().any(|e| matches!(
            e,
            EngineEvent::PhaseChanged {
                phase: Phase::ThermalBaseline
            }
        )),
        "the final revert-only resume must never enter Phase::ThermalBaseline, got: {events:?}"
    );
}

/// M3 safety review, round 3 -- the root-cause fix underneath both 9e7698d
/// and 71d672c: neither `apply_stage` nor `revert_stage` reads `ctx.abort`,
/// and `maybe_break` (the loop's one abort checkpoint) is only reached on
/// the `Transition::None` path, never before any of `scenario_loop`'s three
/// `reboot_sequence` calls -- so an Abort clicked in the window before a
/// mid-run reboot used to vanish silently and the reboot proceeded anyway.
/// This exercises the `Transition::ApplyNextThenReboot` site specifically:
/// the armer flips `abort` to `true` once `mock.quit_cs2_calls() > 0` --
/// after baseline's own measurement loop (and every wait inside it, e.g.
/// CS2-readiness detection) has already run to completion, and well before
/// `apply_stage(hags)` / `reboot_sequence` run, neither of which check abort
/// themselves -- so this hits exactly the new Layer-1 check and no
/// pre-existing abort catch.
/// Confirmed to fail against the pre-fix code (reaches `RunOutcome::
/// RebootPending` with `reboot_calls() == 1` instead of `Err`) before this
/// fix was applied.
#[tokio::test]
async fn abort_observed_immediately_before_a_reboot_stops_it_instead_of_rebooting() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let config = fresh_config(dir.path(), "r1", vec![hags_scenario("hags")]);

    let (events_tx, _events_rx) = mpsc::channel(1024);
    let (_control_tx, control_rx) = mpsc::channel(4);
    let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

    let exec = execute(
        sys,
        capture_for(3),
        RunStart::Fresh(config),
        events_tx,
        control_rx,
        abort_rx,
        shutdown_toggle_rx,
    );
    // `quit_cs2_calls()` fires only after baseline's own measurement loop
    // has run every iteration to completion -- crossing several real
    // `tokio::time::sleep` waits (e.g. `settle_before_capture`) along the
    // way. Under `tokio::time::pause()` those only ever elapse via the
    // runtime's own auto-advance-to-the-next-timer behavior, which requires
    // every task to be genuinely idle (parked on a timer) at once -- a
    // busy `yield_now()` poll loop (as the shutdown-toggle tests above use,
    // safe there only because their own trigger point is reached before any
    // real sleep) would starve that auto-advance forever. Polling via a
    // short `sleep` instead keeps this armer itself parked on a timer
    // between checks, so it participates in that auto-advance rather than
    // blocking it.
    let armer = async {
        for _ in 0..120u32 {
            if mock.quit_cs2_calls() > 0 {
                abort_tx.send(true).unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!("armer never saw quit_cs2");
    };
    let (result, ()) = tokio::join!(exec, armer);

    let err = result.unwrap_err();
    assert!(err.is_aborted(), "{err}");
    assert_eq!(
        mock.reboot_calls(),
        0,
        "the reboot must not have happened once abort was observed"
    );
    assert_eq!(mock.shutdown_calls(), 0);
    let progress = RunProgress::load(&run_dir).unwrap();
    assert!(
        progress.abort_requested,
        "abort_requested must be persisted so a hypothetical future reboot path is still \
         caught by begin_resume's own Layer-2 guard"
    );
}

/// Layer 2 (defense-in-depth): `begin_resume` itself refuses to continue a
/// resumed run whose on-disk `progress.abort_requested` is already `true` --
/// simulating the hypothetical case where a reboot proceeded anyway despite
/// Layer 1 -- without relying on Layer 1 having worked at all. Built via
/// `progress_at_stage` (the same hand-built-`RunProgress`-at-a-specific-
/// `Stage` technique `begin_resume_skips_the_settle_wait_on_the_final_
/// revert_only_resume` and `71d672c`'s own `finish_run_never_shuts_down_
/// after_an_operator_abort_reaches_the_success_path` already use), with
/// `abort_requested` forced to `true` on top of it.
#[tokio::test]
async fn begin_resume_refuses_to_continue_when_abort_requested_is_already_set() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let scenarios = vec![hags_scenario("hags")];
    let config = fresh_config(dir.path(), "r1", scenarios.clone());
    let mut progress = progress_at_stage(config.project.clone(), Stage::Measure);
    progress.abort_requested = true;

    let (result, _events) = run(sys, capture_for(3), RunStart::Resume { config, progress }).await;

    let err = result.unwrap_err();
    assert!(err.is_aborted(), "{err}");
    assert_eq!(mock.reboot_calls(), 0);
    assert_eq!(mock.shutdown_calls(), 0);
}

/// Fifth round, same incident: `begin_resume` failing (here, via the same
/// Layer-2 `abort_requested` mechanism the test above drives) used to make
/// `execute()`'s `begin_resume(...).await?` propagate straight out,
/// bypassing ROLLBACK and `teardown()` entirely -- so `RESUME_TASK_NAME`
/// kept `voidframe.exe --resume` auto-launching at every future logon, and
/// the run-start power plan was never restored. Unlike
/// `begin_resume_refuses_to_continue_when_abort_requested_is_already_set`
/// (a hand-built `RunProgress` with no real prior reboot, so there is
/// nothing registered to prove got torn down), this drives a real Fresh run
/// through to `RebootPending` first, so `RESUME_TASK_NAME`/
/// `DEADMAN_TASK_NAME` are genuinely registered, and switches the mock's
/// active power plan away from the run-start one before the failing resume,
/// so a genuine ROLLBACK restore is observable too. Confirmed to fail
/// against the pre-fix code (`mock.registered_tasks()` non-empty and the
/// power plan left switched) before this fix was applied.
#[tokio::test]
async fn a_failed_resume_still_runs_rollback_and_teardown() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let scenarios = vec![hags_scenario("hags")];
    let (first, _) = run(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", scenarios.clone())),
    )
    .await;
    assert!(matches!(first.unwrap(), RunOutcome::RebootPending(_)));
    assert!(
        !mock.registered_tasks().is_empty(),
        "sanity: the ApplyNext reboot must have registered both tasks"
    );

    // Simulate the active power plan having drifted from the run-start
    // plan (Balanced) by the time this resume runs -- `rollback()`'s real
    // job is restoring `ctx.start_power_plan` regardless of how the drift
    // happened, so this is a faithful stand-in for whatever mutation would
    // ordinarily be live at this point in a real run.
    mock.set_active_power_plan(
        "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c",
        &MutationCtx {
            run_id: "r1".into(),
            scenario_id: "test-setup".into(),
            step_index: 0,
        },
    )
    .await
    .unwrap();

    let config = fresh_config(dir.path(), "r1", scenarios);
    let mut progress = RunProgress::load(&run_dir).unwrap();
    progress.abort_requested = true;

    let (result, _events) = run(sys, capture_for(3), RunStart::Resume { config, progress }).await;

    let err = result.unwrap_err();
    assert!(err.is_aborted(), "{err}");
    assert!(
        mock.registered_tasks().is_empty(),
        "teardown must deregister both scheduled tasks even when begin_resume itself fails -- \
         otherwise voidframe.exe --resume keeps auto-launching at every future logon"
    );
    assert_eq!(
        mock.active_power_plan().await.unwrap().guid,
        "381b4222-f694-41f0-9685-ff5bb260df2e",
        "ROLLBACK must restore the run-start power plan even when begin_resume itself fails"
    );
}

/// Sixth round, same incident: the fifth round's fix (`progress_before`, a
/// clone taken *before* `begin_resume` runs) closed the ROLLBACK/teardown
/// gap, but `handle_body_failure`'s own `drain_shutdown_toggle` call writes
/// that *entire* `progress` value back to `progress.json` whenever the live
/// shutdown toggle differs from it -- which, using the stale pre-call clone,
/// silently clobbers whatever `begin_resume` itself had already durably
/// written before failing (its early `boot_count` increment). This fails
/// `begin_resume` *after* that early save, while also toggling
/// `shutdown_when_complete` away from what's on disk -- forcing
/// `drain_shutdown_toggle`'s write branch to actually run. Confirmed to fail
/// against the pre-fix code (`boot_count` regresses back to its pre-increment
/// value on disk) before this fix was applied.
///
/// Drives `run_body` rather than `execute()` (2026-09-07 instant-abort
/// spec). The line under test is `run_body`'s own
/// `RunProgress::load(&run_dir(&config))` on the resume-failure path -- the
/// round-6 fix is precisely the choice to reload from disk instead of
/// handing `handle_body_failure` the stale `progress_before` clone -- so the
/// test has to run that code, not re-implement it by loading `progress`
/// itself and calling `begin_resume`/`handle_body_failure` separately (which
/// would move the fix into the test and make the guard vacuous). Going
/// through `execute()` is what no longer works: its whole-body race would
/// cut the body before `begin_resume`'s early save on some polls and after
/// it on others (`tokio::select!` picks a branch order at random), so which
/// cleanup path runs -- and therefore whether the clobbering is exercised at
/// all -- would be a coin flip. `run_body` is the same code with the race
/// peeled off, so this stays deterministic.
///
/// Originally failed `begin_resume` via an `abort` pre-set to `true`, racing
/// the post-boot settle wait's own (then-existing) `abortable` wrapper. Task
/// 6 removed that wrapper -- it was the *only* place inside `begin_resume`
/// that ever observed `abort` directly, so `begin_resume` driven straight
/// through `run_body` (bypassing `execute()`'s outer race, which is the whole
/// point of this test -- see above) can no longer be made to fail via abort
/// at all. The round-6 guard itself has nothing to do with abort specifically
/// -- it cares about *any* `begin_resume` failure after the early save -- so
/// this now reaches the same post-early-save failure a different way:
/// `boot.rs::classify_boot` returns `UnexpectedExtraReboot` whenever the
/// *pre-increment* `boot_count` is already >= 1 (checked first, before it
/// ever looks at the mock's own clean `BootReport`), and an out-of-range
/// `cursor.index` then makes the `class != Clean` branch's own
/// `scenarios.get(index).ok_or_else(..)` fail -- both set directly on the
/// resumed `progress`, both firing well after `begin_resume`'s early
/// `boot_count`-increment save.
#[tokio::test]
async fn a_failed_resume_after_the_early_boot_count_save_does_not_clobber_begin_resumes_own_disk_writes()
 {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let scenarios = vec![hags_scenario("hags")];
    let (first, _) = run(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", scenarios.clone())),
    )
    .await;
    assert!(matches!(first.unwrap(), RunOutcome::RebootPending(_)));
    let mut progress = RunProgress::load(&run_dir).unwrap();
    assert_eq!(
        progress.reboot.as_ref().unwrap().boot_count,
        0,
        "sanity: the reboot recorded by the Fresh run's own reboot_sequence must start at \
         boot_count 0"
    );
    assert!(
        !progress.shutdown_when_complete,
        "sanity: fresh_config's shutdown_when_complete defaults to false"
    );

    // Forces `classify_boot` to `UnexpectedExtraReboot` (checked before the
    // mock's own boot report) and, combined with an out-of-range cursor
    // index, makes `begin_resume`'s own `scenarios.get(index).ok_or_else(..)`
    // fail -- after its early boot_count-increment save, per this test's own
    // doc comment.
    progress.reboot.as_mut().unwrap().boot_count = 1;
    progress.cursor = Cursor {
        index: 99,
        stage: Stage::Measure,
    };

    let config = fresh_config(dir.path(), "r1", scenarios);

    let (events_tx, _events_rx) = mpsc::channel(1024);
    let (_control_tx, control_rx) = mpsc::channel(4);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(true);

    let result = run_body(
        sys,
        capture_for(3),
        RunStart::Resume { config, progress },
        events_tx,
        control_rx,
        abort_rx,
        shutdown_toggle_rx,
        tokio::sync::watch::channel(false).0,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
    .await;

    let err = result.unwrap_err();
    assert!(
        err.to_string().contains("out of range"),
        "expected the forced out-of-range cursor failure, got: {err}"
    );

    let on_disk_after = RunProgress::load(&run_dir).unwrap();
    assert_eq!(
        on_disk_after.reboot.as_ref().unwrap().boot_count,
        2,
        "begin_resume's own boot_count increment, already durably saved before the forced \
         post-save failure, must not be clobbered by handle_body_failure's shutdown-toggle \
         drain writing back a stale pre-begin_resume clone"
    );
    assert!(
        on_disk_after.shutdown_when_complete,
        "the live shutdown toggle must still be correctly drained and persisted"
    );
}

/// Regression guard for the two tests above: a normal run with `abort`
/// never set must still reboot exactly as before -- neither Layer-1 nor
/// Layer-2 of this fix should ever fire on the happy path.
#[tokio::test]
async fn a_normal_run_with_abort_never_set_still_reboots() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");

    let (result, _events) = run(
        sys,
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", vec![hags_scenario("hags")])),
    )
    .await;

    assert!(matches!(
        result.unwrap(),
        RunOutcome::RebootPending(RebootReason::ApplyNext)
    ));
    assert_eq!(
        mock.reboot_calls(),
        1,
        "the reboot must still happen when abort is never set"
    );
    let progress = RunProgress::load(&run_dir).unwrap();
    assert!(!progress.abort_requested);
}

#[tokio::test]
async fn a_run_without_reboot_scenarios_still_completes_and_leaves_no_tasks() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let (result, _) = run(
        sys,
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", vec![plain_scenario("p")])),
    )
    .await;
    assert!(matches!(result.unwrap(), RunOutcome::Complete(_)));
    assert_eq!(mock.reboot_calls(), 0);
    assert!(mock.registered_tasks().is_empty());
    assert!(
        !dir.path()
            .join("runs")
            .join("r1")
            .join("progress.json")
            .exists()
    );
}

/// Fourth gap in this same incident (M3 safety review, round 4): unlike the
/// D3 shutdown block just above it, `finish_run`'s own `if
/// final_revert_needs_reboot` block called `reboot_sequence` with no abort
/// check at all -- reached whenever that D3 block didn't return, which
/// includes precisely the case where abort WAS true (that's exactly why D3
/// was skipped). Mirrors `resume_after_a_clean_boot_measures_reverts_and_
/// ends_with_a_final_revert_reboot`'s own two-call shape (a Fresh run stops
/// at `RebootPending(ApplyNext)`, then one resume measures/reverts hags and
/// reaches the final-revert-reboot decision in that same call) but, like
/// `abort_observed_immediately_before_a_reboot_stops_it_instead_of_
/// rebooting`, flips `abort` via an armer keyed on `quit_cs2_calls()` --
/// specifically once hags's own resumed measurement finishes (after every
/// wait inside `measure_stage` has already resolved) and well before
/// `revert_stage` (which reads no abort signal at all, per its own module
/// doc comment) or `finish_run`'s own reboot decision. A static `abort` of
/// `true` for the whole second call doesn't work here: `execute()`'s
/// whole-body race would drop the run before ever reaching `Stage::Revert`,
/// so it can't tell this fix apart from that unrelated, already-correct
/// behavior.
/// Confirmed to fail against the pre-fix code (reaches
/// `Ok(RunOutcome::RebootPending(..))` with `reboot_calls() == 2` and
/// `progress.abort_requested == false` instead of `Err`) before this fix was
/// applied.
#[tokio::test]
async fn finish_run_never_reboots_after_an_operator_abort_reaches_the_final_revert_reboot_decision()
{
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let (first, _) = run(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", vec![hags_scenario("hags")])),
    )
    .await;
    assert!(matches!(first.unwrap(), RunOutcome::RebootPending(_)));
    assert_eq!(
        mock.reboot_calls(),
        1,
        "the ApplyNext reboot happened as normal"
    );
    let quit_before_resume = mock.quit_cs2_calls();

    let config = fresh_config(dir.path(), "r1", vec![hags_scenario("hags")]);
    let progress = RunProgress::load(&run_dir).unwrap();

    let (events_tx, _events_rx) = mpsc::channel(1024);
    let (_control_tx, control_rx) = mpsc::channel(4);
    let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

    let exec = execute(
        sys,
        capture_for(3),
        RunStart::Resume { config, progress },
        events_tx,
        control_rx,
        abort_rx,
        shutdown_toggle_rx,
    );
    let armer = async {
        for _ in 0..120u32 {
            if mock.quit_cs2_calls() > quit_before_resume {
                abort_tx.send(true).unwrap();
                return;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        panic!("armer never saw hags's own quit_cs2");
    };
    let (result, ()) = tokio::join!(exec, armer);

    let err = result.unwrap_err();
    assert!(err.is_aborted(), "{err}");
    assert_eq!(
        mock.reboot_calls(),
        1,
        "only the first (ApplyNext) reboot must have happened -- the final revert reboot must \
         not fire once abort was observed"
    );
    let progress = RunProgress::load(&run_dir).unwrap();
    assert!(
        progress.abort_requested,
        "abort_requested must be persisted, matching the three scenario_loop sites' own pattern"
    );
}

/// `RunConfig::dry_run` was never read by the run loop: a dry run of a
/// reboot project called `reboot_sequence`, `DryRunController`'s
/// `reboot`/`register_task` no-op'd with `Ok`, and the run returned
/// `RebootPending` -- which the Tauri shell deliberately leaves `active_run`
/// parked on, waiting for an OS reboot that never comes. Under dry run a
/// reboot scenario must be walked in-process like a plain one.
#[tokio::test]
async fn a_dry_run_walks_reboot_scenarios_in_process_and_never_reboots() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let mut config = fresh_config(
        dir.path(),
        "r1",
        vec![hags_scenario("hags"), plain_scenario("p")],
    );
    config.dry_run = true;

    let (result, events) = run(sys, capture_for(9), RunStart::Fresh(config)).await;

    assert!(
        matches!(result.unwrap(), RunOutcome::Complete(_)),
        "a dry run must finish in one process, never stop at RebootPending"
    );
    assert_eq!(mock.reboot_calls(), 0);
    assert!(mock.registered_tasks().is_empty());
    assert!(events.iter().any(
        |e| matches!(e, EngineEvent::ScenarioComplete { result } if result.scenario_id == "hags")
    ));
    assert!(
        events.iter().any(|e| matches!(
            e,
            EngineEvent::LogLine { text } if text.starts_with("Dry run:") && text.contains("would reboot")
        )),
        "the skipped reboot must be announced"
    );
    assert!(
        !dir.path()
            .join("runs")
            .join("r1")
            .join("progress.json")
            .exists()
    );
}

/// A failed ROLLBACK leaves the journal in place for manual rollback -- and
/// the standalone `VOIDFRAME_RESTORE.bat` (docs/07 §5's recovery path for a
/// machine VOIDFRAME can no longer launch on) is that journal's human-
/// runnable form. `teardown` used to delete it unconditionally, right after
/// returning "journal left in place for manual rollback".
#[tokio::test]
async fn a_failed_rollback_keeps_the_restore_script() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let mut config = fresh_config(dir.path(), "r1", vec![plain_scenario("p")]);
    config.shutdown_when_complete = true;
    let restore_bat = dir
        .path()
        .join("recovery")
        .join(crate::journal::restore_script::RESTORE_SCRIPT_NAME);
    let exec = run(sys, capture_for(3), RunStart::Fresh(config));
    let armer = async {
        for _ in 0..100_000u32 {
            if mock.launch_cs2_calls() > 0 {
                assert!(
                    restore_bat.exists(),
                    "sanity: begin_fresh writes the recovery copy before any scenario runs"
                );
                mock.fail_next_write("disk full");
                mock.delete_power_plan(
                    "381b4222-f694-41f0-9685-ff5bb260df2e",
                    &MutationCtx {
                        run_id: "r1".into(),
                        scenario_id: "test-armer".into(),
                        step_index: 0,
                    },
                )
                .await
                .unwrap();
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("armer never saw launch_cs2");
    };
    let ((result, _events), ()) = tokio::join!(exec, armer);
    assert!(result.is_err());
    assert_eq!(
        mock.shutdown_calls(),
        0,
        "sanity: ROLLBACK itself must have failed (see the D8 test above)"
    );
    assert!(
        restore_bat.exists(),
        "the out-of-app restore script must survive a failed ROLLBACK"
    );
}

/// A run that completes normally still removes the restore script -- the
/// counterpart of `a_failed_rollback_keeps_the_restore_script`.
#[tokio::test]
async fn a_clean_run_removes_the_restore_script() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let sys: Arc<dyn SystemController> = Arc::new(ready_mock());
    let (result, _) = run(
        sys,
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", vec![plain_scenario("p")])),
    )
    .await;
    assert!(matches!(result.unwrap(), RunOutcome::Complete(_)));
    assert!(
        !dir.path()
            .join("recovery")
            .join(crate::journal::restore_script::RESTORE_SCRIPT_NAME)
            .exists()
    );
}

/// D3's `sys.shutdown(..)` used to be `?`-propagated after `results.json`
/// was already saved and ROLLBACK had already succeeded, turning a finished
/// run into `RunFailed` and skipping the `final_revert_needs_reboot` reboot
/// the last scenario's revert still needed.
#[tokio::test]
async fn a_shutdown_scheduling_failure_after_a_complete_run_still_reboots_for_the_final_revert() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock().with_shutdown_failing("no shutdown privilege");
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let scenarios = vec![hags_scenario("hags")];
    let (first, _) = run(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", scenarios.clone())),
    )
    .await;
    assert!(matches!(first.unwrap(), RunOutcome::RebootPending(_)));

    let mut progress = RunProgress::load(&run_dir).unwrap();
    progress.shutdown_when_complete = true;
    let config = fresh_config(dir.path(), "r1", scenarios);
    let (second, events) = run(sys, capture_for(3), RunStart::Resume { config, progress }).await;

    assert!(
        matches!(
            second.unwrap(),
            RunOutcome::RebootPending(RebootReason::RevertOnly)
        ),
        "the final-revert reboot must still fire when the shutdown could not be scheduled"
    );
    assert_eq!(mock.shutdown_calls(), 1);
    assert!(run_dir.join("results.json").exists());
    assert!(events.iter().any(
        |e| matches!(e, EngineEvent::LogLine { text } if text.contains("Could not schedule the shutdown"))
    ));
}

/// A Measure-stage failure of a reboot scenario force-reverts its registry
/// writes at ROLLBACK, but the running kernel still has the applied values
/// until the machine reboots -- and nothing on the failure path schedules
/// one. The operator has to be told.
#[tokio::test]
async fn a_failed_reboot_scenario_warns_that_its_revert_needs_a_reboot() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let run_dir = dir.path().join("runs").join("r1");
    let scenarios = vec![hags_scenario("hags")];
    let (first, _) = run(
        sys.clone(),
        capture_for(3),
        RunStart::Fresh(fresh_config(dir.path(), "r1", scenarios.clone())),
    )
    .await;
    assert!(matches!(first.unwrap(), RunOutcome::RebootPending(_)));

    // hags's own resumed measurement fails at its first capture.
    let progress = RunProgress::load(&run_dir).unwrap();
    let config = fresh_config(dir.path(), "r1", scenarios);
    let capture = MockCaptureRunner::new(vec![crate::mock_harness::mock_metrics(); 3]);
    capture.fail_next();
    let (second, events) = run(
        sys,
        Arc::new(capture),
        RunStart::Resume { config, progress },
    )
    .await;

    assert!(second.is_err());
    assert_eq!(
        mock.read_registry(&hags_key()).await.unwrap(),
        RegValue::Absent,
        "sanity: ROLLBACK force-reverted the in-flight reboot scenario"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            EngineEvent::LogLine { text } if text.contains("HAGS hags") && text.contains("need a reboot")
        )),
        "{events:#?}"
    );
}

/// The deadman (spec §5.2) is the safety net for exactly the window a
/// resume crosses first -- the post-boot settle wait -- so it must stay
/// registered until that wait and the boot classification are behind us,
/// not be deregistered as `begin_resume`'s opening move.
#[tokio::test]
async fn begin_resume_keeps_the_deadman_registered_through_the_settle_wait() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let mock = ready_mock();
    mock.register_task(&TaskSpec {
        name: DEADMAN_TASK_NAME.into(),
        description: "r1".into(),
        trigger: TaskTrigger::AtBootDelayed { minutes: 10 },
        principal: TaskPrincipal::LocalSystem,
        exe: PathBuf::from("voidframe.exe"),
        args: "--recover".into(),
    })
    .await
    .unwrap();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let mut config = fresh_config(dir.path(), "r1", vec![]);
    config.post_boot_settle = Duration::from_secs(180);
    let progress = progress_at_stage(config.project.clone(), Stage::Measure);

    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let mut cooldown_checks = Vec::new();

    let resume = async {
        let r = begin_resume(
            sys.as_ref(),
            &config,
            progress,
            &events_tx,
            &mut control_rx,
            &shutdown_toggle_rx,
            &abort_rx,
            &tokio::sync::watch::channel(false).0,
            &mut cooldown_checks,
            &std::sync::atomic::AtomicBool::new(false),
        )
        .await;
        drop(events_tx);
        r
    };
    let observer = async {
        // The first settle LogLine means the wait is under way.
        loop {
            if let EngineEvent::LogLine { text } = events_rx.recv().await.unwrap()
                && text.starts_with("Post-boot settle")
            {
                break;
            }
        }
        assert!(
            mock.registered_tasks()
                .iter()
                .any(|t| t.name == DEADMAN_TASK_NAME),
            "the deadman must still be registered while the settle wait is running"
        );
        // Keep draining so the bounded channel never blocks the resume.
        while events_rx.recv().await.is_some() {}
    };
    let (result, ()) = tokio::join!(resume, observer);
    result.unwrap();
    assert!(
        mock.registered_tasks().is_empty(),
        "once the settle wait and boot classification are done, the deadman is deregistered"
    );
}
