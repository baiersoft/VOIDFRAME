use super::*;

fn scoring_metrics(avg: f64, p1: f64, p01: f64, cv: f64, stutter: f64, anim_err: f64) -> Metrics {
    Metrics {
        avg_fps: avg,
        median_fps: avg,
        p1_fps: p1,
        p01_fps: p01,
        frame_time_mean_ms: 1000.0 / avg,
        frame_time_stddev_ms: 0.1,
        frame_time_cv: cv,
        adaptive_frame_time_cv: cv,
        stutter_count_pct: stutter,
        mean_abs_animation_error_ms: Some(anim_err),
        gpu_busy_ms: None,
        bottleneck_ratio: None,
        render_latency_ms: None,
        dominant_present_mode: "Hardware: Independent Flip".into(),
        present_mode_consistent: true,
        present_mode_warning: None,
    }
}

/// Regression test for the run.log/EngineEvent mismatch found via real user
/// data (study/pamuk/ab-test-analysis-report.md §3): `ScenarioComplete`
/// fires with a not-yet-scored placeholder BEFORE `score_scenarios()` runs,
/// so run.log always showed `wcps=0.00 verdict=confirmed_same` for every
/// scenario regardless of the real outcome. `EngineEvent::ScenarioScored`
/// (emitted once per non-baseline scenario from `finish_run`, after real
/// scoring) must carry the exact same `wcps`/`verdict` the final
/// `RunResults` reports.
#[tokio::test]
async fn scenario_scored_event_carries_the_real_wcps_and_verdict() {
    tokio::time::pause();
    let dir = tempfile::tempdir().unwrap();
    write_signatures(dir.path());
    let settings = settings_with(0, 3, 5);
    let mock = MockController::new()
        .with_steam_status(SteamStatus {
            running: true,
            elevated: false,
        })
        .with_process("steam.exe", 8001)
        .with_process_on_deelevate("steam.exe", 8002)
        .with_process("steamwebhelper.exe", 8099)
        .with_process_on_launch("cs2.exe", 8500)
        .with_launch_options("");
    let sys: Arc<dyn SystemController> = Arc::new(mock.clone());
    let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![
        // Baseline's 3 measure iterations: high, stable fps.
        scoring_metrics(880.0, 338.0, 298.0, 0.41, 7.0, 0.22),
        scoring_metrics(878.0, 337.0, 297.0, 0.41, 7.1, 0.23),
        scoring_metrics(881.0, 339.0, 299.0, 0.42, 6.9, 0.21),
        // Scenario's 3 measure iterations: clearly worse on every axis.
        scoring_metrics(550.0, 200.0, 170.0, 0.55, 12.0, 0.40),
        scoring_metrics(548.0, 199.0, 169.0, 0.56, 12.2, 0.41),
        scoring_metrics(551.0, 201.0, 171.0, 0.55, 11.8, 0.39),
    ]));

    let mut config = config_with(
        dir.path().to_path_buf(),
        settings,
        dir.path().join("console.log"),
    );
    config.mock_cs2_log = Some(Duration::from_millis(0));
    config.project.scenarios.push(Scenario {
        id: "sc-worse".into(),
        name: "Worse Scenario".into(),
        description: "d".into(),
        enabled: true,
        modules: vec![],
    });

    let (events_tx, mut events_rx) = mpsc::channel(256);
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
    result.expect("run should complete successfully");

    let mut scored_events = vec![];
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::ScenarioScored { result } = ev {
            scored_events.push(result);
        }
    }

    assert_eq!(
        scored_events.len(),
        1,
        "expected exactly one ScenarioScored event, for the one non-baseline scenario"
    );
    let scored = &scored_events[0];
    assert_eq!(scored.scenario_id, "sc-worse");
    assert_eq!(
        scored.verdict,
        crate::model::results::Verdict::Worse,
        "a clear regression across every metric must score Worse, not the placeholder ConfirmedSame"
    );
    assert!(
        scored.wcps < 0.0,
        "a Worse verdict must carry a negative wcps, got {}",
        scored.wcps
    );
}
