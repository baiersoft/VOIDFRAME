use super::*;

fn metrics(avg_fps: f64) -> Metrics {
    Metrics {
        avg_fps,
        median_fps: avg_fps,
        p1_fps: avg_fps * 0.8,
        p01_fps: avg_fps * 0.6,
        frame_time_mean_ms: 1000.0 / avg_fps,
        frame_time_stddev_ms: 0.1,
        frame_time_cv: 0.05,
        adaptive_frame_time_cv: 0.05,
        stutter_count_pct: 1.0,
        mean_abs_animation_error_ms: Some(0.1),
        gpu_busy_ms: None,
        bottleneck_ratio: None,
        render_latency_ms: None,
        dominant_present_mode: "Hardware: Independent Flip".into(),
        present_mode_consistent: true,
        present_mode_warning: None,
    }
}

/// Regression test for the affinity-timing reorder: CPU affinity must be
/// applied only after CS2's own menu-ready signal is observed, never right
/// after the process is merely discovered (a process existing is not the
/// same as the game having actually finished loading). Proven via real
/// timing under `tokio::time::pause()`, not just end-state: the menu-ready
/// console.log line is written only after a deliberate delay, and this
/// asserts `set_process_affinity` was called no earlier than that.
#[tokio::test]
async fn cpu_affinity_is_applied_only_after_menu_ready_not_right_after_process_discovery() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());
    // 0 warmup, 0 measure: only the pre-iteration launch flow (discovery ->
    // menu-ready -> affinity -> first send_console_command) matters for this test.
    let settings = settings_with(0, 0, 5);
    // Pre-seeded to already match the reconciled value (no launch_args
    // module on this scenario) -- this test is about affinity timing, not
    // the launch-args-differ relaunch flow, so it must not also trigger
    // that unrelated branch.
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&crate::cs2::keybind_cfg::reconcile(""))
        .with_process_on_launch("cs2.exe", 8001);
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = Scenario {
        id: "sc-affinity-timing".into(),
        name: "AffinityTiming".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![Module::AffinityCpu(AffinityCpuPayload {
            mode: AffinityMode::ExcludeCore0,
            mask_hex: None,
        })],
    };
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path.clone());
    let ctx = context_for(&config);
    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    let mut launched_pid = None;

    // Deliberately delayed: if affinity were still applied right after
    // process discovery (the old, buggy order), it would already have
    // happened well before this line ever lands, so
    // `affinity_applied_at()` would predate this write's own timestamp.
    let writer = async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        append_line(
            &console_log_path,
            "[SteamNetSockets] SDR RelayNetworkStatus:  avail=OK  config=OK  anyrelay=OK\n",
        )
        .await;
        tokio::time::Instant::now()
    };

    let (menu_ready_line_written_at, _) = tokio::join!(
        writer,
        run_scenario_inner(
            &sys,
            &capture,
            &scenario,
            &ctx,
            &events_tx,
            &mut control_rx,
            &mut launched_pid,
        )
    );

    let affinity_applied_at = sys
        .affinity_applied_at()
        .expect("affinity must have been applied for a scenario with an AffinityCpu module");
    assert!(
        affinity_applied_at >= menu_ready_line_written_at,
        "affinity was applied before the menu-ready line landed on disk -- \
         it must wait for menu-ready, not fire right after process discovery"
    );
}

#[tokio::test]
async fn recording_started_and_stopped_bracket_each_measure_iteration_only() {
    // RecordingStarted/RecordingStopped must fire exactly once per MEASURE
    // iteration (not warmup -- warmup never captures) so a listener (e.g.
    // voidframe-cli's --sound cue) can bracket precisely the real
    // PresentMon window, distinct from CapturePending/CaptureResumed's
    // wider settle+benchmark-end-wait window (which this test leaves
    // untouched and doesn't assert on).
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());

    let settings = settings_with(1, 2, 10); // 1 warmup, 2 measure
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 4242);
    let capture = MockCaptureRunner::new(vec![metrics(400.0), metrics(410.0)]);
    let scenario = scenario_with_modules("sc1");
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path.clone());
    let ctx = context_for(&config);

    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);

    let (result, ()) = tokio::join!(
        run_scenario(
            &sys,
            &capture,
            &scenario,
            false,
            &ctx,
            &events_tx,
            &mut control_rx,
        ),
        write_console_log_lines(console_log_path, 3), // 1 warmup + 2 measure
    );
    result.unwrap();
    drop(events_tx);

    let mut started = 0;
    let mut stopped = 0;
    while let Ok(ev) = events_rx.try_recv() {
        match ev {
            EngineEvent::RecordingStarted => started += 1,
            EngineEvent::RecordingStopped => stopped += 1,
            _ => {}
        }
    }
    assert_eq!(started, 2, "one RecordingStarted per measure iteration");
    assert_eq!(stopped, 2, "one RecordingStopped per measure iteration");
}

#[tokio::test]
async fn measure_captures_land_under_the_run_scenario_directory_not_a_temp_file() {
    // Every real capture's raw PresentMon CSV must be kept on disk under
    // this scenario's own directory (data_root/runs/<run_id>/scenario-<id>/)
    // instead of a leaked, never-cleaned-up temp file -- so a user can open
    // the recording in another tool, and VOIDFRAME's own aggregated metrics
    // can be independently validated against it. `MockCaptureRunner` never
    // touches the filesystem itself (it just returns canned `Metrics`,
    // ignoring `PmArgs` entirely), so this test can only prove the
    // directory-creation side effect -- the real file-writing is
    // `PresentMonController`'s job, not exercised here (same "not available
    // in CI" limitation this file's own doc comments already state for
    // every other real-PresentMon-dependent path).
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());

    let settings = settings_with(1, 2, 10);
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 4242);
    let capture = MockCaptureRunner::new(vec![metrics(400.0), metrics(410.0)]);
    let scenario = scenario_with_modules("sc1");
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path.clone());
    let ctx = context_for(&config);

    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);

    let (result, ()) = tokio::join!(
        run_scenario(
            &sys,
            &capture,
            &scenario,
            false,
            &ctx,
            &events_tx,
            &mut control_rx,
        ),
        write_console_log_lines(console_log_path, 3), // 1 warmup + 2 measure
    );
    result.unwrap();
    drop(events_tx);
    while events_rx.try_recv().is_ok() {}

    let capture_dir = dir.path().join("runs").join("r1").join("scenario-sc1");
    assert!(
        capture_dir.is_dir(),
        "expected the scenario's capture directory to exist at {}",
        capture_dir.display()
    );
}

#[tokio::test]
async fn happy_path_runs_warmup_and_measure_iterations_and_reverts_cleanly() {
    // Dust2's 8s `settle_before_capture` alone would add 8s of real wall-clock time
    // per measure iteration (3, here) — tokio's virtual clock collapses
    // every `sleep`/`timeout` in this test (this one and the several
    // already inside `write_console_log_lines`/`LogTail::wait_for`) to
    // whatever it actually takes to run, while preserving their real
    // relative ordering via auto-advance-on-idle.
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());

    let settings = settings_with(1, 3, 10);
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 4242)
        .with_powercfg("sub_processor", "IDLEDISABLE", AcDc { ac: 0, dc: 0 });
    let capture = MockCaptureRunner::new(vec![metrics(400.0), metrics(410.0), metrics(420.0)]);
    let scenario = scenario_with_modules("sc1");
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path.clone());
    let ctx = context_for(&config);

    let before = sys.snapshot();
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);

    let (result, ()) = tokio::join!(
        run_scenario(
            &sys,
            &capture,
            &scenario,
            false,
            &ctx,
            &events_tx,
            &mut control_rx,
        ),
        write_console_log_lines(console_log_path, 4),
    );

    let sr = result.unwrap();
    assert_eq!(sr.per_iteration.len(), 3);
    assert_eq!(
        sys.send_console_command_calls().len(),
        4,
        "1 warmup + 3 measure send_console_command calls"
    );
    assert_eq!(
        sys.hide_console_calls(),
        4,
        "console is closed once per iteration, after the map-load marker is confirmed"
    );
    assert_eq!(
        sys.launch_cs2_calls(),
        1,
        "CS2 launches once per scenario, not once per iteration"
    );
    assert_eq!(
        sys.quit_cs2_calls(),
        1,
        "graceful_kill_cs2 tries quit_cs2_gracefully before force-killing"
    );
    assert_eq!(
        sys.killed_pids(),
        vec![4242],
        "the mock's quit_cs2_gracefully doesn't remove the process by default, \
         so graceful_kill_cs2 still falls back to a force kill"
    );
    assert_eq!(
        before,
        sys.snapshot(),
        "registry + powercfg mutations must be fully reverted"
    );

    drop(events_tx);
    let mut iteration_complete = 0;
    let mut log_lines = 0;
    while let Ok(ev) = events_rx.try_recv() {
        match ev {
            EngineEvent::IterationComplete { .. } => iteration_complete += 1,
            EngineEvent::LogLine { .. } => log_lines += 1,
            _ => {}
        }
    }
    assert_eq!(iteration_complete, 3);
    assert!(log_lines > 0);
}

#[tokio::test]
async fn mutation_fails_mid_apply_reverts_and_never_launches_cs2() {
    let dir = tempfile::tempdir().unwrap();
    let settings = settings_with(1, 3, 5);
    let sys = MockController::new().with_steam_status(SteamStatus {
        running: true,
        elevated: false,
    });
    sys.fail_next_write("boom");
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = scenario_with_modules("sc2");
    let config = config_with(
        dir.path().to_path_buf(),
        settings,
        dir.path().join("console.log"),
    );

    let before = sys.snapshot();
    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);

    let result = run_scenario(
        &sys,
        &capture,
        &scenario,
        false,
        &context_for(&config),
        &events_tx,
        &mut control_rx,
    )
    .await;

    assert!(result.is_err());
    assert_eq!(
        before,
        sys.snapshot(),
        "the failed module (and anything applied before it) must be reverted"
    );
    assert_eq!(sys.launch_cs2_calls(), 0);
}

#[tokio::test]
async fn watchdog_timeout_kills_and_reverts_after_one_failed_retry() {
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    // Never appended to — the detection channel must genuinely time out.
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());

    let settings = settings_with(1, 3, 1); // watchdog_seconds = 1: fast test
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 9001);
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = Scenario {
        id: "sc3".into(),
        name: "Scenario Three".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![],
    };
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path);

    let before = sys.snapshot();
    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);

    let result = run_scenario(
        &sys,
        &capture,
        &scenario,
        false,
        &context_for(&config),
        &events_tx,
        &mut control_rx,
    )
    .await;

    let err = result.expect_err("watchdog should hard-fail after one failed retry");
    assert!(err.to_string().contains("watchdog"));
    assert_eq!(
        before,
        sys.snapshot(),
        "revert still runs on a hard failure"
    );
    assert!(
        sys.launch_cs2_calls() >= 2,
        "the initial launch plus at least one watchdog relaunch"
    );
    assert!(
        sys.killed_pids().len() >= 2,
        "the watchdog's own retry-kill plus run_scenario's final cleanup kill"
    );
}

/// A build-freeze-check failure (here: the operator's
/// control channel closing mid-prompt, the same failure mode as an
/// explicit `Abort`) must still revert this scenario's already-applied
/// mutations, and must never launch CS2 (the check now runs before
/// Steam-readiness, inside the same protected region as everything
/// else). The mocked `sys.app_manifest(730)` and `ctx.start_build_id`
/// below are scripted to genuinely differ, so this deterministically
/// drives the mismatch branch rather than depending on this machine's
/// real Steam/CS2 install.
#[tokio::test]
async fn build_freeze_mismatch_failure_still_reverts_and_never_launches_cs2() {
    let dir = tempfile::tempdir().unwrap();
    let settings = settings_with(1, 3, 5);
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_app_manifest(Some(AppManifest {
            build_id: Some("25000182".into()),
            state_flags: Some(4),
        }))
        // Pre-seed the powercfg key that `scenario_with_modules` writes,
        // matching `happy_path`'s own pattern — otherwise the "before"
        // snapshot has the key absent from the map entirely while the
        // "after" snapshot has it present-with-the-original-value (0),
        // which are semantically the same state but not `==` as
        // `MockSnapshot`s (a pre-existing mock quirk, unrelated to
        // the build-freeze-check revert fix above — apples-to-apples requires seeding either way).
        .with_powercfg("sub_processor", "IDLEDISABLE", AcDc { ac: 0, dc: 0 });
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = scenario_with_modules("sc4");
    let config = config_with(
        dir.path().to_path_buf(),
        settings,
        dir.path().join("console.log"),
    );
    let mut ctx = context_for(&config);
    // Guaranteed to differ from the mocked `app_manifest` build id above.
    ctx.start_build_id = Some("not-25000182".into());

    let before = sys.snapshot();
    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (control_tx, mut control_rx) = mpsc::channel(4);
    // Simulates the operator vanishing mid-prompt: `wait_for_operator_ack`
    // treats a closed channel the same as an explicit `Abort`, both `Err`.
    drop(control_tx);

    let result = run_scenario(
        &sys,
        &capture,
        &scenario,
        false,
        &ctx,
        &events_tx,
        &mut control_rx,
    )
    .await;

    assert!(
        result.is_err(),
        "the control channel closing mid-prompt must surface as a failure"
    );
    assert_eq!(
        before,
        sys.snapshot(),
        "APPLY_MODULES mutations must still be reverted even though the build-freeze \
         prompt itself is what failed — this is Finding 1's fix"
    );
    assert_eq!(
        sys.launch_cs2_calls(),
        0,
        "the build-freeze check runs before Steam-ready/launch, so CS2 must never launch"
    );

    let mut saw_build_prompt = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::OperatorPrompt { text } = ev
            && text.contains("build")
        {
            saw_build_prompt = true;
        }
    }
    assert!(
        saw_build_prompt,
        "the operator must have been prompted about the build mismatch"
    );
}

/// The webview quiet-window suspend must be resumed even
/// when the capture itself fails mid-iteration — not currently
/// reachable in this plan (`webview_root_pid` is always `None`
/// elsewhere), but exercised directly here via `RunConfig` so the fix
/// has real coverage rather than resting on code reading alone.
#[tokio::test]
async fn webview_suspend_is_resumed_even_when_capture_fails() {
    // See the identical comment on the happy-path test above.
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());

    let settings = settings_with(0, 1, 5); // no warmup, one measure iteration
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 5555);
    let capture = MockCaptureRunner::new(vec![metrics(400.0)]);
    capture.fail_next();
    let scenario = Scenario {
        id: "sc5".into(),
        name: "Scenario Five".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![],
    };
    let mut config = config_with(dir.path().to_path_buf(), settings, console_log_path.clone());
    config.webview_root_pid = Some(9999);
    let ctx = context_for(&config);

    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);

    let (result, ()) = tokio::join!(
        run_scenario(
            &sys,
            &capture,
            &scenario,
            false,
            &ctx,
            &events_tx,
            &mut control_rx,
        ),
        write_console_log_lines(console_log_path, 1),
    );

    assert!(result.is_err(), "the forced capture failure must propagate");
    assert!(
        !sys.suspended_pids().contains(&9999),
        "the webview tree must be resumed even though capture failed mid-iteration"
    );
}

#[tokio::test]
async fn run_scenario_inner_auto_launches_steam_when_not_running() {
    // With `cs2.exe` now discoverable immediately (below), this test
    // proceeds well past the Steam-readiness check it's meant to
    // exercise -- into the menu-ready wait and graceful-quit poll,
    // both real sleeps otherwise. Same collapsing technique and
    // rationale as `happy_path_runs_warmup_and_measure_iterations_and_reverts_cleanly`
    // above.
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 0, 5);
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: false,
            elevated: false,
        })
        .with_process_on_deelevate("steam.exe", 6001)
        .with_process("steamwebhelper.exe", 6099)
        .with_process_on_launch("cs2.exe", 6002)
        .with_launch_options(&desired_args);
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = Scenario {
        id: "sc-auto".into(),
        name: "Auto".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![],
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
        sys.deelevate_calls(),
        1,
        "Steam wasn't running -- the per-scenario check must have called ensure_steam_running"
    );
}

#[tokio::test]
async fn run_scenario_inner_warns_not_blocks_when_steam_elevated() {
    // Same real-sleep collapsing as the sibling test above -- this one
    // also runs past its own check into the menu-ready wait and
    // graceful-quit poll now that `cs2.exe` is discoverable.
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 0, 5);
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: true,
        })
        .with_process_on_launch("cs2.exe", 6003)
        .with_launch_options(&desired_args);
    let capture = MockCaptureRunner::new(vec![]);
    let scenario = Scenario {
        id: "sc-elevated".into(),
        name: "Elevated".into(),
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

    assert_eq!(
        sys.deelevate_calls(),
        0,
        "Steam is running (even though elevated) -- must never attempt a de-elevated launch"
    );
    assert_eq!(
        sys.launch_cs2_calls(),
        1,
        "must actually proceed past the Steam-readiness gate and launch CS2 -- proves \
         this isn't blocked the way the old code blocked"
    );
    let mut saw_elevated_warning = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.contains("elevated")
        {
            saw_elevated_warning = true;
        }
    }
    assert!(
        saw_elevated_warning,
        "must emit the informational LogLine about Steam running elevated"
    );
}

/// I1 (2026-09-07 follow-up review): `revert_stage`'s reverse replay must be
/// atomic with respect to operator cancellation. Abort is (correctly) still
/// offered during `Phase::Scenario`/`Phase::Baseline`, but an Abort landing
/// *inside* the replay would drop the run body with the journal only partly
/// reverted -- and since a revert never clears a record's `applied` flag,
/// that partial state is indistinguishable from an untouched journal, which
/// is exactly why `rollback::in_flight_journal` refuses to force-revert a
/// `Stage::Revert` cursor. The shield defers such an Abort instead of losing
/// the revert half-done.
///
/// Observes the real `true` -> `false` transition, not just "didn't panic",
/// following `mutation::power_plan`'s own shield test: the mock is given a
/// genuine `.await` inside `write_powercfg` (the one call this journal
/// record's revert makes), so the observer half of the `join!` runs while
/// the replay is still in flight and can read the shield as `true` at that
/// moment, then `false` once `revert_stage` returns. `tokio::time::pause()`
/// makes that ordering deterministic rather than wall-clock dependent.
#[tokio::test]
async fn revert_stage_shields_its_journal_replay_and_releases_it_afterwards() {
    const SUB: &str = "sub_processor";
    const SETTING: &str = "IDLEDISABLE";

    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let config = config_with(
        dir.path().to_path_buf(),
        settings_with(0, 0, 5),
        dir.path().join("console.log"),
    );
    let scenario = Scenario {
        id: "sc-revert-shield".into(),
        name: "RevertShield".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![Module::Powercfg {
            sub: SUB.into(),
            setting: SETTING.into(),
            value: 1,
        }],
    };
    let mock = MockController::new()
        .with_powercfg(SUB, SETTING, AcDc { ac: 0, dc: 0 })
        .with_write_powercfg_delay(Duration::from_millis(150));
    let (no_return_tx, mut no_return_rx) = tokio::sync::watch::channel(false);
    let mut ctx = context_for(&config);
    ctx.no_return = no_return_tx;
    let (events_tx, _events_rx) = mpsc::channel(64);

    // A real, fully-confirmed journal record for the replay to walk.
    // `apply_stage` engages no shield of its own for a powercfg module, so
    // the only `true` this test can observe below is `revert_stage`'s.
    apply_stage(&mock, &scenario, &ctx, &events_tx)
        .await
        .unwrap();
    assert_eq!(
        mock.read_powercfg(SUB, SETTING).await.unwrap().ac,
        1,
        "sanity: there must be something live for the replay to put back"
    );
    assert!(
        !*no_return_rx.borrow_and_update(),
        "apply must not leave a shield engaged"
    );

    let revert = revert_stage(&mock, &scenario, &ctx, &events_tx);
    // Bounded so a regression (the shield removed) fails here instead of
    // hanging. Under paused time this budget costs nothing when the shield
    // is present -- the `send(true)` happens before the mock's own `.await`,
    // so `changed()` resolves with no time advanced at all.
    let observer = async {
        tokio::time::timeout(Duration::from_secs(5), no_return_rx.changed())
            .await
            .expect("the shield must engage while the journal replay is in flight")
            .expect("the shield's sender must still be alive");
        *no_return_rx.borrow_and_update()
    };
    let (report, engaged_mid_replay) = tokio::join!(revert, observer);
    report.unwrap();

    assert!(
        engaged_mid_replay,
        "the shield must read `true` while the reverse replay is still awaiting"
    );
    assert!(
        !*no_return_rx.borrow(),
        "the shield must be released again once `revert_stage` returns -- a permanently-set \
         shield would make Abort dead for the rest of the run"
    );
    assert_eq!(
        mock.read_powercfg(SUB, SETTING).await.unwrap().ac,
        0,
        "sanity: the shielded replay must actually have reverted the record"
    );
}

/// AveYo's `benchmark.cfg` v2 must be triggered via its confirmed
/// single-run alias body (`alias set v2;sv_cheats 1;exec_async benchmark`),
/// never the Workshop-map `map_workshop ... de_dust2` command -- confirmed
/// directly with the project's maintainer, not a guess (see
/// `scenario.rs`'s `map_cmd` match arm doc comment).
#[tokio::test]
async fn aveyo_kind_triggers_the_confirmed_bb_command_instead_of_map_workshop() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());

    let mut settings = settings_with(1, 1, 10); // 1 warmup + 1 measure
    settings.benchmark_kind = crate::model::BenchmarkKind::AveYoCfgV2;
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 4242);
    let capture = MockCaptureRunner::new(vec![metrics(400.0)]);
    let scenario = scenario_with_modules("sc1");
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path.clone());
    let ctx = context_for(&config);

    let (events_tx, mut events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);

    let (result, ()) = tokio::join!(
        run_scenario(
            &sys,
            &capture,
            &scenario,
            false,
            &ctx,
            &events_tx,
            &mut control_rx,
        ),
        write_console_log_lines(console_log_path, 2), // 1 warmup + 1 measure
    );
    result.unwrap();
    drop(events_tx);
    while events_rx.try_recv().is_ok() {}

    let calls = sys.send_console_command_calls();
    assert_eq!(
        calls.len(),
        2,
        "1 warmup + 1 measure send_console_command call"
    );
    assert!(
        calls
            .iter()
            .all(|c| c == "alias set v2;sv_cheats 1;exec_async benchmark"),
        "expected only the confirmed AveYo BB trigger, got {calls:?}"
    );
}

#[tokio::test]
async fn aveyo_kind_writes_both_aveyo_cfg_files_alongside_the_keybind_cfg() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());

    let mut settings = settings_with(0, 1, 10);
    settings.benchmark_kind = crate::model::BenchmarkKind::AveYoCfgV2;
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 4242);
    let scenario = scenario_with_modules("sc1");
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path);
    let ctx = context_for(&config);

    // The cfg files are written by `prepare_cs2_session`, before any
    // iteration runs -- drive exactly that step, not a whole capture.
    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    prepare_cs2_session(&sys, &scenario, &ctx, &events_tx, &mut control_rx)
        .await
        .unwrap();

    let cfg_dir = dir.path().join("cfg");
    assert!(cfg_dir.join(crate::cs2::keybind_cfg::CFG_FILENAME).exists());
    assert!(
        cfg_dir
            .join(crate::cs2::aveyo_cfg::BENCHMARK_CFG_FILENAME)
            .exists()
    );
    assert!(
        cfg_dir
            .join(crate::cs2::aveyo_cfg::BENCHMARK2_CFG_FILENAME)
            .exists()
    );
}

#[tokio::test]
async fn workshop_dust2_kind_never_writes_the_aveyo_cfg_files() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());

    let settings = settings_with(0, 1, 10); // defaults to WorkshopDust2
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 4242);
    let scenario = scenario_with_modules("sc1");
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path);
    let ctx = context_for(&config);

    // The cfg files are written by `prepare_cs2_session`, before any
    // iteration runs -- drive exactly that step, not a whole capture.
    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel(4);
    prepare_cs2_session(&sys, &scenario, &ctx, &events_tx, &mut control_rx)
        .await
        .unwrap();

    let cfg_dir = dir.path().join("cfg");
    assert!(cfg_dir.join(crate::cs2::keybind_cfg::CFG_FILENAME).exists());
    assert!(
        !cfg_dir
            .join(crate::cs2::aveyo_cfg::BENCHMARK_CFG_FILENAME)
            .exists()
    );
    assert!(
        !cfg_dir
            .join(crate::cs2::aveyo_cfg::BENCHMARK2_CFG_FILENAME)
            .exists()
    );
}
