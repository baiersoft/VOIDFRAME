//! `collect_thermal_sample` — samples HWiNFO's CPU/GPU temperature sensors
//! `sample_count` times, `sample_interval` apart, and averages them into one
//! [`ThermalReading`]. The reusable primitive Tasks 4-5's thermal-cooldown
//! wait loop is built on top of (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2); this module owns only the
//! sample-and-average step, not the "keep sampling until cool" polling loop
//! itself.
//!
//! Also owns HWiNFO lifecycle around the sampling: per docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2, VOIDFRAME
//! only ever starts/closes HWiNFO itself when it wasn't already running when
//! sampling began -- the same "only touch what we started" precedent as
//! `close_steam_window`.

use crate::error::Result;
use crate::system::SystemController;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// The real production sampling interval (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2): one reading per
/// second. Used by Tasks 4-5, not by [`collect_thermal_sample`] itself --
/// interval/count are parameters precisely so tests can use tiny values
/// instead.
pub const THERMAL_SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
/// The real production sample count (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2): 30 readings, averaged.
pub const THERMAL_SAMPLE_COUNT: u32 = 30;

/// The result of averaging `sample_count` HWiNFO sensor readings,
/// `sample_interval` apart. Persisted verbatim into `thermal.json`
/// (`model::thermal::ThermalLog`) -- `Serialize`/`Deserialize` are for that
/// file round-trip, not IPC (nothing in this plan exposes `ThermalReading`
/// over Tauri directly, so no `specta::Type` derive here).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalReading {
    pub cpu_temp_celsius: f64,
    pub gpu_temp_celsius: Option<f64>,
    pub sample_count: u32,
}

/// Samples HWiNFO's CPU/GPU temperature sensors `sample_count` times,
/// `sample_interval` apart, and returns their average. Starts HWiNFO first
/// (from `hwinfo_path`) only if it isn't already running, and closes it
/// again afterward only in that same case -- docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2's "only touch what we
/// started" rule, mirroring `close_steam_window`.
///
/// # Errors
///
/// Returns `Err` if checking/starting/closing HWiNFO or reading its sensors
/// fails.
pub async fn collect_thermal_sample(
    sys: &dyn SystemController,
    hwinfo_path: &Path,
    sample_interval: Duration,
    sample_count: u32,
) -> Result<ThermalReading> {
    let already_running = sys.hwinfo_already_running().await?;
    if !already_running {
        sys.start_hwinfo(hwinfo_path).await?;
    }

    // Captured rather than propagated immediately via `?`: a sensor-read
    // failure partway through the loop must still reach the cleanup below
    // when this call is the one that started HWiNFO -- otherwise a failed
    // sample leaves HWiNFO running, orphaned, forever. (The cooldown-wait
    // loop in `run::execute`'s `wait_for_thermal_cooldown` no longer calls
    // this function repeatedly -- it manages HWiNFO's lifecycle itself,
    // once, around its own loop over `sample_loop` directly -- but this
    // function's own callers, like `execute()`'s one-shot pre-run baseline
    // collection, still depend on this same cleanup guarantee.)
    let sample_result = sample_loop(sys, sample_interval, sample_count).await;

    if !already_running && let Err(close_err) = sys.close_hwinfo().await {
        // A close failure must never mask the original sampling error
        // (or hide a successful read behind a spurious one) -- log and
        // move on, returning `sample_result` untouched either way.
        tracing::warn!(
            error = %close_err,
            "failed to close HWiNFO after a thermal sample attempt"
        );
    }

    sample_result
}

/// The sample-and-average loop itself, factored out so its `Result` can be
/// captured by [`collect_thermal_sample`] instead of propagated immediately
/// via `?` -- see that function's own comment for why. `pub(super)` (rather
/// than private) so `run::execute`'s own `wait_for_thermal_cooldown` can
/// sample repeatedly against one HWiNFO instance it starts/closes itself,
/// instead of going through [`collect_thermal_sample`]'s own start-sample-
/// close cycle on every check -- see that function's doc comment.
pub(super) async fn sample_loop(
    sys: &dyn SystemController,
    sample_interval: Duration,
    sample_count: u32,
) -> Result<ThermalReading> {
    let mut cpu_sum = 0.0;
    let mut gpu_sum = 0.0;
    let mut gpu_seen = 0u32;
    for i in 0..sample_count {
        let reading = sys.read_hwinfo_sensors().await?;
        cpu_sum += reading.cpu_temp_celsius;
        if let Some(gpu) = reading.gpu_temp_celsius {
            gpu_sum += gpu;
            gpu_seen += 1;
        }
        if i + 1 < sample_count {
            tokio::time::sleep(sample_interval).await;
        }
    }

    Ok(ThermalReading {
        cpu_temp_celsius: cpu_sum / f64::from(sample_count),
        gpu_temp_celsius: if gpu_seen > 0 {
            Some(gpu_sum / f64::from(gpu_seen))
        } else {
            None
        },
        sample_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::MockController;

    fn dummy_hwinfo_path() -> std::path::PathBuf {
        std::path::PathBuf::from("C:\\fake\\hwinfo.exe")
    }

    #[tokio::test]
    async fn averages_readings_across_every_sample() {
        tokio::time::pause();
        let sys = MockController::new().with_hwinfo_readings(vec![
            (60.0, Some(50.0)),
            (62.0, Some(52.0)),
            (61.0, Some(51.0)),
        ]);
        let reading =
            collect_thermal_sample(&sys, &dummy_hwinfo_path(), Duration::from_millis(1), 3)
                .await
                .unwrap();
        assert_eq!(reading.cpu_temp_celsius, 61.0); // (60+62+61)/3
        assert_eq!(reading.gpu_temp_celsius, Some(51.0)); // (50+52+51)/3
        assert_eq!(reading.sample_count, 3);
    }

    #[tokio::test]
    async fn starts_hwinfo_only_when_not_already_running_and_closes_only_what_it_started() {
        tokio::time::pause();
        let sys = MockController::new(); // not already running
        let _ =
            collect_thermal_sample(&sys, &dummy_hwinfo_path(), Duration::from_millis(1), 2).await;
        assert_eq!(sys.start_hwinfo_calls(), 1);
        assert_eq!(sys.close_hwinfo_calls(), 1);

        let sys2 = MockController::new().with_process("HWiNFO64.exe", 4242); // already running
        let _ =
            collect_thermal_sample(&sys2, &dummy_hwinfo_path(), Duration::from_millis(1), 2).await;
        assert_eq!(sys2.start_hwinfo_calls(), 0);
        assert_eq!(sys2.close_hwinfo_calls(), 0);
    }

    /// Regression test: a sensor read failing partway through the sample
    /// loop must not skip HWiNFO cleanup when this call is the one that
    /// started it -- otherwise HWiNFO is left running, orphaned, forever
    /// (compounds badly once the pre-run thermal-baseline phase's
    /// cooldown-wait loop, `docs/superpowers/plans/2026-09-04-thermal-cooldown.md`,
    /// calls this repeatedly).
    #[tokio::test]
    async fn close_hwinfo_still_runs_when_sampling_fails_partway_through() {
        tokio::time::pause();
        let sys = MockController::new().with_hwinfo_read_failing_after(1); // not already running
        let err = collect_thermal_sample(&sys, &dummy_hwinfo_path(), Duration::from_millis(1), 3)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("hwinfo sensor read failed"));
        assert_eq!(sys.start_hwinfo_calls(), 1);
        assert_eq!(sys.close_hwinfo_calls(), 1);
    }
}
