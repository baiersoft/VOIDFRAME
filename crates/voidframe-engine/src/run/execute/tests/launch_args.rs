use super::*;

#[tokio::test]
async fn launch_args_differ_closes_edits_and_relaunches_steam_automatically() {
    // Killing the mocked steam.exe flips `steam_status.running` to
    // false (see `MockController::kill_process_tree`), so this test's
    // automated relaunch runs through `ensure_steam_running` for real.
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 0, 5);
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 7001)
        .with_process_on_deelevate("steam.exe", 7002)
        .with_process("steamwebhelper.exe", 7099)
        // Deliberately NOT pre-seeded with the desired args -- this is
        // what triggers the "differ" branch.
        .with_launch_options("-some-stale-arg");
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = Scenario {
        id: "sc-launchargs".into(),
        name: "LaunchArgs".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![Module::LaunchArgs {
            args: "-novid".into(),
        }],
    };
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path);
    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    let mut launched_pid = None;

    let _ = run_scenario_inner(
        &sys,
        &capture,
        &scenario,
        &context_for(&config),
        &events_tx,
        &mut control_rx,
        &mut launched_pid,
    )
    .await;

    assert_eq!(
        sys.killed_pids(),
        vec![7001],
        "the stale-args Steam process must be killed before rewriting launch options"
    );
    let desired = crate::cs2::keybind_cfg::reconcile("-novid");
    assert_eq!(sys.read_cs2_launch_options().await.unwrap(), desired);
    assert_eq!(
        sys.deelevate_calls(),
        1,
        "must attempt a de-elevated relaunch after writing the new options"
    );
}

/// Regression test for the actual overwrite bug: a scenario with NO
/// `launch_args` module must not wipe the user's own real Steam launch
/// options -- it should only ever ADD VOIDFRAME's reserved tokens on top of
/// whatever the user already has, never replace it.
#[tokio::test]
async fn scenario_with_no_launch_args_module_preserves_the_users_existing_options() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 0, 5);
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 7201)
        .with_process_on_deelevate("steam.exe", 7202)
        .with_process("steamwebhelper.exe", 7299)
        // The user's own real launch options, set entirely outside
        // VOIDFRAME (no reserved tokens present) -- the bug this test
        // guards against was VOIDFRAME silently wiping this to just its
        // own reserved tokens whenever a scenario had no launch_args
        // module at all.
        .with_launch_options("-high -threads 8");
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = Scenario {
        id: "sc-no-launchargs".into(),
        name: "NoLaunchArgs".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![],
    };
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path);
    // Simulates `execute()`'s own SNAPSHOT-phase capture, taken before this
    // scenario runs -- the fallback below reads this, not a live
    // `read_cs2_launch_options()`, so it must be seeded to match what's
    // mocked as already live on this machine.
    let mut ctx = context_for(&config);
    ctx.start_launch_args = "-high -threads 8".into();
    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    let mut launched_pid = None;

    let _ = run_scenario_inner(
        &sys,
        &capture,
        &scenario,
        &ctx,
        &events_tx,
        &mut control_rx,
        &mut launched_pid,
    )
    .await;

    let expected = crate::cs2::keybind_cfg::reconcile("-high -threads 8");
    assert_eq!(
        sys.read_cs2_launch_options().await.unwrap(),
        expected,
        "the user's own launch options must survive, with only VOIDFRAME's reserved tokens added"
    );
}

/// Regression test for the reconcile-narration gap: a scenario whose
/// launch options actually change must narrate the before/after values as
/// a `LogLine`, so the run's own narrative (and the per-run log file it
/// feeds) shows what changed, not just that a kill/relaunch happened.
#[tokio::test]
async fn launch_args_differ_emits_a_before_after_log_line() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 0, 5);
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 7301)
        .with_process_on_deelevate("steam.exe", 7302)
        .with_process("steamwebhelper.exe", 7399)
        .with_launch_options("-some-stale-arg");
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = Scenario {
        id: "sc-launchargs-narration".into(),
        name: "LaunchArgsNarration".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![Module::LaunchArgs {
            args: "-novid".into(),
        }],
    };
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    let mut launched_pid = None;

    let _ = run_scenario_inner(
        &sys,
        &capture,
        &scenario,
        &context_for(&config),
        &events_tx,
        &mut control_rx,
        &mut launched_pid,
    )
    .await;

    let mut saw_narration = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.contains("-some-stale-arg")
            && text.contains(&crate::cs2::keybind_cfg::reconcile("-novid"))
        {
            saw_narration = true;
        }
    }
    assert!(
        saw_narration,
        "expected a LogLine narrating both the before and after launch options"
    );
}

/// The mirror case: when a scenario's effective launch options already
/// match what's live, that must also be narrated (as "unchanged"), not
/// silently skipped -- otherwise the run's narrative has no record of the
/// reconciliation having been checked at all.
#[tokio::test]
async fn launch_args_unchanged_emits_a_log_line() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 0, 5);
    let already_reconciled = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 7401)
        .with_process("steamwebhelper.exe", 7499)
        .with_launch_options(&already_reconciled);
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = Scenario {
        id: "sc-launchargs-unchanged".into(),
        name: "LaunchArgsUnchanged".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![],
    };
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    let mut launched_pid = None;

    let _ = run_scenario_inner(
        &sys,
        &capture,
        &scenario,
        &context_for(&config),
        &events_tx,
        &mut control_rx,
        &mut launched_pid,
    )
    .await;

    assert!(
        sys.killed_pids().is_empty(),
        "unchanged launch options must never trigger a Steam kill/relaunch"
    );
    let mut saw_unchanged_narration = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.to_lowercase().contains("launch options")
            && text.to_lowercase().contains("unchanged")
        {
            saw_unchanged_narration = true;
        }
    }
    assert!(
        saw_unchanged_narration,
        "expected a LogLine narrating that launch options were already unchanged"
    );
}

#[tokio::test]
async fn launch_args_differ_falls_back_to_manual_prompt_when_relaunch_fails() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 0, 5);
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 7101)
        .with_deelevate_failing("no interactive desktop")
        .with_launch_options("-some-stale-arg");
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = Scenario {
        id: "sc-launchargs-fallback".into(),
        name: "LaunchArgsFallback".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![Module::LaunchArgs {
            args: "-novid".into(),
        }],
    };
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path);
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (control_tx, mut control_rx) = mpsc::channel(4);
    let mut launched_pid = None;
    // Operator vanishes mid-prompt -- same technique
    // `build_freeze_mismatch_failure_still_reverts_and_never_launches_cs2`
    // uses: `wait_for_operator_ack` treats a closed channel as Abort.
    drop(control_tx);

    let result = run_scenario_inner(
        &sys,
        &capture,
        &scenario,
        &context_for(&config),
        &events_tx,
        &mut control_rx,
        &mut launched_pid,
    )
    .await;

    assert!(
        result.is_err(),
        "the operator vanishing during the fallback prompt must surface as a failure"
    );
    assert_eq!(sys.killed_pids(), vec![7101]);
    let mut saw_fallback_prompt = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::OperatorPrompt { text } = ev
            && text.contains("reopen Steam")
        {
            saw_fallback_prompt = true;
        }
    }
    assert!(
        saw_fallback_prompt,
        "must fall back to the manual reopen-Steam prompt when the automated relaunch fails"
    );
}

/// Regression test for docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes.md task 7:
/// a run whose last executed scenario changed the CS2 launch options via a
/// `Module::LaunchArgs` must leave Steam's `LaunchOptions` restored to the
/// run-start value once ROLLBACK runs, not the scenario's own string. Drives
/// a full `execute()` (baseline -> one scenario -> rollback -> report), with
/// `settings_with`'s `measure_loops` set to `3` -- the smallest value on
/// `stats::v3::CALIBRATED_N_LADDER` -- so scoring at the end of `execute()`'s
/// own `body` succeeds and ROLLBACK is actually reached on the success path
/// (every other test in this file only ever drives `run_scenario_inner`
/// directly, which never reaches ROLLBACK at all).
#[tokio::test]
async fn rollback_restores_run_start_launch_options_after_a_scenario_changed_them() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 3, 5);
    // The run-start options: already fully reconciled (no bare user args),
    // so BASELINE (no `launch_args` module) sees no diff against them and
    // never has to kill/relaunch Steam itself -- only the scenario below
    // does, keeping this test's own Steam-restart bookkeeping to one cycle
    // instead of two.
    let start_args = crate::cs2::keybind_cfg::reconcile("");
    let mock = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 9001)
        .with_process_on_deelevate("steam.exe", 9002)
        .with_process("steamwebhelper.exe", 9099)
        .with_process_on_launch("cs2.exe", 9500)
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
        id: "sc-launchargs-rollback".into(),
        name: "LaunchArgsRollback".into(),
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

    result.expect("run should complete successfully");

    assert_eq!(
        mock.read_cs2_launch_options().await.unwrap(),
        start_args,
        "ROLLBACK must restore the run-start launch options, not leave the scenario's own \
         string in place"
    );
}

/// Regression test for Important 1 in
/// docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes/final-fix-brief.md: a run whose
/// run-start launch options are un-reconciled (a first run against real
/// user-set options such as `-high -threads 8`) and that fails BEFORE
/// BASELINE's own `prepare_cs2_session` ever writes anything must leave
/// ROLLBACK a no-op -- not kill Steam and write reserved tokens into a
/// config nothing here actually changed. No `steam.exe` process is
/// registered, so BASELINE's own launch-args "Differ" branch skips the kill
/// and goes straight to `wait_until_steam_closed`, which polls
/// `steam_status().running` (left `true` forever, since nothing here ever
/// flips it) and times out -- failing inside
/// `write_launch_options_with_steam_closed`, strictly before its own
/// `write_cs2_launch_options` call.
#[tokio::test]
async fn rollback_does_not_restore_when_baseline_never_reconciled_launch_options() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 0, 5);
    let mock = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options("-high -threads 8");
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));

    let mut config = config_with(
        dir.path().to_path_buf(),
        settings,
        dir.path().join("console.log"),
    );
    config.mock_cs2_log = Some(Duration::from_millis(0));

    let (events_tx, events_rx) = mpsc::channel(256);
    let (_control_tx, control_rx) = mpsc::channel(4);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

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

    assert!(
        result.is_err(),
        "expected BASELINE's own Steam-close wait to time out before ever writing"
    );
    assert_eq!(
        mock.write_launch_options_calls(),
        0,
        "ROLLBACK must not write launch options when BASELINE never got as far as its own write"
    );
    assert_eq!(
        mock.read_cs2_launch_options().await.unwrap(),
        "-high -threads 8",
        "the user's launch options must be left exactly as they were at run start"
    );
}

/// A run whose scenarios (baseline included) never carry a
/// `Module::LaunchArgs` still gets its launch options restored to the true
/// pristine pre-run string at ROLLBACK -- not left at whatever BASELINE's
/// own reconciled write produced. `write_launch_options_calls()` must be 2:
/// BASELINE's own write, then ROLLBACK's restore.
#[tokio::test]
async fn rollback_restores_pristine_options_even_when_no_scenario_ever_had_a_launch_args_module() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 3, 5);
    // Deliberately NOT already reconciled -- this is what makes BASELINE
    // itself perform exactly one kill/write/relaunch cycle (BASELINE's own
    // write, the first of the two this run now makes -- see this test's
    // own tail assertion for the second, ROLLBACK's restore).
    let mock = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 9101)
        .with_process_on_deelevate("steam.exe", 9102)
        .with_process("steamwebhelper.exe", 9199)
        .with_process_on_launch("cs2.exe", 9600)
        .with_launch_options("-high -threads 8");
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
        id: "sc-no-launchargs-rollback".into(),
        name: "NoLaunchArgsRollback".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![],
    });

    let (events_tx, events_rx) = mpsc::channel(256);
    let (_control_tx, control_rx) = mpsc::channel(4);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

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

    result.expect("run should complete successfully");

    assert_eq!(
        mock.read_cs2_launch_options().await.unwrap(),
        "-high -threads 8",
        "ROLLBACK must restore the true pristine pre-run string, not leave BASELINE's own \
         reconciled write in place"
    );
    assert_eq!(
        mock.write_launch_options_calls(),
        2,
        "BASELINE's own write, then ROLLBACK's restore -- exactly 2 writes total"
    );
}
