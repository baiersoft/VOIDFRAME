//! Parses PresentMon v2.5.1 CSV captures into per-frame samples, then
//! aggregates those into one measurement iteration's `Metrics`.

use crate::capture::present_mode::misconfiguration_warning;
use crate::error::{Error, Result};
use crate::model::results::Metrics;
use crate::stats::{mean, percentile, stddev_sample};
use std::collections::BTreeMap;
use std::path::Path;

/// One frame's timing data, decoded from a PresentMon CSV row. Only the
/// columns VOIDFRAME's metrics need are kept — not all 28 PresentMon
/// columns.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameSample {
    pub frame_time_ms: f64,
    pub gpu_busy_ms: Option<f64>,
    pub cpu_busy_ms: Option<f64>,
    pub render_latency_ms: Option<f64>,
    /// PresentMon's `MsAnimationError` column — `None` on the first frame
    /// of a capture (PresentMon has no prior frame to compare against yet,
    /// always `"NA"` there) and only ever needed by
    /// `stats::v3::mean_abs_animation_error_ms`, not WCPS v2's `Metrics`.
    /// A caller building that metric's input array filters out every
    /// `None` first (matches `study/research/scripts/metrics_v3.py`'s own
    /// documented caller contract — this column's `None`s are dropped
    /// upstream, never treated as zero).
    pub animation_error_ms: Option<f64>,
    /// PresentMon's raw `PresentMode` string (e.g. `"Hardware: Independent
    /// Flip"`, `"Composed: Flip"`) — always present, never `"NA"`.
    /// Classified by `capture::present_mode`
    /// (`docs/superpowers/plans/2026-09-01-m1-phase-2-capture-and-scoring.md`).
    pub present_mode: String,
}

/// The adaptive-CV sliding window, and the stutter-detection factor `k` --
/// both calibrated in the research trail (`study/research/wcps-v3-validation.md`
/// §2.2: `k=2.0`, a deliberate deviation from CapFrameX's own `k=2.5`,
/// because `k=2.5` measured a 3.7x noisier null on this benchmark's real
/// data). Not re-derived here -- these are the same constants
/// `stats::v3`'s own golden-value fixtures were generated against.
const ADAPTIVE_WINDOW_MS: f64 = 500.0;
const STUTTER_K: f64 = 2.0;

const BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];

fn strip_bom(bytes: &[u8]) -> &[u8] {
    if bytes.starts_with(&BOM) {
        &bytes[3..]
    } else {
        bytes
    }
}

/// "NA" (PresentMon's missing-value marker, case-insensitive) or an empty
/// field become `None`; anything else parses as `f64` or the row is
/// rejected by the caller.
fn parse_opt_f64(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() || t.eq_ignore_ascii_case("na") {
        None
    } else {
        t.parse().ok()
    }
}

struct ColumnIndex {
    ms_between_presents: usize,
    ms_gpu_busy: usize,
    ms_cpu_busy: usize,
    ms_render_present_latency: usize,
    ms_animation_error: usize,
    present_mode: usize,
}

impl ColumnIndex {
    fn resolve(headers: &csv::StringRecord) -> Result<Self> {
        let find = |name: &str| -> Result<usize> {
            headers.iter().position(|h| h == name).ok_or_else(|| {
                Error::msg(format!("PresentMon CSV missing expected column `{name}`"))
            })
        };
        Ok(Self {
            ms_between_presents: find("MsBetweenPresents")?,
            ms_gpu_busy: find("MsGPUBusy")?,
            ms_cpu_busy: find("MsCPUBusy")?,
            ms_render_present_latency: find("MsRenderPresentLatency")?,
            ms_animation_error: find("MsAnimationError")?,
            present_mode: find("PresentMode")?,
        })
    }
}

/// Reads a PresentMon v2.5.1 CSV capture into per-frame samples.
///
/// Header-name column resolution only (never a hardcoded index). Strips a
/// leading UTF-8 BOM if present (confirmed present in real PresentMon
/// v2.5.1 output — `spike/findings.md` §3). `"NA"` values become `None` for
/// the optional fields; `MsBetweenPresents` (frame time) is required per
/// row and a missing/unparseable value fails the whole parse — never a
/// silent data point.
pub fn parse_presentmon_csv(path: &Path) -> Result<Vec<FrameSample>> {
    let bytes = std::fs::read(path)?;
    let bytes = strip_bom(&bytes);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(bytes);
    let cols = ColumnIndex::resolve(reader.headers()?)?;

    let mut samples = Vec::new();
    for result in reader.records() {
        let record = result?;
        let frame_time_str = record
            .get(cols.ms_between_presents)
            .ok_or_else(|| Error::msg("row missing MsBetweenPresents field".into()))?;
        let frame_time_ms: f64 = frame_time_str.trim().parse().map_err(|_| {
            Error::msg(format!(
                "unparseable MsBetweenPresents value: `{frame_time_str}`"
            ))
        })?;
        let present_mode = record
            .get(cols.present_mode)
            .ok_or_else(|| Error::msg("row missing PresentMode field".into()))?
            .to_string();

        samples.push(FrameSample {
            frame_time_ms,
            gpu_busy_ms: record.get(cols.ms_gpu_busy).and_then(parse_opt_f64),
            cpu_busy_ms: record.get(cols.ms_cpu_busy).and_then(parse_opt_f64),
            render_latency_ms: record
                .get(cols.ms_render_present_latency)
                .and_then(parse_opt_f64),
            animation_error_ms: record.get(cols.ms_animation_error).and_then(parse_opt_f64),
            present_mode,
        });
    }

    if samples.is_empty() {
        return Err(Error::msg(
            "PresentMon CSV has no data rows (empty or truncated capture)".into(),
        ));
    }
    Ok(samples)
}

/// Aggregates one measurement iteration's frame samples into its `Metrics`.
/// See docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §8 for the exact derivation of each field.
pub fn aggregate_metrics(samples: &[FrameSample]) -> Result<Metrics> {
    if samples.is_empty() {
        return Err(Error::msg("cannot aggregate zero frame samples".into()));
    }

    let frame_times: Vec<f64> = samples.iter().map(|s| s.frame_time_ms).collect();
    let frame_time_mean_ms = mean(&frame_times);
    let frame_time_stddev_ms = stddev_sample(&frame_times);

    let mut sorted_ft = frame_times.clone();
    sorted_ft.sort_by(|a, b| a.partial_cmp(b).expect("frame times are never NaN"));
    let median_ft = percentile(&sorted_ft, 50.0);
    let p99_ft = percentile(&sorted_ft, 99.0);
    let p999_ft = percentile(&sorted_ft, 99.9);

    let gpu_vals: Vec<f64> = samples.iter().filter_map(|s| s.gpu_busy_ms).collect();
    let gpu_busy_ms = (!gpu_vals.is_empty()).then(|| mean(&gpu_vals));

    let render_vals: Vec<f64> = samples.iter().filter_map(|s| s.render_latency_ms).collect();
    let render_latency_ms = (!render_vals.is_empty()).then(|| mean(&render_vals));

    let bottleneck_ratio = gpu_busy_ms.map(|g| g / frame_time_mean_ms);

    // Present-mode summary. BTreeMap for deterministic (sorted-key)
    // iteration order -- max_by_key ties break toward the alphabetically-
    // last mode name, arbitrary but reproducible run-to-run for the same
    // input (a HashMap's non-deterministic order would make an occasional
    // exact tie pick a different "dominant" mode on every run).
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for s in samples {
        *counts.entry(s.present_mode.as_str()).or_insert(0) += 1;
    }
    let dominant_present_mode = counts
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(mode, _)| (*mode).to_string())
        .expect("samples is non-empty, checked above");
    let present_mode_consistent = counts.len() == 1;
    let present_mode_warning = misconfiguration_warning(&dominant_present_mode);

    let animation_errors: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.animation_error_ms)
        .collect();
    let adaptive_frame_time_cv =
        crate::stats::v3::adaptive_frame_time_cv(&frame_times, ADAPTIVE_WINDOW_MS);
    let stutter_count_pct = crate::stats::v3::stutter_count_pct(&frame_times, STUTTER_K);
    let mean_abs_animation_error_ms = (!animation_errors.is_empty())
        .then(|| crate::stats::v3::mean_abs_animation_error_ms(&animation_errors));

    Ok(Metrics {
        avg_fps: 1000.0 / frame_time_mean_ms,
        median_fps: 1000.0 / median_ft,
        p1_fps: 1000.0 / p99_ft,
        p01_fps: 1000.0 / p999_ft,
        frame_time_mean_ms,
        frame_time_stddev_ms,
        frame_time_cv: frame_time_stddev_ms / frame_time_mean_ms,
        adaptive_frame_time_cv,
        stutter_count_pct,
        mean_abs_animation_error_ms,
        gpu_busy_ms,
        bottleneck_ratio,
        render_latency_ms,
        dominant_present_mode,
        present_mode_consistent,
        present_mode_warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(frame_time_ms: f64, gpu_busy_ms: Option<f64>, present_mode: &str) -> FrameSample {
        FrameSample {
            frame_time_ms,
            gpu_busy_ms,
            cpu_busy_ms: None,
            render_latency_ms: None,
            animation_error_ms: None,
            present_mode: present_mode.to_string(),
        }
    }

    fn frame_with_anim(
        frame_time_ms: f64,
        gpu_busy_ms: Option<f64>,
        animation_error_ms: Option<f64>,
        present_mode: &str,
    ) -> FrameSample {
        FrameSample {
            frame_time_ms,
            gpu_busy_ms,
            cpu_busy_ms: None,
            render_latency_ms: None,
            animation_error_ms,
            present_mode: present_mode.to_string(),
        }
    }

    #[test]
    fn aggregate_metrics_computes_the_3_new_v3_metrics_from_real_frame_data() {
        // 5 frames, one clear stutter (30ms against a ~12.5ms mean) --
        // enough for stutter_count_pct to be nonzero and
        // adaptive_frame_time_cv to be meaningfully above zero. First frame
        // has no animation-error value (matches real PresentMon behavior --
        // frame 0 is always NA), the rest do.
        let samples = vec![
            frame_with_anim(10.0, None, None, "Hardware: Independent Flip"),
            frame_with_anim(10.0, None, Some(0.05), "Hardware: Independent Flip"),
            frame_with_anim(10.0, None, Some(0.06), "Hardware: Independent Flip"),
            frame_with_anim(10.0, None, Some(0.04), "Hardware: Independent Flip"),
            frame_with_anim(30.0, None, Some(0.20), "Hardware: Independent Flip"),
        ];
        let m = aggregate_metrics(&samples).unwrap();

        // mean frame time = 14.0ms; k=2.0 threshold = 28.0ms; only the 30ms
        // frame exceeds it -> 1/5 = 20%.
        assert!((m.stutter_count_pct - 20.0).abs() < 1e-9);
        // Sanity, not an exact hand-derived value (the windowed-average
        // formula is already golden-value tested against Python in
        // `stats::v3`'s own test suite) -- just confirm it's finite,
        // nonzero, and in the right ballpark for a capture with one real
        // stutter.
        assert!(m.adaptive_frame_time_cv.is_finite());
        assert!(m.adaptive_frame_time_cv > 0.0);
        // mean of |0.05|,|0.06|,|0.04|,|0.20| = 0.0875, frame 0's None
        // correctly excluded rather than treated as 0.0.
        assert!((m.mean_abs_animation_error_ms.unwrap() - 0.0875).abs() < 1e-9);
    }

    #[test]
    fn aggregate_metrics_animation_error_is_none_when_every_frame_is_na() {
        let samples = vec![
            frame_with_anim(10.0, None, None, "Hardware: Independent Flip"),
            frame_with_anim(10.0, None, None, "Hardware: Independent Flip"),
        ];
        let m = aggregate_metrics(&samples).unwrap();
        assert_eq!(m.mean_abs_animation_error_ms, None);
    }

    #[test]
    fn aggregate_metrics_of_four_known_frames() {
        // frame times 10, 20, 30, 40 ms -> mean 25ms -> avg_fps = 40.0
        let samples = vec![
            frame(10.0, Some(5.0), "Hardware: Independent Flip"),
            frame(20.0, Some(10.0), "Hardware: Independent Flip"),
            frame(30.0, Some(15.0), "Hardware: Independent Flip"),
            frame(40.0, Some(20.0), "Hardware: Independent Flip"),
        ];
        let m = aggregate_metrics(&samples).unwrap();
        assert!((m.frame_time_mean_ms - 25.0).abs() < 1e-9);
        assert!((m.avg_fps - 40.0).abs() < 1e-9);
        // gpu_busy_ms mean of 5,10,15,20 = 12.5
        assert_eq!(m.gpu_busy_ms, Some(12.5));
        assert!((m.bottleneck_ratio.unwrap() - (12.5 / 25.0)).abs() < 1e-9);
        // no render latency samples present at all -> None, not 0.0
        assert_eq!(m.render_latency_ms, None);
        assert_eq!(m.dominant_present_mode, "Hardware: Independent Flip");
        assert!(m.present_mode_consistent);
        assert_eq!(m.present_mode_warning, None);
    }

    #[test]
    fn aggregate_metrics_rejects_empty_input() {
        assert!(aggregate_metrics(&[]).is_err());
    }

    #[test]
    fn mixed_present_mode_is_inconsistent_but_exclusive_dominant_has_no_warning() {
        let samples = vec![
            frame(10.0, None, "Hardware: Independent Flip"),
            frame(10.0, None, "Hardware: Independent Flip"),
            frame(10.0, None, "Hardware: Independent Flip"),
            frame(10.0, None, "Composed: Flip"),
        ];
        let m = aggregate_metrics(&samples).unwrap();
        assert_eq!(m.dominant_present_mode, "Hardware: Independent Flip");
        assert!(!m.present_mode_consistent);
        // dominant mode is still Exclusive -- a handful of stray composed
        // frames (e.g. a brief compositor handoff) doesn't warrant a
        // warning on its own.
        assert_eq!(m.present_mode_warning, None);
    }

    #[test]
    fn compositor_dominant_present_mode_produces_a_warning() {
        let samples = vec![
            frame(10.0, None, "Composed: Flip"),
            frame(10.0, None, "Composed: Flip"),
            frame(10.0, None, "Hardware: Independent Flip"),
        ];
        let m = aggregate_metrics(&samples).unwrap();
        assert_eq!(m.dominant_present_mode, "Composed: Flip");
        assert!(!m.present_mode_consistent);
        let warning = m
            .present_mode_warning
            .expect("expected a warning for a Composed dominant mode");
        assert!(warning.contains("compositor"));
    }

    #[test]
    fn unknown_or_empty_dominant_present_mode_produces_no_warning() {
        // An unrecognised PresentMode string (e.g. a future PresentMon
        // version) classifies as Unknown, not Composed -- we have no
        // evidence of a compositor misconfiguration, so no warning should
        // claim CS2 isn't in exclusive Fullscreen.
        let unknown_samples = vec![
            frame(10.0, None, "Something Future PresentMon Adds"),
            frame(10.0, None, "Something Future PresentMon Adds"),
        ];
        let m = aggregate_metrics(&unknown_samples).unwrap();
        assert_eq!(m.dominant_present_mode, "Something Future PresentMon Adds");
        assert_eq!(m.present_mode_warning, None);

        // An empty PresentMode string (e.g. reachable via `Metrics`'s
        // `#[serde(default)]` when deserializing a legacy results.json)
        // also classifies as Unknown -- same expectation.
        let empty_samples = vec![frame(10.0, None, ""), frame(10.0, None, "")];
        let m = aggregate_metrics(&empty_samples).unwrap();
        assert_eq!(m.dominant_present_mode, "");
        assert_eq!(m.present_mode_warning, None);
    }

    // Real header + 2 real data rows, byte-for-byte from the verification
    // spike (`docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md`
    // §12) capture (spike/output/presentmon-20260901-030459.csv) —
    // including the genuine UTF-8 BOM that spike confirmed precedes the
    // header, and the real "NA" values PresentMon writes for columns with no data that frame.
    const REAL_SAMPLE: &str = "\u{feff}Application,ProcessID,SwapChainAddress,PresentRuntime,SyncInterval,PresentFlags,AllowsTearing,PresentMode,TimeInMs,MsBetweenSimulationStart,MsBetweenPresents,MsBetweenDisplayChange,MsInPresentAPI,MsRenderPresentLatency,MsUntilDisplayed,CPUStartTimeInMs,MsBetweenAppStart,MsCPUBusy,MsCPUWait,MsGPULatency,MsGPUTime,MsGPUBusy,MsGPUWait,MsAnimationError,AnimationTime,MsFlipDelay,MsAllInputToPhotonLatency,MsClickToPhotonLatency\ncs2.exe,12332,0x2AC3BFD7650,DXGI,0,512,1,Hardware: Independent Flip,40.0844,NA,19.21460000000000,19.09680000000000,0.02200000000000,1.66530000000000,1.6653,20.9582,19.1482,19.1262,0.0220,1.6947,19.0968,1.5390,17.5578,NA,20.9582,NA,NA,NA\ncs2.exe,12332,0x2AC3BFD7650,DXGI,0,512,1,Hardware: Independent Flip,59.0459,NA,18.96150000000000,19.08600000000000,0.03450000000000,1.78980000000000,1.7898,40.1064,18.9740,18.9395,0.0345,1.6433,19.0860,1.6529,17.4331,0.0622,40.1064,NA,NA,NA\n";

    fn write_temp_csv(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.csv");
        std::fs::write(&path, contents).unwrap();
        (dir, path)
    }

    #[test]
    fn parses_real_bom_prefixed_rows_with_na_values() {
        let (_dir, path) = write_temp_csv(REAL_SAMPLE);
        let samples = parse_presentmon_csv(&path).unwrap();
        assert_eq!(samples.len(), 2);

        // Row 1: MsBetweenPresents=19.2146, MsGPUBusy=1.5390,
        // MsRenderPresentLatency=1.6653, MsAnimationError=NA (real
        // PresentMon behavior on the very first frame of a capture -- no
        // prior frame to compare against yet), PresentMode="Hardware:
        // Independent Flip" (all real values).
        assert!((samples[0].frame_time_ms - 19.2146).abs() < 1e-4);
        assert_eq!(samples[0].gpu_busy_ms, Some(1.5390));
        assert_eq!(samples[0].render_latency_ms, Some(1.6653));
        assert_eq!(samples[0].animation_error_ms, None);
        assert_eq!(samples[0].present_mode, "Hardware: Independent Flip");

        // Row 2: MsAnimationError=0.0622, a genuine non-NA real value --
        // confirms the happy-path numeric parse, not just the NA branch.
        assert_eq!(samples[1].animation_error_ms, Some(0.0622));

        // Confirms successful end-to-end parsing of a real BOM-prefixed
        // PresentMon capture: header resolution, row decoding, and value
        // parsing all agree with the known-good values from that spike.
    }

    #[test]
    fn strip_bom_removes_exactly_the_bom_and_nothing_else() {
        let with_bom = "\u{feff}Application,X\ncs2.exe,1\n".as_bytes();
        let without_bom = "Application,X\ncs2.exe,1\n".as_bytes();
        assert_eq!(strip_bom(with_bom), without_bom);
        // no-op when there's no BOM to strip
        assert_eq!(strip_bom(without_bom), without_bom);
    }

    #[test]
    fn cpu_busy_ms_parses_from_real_populated_value() {
        let (_dir, path) = write_temp_csv(REAL_SAMPLE);
        let samples = parse_presentmon_csv(&path).unwrap();
        // cpu_busy_ms IS populated in this sample (MsCPUBusy=19.1262) --
        // none of the FrameSample-tracked optional columns are "NA" in
        // REAL_SAMPLE, so this only confirms ordinary numeric parsing.
        // The NA-to-None branch itself is exercised by
        // `na_value_in_tracked_optional_column_becomes_none` below.
        assert_eq!(samples[0].cpu_busy_ms, Some(19.1262));
    }

    #[test]
    fn na_value_in_tracked_optional_column_becomes_none() {
        // Synthetic CSV (not pulled from a real capture) -- none of the
        // FrameSample-tracked optional columns (MsGPUBusy, MsCPUBusy,
        // MsRenderPresentLatency) are ever "NA" in REAL_SAMPLE's two rows,
        // so this constructs a row where one genuinely is, to actually
        // exercise the NA-to-None branch for a tracked field.
        // MsAnimationError is genuinely "NA" here too, matching a real
        // capture's own first-frame behavior.
        let csv = "PresentMode,MsBetweenPresents,MsGPUBusy,MsCPUBusy,MsRenderPresentLatency,MsAnimationError\nHardware: Independent Flip,19.2146,NA,19.1262,1.6653,NA\n";
        let (_dir, path) = write_temp_csv(csv);
        let samples = parse_presentmon_csv(&path).unwrap();
        assert_eq!(samples[0].gpu_busy_ms, None);
        assert_eq!(samples[0].cpu_busy_ms, Some(19.1262));
        assert_eq!(samples[0].render_latency_ms, Some(1.6653));
        assert_eq!(samples[0].animation_error_ms, None);
    }

    #[test]
    fn rejects_empty_csv() {
        let (_dir, path) = write_temp_csv("");
        let err = parse_presentmon_csv(&path).unwrap_err();
        assert!(err.is_msg() || err.is_csv());
    }

    #[test]
    fn rejects_header_only_csv() {
        let header_only = "\u{feff}Application,ProcessID,SwapChainAddress,PresentRuntime,SyncInterval,PresentFlags,AllowsTearing,PresentMode,TimeInMs,MsBetweenSimulationStart,MsBetweenPresents,MsBetweenDisplayChange,MsInPresentAPI,MsRenderPresentLatency,MsUntilDisplayed,CPUStartTimeInMs,MsBetweenAppStart,MsCPUBusy,MsCPUWait,MsGPULatency,MsGPUTime,MsGPUBusy,MsGPUWait,MsAnimationError,AnimationTime,MsFlipDelay,MsAllInputToPhotonLatency,MsClickToPhotonLatency\n";
        let (_dir, path) = write_temp_csv(header_only);
        let err = parse_presentmon_csv(&path).unwrap_err();
        assert!(err.is_msg() && err.to_string().contains("no data rows"));
    }

    #[test]
    fn rejects_csv_missing_a_required_column() {
        let (_dir, path) = write_temp_csv("Application,ProcessID\ncs2.exe,1\n");
        let err = parse_presentmon_csv(&path).unwrap_err();
        assert!(err.is_msg() && err.to_string().contains("MsBetweenPresents"));
    }
}
