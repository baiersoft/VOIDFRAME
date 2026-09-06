//! Standalone diagnostic checks for the Steam-readiness/launch flow and the
//! HWiNFO sensor-read flow -- exercised by hand via `voidframe-cli
//! steam-launch-check`/`voidframe-cli hwinfo-check` against a real machine,
//! without needing a full project/scenario or the Tauri UI. Re-exported by
//! `super::execute` (`mod.rs`) so `voidframe_engine::run::execute::{steam_launch_check, hwinfo_check}`
//! keeps resolving for existing callers.

use super::steam::{ensure_steam_running, graceful_kill_cs2, wait_for_process};
use crate::system::{Cs2LaunchSpec, SystemController};
use std::time::{Duration, Instant};

/// One step of a [`steam_launch_check`] run.
#[derive(Debug, Clone)]
pub struct SteamLaunchCheckStep {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct SteamLaunchCheckReport {
    pub steps: Vec<SteamLaunchCheckStep>,
    pub overall_ok: bool,
}

/// Exercises the Steam-readiness/launch flow standalone -- initial status,
/// `ensure_steam_running` (the steamwebhelper-aware readiness wait, window
/// close, and post-close safety net all happen inside that one call), then
/// `launch_cs2` + waiting for `cs2.exe` to appear -- with per-step timing
/// and outcome, against a real machine. Exists so this flow can be verified
/// by hand (`voidframe-cli steam-launch-check --controller windows`)
/// without needing a full project/scenario or the Tauri UI. Never panics or
/// propagates an `Err`: every step's failure is recorded in the report
/// (`overall_ok = false`, that step's `ok = false`) so the caller always
/// gets a complete picture of how far it got, not just the first error.
pub async fn steam_launch_check(
    sys: &dyn SystemController,
    kill_cs2_after: bool,
) -> SteamLaunchCheckReport {
    let mut report = SteamLaunchCheckReport {
        steps: Vec::new(),
        overall_ok: true,
    };

    let t = Instant::now();
    match sys.steam_status().await {
        Ok(s) => report.steps.push(SteamLaunchCheckStep {
            name: "initial_steam_status".into(),
            ok: true,
            detail: format!("running={} elevated={}", s.running, s.elevated),
            duration_ms: t.elapsed().as_millis(),
        }),
        Err(e) => {
            report.overall_ok = false;
            report.steps.push(SteamLaunchCheckStep {
                name: "initial_steam_status".into(),
                ok: false,
                detail: e.to_string(),
                duration_ms: t.elapsed().as_millis(),
            });
            return report;
        }
    }

    let t = Instant::now();
    match ensure_steam_running(sys).await {
        Ok(()) => report.steps.push(SteamLaunchCheckStep {
            name: "ensure_steam_running".into(),
            ok: true,
            detail: "Steam ready (steam.exe + steamwebhelper.exe discoverable; its window was \
                      closed if this call is what launched it)"
                .into(),
            duration_ms: t.elapsed().as_millis(),
        }),
        Err(e) => {
            report.overall_ok = false;
            report.steps.push(SteamLaunchCheckStep {
                name: "ensure_steam_running".into(),
                ok: false,
                detail: e.to_string(),
                duration_ms: t.elapsed().as_millis(),
            });
            return report;
        }
    }

    let t = Instant::now();
    let cs2_result = async {
        sys.launch_cs2(&Cs2LaunchSpec { app_id: 730 }).await?;
        wait_for_process(sys, "cs2.exe", Duration::from_secs(60), true).await
    }
    .await;
    match cs2_result {
        Ok(handle) => report.steps.push(SteamLaunchCheckStep {
            name: "launch_cs2".into(),
            ok: true,
            detail: format!("cs2.exe discovered, pid={}", handle.pid),
            duration_ms: t.elapsed().as_millis(),
        }),
        Err(e) => {
            report.overall_ok = false;
            report.steps.push(SteamLaunchCheckStep {
                name: "launch_cs2".into(),
                ok: false,
                detail: e.to_string(),
                duration_ms: t.elapsed().as_millis(),
            });
        }
    }

    if kill_cs2_after {
        let _ = graceful_kill_cs2(sys).await;
    }

    report
}

/// One step of an [`hwinfo_check`] run.
#[derive(Debug, Clone)]
pub struct HwinfoCheckStep {
    pub name: String,
    pub ok: bool,
    pub detail: String,
    pub duration_ms: u128,
}

#[derive(Debug, Clone, Default)]
pub struct HwinfoCheckReport {
    pub steps: Vec<HwinfoCheckStep>,
    pub overall_ok: bool,
}

/// Exercises the HWiNFO sensor-read flow standalone -- checks whether
/// HWiNFO is already running, starts it from `hwinfo_path` if not, reads
/// its CPU/GPU temperature sensors (the step whose `detail` carries the
/// real numbers -- the actual point of this diagnostic), then closes
/// HWiNFO again if this call is what started it -- with per-step timing
/// and outcome, against a real machine. Exists so this flow can be
/// verified by hand (`voidframe-cli hwinfo-check --controller windows`)
/// without needing a full project/scenario or the Tauri UI. Never panics
/// or propagates an `Err`: every step's failure is recorded in the report
/// (`overall_ok = false`, that step's `ok = false`) so the caller always
/// gets a complete picture of how far it got, not just the first error.
/// A failing `read_hwinfo_sensors` step does not skip the closing step --
/// mirroring `collect_thermal_sample`'s own "cleanup must still run" rule
/// for the same reason: skipping it would leave HWiNFO running, orphaned,
/// whenever this check is what started it.
pub async fn hwinfo_check(
    sys: &dyn SystemController,
    hwinfo_path: &std::path::Path,
) -> HwinfoCheckReport {
    let mut report = HwinfoCheckReport {
        steps: Vec::new(),
        overall_ok: true,
    };

    let t = Instant::now();
    let already_running = match sys.hwinfo_already_running().await {
        Ok(r) => {
            report.steps.push(HwinfoCheckStep {
                name: "hwinfo_already_running".into(),
                ok: true,
                detail: format!("already_running={r}"),
                duration_ms: t.elapsed().as_millis(),
            });
            r
        }
        Err(e) => {
            report.overall_ok = false;
            report.steps.push(HwinfoCheckStep {
                name: "hwinfo_already_running".into(),
                ok: false,
                detail: e.to_string(),
                duration_ms: t.elapsed().as_millis(),
            });
            return report;
        }
    };

    if !already_running {
        let t = Instant::now();
        match sys.start_hwinfo(hwinfo_path).await {
            Ok(()) => report.steps.push(HwinfoCheckStep {
                name: "start_hwinfo".into(),
                ok: true,
                detail: "HWiNFO started".into(),
                duration_ms: t.elapsed().as_millis(),
            }),
            Err(e) => {
                report.overall_ok = false;
                report.steps.push(HwinfoCheckStep {
                    name: "start_hwinfo".into(),
                    ok: false,
                    detail: e.to_string(),
                    duration_ms: t.elapsed().as_millis(),
                });
                return report;
            }
        }
    }

    let t = Instant::now();
    match sys.read_hwinfo_sensors().await {
        Ok(snapshot) => report.steps.push(HwinfoCheckStep {
            name: "read_hwinfo_sensors".into(),
            ok: true,
            detail: format!(
                "CPU {:.1}C, GPU {}",
                snapshot.cpu_temp_celsius,
                snapshot
                    .gpu_temp_celsius
                    .map(|g| format!("{g:.1}C"))
                    .unwrap_or_else(|| "n/a".into())
            ),
            duration_ms: t.elapsed().as_millis(),
        }),
        Err(e) => {
            report.overall_ok = false;
            report.steps.push(HwinfoCheckStep {
                name: "read_hwinfo_sensors".into(),
                ok: false,
                detail: e.to_string(),
                duration_ms: t.elapsed().as_millis(),
            });
        }
    }

    if !already_running {
        let t = Instant::now();
        match sys.close_hwinfo().await {
            Ok(()) => report.steps.push(HwinfoCheckStep {
                name: "close_hwinfo".into(),
                ok: true,
                detail: "HWiNFO closed".into(),
                duration_ms: t.elapsed().as_millis(),
            }),
            Err(e) => {
                report.overall_ok = false;
                report.steps.push(HwinfoCheckStep {
                    name: "close_hwinfo".into(),
                    ok: false,
                    detail: e.to_string(),
                    duration_ms: t.elapsed().as_millis(),
                });
            }
        }
    }

    report
}
