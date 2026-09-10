//! Per-run plain-text narrative log (`runs/<run_id>/run.log`): a durable,
//! human-readable record of everything a run emits as an `EngineEvent` --
//! independent of the Tauri shell's own rotating global app log (which
//! `voidframe-cli` doesn't even have) and the UI's transient Live Monitor,
//! both of which lose the story once the process exits or the window
//! scrolls. Tapped once, centrally, in `spawn::spawn_run` -- the one place
//! every event either shell ever sees already passes through -- so both
//! callers get a persisted `run.log` for free.

use super::EngineEvent;
use std::io::Write;
use std::path::{Path, PathBuf};

pub(super) struct RunLog {
    path: PathBuf,
}

impl RunLog {
    /// Does not touch the filesystem yet — the run directory and file are
    /// created lazily on the first `append` call, mirroring
    /// `Journal::open`'s own lazy `create_dir_all`.
    pub(super) fn new(data_root: &Path, run_id: &str) -> RunLog {
        RunLog {
            path: data_root.join("runs").join(run_id).join("run.log"),
        }
    }

    /// Formats `ev` on the async side, then does the actual filesystem
    /// write (`create_dir_all` + open + append) under
    /// `tokio::task::spawn_blocking`, so a slow or contended disk never
    /// blocks a Tokio worker thread. Best-effort throughout: a write
    /// failure, or a `spawn_blocking` join failure, is logged and never
    /// propagated — losing a log line must not fail (or even interrupt)
    /// the run itself, mirroring `commands/run.rs`'s own "a store-update
    /// failure must never stop event forwarding" precedent.
    pub(super) async fn append(&self, ev: &EngineEvent) {
        let line = format_event(ev);
        let path = self.path.clone();
        let result = tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                tracing::warn!(error = %e, "failed to create run log directory");
                return;
            }
            let result = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .and_then(|mut f| writeln!(f, "{line}"));
            if let Err(e) = result {
                tracing::warn!(error = %e, path = %path.display(), "failed to write to run log");
            }
        })
        .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, "run log write task panicked");
        }
    }
}

fn format_event(ev: &EngineEvent) -> String {
    let ts = crate::model::results::utc_timestamp_now();
    let body = match ev {
        EngineEvent::PhaseChanged { phase } => format!("PHASE {phase}"),
        EngineEvent::LogLine { text } => text.clone(),
        EngineEvent::IterationStarted { kind, index } => {
            format!("Iteration {kind:?} #{index} started")
        }
        EngineEvent::ThermalProgress {
            sample,
            total,
            cpu_temp_celsius,
            gpu_temp_celsius,
        } => format!(
            "Thermal sample {sample}/{total}: cpu={cpu_temp_celsius:.1}C gpu={}",
            gpu_temp_celsius.map_or_else(|| "n/a".to_string(), |g| format!("{g:.1}C"))
        ),
        EngineEvent::CapturePending => "Capture pending".into(),
        EngineEvent::CaptureResumed => "Capture resumed".into(),
        EngineEvent::RecordingStarted => "Recording started".into(),
        EngineEvent::RecordingStopped => "Recording stopped".into(),
        EngineEvent::IterationComplete { metrics } => format!(
            "Iteration complete: avg_fps={:.1} p1_fps={:.1} p01_fps={:.1}",
            metrics.avg_fps, metrics.p1_fps, metrics.p01_fps
        ),
        EngineEvent::ScenarioComplete { result } => {
            format!("Scenario '{}' complete", result.name)
        }
        EngineEvent::ScenarioScored { result } => format!(
            "Scenario '{}' scored: wcps={:.2} verdict={:?}",
            result.name, result.wcps, result.verdict
        ),
        EngineEvent::OperatorPrompt { text } => format!("OPERATOR PROMPT: {text}"),
        EngineEvent::RunComplete { run_id } => format!("Run complete (run_id={run_id})"),
        EngineEvent::RunFailed { reason } => format!("RUN FAILED: {reason}"),
        EngineEvent::RollbackProgress { reverted } => {
            format!("Rollback progress: {reverted} entrie(s) reverted")
        }
    };
    format!("[{ts}] {body}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::results::{Metrics, ScenarioResult, Verdict};

    fn scenario_result(name: &str, wcps: f64) -> ScenarioResult {
        ScenarioResult {
            scenario_id: "sc1".into(),
            name: name.into(),
            is_baseline: false,
            aggregated: Metrics {
                avg_fps: 300.0,
                median_fps: 300.0,
                p1_fps: 250.0,
                p01_fps: 200.0,
                frame_time_mean_ms: 3.3,
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
            },
            per_iteration: vec![],
            metric_deltas: vec![],
            wcps,
            verdict: Verdict::Better,
            script_reverted_unverified: false,
        }
    }

    #[tokio::test]
    async fn append_creates_the_run_directory_and_writes_a_readable_line() {
        let dir = tempfile::tempdir().unwrap();
        let log = RunLog::new(dir.path(), "run-1");

        log.append(&EngineEvent::LogLine {
            text: "Steam is not running -- attempting to start it automatically...".into(),
        })
        .await;

        let contents =
            std::fs::read_to_string(dir.path().join("runs").join("run-1").join("run.log")).unwrap();
        assert!(
            contents.contains("Steam is not running -- attempting to start it automatically..."),
            "run.log must contain the LogLine's own text, got: {contents}"
        );
    }

    #[tokio::test]
    async fn append_writes_multiple_events_as_separate_lines_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let log = RunLog::new(dir.path(), "run-2");

        log.append(&EngineEvent::PhaseChanged {
            phase: crate::run::Phase::Preflight,
        })
        .await;
        log.append(&EngineEvent::LogLine {
            text: "second".into(),
        })
        .await;

        let contents =
            std::fs::read_to_string(dir.path().join("runs").join("run-2").join("run.log")).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "got: {contents}");
        assert!(lines[0].contains("preflight"), "got: {}", lines[0]);
        assert!(lines[1].contains("second"), "got: {}", lines[1]);
    }

    #[tokio::test]
    async fn append_summarizes_a_scenario_complete_event_readably() {
        let dir = tempfile::tempdir().unwrap();
        let log = RunLog::new(dir.path(), "run-3");

        log.append(&EngineEvent::ScenarioComplete {
            result: scenario_result("High FPS", 12.5),
        })
        .await;

        let contents =
            std::fs::read_to_string(dir.path().join("runs").join("run-3").join("run.log")).unwrap();
        assert!(contents.contains("High FPS"), "got: {contents}");
        assert!(
            !contents.contains("wcps"),
            "ScenarioComplete carries a not-yet-scored placeholder -- run.log must not print it \
             as if it were real (that's the bug ScenarioScored was added to fix); got: {contents}"
        );
    }

    #[tokio::test]
    async fn append_summarizes_a_scenario_scored_event_readably() {
        let dir = tempfile::tempdir().unwrap();
        let log = RunLog::new(dir.path(), "run-4");

        log.append(&EngineEvent::ScenarioScored {
            result: scenario_result("High FPS", 12.5),
        })
        .await;

        let contents =
            std::fs::read_to_string(dir.path().join("runs").join("run-4").join("run.log")).unwrap();
        assert!(contents.contains("scored"), "got: {contents}");
        assert!(contents.contains("High FPS"), "got: {contents}");
        assert!(contents.contains("12.5"), "got: {contents}");
    }
}
