//! `spawn_run` -- the one way both shells (`voidframe-cli`, the Tauri app)
//! start a run. Owns the "spawn `execute`, turn its `Err` into a single
//! `RunFailed`" wiring that each shell used to copy, plus three things
//! specific to this boundary: teeing the raw `ControlMsg` channel so `Abort`
//! also sets a cheap, low-latency `watch` signal `execute()`'s long waits
//! race against (see `RunContext::abort`'s own doc comment); teeing every
//! `EngineEvent` through this run's own on-disk `run_log::RunLog` before
//! forwarding it to the caller; and pruning a run's forensic-value-free
//! on-disk files (capture CSVs, `thermal.json`) after a genuine operator
//! Abort, while leaving its journal(s) and `run.log` in place.

use crate::capture::runner::CaptureRunner;
use crate::error::Result;
use crate::model::results::RunResults;
use crate::run::execute::{RunConfig, execute};
use crate::run::run_log::RunLog;
use crate::run::{ControlMsg, EngineEvent};
use crate::system::SystemController;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

pub struct RunHandle {
    pub events: mpsc::Receiver<EngineEvent>,
    pub join: JoinHandle<Result<RunResults>>,
}

/// Forwards every `ControlMsg` from the caller's raw `control` channel to
/// `execute()`'s own internal one unchanged (so Pause/Resume/
/// OperatorAcknowledged/Abort keep working exactly as they always have,
/// consumed at the same existing checkpoints), while also flipping
/// `abort_tx` the instant an `Abort` is *read off `raw`* -- independent of
/// whether/when `execute()`'s own logic happens to next poll the `mpsc`
/// channel, and independent of whether `inner` currently has capacity.
/// Messages that can't be forwarded immediately (because `inner` is full)
/// are buffered in `pending` rather than blocking the read of `raw`, so a
/// later `Abort` is never delayed behind a backlog of undrained messages;
/// order into `inner` is still preserved. Ends when the raw channel closes
/// and `pending` has been fully drained, or `execute()` has already finished
/// and dropped its receiver.
async fn tee_control_for_abort(
    mut raw: mpsc::Receiver<ControlMsg>,
    inner: mpsc::Sender<ControlMsg>,
    abort_tx: watch::Sender<bool>,
) {
    let mut pending = std::collections::VecDeque::new();
    let mut raw_open = true;
    loop {
        tokio::select! {
            msg = raw.recv(), if raw_open => match msg {
                Some(msg) => {
                    if matches!(msg, ControlMsg::Abort) {
                        let _ = abort_tx.send(true);
                    }
                    pending.push_back(msg);
                }
                None => raw_open = false,
            },
            permit = inner.reserve(), if !pending.is_empty() => match permit {
                Ok(permit) => permit.send(pending.pop_front().expect("guarded by !is_empty")),
                Err(_) => break,
            },
            else => break,
        }
    }
}

pub fn spawn_run(
    sys: Arc<dyn SystemController>,
    capture: Arc<dyn CaptureRunner>,
    config: RunConfig,
    control: mpsc::Receiver<ControlMsg>,
) -> RunHandle {
    let (events_tx, mut events_rx) = mpsc::channel::<EngineEvent>(64);
    let (abort_tx, abort_rx) = watch::channel(false);
    let (inner_control_tx, inner_control_rx) = mpsc::channel::<ControlMsg>(16);
    tokio::spawn(tee_control_for_abort(control, inner_control_tx, abort_tx));

    // Captured before `config` is moved into `execute()` below -- this is
    // the only information `run_dir_to_prune_on_abort` needs, and
    // `execute()` consumes its `RunConfig` by value.
    let run_dir = config.data_root.join("runs").join(config.run_id.clone());

    // Tee every event through this run's own on-disk narrative log
    // (`RunLog`) before forwarding it on unchanged. `events_rx` above is
    // this function's own internal receiver -- every `EngineEvent` either
    // shell (the Tauri app, `voidframe-cli`) ever sees, from PREFLIGHT
    // through this function's own late `RunFailed` below, already passes
    // through it, so this is the one place a per-run log file can be
    // written once for both callers instead of each shell doing it itself.
    let (public_tx, public_rx) = mpsc::channel::<EngineEvent>(64);
    let run_log = RunLog::new(&config.data_root, &config.run_id);
    tokio::spawn(async move {
        // Once the caller drops its `events` receiver, `public_tx.send`
        // starts failing -- but the run itself keeps going, and every event
        // it emits from here on (including this function's own late
        // `RunFailed`) must still land in `run.log`. So forwarding stops
        // the moment the consumer is gone, while logging never does.
        let mut consumer_gone = false;
        while let Some(ev) = events_rx.recv().await {
            run_log.append(&ev).await;
            if !consumer_gone && public_tx.send(ev).await.is_err() {
                consumer_gone = true;
            }
        }
    });

    let join = tokio::spawn(async move {
        let result = execute(
            sys,
            capture,
            config,
            events_tx.clone(),
            inner_control_rx,
            abort_rx,
        )
        .await;
        if let Err(e) = &result {
            if e.is_aborted() {
                prune_run_dir(&run_dir).await;
            }
            let _ = events_tx
                .send(EngineEvent::RunFailed {
                    reason: e.to_string(),
                })
                .await;
        }
        result
    });
    RunHandle {
        events: public_rx,
        join,
    }
}

/// Deletes only this run's forensic-value-free files after a genuine
/// operator Abort: every `scenario-*` directory (partial capture CSVs) and
/// `thermal.json`. Leaves `journal-*.jsonl` and `run.log` in place --
/// unlike a `results.json` (an aborted run never produces one, since
/// `execute()`'s `body` returns `Err` before REPORT), a scenario's journal
/// is the only record that lets `rollback_now`/`emergency_rollback` repair
/// the machine afterward, and the revert that ran alongside this Abort may
/// itself have failed (see `run_scenario`'s and `execute`'s own combined-
/// error handling for that case) -- deleting it here would make that
/// failure irrecoverable. Best-effort: a failure here is logged, never
/// escalated -- the abort itself (CS2 already killed, mutations already
/// rolled back by the time this runs) has already succeeded regardless of
/// whether cleanup does.
async fn prune_run_dir(run_dir: &PathBuf) {
    let mut entries = match tokio::fs::read_dir(run_dir).await {
        Ok(e) => e,
        Err(_) => return,
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let result = if name.starts_with("scenario-") {
            tokio::fs::remove_dir_all(entry.path()).await
        } else if name == "thermal.json" {
            tokio::fs::remove_file(entry.path()).await
        } else {
            continue; // journal-*.jsonl and run.log stay
        };
        if let Err(e) = result
            && e.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(
                path = %entry.path().display(),
                error = %e,
                "failed to prune this run's capture files after an operator Abort"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::runner::MockCaptureRunner;
    use crate::system::{MockController, SteamStatus};

    /// `prune_run_dir` must delete only forensic-value-free files
    /// (`scenario-*` capture directories, `thermal.json`) and leave
    /// `journal-*.jsonl`/`run.log` in place -- the fix for the High-severity
    /// finding that Abort used to delete a scenario's own journal even when
    /// the revert running alongside it had failed.
    #[tokio::test]
    async fn prune_run_dir_deletes_captures_and_thermal_json_but_keeps_journals_and_run_log() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        let scenario_dir = run_dir.join("scenario-s1");
        tokio::fs::create_dir_all(&scenario_dir).await.unwrap();
        tokio::fs::write(scenario_dir.join("iter-00.csv"), "a,b\n1,2\n")
            .await
            .unwrap();
        tokio::fs::write(run_dir.join("journal-s1.jsonl"), "{}\n")
            .await
            .unwrap();
        tokio::fs::write(run_dir.join("run.log"), "some narrative\n")
            .await
            .unwrap();
        tokio::fs::write(run_dir.join("thermal.json"), "{}")
            .await
            .unwrap();

        prune_run_dir(&run_dir).await;

        assert!(
            !scenario_dir.exists(),
            "scenario-* capture directories must be pruned"
        );
        assert!(
            !run_dir.join("thermal.json").exists(),
            "thermal.json must be pruned"
        );
        assert!(
            run_dir.join("journal-s1.jsonl").exists(),
            "journal-*.jsonl must never be pruned -- it's the only record that \
             lets rollback_now/emergency_rollback repair the machine"
        );
        assert!(
            run_dir.join("run.log").exists(),
            "run.log must never be pruned"
        );
    }

    #[tokio::test]
    async fn a_failing_run_emits_exactly_one_run_failed_and_returns_the_error() {
        // Preflight blocks (Steam not running, auto-launch fails) -> execute() returns Err.
        let sys: Arc<dyn SystemController> = Arc::new(
            MockController::new()
                .with_steam_status(SteamStatus {
                    running: false,
                    elevated: false,
                })
                .with_deelevate_failing("no interactive desktop"),
        );
        let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));
        let project: crate::model::Project = serde_json::from_value(serde_json::json!({
            "schema_version": crate::model::SCHEMA_VERSION, "id": "p", "name": "P", "description": "d",
            "created_at": "2026-09-01T00:00:00Z", "settings": {}, "baseline": {"name": "b", "description": "d"},
            "scenarios": []
        })).unwrap();
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
        };
        let (_control_tx, control_rx) = mpsc::channel(1);
        let mut handle = spawn_run(sys, capture, config, control_rx);

        let mut failed = 0;
        while let Some(ev) = handle.events.recv().await {
            if matches!(ev, EngineEvent::RunFailed { .. }) {
                failed += 1;
            }
        }
        assert_eq!(failed, 1, "exactly one RunFailed per failing run");
        assert!(handle.join.await.unwrap().is_err());
    }

    /// `spawn_run` must persist every `EngineEvent` it forwards -- including
    /// its own late `RunFailed`, sent from outside `execute()` entirely --
    /// to this run's own `runs/<run_id>/run.log`, so the run leaves a
    /// self-contained narrative behind on disk regardless of which caller
    /// (Tauri app, `voidframe-cli`) started it.
    #[tokio::test]
    async fn spawn_run_persists_the_full_event_stream_to_run_log() {
        let sys: Arc<dyn SystemController> = Arc::new(
            MockController::new()
                .with_steam_status(SteamStatus {
                    running: false,
                    elevated: false,
                })
                .with_deelevate_failing("no interactive desktop"),
        );
        let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));
        let project: crate::model::Project = serde_json::from_value(serde_json::json!({
            "schema_version": crate::model::SCHEMA_VERSION, "id": "p", "name": "P", "description": "d",
            "created_at": "2026-09-01T00:00:00Z", "settings": {}, "baseline": {"name": "b", "description": "d"},
            "scenarios": []
        })).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config = RunConfig {
            project,
            run_id: "r-log".into(),
            dry_run: false,
            data_root: dir.path().to_path_buf(),
            webview_root_pid: None,
            console_log_override: None,
            mock_cs2_log: None,
            thermal_sample_override: None,
            inter_scenario_break_seconds: 0,
            hwinfo_path: None,
        };
        let (_control_tx, control_rx) = mpsc::channel(1);
        let mut handle = spawn_run(sys, capture, config, control_rx);

        while handle.events.recv().await.is_some() {}
        let _ = handle.join.await.unwrap();

        let log_path = dir.path().join("runs").join("r-log").join("run.log");
        let contents = std::fs::read_to_string(&log_path)
            .unwrap_or_else(|e| panic!("expected {} to exist: {e}", log_path.display()));
        assert!(
            contents.contains("preflight"),
            "must log the PREFLIGHT phase change, got: {contents}"
        );
        assert!(
            contents.contains("RUN FAILED"),
            "must log spawn_run's own late RunFailed, got: {contents}"
        );
    }

    /// The Low-severity finding this fixes: once the caller drops its
    /// `RunHandle.events` receiver, the event tee must stop *forwarding*
    /// (further `public_tx.send` calls would just fail forever) but must
    /// keep appending every remaining event -- including this function's
    /// own late `RunFailed` -- to `run.log`, so a caller that stops
    /// listening early still gets a complete on-disk narrative.
    #[tokio::test]
    async fn run_log_still_gets_run_failed_even_after_the_consumer_drops_events() {
        let sys: Arc<dyn SystemController> = Arc::new(
            MockController::new()
                .with_steam_status(SteamStatus {
                    running: false,
                    elevated: false,
                })
                .with_deelevate_failing("no interactive desktop"),
        );
        let capture: Arc<dyn CaptureRunner> = Arc::new(MockCaptureRunner::new(vec![]));
        let project: crate::model::Project = serde_json::from_value(serde_json::json!({
            "schema_version": crate::model::SCHEMA_VERSION, "id": "p", "name": "P", "description": "d",
            "created_at": "2026-09-01T00:00:00Z", "settings": {}, "baseline": {"name": "b", "description": "d"},
            "scenarios": []
        })).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let config = RunConfig {
            project,
            run_id: "r-dropped".into(),
            dry_run: false,
            data_root: dir.path().to_path_buf(),
            webview_root_pid: None,
            console_log_override: None,
            mock_cs2_log: None,
            thermal_sample_override: None,
            inter_scenario_break_seconds: 0,
            hwinfo_path: None,
        };
        let (_control_tx, control_rx) = mpsc::channel(1);
        let handle = spawn_run(sys, capture, config, control_rx);
        drop(handle.events);

        let _ = handle.join.await.unwrap();

        // `join` resolving only means spawn_run's own `RunFailed` was
        // *sent* into the internal channel -- the event tee task that
        // appends it to run.log is a separate, concurrently-scheduled
        // task, so poll for the line rather than assuming it landed the
        // instant `join` completed.
        let log_path = dir.path().join("runs").join("r-dropped").join("run.log");
        let contents = tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(c) = std::fs::read_to_string(&log_path)
                    && c.contains("RUN FAILED")
                {
                    return c;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for RUN FAILED to appear in {log_path:?}"));
        assert!(
            contents.contains("RUN FAILED"),
            "must still log RunFailed after the consumer dropped events, got: {contents}"
        );
    }

    /// The Medium-severity finding this fixes: `tee_control_for_abort` used
    /// to forward with `inner.send(msg).await`, which blocks once `inner`
    /// (bounded 16) is full -- stopping it from reading `raw` at all, so a
    /// later `Abort` never flipped the watch until `execute()` drained room.
    /// Here `inner` is a `mpsc::channel(1)` receiver the test holds but never
    /// reads, so it fills immediately; the watch must still observe `true`
    /// promptly after `Abort` is sent on `raw`.
    #[tokio::test]
    async fn tee_flips_abort_watch_even_with_a_full_inner_channel_and_preserves_order() {
        let (raw_tx, raw_rx) = mpsc::channel::<ControlMsg>(16);
        let (inner_tx, mut inner_rx) = mpsc::channel::<ControlMsg>(1);
        let (abort_tx, abort_rx) = watch::channel(false);

        tokio::spawn(tee_control_for_abort(raw_rx, inner_tx, abort_tx));

        // Fill and overflow `inner`'s capacity-1 buffer without ever
        // draining it, then send Abort -- the tee must still notice.
        raw_tx.send(ControlMsg::Pause).await.unwrap();
        raw_tx.send(ControlMsg::Resume).await.unwrap();
        raw_tx.send(ControlMsg::OperatorAcknowledged).await.unwrap();
        raw_tx.send(ControlMsg::Abort).await.unwrap();

        let mut abort_rx = abort_rx;
        tokio::time::timeout(std::time::Duration::from_millis(500), async {
            loop {
                if *abort_rx.borrow() {
                    return;
                }
                abort_rx.changed().await.unwrap();
            }
        })
        .await
        .expect("abort watch must flip promptly even though `inner` was never drained");

        drop(raw_tx);

        // `ControlMsg` doesn't derive `PartialEq`; format each variant's
        // discriminant name instead of matching it structurally.
        let mut forwarded = Vec::new();
        while let Some(msg) = inner_rx.recv().await {
            forwarded.push(match msg {
                ControlMsg::Pause => "Pause",
                ControlMsg::Resume => "Resume",
                ControlMsg::Abort => "Abort",
                ControlMsg::OperatorAcknowledged => "OperatorAcknowledged",
            });
        }
        assert_eq!(
            forwarded,
            vec!["Pause", "Resume", "OperatorAcknowledged", "Abort"],
            "message order into `inner` must be preserved"
        );
    }
}
