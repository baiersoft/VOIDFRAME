//! Abstracts "run one PresentMon capture window and get metrics back" so
//! `run/` is testable against a fixture without a real `PresentMon.exe`
//! (which isn't available in CI — same reasoning `PresentMonController`'s
//! own doc comment already gives). Deliberately NOT part of
//! `SystemController` — see the plan's Global Constraints for why.

use crate::capture::presentmon::{PmArgs, PresentMonController};
use crate::error::Result;
use crate::model::results::Metrics;
use async_trait::async_trait;
use std::path::PathBuf;

#[async_trait]
pub trait CaptureRunner: Send + Sync {
    /// Runs one capture window and returns the parsed, aggregated metrics
    /// for it. Implementations own the "spawn PresentMon, wait for
    /// `--terminate_after_timed`, parse the CSV, aggregate" pipeline.
    async fn capture(&self, args: &PmArgs) -> Result<Metrics>;
}

/// Wraps the real `PresentMonController` + the real CSV parser into one
/// call (`docs/superpowers/plans/2026-09-01-m1-phase-2-capture-and-scoring.md`).
pub struct RealCaptureRunner {
    inner: PresentMonController,
}

impl RealCaptureRunner {
    pub fn new(exe_path: PathBuf) -> Self {
        Self {
            inner: PresentMonController::new(exe_path),
        }
    }
}

#[async_trait]
impl CaptureRunner for RealCaptureRunner {
    async fn capture(&self, args: &PmArgs) -> Result<Metrics> {
        let csv_path = self.inner.capture(args).await?;
        let samples = crate::capture::parser::parse_presentmon_csv(&csv_path)?;
        crate::capture::parser::aggregate_metrics(&samples)
    }
}

/// Scriptable stand-in for tests: returns a queued `Metrics` per call, in
/// order, cycling the last one if more calls happen than were queued (so a
/// test doesn't have to queue exactly N+1 entries for N+1 iterations if
/// every iteration should look the same).
pub struct MockCaptureRunner {
    queued: std::sync::Mutex<Vec<Metrics>>,
    calls: std::sync::Mutex<u32>,
    /// Set by [`Self::fail_next`]; the next `capture` call returns
    /// `Error::Mock` once, then clears — mirrors
    /// `MockController::fail_next_write`'s exact pattern.
    fail_next: std::sync::Mutex<bool>,
    /// Set by [`Self::with_realistic_duration`]. `false` (every existing
    /// caller, including every test in this workspace) returns instantly,
    /// unchanged. `true` (only the E2E mock-run mode's own "realistic
    /// timing" preset -- see `src-tauri`'s `commands::run::prepare_run`)
    /// sleeps `args.timed_seconds` for real before returning, so a manual
    /// observer sees the same real capture-window duration a real
    /// PresentMon run would take.
    simulate_real_duration: bool,
}

impl MockCaptureRunner {
    pub fn new(queued: Vec<Metrics>) -> Self {
        Self {
            queued: std::sync::Mutex::new(queued),
            calls: std::sync::Mutex::new(0),
            fail_next: std::sync::Mutex::new(false),
            simulate_real_duration: false,
        }
    }
    /// Opt into sleeping `args.timed_seconds` for real on every `capture`
    /// call instead of returning instantly -- see the field's own doc
    /// comment.
    pub fn with_realistic_duration(mut self) -> Self {
        self.simulate_real_duration = true;
        self
    }
    pub fn call_count(&self) -> u32 {
        *self.calls.lock().unwrap()
    }
    /// The next `capture` call fails once, then subsequent calls succeed
    /// normally again.
    pub fn fail_next(&self) {
        *self.fail_next.lock().unwrap() = true;
    }
}

#[async_trait]
impl CaptureRunner for MockCaptureRunner {
    async fn capture(&self, args: &PmArgs) -> Result<Metrics> {
        {
            let mut fail = self.fail_next.lock().unwrap();
            if *fail {
                *fail = false;
                return Err(crate::error::Error::mock("forced capture failure".into()));
            }
        }
        if self.simulate_real_duration {
            tokio::time::sleep(std::time::Duration::from_secs(args.timed_seconds as u64)).await;
        }
        let mut calls = self.calls.lock().unwrap();
        let idx = (*calls as usize).min(self.queued.lock().unwrap().len().saturating_sub(1));
        *calls += 1;
        Ok(self.queued.lock().unwrap()[idx].clone())
    }
}
