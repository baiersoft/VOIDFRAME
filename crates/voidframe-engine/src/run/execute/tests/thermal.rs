use super::*;
// Named explicitly (not just via the globs above) because this file's own
// module is *also* named `thermal` (`tests::thermal`, this file) -- an
// explicit import of `execute::thermal` here takes priority over the
// glob-imported candidates, which would otherwise be ambiguous between
// `execute::thermal` (via `super::super::*`) and this module's own name
// shadowing `tests`'s glob-imported copy of it (via `super::*`).
use super::super::thermal;

/// Every test in this file drives `maybe_break`/`wait_for_thermal_cooldown`
/// directly (not through a real `execute()` run), so each needs its own
/// stand-in `ControlMsg` channel and abort signal -- neither is exercised by
/// these tests (that's `scenario.rs`'s own tests' job), so a channel nothing
/// ever sends on and an abort signal that's never set are the right
/// defaults here.
fn no_control_or_abort() -> (
    mpsc::Receiver<ControlMsg>,
    tokio::sync::watch::Receiver<bool>,
) {
    let (_tx, rx) = mpsc::channel(1);
    let (_abort_tx, abort_rx) = tokio::sync::watch::channel(false);
    (rx, abort_rx)
}

#[tokio::test]
async fn maybe_break_does_nothing_when_seconds_is_zero() {
    let sys = MockController::new();
    let (events_tx, mut events_rx) = mpsc::channel(4);
    let (mut control_rx, abort_rx) = no_control_or_abort();
    let checks = maybe_break(
        &sys,
        &events_tx,
        &mut control_rx,
        abort_rx,
        0,
        None,
        None,
        thermal::THERMAL_SAMPLE_INTERVAL,
        thermal::THERMAL_SAMPLE_COUNT,
    )
    .await
    .unwrap();
    drop(events_tx);
    assert!(
        events_rx.try_recv().is_err(),
        "zero seconds must emit no LogLine and not sleep at all"
    );
    assert!(
        checks.is_empty(),
        "the fixed-time path never has cooldown-check readings to report"
    );
}

#[tokio::test]
async fn maybe_break_sleeps_and_logs_when_seconds_is_nonzero() {
    tokio::time::pause();
    let sys = MockController::new();
    let start = tokio::time::Instant::now();
    let (events_tx, mut events_rx) = mpsc::channel(4);
    let (mut control_rx, abort_rx) = no_control_or_abort();
    let checks = maybe_break(
        &sys,
        &events_tx,
        &mut control_rx,
        abort_rx,
        30,
        None,
        None,
        thermal::THERMAL_SAMPLE_INTERVAL,
        thermal::THERMAL_SAMPLE_COUNT,
    )
    .await
    .unwrap();
    assert!(
        checks.is_empty(),
        "the fixed-time path never has cooldown-check readings to report"
    );
    assert!(
        tokio::time::Instant::now() - start >= Duration::from_secs(30),
        "expected at least 30s of (virtual) elapsed time"
    );
    drop(events_tx);
    let mut saw_cooldown_log = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.contains("30")
        {
            saw_cooldown_log = true;
        }
    }
    assert!(
        saw_cooldown_log,
        "expected a LogLine mentioning the break duration"
    );
}

/// Regression/fallback-unchanged check: with no thermal baseline (the
/// default for every run until HWiNFO is configured), `maybe_break` must
/// behave exactly as before -- a plain fixed-time sleep, never touching
/// HWiNFO at all.
#[tokio::test]
async fn maybe_break_with_no_thermal_baseline_sleeps_the_fixed_duration_unchanged() {
    tokio::time::pause();
    let sys = MockController::new();
    let (events_tx, mut events_rx) = mpsc::channel(4);
    let (mut control_rx, abort_rx) = no_control_or_abort();
    let start = tokio::time::Instant::now();

    let checks = maybe_break(
        &sys,
        &events_tx,
        &mut control_rx,
        abort_rx,
        5,
        None,
        None,
        thermal::THERMAL_SAMPLE_INTERVAL,
        thermal::THERMAL_SAMPLE_COUNT,
    )
    .await
    .unwrap();

    assert!(
        tokio::time::Instant::now() - start >= Duration::from_secs(5),
        "expected at least 5s of (virtual) elapsed time"
    );
    assert_eq!(
        sys.start_hwinfo_calls(),
        0,
        "no thermal baseline means hwinfo is never touched"
    );
    assert!(
        checks.is_empty(),
        "the fixed-time path never has cooldown-check readings to report"
    );
    let ev = events_rx.try_recv().unwrap();
    assert!(matches!(ev, EngineEvent::LogLine { text } if text.contains("5s")));
}

/// Builds a `read_hwinfo_sensors` script that reproduces one specific
/// `collect_thermal_sample` average per thermal-cooldown check -- each
/// check inside `wait_for_thermal_cooldown` samples
/// `thermal::THERMAL_SAMPLE_COUNT` times internally (the real production
/// constant, not a test-only override), so this repeats each target
/// reading that many times in a row rather than supplying one entry per
/// check.
fn hwinfo_readings_for_checks(cpu_temps: &[f64]) -> Vec<(f64, Option<f64>)> {
    cpu_temps
        .iter()
        .flat_map(|&t| std::iter::repeat_n((t, None), thermal::THERMAL_SAMPLE_COUNT as usize))
        .collect()
}

/// Thermal-mode branch: `maybe_break` must poll `collect_thermal_sample`
/// repeatedly until the reading is back within
/// `THERMAL_COOLDOWN_THRESHOLD_C` of the pre-run baseline, logging its
/// progress each time -- not just sleep a fixed duration.
///
/// Also the regression test for the "restarts HWiNFO every check cycle"
/// bug: with three check cycles (75C, 68C, 62C) it must still start and
/// close HWiNFO exactly once for the *whole* wait, not once per cycle --
/// starting/closing a process on every ~60s check is itself a brief CPU
/// spike that could keep re-heating the system being measured, defeating
/// the cooldown wait's entire purpose.
#[tokio::test]
async fn maybe_break_with_a_thermal_baseline_waits_until_within_3_degrees() {
    tokio::time::pause();
    let sys = MockController::new().with_hwinfo_readings(hwinfo_readings_for_checks(&[
        75.0, // check 1: 15C above baseline of 60 -- keep waiting
        68.0, // check 2: 8C above -- keep waiting
        62.0, // check 3: 2C above -- within +/-3, stop
    ]));
    let baseline = thermal::ThermalReading {
        cpu_temp_celsius: 60.0,
        gpu_temp_celsius: None,
        sample_count: 30,
    };
    let hwinfo_path = std::path::PathBuf::from("C:\\fake\\hwinfo.exe");
    let (events_tx, mut events_rx) = mpsc::channel(256);
    let (mut control_rx, abort_rx) = no_control_or_abort();

    let checks = maybe_break(
        &sys,
        &events_tx,
        &mut control_rx,
        abort_rx,
        0,
        Some(&baseline),
        Some(&hwinfo_path),
        thermal::THERMAL_SAMPLE_INTERVAL,
        thermal::THERMAL_SAMPLE_COUNT,
    )
    .await
    .unwrap();

    assert_eq!(
        checks.len(),
        3,
        "expected one returned reading per cooldown check taken (75C, 68C, 62C)"
    );
    assert_eq!(checks[0].cpu_temp_celsius, 75.0);
    assert_eq!(checks[2].cpu_temp_celsius, 62.0);
    assert_eq!(
        sys.start_hwinfo_calls(),
        1,
        "HWiNFO must be started once for the whole cooldown wait, not once per check cycle"
    );
    assert_eq!(
        sys.close_hwinfo_calls(),
        1,
        "HWiNFO must be closed once for the whole cooldown wait, not once per check cycle"
    );

    let mut saw_cooldown = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.to_lowercase().contains("cooldown")
        {
            saw_cooldown = true;
        }
    }
    assert!(
        saw_cooldown,
        "expected at least one cooldown-progress log line"
    );
}

/// When HWiNFO is already running externally, the whole cooldown wait must
/// never start or close it -- docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2's "only touch what we started" rule,
/// same as [`maybe_break_with_a_thermal_baseline_waits_until_within_3_degrees`]
/// covers for the case this call does start it.
#[tokio::test]
async fn maybe_break_thermal_cooldown_never_touches_hwinfo_when_already_running() {
    tokio::time::pause();
    let sys = MockController::new()
        .with_process("HWiNFO64.exe", 4242)
        .with_hwinfo_readings(hwinfo_readings_for_checks(&[62.0]));
    let baseline = thermal::ThermalReading {
        cpu_temp_celsius: 60.0,
        gpu_temp_celsius: None,
        sample_count: 30,
    };
    let hwinfo_path = std::path::PathBuf::from("C:\\fake\\hwinfo.exe");
    let (events_tx, _events_rx) = mpsc::channel(256);
    let (mut control_rx, abort_rx) = no_control_or_abort();

    let checks = maybe_break(
        &sys,
        &events_tx,
        &mut control_rx,
        abort_rx,
        0,
        Some(&baseline),
        Some(&hwinfo_path),
        thermal::THERMAL_SAMPLE_INTERVAL,
        thermal::THERMAL_SAMPLE_COUNT,
    )
    .await
    .unwrap();

    assert_eq!(checks.len(), 1);
    assert_eq!(
        sys.start_hwinfo_calls(),
        0,
        "HWiNFO was already running -- this call must never start it"
    );
    assert_eq!(
        sys.close_hwinfo_calls(),
        0,
        "HWiNFO was already running -- this call must never close it"
    );
}

/// The 10-minute-cap exit path: a system that never cools back down within
/// `THERMAL_COOLDOWN_THRESHOLD_C` must still be closed exactly once when
/// `THERMAL_COOLDOWN_MAX_WAIT` is reached -- not left running forever just
/// because the wait itself gave up. Not a failure path (the wait genuinely
/// ran for real, up to the full cap), so this must NOT trigger Fix 3's
/// fixed-time fallback -- asserted below via elapsed virtual time staying
/// close to `THERMAL_COOLDOWN_MAX_WAIT`, not that plus a bonus sleep.
#[tokio::test]
async fn wait_for_thermal_cooldown_closes_hwinfo_once_when_the_max_wait_cap_is_reached() {
    tokio::time::pause();
    // Fixed reading, always 40C above baseline -- never converges within
    // THERMAL_COOLDOWN_THRESHOLD_C, so the loop can only exit via the cap.
    let sys = MockController::new().with_hwinfo_reading(100.0, None);
    let baseline = thermal::ThermalReading {
        cpu_temp_celsius: 60.0,
        gpu_temp_celsius: None,
        sample_count: 30,
    };
    let hwinfo_path = std::path::PathBuf::from("C:\\fake\\hwinfo.exe");
    let (events_tx, mut events_rx) = mpsc::channel(1024);
    let (mut control_rx, abort_rx) = no_control_or_abort();
    let start = tokio::time::Instant::now();

    let checks = wait_for_thermal_cooldown(
        &sys,
        &events_tx,
        &mut control_rx,
        abort_rx,
        &baseline,
        &hwinfo_path,
        30,
        thermal::THERMAL_SAMPLE_INTERVAL,
        thermal::THERMAL_SAMPLE_COUNT,
    )
    .await
    .unwrap();

    assert!(
        !checks.is_empty(),
        "expected at least one cooldown-check reading before the cap was reached"
    );
    assert_eq!(sys.start_hwinfo_calls(), 1);
    assert_eq!(
        sys.close_hwinfo_calls(),
        1,
        "must close what it started even though the system never cooled down"
    );
    let elapsed = tokio::time::Instant::now() - start;
    assert!(
        elapsed >= THERMAL_COOLDOWN_MAX_WAIT,
        "expected at least the full cap's worth of (virtual) elapsed time: {elapsed:?}"
    );
    assert!(
        elapsed < THERMAL_COOLDOWN_MAX_WAIT + Duration::from_secs(30),
        "the cap-reached exit is not a failure -- it must NOT also run Fix 3's fixed-time \
         fallback sleep on top: {elapsed:?}"
    );

    drop(events_tx);
    let mut saw_cap_reached = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.contains("10-minute cap")
        {
            saw_cap_reached = true;
        }
    }
    assert!(saw_cap_reached, "expected the cap-reached LogLine");
}

/// The sensor-read-error mid-loop exit path: a read failing partway through
/// must still close what this call started, exactly once -- mirrors
/// `thermal.rs`'s own `close_hwinfo_still_runs_when_sampling_fails_partway_through`
/// regression test, one level up.
#[tokio::test]
async fn wait_for_thermal_cooldown_closes_hwinfo_once_when_a_sensor_read_fails_mid_loop() {
    tokio::time::pause();
    // The first check's full THERMAL_SAMPLE_COUNT reads succeed (50C, the
    // mock's fixed default -- 10C above the 60C baseline below, so the
    // first check alone doesn't converge and the loop goes around again);
    // the very first read of the second check then fails.
    let sys = MockController::new().with_hwinfo_read_failing_after(thermal::THERMAL_SAMPLE_COUNT);
    let baseline = thermal::ThermalReading {
        cpu_temp_celsius: 60.0,
        gpu_temp_celsius: None,
        sample_count: 30,
    };
    let hwinfo_path = std::path::PathBuf::from("C:\\fake\\hwinfo.exe");
    let (events_tx, mut events_rx) = mpsc::channel(1024);
    let (mut control_rx, abort_rx) = no_control_or_abort();

    let checks = wait_for_thermal_cooldown(
        &sys,
        &events_tx,
        &mut control_rx,
        abort_rx,
        &baseline,
        &hwinfo_path,
        0,
        thermal::THERMAL_SAMPLE_INTERVAL,
        thermal::THERMAL_SAMPLE_COUNT,
    )
    .await
    .unwrap();

    assert_eq!(
        checks.len(),
        1,
        "exactly one full check completed before the read failure"
    );
    assert_eq!(sys.start_hwinfo_calls(), 1);
    assert_eq!(
        sys.close_hwinfo_calls(),
        1,
        "must close what it started even though a read failed mid-loop"
    );

    drop(events_tx);
    let mut saw_failure_log = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev
            && text.contains("sensor read failed")
        {
            saw_failure_log = true;
        }
    }
    assert!(saw_failure_log, "expected a sensor-read-failure LogLine");
}

/// Fix 3's fallback-sleep regression: when `start_hwinfo` fails (one of the
/// three failure conditions -- `hwinfo_already_running` erroring, a failed
/// `start_hwinfo`, or a mid-loop sensor read failure -- all fall back the
/// same way), a nonzero `seconds` must still result in a real fixed-time
/// sleep of that duration, not an immediate return with no wait at all.
#[tokio::test]
async fn wait_for_thermal_cooldown_falls_back_to_the_fixed_sleep_when_start_hwinfo_fails() {
    tokio::time::pause();
    let sys = MockController::new().with_start_hwinfo_failing("no interactive desktop");
    let baseline = thermal::ThermalReading {
        cpu_temp_celsius: 60.0,
        gpu_temp_celsius: None,
        sample_count: 30,
    };
    let hwinfo_path = std::path::PathBuf::from("C:\\fake\\hwinfo.exe");
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let (mut control_rx, abort_rx) = no_control_or_abort();
    let start = tokio::time::Instant::now();

    let checks = wait_for_thermal_cooldown(
        &sys,
        &events_tx,
        &mut control_rx,
        abort_rx,
        &baseline,
        &hwinfo_path,
        45,
        thermal::THERMAL_SAMPLE_INTERVAL,
        thermal::THERMAL_SAMPLE_COUNT,
    )
    .await
    .unwrap();

    assert!(
        checks.is_empty(),
        "start_hwinfo failing before any check ever ran means there's nothing to report"
    );
    assert!(
        tokio::time::Instant::now() - start >= Duration::from_secs(45),
        "expected the fixed-time fallback sleep to have actually run"
    );

    drop(events_tx);
    let mut saw_failure_log = false;
    let mut saw_break_log = false;
    while let Ok(ev) = events_rx.try_recv() {
        if let EngineEvent::LogLine { text } = ev {
            if text.contains("failed to start HWiNFO") {
                saw_failure_log = true;
            }
            if text.contains("45") {
                saw_break_log = true;
            }
        }
    }
    assert!(saw_failure_log, "expected the start_hwinfo-failure LogLine");
    assert!(
        saw_break_log,
        "expected the fixed-time fallback's own break-duration LogLine"
    );
}

/// New regression: an operator Abort seen at the top of
/// `wait_for_thermal_cooldown`'s loop must end the wait with an aborted
/// error (not a plain empty `Ok` result), and must still close HWiNFO if
/// this call was the one that started it -- the same "always clean up"
/// guarantee every other exit path already has.
#[tokio::test]
async fn wait_for_thermal_cooldown_aborts_and_still_closes_hwinfo_it_started() {
    tokio::time::pause();
    let sys = MockController::new().with_hwinfo_reading(100.0, None);
    let baseline = thermal::ThermalReading {
        cpu_temp_celsius: 60.0,
        gpu_temp_celsius: None,
        sample_count: 30,
    };
    let hwinfo_path = std::path::PathBuf::from("C:\\fake\\hwinfo.exe");
    let (events_tx, _events_rx) = mpsc::channel(1024);
    let (mut control_rx, abort_tx_rx) = {
        let (_tx, rx) = mpsc::channel(1);
        let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        abort_tx.send(true).unwrap();
        (rx, abort_rx)
    };

    let err = wait_for_thermal_cooldown(
        &sys,
        &events_tx,
        &mut control_rx,
        abort_tx_rx,
        &baseline,
        &hwinfo_path,
        0,
        thermal::THERMAL_SAMPLE_INTERVAL,
        thermal::THERMAL_SAMPLE_COUNT,
    )
    .await
    .unwrap_err();

    assert!(err.is_aborted(), "expected an aborted error, got: {err}");
    assert_eq!(
        sys.close_hwinfo_calls(),
        1,
        "must close HWiNFO it started even when exiting via abort"
    );
}
