//! The run loop — `run::execute` is the whole benchmark run, phases per
//! docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1: PREFLIGHT -> SNAPSHOT -> BASELINE -> SCENARIO* -> ROLLBACK ->
//! REPORT.

pub mod execute;
pub mod phase;
mod run_log;
pub mod spawn;

use crate::model::results::{Metrics, ScenarioResult};
pub use phase::{DetectionTier, IterationKind, Phase};
use serde::Serialize;
pub use spawn::{RunHandle, spawn_run};

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "type")]
pub enum EngineEvent {
    PhaseChanged {
        phase: Phase,
    },
    LogLine {
        text: String,
    },
    IterationStarted {
        kind: IterationKind,
        index: u32,
    },
    CapturePending,
    CaptureResumed,
    /// Bracket ONLY the real PresentMon capture call itself (not the settle
    /// sleep before it, not the post-capture wait for the benchmark-ended
    /// marker — those stay covered by `CapturePending`/`CaptureResumed`,
    /// whose wider window exists for the webview-suspend feature). Sent
    /// once per measure iteration; never sent for a warmup iteration
    /// (warmup never captures).
    RecordingStarted,
    RecordingStopped,
    IterationComplete {
        metrics: Metrics,
    },
    ScenarioComplete {
        result: ScenarioResult,
    },
    OperatorPrompt {
        text: String,
    },
    RunComplete {
        run_id: String,
    },
    RunFailed {
        reason: String,
    },
    RollbackProgress {
        reverted: u32,
    },
}

#[derive(Debug, Clone, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub enum ControlMsg {
    Pause,
    Resume,
    Abort,
    /// Answers an `OperatorPrompt` — e.g. "I've closed Steam" / "I've
    /// reopened Steam" for the launch-args-changed manual-restart flow
    /// (`docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md` §7.2 step 2). The prompt's own text distinguishes which
    /// question is being answered; `execute` polls `steam_status()` itself
    /// rather than trusting this message's timing exactly (a user might
    /// click "done" before Steam has actually finished starting).
    OperatorAcknowledged,
}
