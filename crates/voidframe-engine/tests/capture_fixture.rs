//! End-to-end: parse the real PresentMon v2.5.1 capture committed from the
//! verification spike (`docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md`
//! §12) and confirm the aggregated Metrics match independently
//! hand-computed values (via PowerShell over the same file, not derived
//! from this crate's own arithmetic — see the plan for the exact commands).

use std::path::PathBuf;
use voidframe_engine::capture::{aggregate_metrics, parse_presentmon_csv};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/presentmon-sample.csv")
}

#[test]
fn parses_the_real_fixture() {
    let samples = parse_presentmon_csv(&fixture_path()).unwrap();
    assert_eq!(samples.len(), 499);
}

#[test]
fn aggregates_to_independently_verified_values() {
    let samples = parse_presentmon_csv(&fixture_path()).unwrap();
    let m = aggregate_metrics(&samples).unwrap();

    // Tolerances are loose enough to absorb summation-order floating-point
    // differences between this Rust implementation and the PowerShell
    // script used to independently verify these numbers, tight enough to
    // catch a real formula bug.
    assert!(
        (m.avg_fps - 119.2568220758).abs() < 1e-3,
        "avg_fps = {}",
        m.avg_fps
    );
    assert!(
        (m.median_fps - 191.6039163841).abs() < 1e-3,
        "median_fps = {}",
        m.median_fps
    );
    assert!(
        (m.p1_fps - 48.5276327988).abs() < 1e-3,
        "p1_fps = {}",
        m.p1_fps
    );
    assert!(
        (m.p01_fps - 45.5455981441).abs() < 1e-3,
        "p01_fps = {}",
        m.p01_fps
    );
    assert!(
        (m.frame_time_mean_ms - 8.3852645291).abs() < 1e-3,
        "frame_time_mean_ms = {}",
        m.frame_time_mean_ms
    );
    assert!(
        (m.frame_time_stddev_ms - 6.1954936181).abs() < 1e-3,
        "frame_time_stddev_ms = {}",
        m.frame_time_stddev_ms
    );
    assert!(
        (m.frame_time_cv - 0.7388548801).abs() < 1e-3,
        "frame_time_cv = {}",
        m.frame_time_cv
    );
    assert!(
        (m.gpu_busy_ms.unwrap() - 0.8426797595).abs() < 1e-3,
        "gpu_busy_ms = {:?}",
        m.gpu_busy_ms
    );
    assert!(
        (m.render_latency_ms.unwrap() - 0.9284370741).abs() < 1e-3,
        "render_latency_ms = {:?}",
        m.render_latency_ms
    );
    assert!(
        (m.bottleneck_ratio.unwrap() - 0.1004953101).abs() < 1e-3,
        "bottleneck_ratio = {:?}",
        m.bottleneck_ratio
    );
}

#[test]
fn real_fixture_present_mode_summary_matches_observed_distribution() {
    // Real distribution, confirmed directly against the fixture file:
    // 493 "Hardware: Independent Flip", 4 "Composed: Flip", 2 "Hardware
    // Composed: Independent Flip" -- not synthesized, the actual capture
    // has a brief mode fluctuation in it.
    let samples = parse_presentmon_csv(&fixture_path()).unwrap();
    let m = aggregate_metrics(&samples).unwrap();
    assert_eq!(m.dominant_present_mode, "Hardware: Independent Flip");
    assert!(
        !m.present_mode_consistent,
        "the real capture has 6 non-dominant frames, not consistent"
    );
    assert_eq!(
        m.present_mode_warning, None,
        "dominant mode is Exclusive, no warning expected"
    );
}
