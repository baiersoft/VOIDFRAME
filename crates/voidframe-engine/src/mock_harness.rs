//! A ready-to-run, fully synthetic `SystemController`/`CaptureRunner` pair:
//! no real registry/power-plan/affinity mutations, no real CS2 launch, no
//! real PresentMon capture. Originally lived only in `voidframe-cli`'s
//! `harness` module (its `--controller mock` dev-harness flag); relocated
//! here so the Tauri shell's own mock-run support (`src-tauri`'s
//! `mock-run` Cargo feature) can reuse the identical, already-tested
//! construction instead of a second copy drifting out of sync.

use crate::cs2::detection::{Cs2LogDetector, DetectionEvent};
use crate::cs2::keybind_cfg;
use crate::error::Result;
use crate::model::results::Metrics;
use crate::system::MockController;
use async_trait::async_trait;
use std::time::Duration;

/// A `MockController` pre-seeded so a run doesn't immediately trip the
/// launch-args-mismatch `OperatorPrompt` flow -- a freshly-loaded project's
/// baseline (whose reconciled launch args are always
/// `keybind_cfg::reconcile("")`, since baseline carries zero modules)
/// doesn't immediately hit the Steam-restart `OperatorPrompt` flow.
/// `MockController::steam_status` is a fixed value, so
/// `wait_until_steam_closed` would otherwise spin for a full minute and
/// then hard-fail. `cs2.exe` is registered as discoverable the moment
/// `launch_cs2` is called.
pub fn mock_ready_controller() -> MockController {
    let desired_args = keybind_cfg::reconcile("");
    MockController::new()
        .with_launch_options(&desired_args)
        .with_process_on_launch("cs2.exe", 999_999)
}

/// One plausible, fixed `Metrics` value returned for every mock capture --
/// its exact numbers are meaningless (nothing in a mock run exercises real
/// frame data), only its presence, so `aggregate_iterations`/scoring have
/// real (if synthetic) data to run over.
pub fn mock_metrics() -> Metrics {
    Metrics {
        avg_fps: 300.0,
        median_fps: 300.0,
        p1_fps: 250.0,
        p01_fps: 200.0,
        frame_time_mean_ms: 1000.0 / 300.0,
        frame_time_stddev_ms: 0.2,
        frame_time_cv: 0.05,
        adaptive_frame_time_cv: 0.05,
        stutter_count_pct: 0.0,
        mean_abs_animation_error_ms: None,
        gpu_busy_ms: None,
        bottleneck_ratio: None,
        render_latency_ms: None,
        dominant_present_mode: "Hardware: Independent Flip".into(),
        present_mode_consistent: true,
        present_mode_warning: None,
    }
}

/// A [`Cs2LogDetector`] that never touches a real file -- built for
/// `RunConfig::mock_cs2_log` (the E2E mock-run mode), where the CS2
/// process backing a real `console.log` was never actually launched
/// (`mock_ready_controller`'s `launch_cs2` is a no-op). Every `wait_for`
/// call tests a small, fixed set of representative `DetectionEvent`s
/// against the caller's own `accept` predicate and returns the first
/// match -- each of the three real call sites
/// (`launch.rs::wait_for_menu_ready`, `scenario.rs`'s map-load/benchmark-
/// started wait, and its benchmark-ended wait) narrows to exactly one of
/// these, so this needs no state or call-order coupling with the run loop
/// to answer all of them correctly, however many times each is called.
pub struct MockLogTail {
    /// How long each `wait_for` call takes before answering --
    /// `RunConfig::mock_cs2_log`'s own `Duration` payload. The fast E2E
    /// preset uses a value just long enough that the real event stream
    /// driving `LiveMonitor.tsx` still reads as a live sequence of phase
    /// transitions rather than an instant jump; the "realistic timing"
    /// manual-testing preset uses a value closer to what a real CS2
    /// boot/map-load actually takes, so a human operator sees roughly what
    /// a real run looks like.
    event_delay: Duration,
}

impl MockLogTail {
    pub fn new(event_delay: Duration) -> Self {
        MockLogTail { event_delay }
    }
}

#[async_trait]
impl Cs2LogDetector for MockLogTail {
    async fn wait_for(
        &mut self,
        _timeout: Duration,
        accept: &mut (dyn for<'r> FnMut(&'r DetectionEvent) -> bool + Send),
    ) -> Result<Option<DetectionEvent>> {
        tokio::time::sleep(self.event_delay).await;
        let candidates = [
            DetectionEvent::MenuReady,
            DetectionEvent::MapLoaded {
                map: "de_dust2".into(),
                addon: "0".into(),
            },
            DetectionEvent::BenchmarkStarted,
            DetectionEvent::VProfFps {
                avg: 300.0,
                p1: 250.0,
            },
            DetectionEvent::BenchmarkEnded,
        ];
        Ok(candidates.into_iter().find(|ev| accept(ev)))
    }

    fn lines_read(&self) -> u64 {
        0
    }

    fn last_line_seen(&self) -> Option<&str> {
        None
    }
}
