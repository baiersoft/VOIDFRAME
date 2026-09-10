use super::*;

#[tokio::test]
async fn preflight_block_stops_before_reaching_run_scenario() {
    let sys: Arc<dyn SystemController> = Arc::new(
        MockController::new()
            .with_steam_status(crate::system::SteamStatus {
                running: false,
                elevated: false,
            })
            .with_deelevate_failing("no interactive desktop"),
    );
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));
    let (_control_tx, control_rx) = tokio::sync::mpsc::channel(1);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

    let project = crate::model::project::Project {
        schema_version: crate::model::SCHEMA_VERSION.to_string(),
        id: "p".into(),
        name: "P".into(),
        description: "d".into(),
        created_at: "2026-09-01T00:00:00Z".into(),
        settings: serde_json::from_str("{}").unwrap(),
        baseline: crate::model::project::Baseline {
            name: "b".into(),
            description: "d".into(),
        },
        scenarios: vec![],
    };
    let dir = tempfile::tempdir().unwrap();
    let config = RunConfig {
        project,
        run_id: "r1".into(),
        dry_run: false,
        data_root: dir.path().to_path_buf(),
        webview_root_pid: None,
        console_log_override: None,
        mock_cs2_log: None,
        thermal_sample_override: None,
        inter_scenario_break_seconds: 0,
        hwinfo_path: None,
        shutdown_when_complete: false,
        post_boot_settle: Duration::from_secs(0),
        exe_path: PathBuf::from("voidframe.exe"),
    };

    // Preflight blocks -> execute() returns Err -> spawn_run turns that Err
    // into exactly one RunFailed (execute() itself no longer emits one).
    let mut handle = crate::run::spawn_run(
        sys,
        capture,
        crate::run::RunStart::Fresh(config),
        control_rx,
        shutdown_toggle_rx,
    );

    let mut saw_failed = false;
    while let Some(ev) = handle.events.recv().await {
        if matches!(ev, EngineEvent::RunFailed { .. }) {
            saw_failed = true;
        }
    }
    assert!(saw_failed);
    assert!(handle.join.await.unwrap().is_err());
}

/// Runs a baseline and a (clearly better) scenario end to end via
/// `run_scenario` -- the same two building blocks the happy-path test
/// above already exercises -- then feeds both into `score_scenarios`
/// and confirms real, non-placeholder `metric_deltas`/`wcps`/`verdict`, and
/// that the resulting `RunResults` genuinely round-trips through
/// `RunResults::save` at the expected `results.json` path.
///
/// Deliberately does not go through `execute()` itself: `execute` runs the
/// full phase sequencer end to end (PREFLIGHT through REPORT), which would
/// mean scripting a `MockController`/`MockCaptureRunner` through every
/// phase transition (the way `build_freeze_mismatch_failure_still_reverts_and_never_launches_cs2`
/// in `scenario.rs` scripts `.with_app_manifest(...)` for just one phase)
/// merely to reach the two functions this test actually wants to exercise.
/// `score_scenarios` and `rollback` are plain functions with no such
/// dependency, so they are exercised directly instead.
/// Per-iteration `Metrics` fixture for the scoring test below, with
/// independent (non-collinear) jitter across every throughput/pacing
/// dimension -- the shared `metrics()` helper above derives `p1_fps`/
/// `p01_fps` as an exact multiple of `avg_fps`, which makes the throughput
/// matrix perfectly collinear and defeats Hotelling's T² (WCPS v3's own
/// significance test needs real per-dimension variance, not a synthetic
/// single-variable fixture). Values reuse the exact pair already proven to
/// cross the n=3 calibrated threshold in `stats::v3::verdict`'s own
/// `a_clear_regression_is_worse_not_just_different` test -- swapped here
/// (the higher-performing set as `scenario`, the lower as `baseline`) so
/// this test can expect `Verdict::Better` by the same symmetry.
fn scoring_metrics(
    avg_fps: f64,
    p1_fps: f64,
    p01_fps: f64,
    adaptive_frame_time_cv: f64,
    stutter_count_pct: f64,
    mean_abs_animation_error_ms: f64,
) -> Metrics {
    Metrics {
        avg_fps,
        median_fps: avg_fps,
        p1_fps,
        p01_fps,
        frame_time_mean_ms: 1000.0 / avg_fps,
        frame_time_stddev_ms: 0.1,
        frame_time_cv: 0.05,
        adaptive_frame_time_cv,
        stutter_count_pct,
        mean_abs_animation_error_ms: Some(mean_abs_animation_error_ms),
        gpu_busy_ms: None,
        bottleneck_ratio: None,
        render_latency_ms: None,
        dominant_present_mode: "Hardware: Independent Flip".into(),
        present_mode_consistent: true,
        present_mode_warning: None,
    }
}

#[tokio::test]
async fn scoring_computes_real_comparisons_and_results_json_round_trips() {
    // See the identical comment on the happy-path test above — this one
    // needs it even more (2 scenarios x 3 measure iterations each).
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    let console_log_path = dir.path().join("console.log");
    tokio::fs::write(&console_log_path, "").await.unwrap();
    write_signatures(dir.path());

    let settings = settings_with(0, 3, 10); // no warmup, 3 measure iterations
    let desired_args = crate::cs2::keybind_cfg::reconcile("");
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 1111);
    let config = config_with(dir.path().to_path_buf(), settings, console_log_path.clone());
    let ctx = context_for(&config);

    let baseline_scenario = Scenario {
        id: "baseline".into(),
        name: "Stock".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![],
    };
    let baseline_capture = MockCaptureRunner::new(vec![
        scoring_metrics(550.0, 200.0, 170.0, 0.55, 12.0, 0.40),
        scoring_metrics(548.0, 199.0, 169.0, 0.56, 12.2, 0.41),
        scoring_metrics(551.0, 201.0, 171.0, 0.55, 11.8, 0.39),
    ]);
    let (events_tx, _events_rx) = mpsc::channel(64);
    let (_control_tx, mut control_rx) = mpsc::channel::<ControlMsg>(4);
    let (baseline_result, ()) = tokio::join!(
        run_scenario(
            &sys,
            &baseline_capture,
            &baseline_scenario,
            true,
            &ctx,
            &events_tx,
            &mut control_rx,
        ),
        write_console_log_lines(console_log_path.clone(), 3),
    );
    let baseline_result = baseline_result.unwrap();

    let scenario = Scenario {
        id: "sc1".into(),
        name: "Scenario One".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![],
    };
    // Clearly better on every scored axis than the baseline above.
    let scenario_capture = MockCaptureRunner::new(vec![
        scoring_metrics(880.0, 338.0, 298.0, 0.41, 7.0, 0.22),
        scoring_metrics(878.0, 337.0, 297.0, 0.41, 7.1, 0.23),
        scoring_metrics(881.0, 339.0, 299.0, 0.42, 6.9, 0.21),
    ]);
    let (events_tx2, _events_rx2) = mpsc::channel(64);
    let (_control_tx2, mut control_rx2) = mpsc::channel::<ControlMsg>(4);
    let (scenario_result, ()) = tokio::join!(
        run_scenario(
            &sys,
            &scenario_capture,
            &scenario,
            false,
            &ctx,
            &events_tx2,
            &mut control_rx2,
        ),
        write_console_log_lines(console_log_path, 3),
    );
    let scenario_result = scenario_result.unwrap();
    assert_eq!(scenario_result.metric_deltas, vec![]);
    assert_eq!(
        scenario_result.wcps, 0.0,
        "still the pre-scoring placeholder"
    );
    assert_eq!(
        scenario_result.verdict,
        Verdict::ConfirmedSame,
        "still the pre-scoring placeholder"
    );

    let mut scenario_results = vec![scenario_result];
    score_scenarios(&baseline_result.per_iteration, &mut scenario_results).unwrap();

    let scored = &scenario_results[0];
    assert_eq!(
        scored.metric_deltas.len(),
        6,
        "4 throughput + 2 pacing metric deltas"
    );
    let names: Vec<&str> = scored
        .metric_deltas
        .iter()
        .map(|c| c.metric.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "avg_fps",
            "p1_fps",
            "p01_fps",
            "adaptive_frame_time_cv",
            "stutter_count_pct",
            "mean_abs_animation_error_ms",
        ]
    );
    assert!(scored.wcps.is_finite());
    assert!(
        scored.wcps > 0.0,
        "clearly-better scenario data should score positive, got {}",
        scored.wcps
    );
    assert_eq!(
        scored.verdict,
        Verdict::Better,
        "clearly-better scenario data should verdict Better, got {:?}",
        scored.verdict
    );

    let results = RunResults {
        schema_version: crate::model::SCHEMA_VERSION.to_string(),
        run_id: config.run_id.clone(),
        project_id: "p1".into(),
        completed_at: crate::model::results::utc_timestamp_now(),
        detection_tier: DetectionTier::LogTail,
        baseline: baseline_result,
        scenarios: scenario_results,
        unstable: vec![],
    };
    let results_path = dir
        .path()
        .join("runs")
        .join(&config.run_id)
        .join("results.json");
    results.save(&results_path).unwrap();
    let back: RunResults = serde_json::from_slice(&std::fs::read(&results_path).unwrap()).unwrap();
    assert_eq!(back, results, "results.json must round-trip byte-for-value");
}

fn rollback_journal_path(data_root: &Path, run_id: &str, scenario_id: &str) -> PathBuf {
    data_root
        .join("runs")
        .join(run_id)
        .join(format!("journal-{scenario_id}.jsonl"))
}

/// The normal case: every record in the run's journal file(s) was
/// already confirmed applied (and, in real operation, already reverted
/// by its own `run_scenario` call) -- `rollback`'s defensive sweep must
/// leave it alone (no `RollbackProgress` event), while still restoring
/// the run-start power plan unconditionally.
#[tokio::test]
async fn rollback_restores_power_plan_and_skips_a_fully_confirmed_journal() {
    let dir = tempfile::tempdir().unwrap();
    let sys = MockController::new();
    let other_plan = sys.active_power_plan().await.unwrap();
    let run_start_plan = PowerPlan {
        guid: "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c".into(), // "High performance" in default_plans()
        name: "High performance".into(),
        active: false,
    };
    assert_ne!(other_plan.guid, run_start_plan.guid);

    let jp = rollback_journal_path(dir.path(), "r1", "sc1");
    let mut j = Journal::open(&jp).unwrap();
    let seq = j
        .record(
            crate::journal::Op::PowercfgWrite,
            &MutationCtx {
                run_id: "r1".into(),
                scenario_id: "sc1".into(),
                step_index: 0,
            },
            serde_json::json!({"sub": "sub_processor", "setting": "IDLEDISABLE"}),
            serde_json::json!(1),
            serde_json::json!({"sub": "sub_processor", "setting": "IDLEDISABLE", "value": 0}),
        )
        .unwrap();
    j.mark_applied(seq).unwrap();
    drop(j);

    let config = RunConfig {
        project: project_with(settings_with(0, 0, 5)),
        run_id: "r1".into(),
        dry_run: false,
        data_root: dir.path().to_path_buf(),
        webview_root_pid: None,
        console_log_override: None,
        mock_cs2_log: None,
        thermal_sample_override: None,
        inter_scenario_break_seconds: 0,
        hwinfo_path: None,
        shutdown_when_complete: false,
        post_boot_settle: Duration::from_secs(0),
        exe_path: PathBuf::from("voidframe.exe"),
    };
    let mut ctx = context_for(&config);
    ctx.start_power_plan = run_start_plan.clone();
    let (events_tx, mut events_rx) = mpsc::channel(16);

    rollback(
        &sys,
        &ctx,
        &progress_at(&config, 0, Stage::Done),
        &events_tx,
    )
    .await
    .unwrap();

    assert_eq!(
        sys.active_power_plan().await.unwrap().guid,
        run_start_plan.guid,
        "the run-start power plan must be restored unconditionally"
    );
    drop(events_tx);
    let mut saw_progress = false;
    while let Ok(ev) = events_rx.try_recv() {
        if matches!(ev, EngineEvent::RollbackProgress { .. }) {
            saw_progress = true;
        }
    }
    assert!(
        !saw_progress,
        "a fully-confirmed journal must not trigger a defensive revert"
    );
}

/// The defensive-recovery case: a journal file left with an unconfirmed
/// (`applied: false`) record -- as if an earlier apply or revert was
/// itself interrupted -- must be swept up by `rollback`: reverted for
/// real, with a `RollbackProgress` event.
#[tokio::test]
async fn rollback_defensively_reverts_a_journal_left_with_unconfirmed_entries() {
    let dir = tempfile::tempdir().unwrap();
    let sys = MockController::new().with_powercfg(
        "sub_processor",
        "IDLEDISABLE",
        AcDc { ac: 1, dc: 1 }, // as if the (never-confirmed) mutation did land
    );
    let run_start_plan = PowerPlan {
        guid: "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c".into(),
        name: "High performance".into(),
        active: false,
    };

    let jp = rollback_journal_path(dir.path(), "r1", "sc1");
    let mut j = Journal::open(&jp).unwrap();
    // Deliberately never `mark_applied` -- simulates a record written
    // but never confirmed, e.g. a crash right after the real mutation
    // landed but before the journal recorded that fact.
    j.record(
        crate::journal::Op::PowercfgWrite,
        &MutationCtx {
            run_id: "r1".into(),
            scenario_id: "sc1".into(),
            step_index: 0,
        },
        serde_json::json!({"sub": "sub_processor", "setting": "IDLEDISABLE"}),
        serde_json::json!(1),
        serde_json::json!({"sub": "sub_processor", "setting": "IDLEDISABLE", "value": 0}),
    )
    .unwrap();
    drop(j);

    let config = RunConfig {
        project: project_with(settings_with(0, 0, 5)),
        run_id: "r1".into(),
        dry_run: false,
        data_root: dir.path().to_path_buf(),
        webview_root_pid: None,
        console_log_override: None,
        mock_cs2_log: None,
        thermal_sample_override: None,
        inter_scenario_break_seconds: 0,
        hwinfo_path: None,
        shutdown_when_complete: false,
        post_boot_settle: Duration::from_secs(0),
        exe_path: PathBuf::from("voidframe.exe"),
    };
    let mut ctx = context_for(&config);
    ctx.start_power_plan = run_start_plan;
    let (events_tx, mut events_rx) = mpsc::channel(16);

    rollback(
        &sys,
        &ctx,
        &progress_at(&config, 0, Stage::Done),
        &events_tx,
    )
    .await
    .unwrap();

    assert_eq!(
        sys.read_powercfg("sub_processor", "IDLEDISABLE")
            .await
            .unwrap()
            .ac,
        0,
        "the unconfirmed mutation must genuinely be reverted"
    );
    drop(events_tx);
    let mut progress_events = vec![];
    let mut saw_sweep_narration = false;
    while let Ok(ev) = events_rx.try_recv() {
        match ev {
            EngineEvent::RollbackProgress { reverted } => progress_events.push(reverted),
            EngineEvent::LogLine { text } if text.to_lowercase().contains("unconfirmed") => {
                saw_sweep_narration = true;
            }
            _ => {}
        }
    }
    assert_eq!(
        progress_events,
        vec![1],
        "exactly one RollbackProgress event, for the one file actually processed"
    );
    assert!(
        saw_sweep_narration,
        "expected a LogLine narrating the defensive sweep of an unconfirmed journal"
    );
}

#[tokio::test]
async fn steam_launch_check_reports_every_step_ok_on_the_happy_path() {
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process_on_launch("cs2.exe", 8001);
    let report = steam_launch_check(&sys, false).await;
    assert!(report.overall_ok, "steps: {:?}", report.steps);
    assert_eq!(report.steps.len(), 3);
    assert!(
        report.steps.iter().all(|s| s.ok),
        "steps: {:?}",
        report.steps
    );
    assert_eq!(
        sys.close_steam_window_calls(),
        0,
        "Steam was already running"
    );
}

#[tokio::test]
async fn steam_launch_check_records_the_failing_step_without_panicking() {
    tokio::time::pause();
    let sys = MockController::new().with_steam_status(SteamStatus {
        running: true,
        elevated: false,
    });
    // cs2.exe never becomes discoverable -- launch_cs2 "succeeds" (mock
    // default) but wait_for_process times out.
    let report = steam_launch_check(&sys, false).await;
    assert!(!report.overall_ok);
    let last = report.steps.last().unwrap();
    assert_eq!(last.name, "launch_cs2");
    assert!(!last.ok);
    assert!(last.detail.contains("cs2.exe"), "detail: {}", last.detail);
}

#[tokio::test]
async fn steam_launch_check_kills_cs2_after_when_asked() {
    tokio::time::pause();
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process_on_launch("cs2.exe", 8002);
    let report = steam_launch_check(&sys, true).await;
    assert!(report.overall_ok);
    assert!(
        sys.find_process("cs2.exe").await.unwrap().is_none(),
        "cs2.exe should have been killed after the check when kill_cs2_after is true"
    );
}

fn dummy_hwinfo_path() -> std::path::PathBuf {
    std::path::PathBuf::from("C:\\fake\\hwinfo.exe")
}

#[tokio::test]
async fn hwinfo_check_reports_every_step_ok_on_the_happy_path() {
    let sys = MockController::new().with_hwinfo_reading(61.5, Some(52.0)); // not already running
    let report = hwinfo_check(&sys, &dummy_hwinfo_path()).await;
    assert!(report.overall_ok, "steps: {:?}", report.steps);
    assert_eq!(report.steps.len(), 4);
    assert!(
        report.steps.iter().all(|s| s.ok),
        "steps: {:?}",
        report.steps
    );
    assert_eq!(sys.start_hwinfo_calls(), 1);
    assert_eq!(sys.close_hwinfo_calls(), 1);
    let read_step = &report.steps[2];
    assert_eq!(read_step.name, "read_hwinfo_sensors");
    assert!(
        read_step.detail.contains("61.5") && read_step.detail.contains("52.0"),
        "detail: {}",
        read_step.detail
    );
}

#[tokio::test]
async fn hwinfo_check_skips_start_and_close_when_already_running() {
    let sys = MockController::new().with_process("HWiNFO64.exe", 4242); // already running
    let report = hwinfo_check(&sys, &dummy_hwinfo_path()).await;
    assert!(report.overall_ok, "steps: {:?}", report.steps);
    assert_eq!(report.steps.len(), 2);
    assert_eq!(report.steps[0].name, "hwinfo_already_running");
    assert_eq!(report.steps[1].name, "read_hwinfo_sensors");
    assert_eq!(sys.start_hwinfo_calls(), 0, "HWiNFO was already running");
    assert_eq!(sys.close_hwinfo_calls(), 0, "HWiNFO was already running");
}

#[tokio::test]
async fn hwinfo_check_records_a_failing_read_but_still_closes_what_it_started() {
    let sys = MockController::new().with_hwinfo_read_failing_after(0); // not already running
    let report = hwinfo_check(&sys, &dummy_hwinfo_path()).await;
    assert!(!report.overall_ok);
    let read_step = report
        .steps
        .iter()
        .find(|s| s.name == "read_hwinfo_sensors")
        .unwrap();
    assert!(!read_step.ok);
    assert!(
        read_step.detail.contains("hwinfo sensor read failed"),
        "detail: {}",
        read_step.detail
    );
    // Cleanup must still run even though the read step failed -- this call
    // started HWiNFO, so it must also close it.
    assert_eq!(sys.close_hwinfo_calls(), 1);
}

/// The engine-side gate for `Settings::validate_capture_window`: a
/// hand-written AveYo project that kept Dust2's 105s capture default must
/// fail at PREFLIGHT, before Steam or CS2 are ever touched.
#[tokio::test]
async fn an_aveyo_project_with_an_oversized_capture_fails_at_preflight() {
    let mock = MockController::new();
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));
    let (_control_tx, control_rx) = tokio::sync::mpsc::channel(1);
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

    let mut settings = settings_with(0, 3, 5);
    settings.benchmark_kind = crate::model::BenchmarkKind::AveYoCfgV2;
    assert_eq!(
        settings.capture_seconds, 105,
        "sanity: the kind-blind serde default"
    );
    let dir = tempfile::tempdir().unwrap();
    let config = config_with(
        dir.path().to_path_buf(),
        settings,
        dir.path().join("console.log"),
    );

    let mut handle = crate::run::spawn_run(
        sys,
        capture,
        crate::run::RunStart::Fresh(config),
        control_rx,
        shutdown_toggle_rx,
    );
    let mut failure = None;
    while let Some(ev) = handle.events.recv().await {
        if let EngineEvent::RunFailed { reason } = ev {
            failure = Some(reason);
        }
    }
    let reason = failure.expect("the run must fail");
    assert!(reason.contains("capture_seconds"), "{reason}");
    assert_eq!(
        mock.launch_cs2_calls(),
        0,
        "refused before anything was launched"
    );
}
