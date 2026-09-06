//! Result model — per-iteration metrics, aggregates, metric deltas, and the
//! top-level `RunResults` document.

use crate::error::Result;
use crate::paths::atomic_write;
use crate::run::DetectionTier;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Metrics for one measurement iteration (or an aggregate over several).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Metrics {
    pub avg_fps: f64,
    pub median_fps: f64,
    pub p1_fps: f64,
    pub p01_fps: f64,
    pub frame_time_mean_ms: f64,
    pub frame_time_stddev_ms: f64,
    pub frame_time_cv: f64,
    /// WCPS v3's pacing-aware frame-time CV: RMS deviation from a
    /// time-windowed backward moving average (not the whole-capture mean),
    /// normalized by the whole-capture mean frame time. See
    /// `stats::v3::metrics::adaptive_frame_time_cv`'s own doc comment for
    /// the full derivation.
    pub adaptive_frame_time_cv: f64,
    /// WCPS v3's stutter-count percentage: the fraction of frames whose
    /// frame time exceeds `k=2.0` times the capture's mean frame time. See
    /// `stats::v3::metrics::stutter_count_pct`.
    pub stutter_count_pct: f64,
    /// WCPS v3's animation-timing-error metric, from PresentMon's own
    /// `MsAnimationError` column. `None` when every frame's value was `NA`
    /// (only expected for a capture with essentially zero usable frames —
    /// a real capture always has this `Some`, since only frame 0 is ever
    /// `NA`). See `stats::v3::metrics::mean_abs_animation_error_ms`.
    pub mean_abs_animation_error_ms: Option<f64>,
    pub gpu_busy_ms: Option<f64>,
    pub bottleneck_ratio: Option<f64>,
    pub render_latency_ms: Option<f64>,
    /// The most common raw `PresentMode` string across this iteration's
    /// frames (see `capture::present_mode`).
    #[serde(default)]
    pub dominant_present_mode: String,
    /// `true` only if every frame shared the exact same `PresentMode`.
    #[serde(default)]
    pub present_mode_consistent: bool,
    /// Set when `dominant_present_mode` indicates the compositor was in
    /// the frame path (see `PresentModeCategory::is_compositor_bypassed`)
    /// — a likely benchmarking misconfiguration (e.g. CS2 not set to
    /// exclusive Fullscreen), surfaced directly in the report rather than
    /// left for the user to infer from raw numbers.
    #[serde(default)]
    pub present_mode_warning: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Better,
    Worse,
    NoMeasurableDifference,
    /// WCPS v3's OR-of-2-Hotelling / AND-of-2-TOST combined verdict when
    /// TOST equivalence holds on both the throughput and pacing axes --
    /// `study/research/wcps-v3-design.md` §6's own plan: split
    /// `NoMeasurableDifference` into two rather than grow a binary enum to
    /// three. Never produced by WCPS v2's `compare_metric`
    /// (`stats/significance.rs`) -- that's still a per-metric CI check with
    /// no equivalence-testing concept, only `stats::v3::evaluate_verdict`
    /// (`docs/superpowers/plans/2026-09-04-wcps-v3-score-and-verdict.md`)
    /// produces this variant.
    ConfirmedSame,
    /// WCPS v3's combined verdict when neither Hotelling flags `Different`
    /// nor TOST confirms equivalence on both axes -- the other half of the
    /// `NoMeasurableDifference` split. Same non-production note as
    /// `ConfirmedSame` above.
    Inconclusive,
}

/// One metric's plain percent delta (scenario mean vs. baseline mean) --
/// descriptive only. WCPS v3's `Verdict` is a single holistic call across
/// all 6 metrics together (Hotelling's T² + TOST), not a per-metric
/// significance test the way WCPS v2's now-deleted `MetricComparison` used
/// to attach one CI+verdict per metric.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct MetricDelta {
    pub metric: String,
    pub delta_pct: f64,
}

/// `verdict` has no meaningful value for the baseline's own `ScenarioResult`
/// -- by convention, `run_scenario` sets it to `Verdict::ConfirmedSame` for
/// the baseline entry, matching "a thing compared against itself is always
/// the same"; the frontend should treat the baseline row's verdict/wcps as
/// decorative, same as today's UI already does by showing "Baseline"/"—"
/// instead of reading those fields for that row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct ScenarioResult {
    pub scenario_id: String,
    pub name: String,
    pub is_baseline: bool,
    pub aggregated: Metrics,
    pub per_iteration: Vec<Metrics>,
    pub metric_deltas: Vec<MetricDelta>,
    pub wcps: f64,
    pub verdict: Verdict,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RunResults {
    pub schema_version: String,
    pub run_id: String,
    pub project_id: String,
    pub completed_at: String,
    pub detection_tier: DetectionTier,
    pub baseline: ScenarioResult,
    pub scenarios: Vec<ScenarioResult>,
}

/// A lightweight summary of a `RunResults` for a results-history listing UI
/// -- everything it needs without shipping every iteration's raw `Metrics`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RunSummary {
    pub run_id: String,
    pub project_id: String,
    pub completed_at: String,
    // `u32`, not `usize`: specta-typescript hard-forbids BigInt-style Rust
    // types (usize/u64/i64/u128/i128) in debug-build exports by default --
    // this field being `usize` made `tauri dev`'s TS-bindings export panic
    // with zero visible output (the app's own panic hook only logs, and the
    // logger isn't attached yet at that point in startup -- see lib.rs's
    // `run()`), which looked like a mysterious exit-101 crash with no
    // diagnostic trail. Matches the same narrowing already applied to
    // `RunState::revision` and `EngineEvent::RollbackProgress::reverted`.
    pub scenario_count: u32,
    pub winner_name: Option<String>,
    pub winner_wcps: Option<f64>,
}

/// Now, as `YYYY-MM-DDTHH:MM:SSZ` (RFC 3339, UTC, second precision). Fixed
/// width, so lexicographic order is chronological order -- `list_results`
/// sorts on the string directly. No date crate: this is the one place a
/// timestamp is produced, and the civil-from-days conversion below is
/// twenty lines (Howard Hinnant's algorithm).
pub fn utc_timestamp_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    utc_timestamp_from_unix(secs)
}

fn utc_timestamp_from_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // days -> civil (proleptic Gregorian), Hinnant's `civil_from_days`.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

impl RunResults {
    /// `winner` is the highest-`wcps` entry among `scenarios` -- baseline
    /// is never a candidate (mirrors `ResultsVisualizer.tsx`'s own
    /// `sortedScenarios[0]`). `None` for a baseline-only run (e.g. a
    /// calibration-shaped session with zero real scenarios).
    pub fn summarize(&self) -> RunSummary {
        let winner = self.scenarios.iter().max_by(|a, b| {
            a.wcps
                .partial_cmp(&b.wcps)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        RunSummary {
            run_id: self.run_id.clone(),
            project_id: self.project_id.clone(),
            completed_at: self.completed_at.clone(),
            scenario_count: 1 + self.scenarios.len() as u32,
            winner_name: winner.map(|w| w.name.clone()),
            winner_wcps: winner.map(|w| w.wcps),
        }
    }

    pub fn load(path: &Path) -> Result<RunResults> {
        let bytes = std::fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        atomic_write(path, &serde_json::to_vec_pretty(self)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamp_from_unix_matches_known_dates() {
        assert_eq!(utc_timestamp_from_unix(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp_from_unix(951_782_400), "2000-02-29T00:00:00Z"); // leap day
        // 1_788_480_000 is 2026-09-04T00:00:00Z (verified against `date -u
        // -d @1788480000`) -- the brief's own draft used 1_788_547_200,
        // which is actually 2026-09-04T18:40:00Z, not midnight.
        assert_eq!(
            utc_timestamp_from_unix(1_788_480_000),
            "2026-09-04T00:00:00Z"
        );
        assert_eq!(
            utc_timestamp_from_unix(1_788_480_000 + 3_661),
            "2026-09-04T01:01:01Z"
        );
    }

    #[test]
    fn utc_timestamps_sort_lexicographically_in_time_order() {
        let a = utc_timestamp_from_unix(1_788_547_200);
        let b = utc_timestamp_from_unix(1_788_547_201);
        assert!(a < b);
    }

    fn dummy_metrics() -> Metrics {
        Metrics {
            avg_fps: 400.0,
            median_fps: 400.0,
            p1_fps: 250.0,
            p01_fps: 180.0,
            frame_time_mean_ms: 2.5,
            frame_time_stddev_ms: 0.2,
            frame_time_cv: 0.08,
            adaptive_frame_time_cv: 0.08,
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

    #[test]
    fn new_v3_verdict_variants_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&Verdict::ConfirmedSame).unwrap(),
            "\"confirmed_same\""
        );
        assert_eq!(
            serde_json::to_string(&Verdict::Inconclusive).unwrap(),
            "\"inconclusive\""
        );
        let round_tripped: Verdict = serde_json::from_str("\"confirmed_same\"").unwrap();
        assert_eq!(round_tripped, Verdict::ConfirmedSame);
    }

    #[test]
    fn summarize_picks_the_highest_wcps_scenario_as_winner() {
        let baseline = ScenarioResult {
            scenario_id: "baseline".into(),
            name: "Stock".into(),
            is_baseline: true,
            aggregated: dummy_metrics(),
            per_iteration: vec![],
            metric_deltas: vec![],
            wcps: 0.0,
            verdict: Verdict::ConfirmedSame,
        };
        let low = ScenarioResult {
            scenario_id: "s1".into(),
            name: "Low".into(),
            is_baseline: false,
            aggregated: dummy_metrics(),
            per_iteration: vec![],
            metric_deltas: vec![],
            wcps: 1.0,
            verdict: Verdict::Better,
        };
        let high = ScenarioResult {
            scenario_id: "s2".into(),
            name: "High".into(),
            is_baseline: false,
            aggregated: dummy_metrics(),
            per_iteration: vec![],
            metric_deltas: vec![],
            wcps: 5.0,
            verdict: Verdict::Better,
        };
        let rr = RunResults {
            schema_version: "1.0.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            completed_at: "2026-09-01T01:00:00Z".into(),
            detection_tier: DetectionTier::LogTail,
            baseline,
            scenarios: vec![low, high],
        };

        let summary = rr.summarize();
        assert_eq!(summary.winner_name, Some("High".into()));
        assert_eq!(summary.winner_wcps, Some(5.0));
        assert_eq!(summary.scenario_count, 3); // baseline + 2 scenarios
        assert_eq!(summary.run_id, "r1");
        assert_eq!(summary.project_id, "p1");
        assert_eq!(summary.completed_at, "2026-09-01T01:00:00Z");
    }

    #[test]
    fn summarize_of_a_baseline_only_run_has_no_winner() {
        let baseline = ScenarioResult {
            scenario_id: "baseline".into(),
            name: "Stock".into(),
            is_baseline: true,
            aggregated: dummy_metrics(),
            per_iteration: vec![],
            metric_deltas: vec![],
            wcps: 0.0,
            verdict: Verdict::ConfirmedSame,
        };
        let rr = RunResults {
            schema_version: "1.0.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            completed_at: "2026-09-01T01:00:00Z".into(),
            detection_tier: DetectionTier::LogTail,
            baseline,
            scenarios: vec![],
        };

        let summary = rr.summarize();
        assert_eq!(summary.winner_name, None);
        assert_eq!(summary.winner_wcps, None);
        assert_eq!(summary.scenario_count, 1);
    }

    #[test]
    fn run_results_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("results.json");
        let m = Metrics {
            avg_fps: 400.0,
            median_fps: 405.0,
            p1_fps: 250.0,
            p01_fps: 180.0,
            frame_time_mean_ms: 2.5,
            frame_time_stddev_ms: 0.4,
            frame_time_cv: 0.16,
            adaptive_frame_time_cv: 0.16,
            stutter_count_pct: 2.0,
            mean_abs_animation_error_ms: Some(0.1),
            gpu_busy_ms: Some(2.2),
            bottleneck_ratio: Some(0.9),
            render_latency_ms: None,
            dominant_present_mode: "Hardware: Independent Flip".into(),
            present_mode_consistent: true,
            present_mode_warning: None,
        };
        let sr = ScenarioResult {
            scenario_id: "baseline".into(),
            name: "Stock".into(),
            is_baseline: true,
            aggregated: m.clone(),
            per_iteration: vec![m.clone()],
            metric_deltas: vec![MetricDelta {
                metric: "avg_fps".into(),
                delta_pct: 5.0,
            }],
            wcps: 0.0,
            verdict: Verdict::ConfirmedSame,
        };
        let rr = RunResults {
            schema_version: "1.0.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            completed_at: "2026-09-01T01:00:00Z".into(),
            detection_tier: DetectionTier::FixedWindow,
            baseline: sr.clone(),
            scenarios: vec![],
        };
        rr.save(&p).unwrap();
        let back: RunResults = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(back.run_id, "r1");
        assert_eq!(back.baseline.aggregated.p1_fps, 250.0);
    }

    #[test]
    fn run_results_load_reads_back_what_save_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("results.json");
        let m = Metrics {
            avg_fps: 400.0,
            median_fps: 405.0,
            p1_fps: 250.0,
            p01_fps: 180.0,
            frame_time_mean_ms: 2.5,
            frame_time_stddev_ms: 0.4,
            frame_time_cv: 0.16,
            adaptive_frame_time_cv: 0.16,
            stutter_count_pct: 0.0,
            mean_abs_animation_error_ms: None,
            gpu_busy_ms: None,
            bottleneck_ratio: None,
            render_latency_ms: None,
            dominant_present_mode: String::new(),
            present_mode_consistent: true,
            present_mode_warning: None,
        };
        let sr = ScenarioResult {
            scenario_id: "baseline".into(),
            name: "Stock".into(),
            is_baseline: true,
            aggregated: m.clone(),
            per_iteration: vec![m],
            metric_deltas: vec![],
            wcps: 0.0,
            verdict: Verdict::ConfirmedSame,
        };
        let rr = RunResults {
            schema_version: "1.0.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            completed_at: "2026-09-01T01:00:00Z".into(),
            detection_tier: DetectionTier::FixedWindow,
            baseline: sr,
            scenarios: vec![],
        };
        rr.save(&p).unwrap();
        let loaded = RunResults::load(&p).unwrap();
        assert_eq!(loaded, rr);
    }
}
