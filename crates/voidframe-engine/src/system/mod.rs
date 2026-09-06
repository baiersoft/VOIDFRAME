//! `SystemController` — the single seam between the engine and the operating
//! system. `MockController` and `DryRunController` ship first
//! (`docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md`); the
//! real `WindowsController` lands later
//! (`docs/superpowers/plans/2026-09-01-m1-phase-3a-windows-controller.md`).
//!
//! Only reads + mutations are on the trait in the engine-foundation plan
//! above. Process control (CS2 launch, PresentMon, process suspend) is
//! added by `docs/superpowers/plans/2026-09-01-m1-phase-3b-cs2-control-run-loop.md`.

pub mod dry_run;
pub mod mock;
#[cfg(windows)]
pub mod windows;

use crate::error::Result;
use crate::model::module::{Hive, RegType};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use dry_run::DryRunController;
pub use mock::MockController;
#[cfg(windows)]
pub use windows::WindowsController;

/// A registry value location.
#[derive(Debug, Clone, PartialEq)]
pub struct RegKey {
    pub hive: Hive,
    pub subkey: String,
    pub value_name: String,
}

/// A registry value, or the fact that the value does not exist.
#[derive(Debug, Clone, PartialEq)]
pub enum RegValue {
    Dword(u32),
    Qword(u64),
    Sz(String),
    Binary(Vec<u8>),
    /// `REG_EXPAND_SZ` — a string containing unexpanded environment-variable
    /// references (e.g. `%SystemRoot%\...`). Stored verbatim; VOIDFRAME never
    /// expands it itself.
    ExpandSz(String),
    /// `REG_MULTI_SZ` — an ordered list of strings. Represents exactly the
    /// wire format's own strings; the NUL-separated / double-NUL-terminated
    /// encoding is an implementation detail of `system::windows::registry`.
    MultiSz(Vec<String>),
    Absent,
}

impl RegValue {
    pub fn describe(&self) -> String {
        match self {
            RegValue::Dword(v) => format!("DWORD:{v}"),
            RegValue::Qword(v) => format!("QWORD:{v}"),
            RegValue::Sz(v) => format!("SZ:{v}"),
            RegValue::Binary(b) => format!("BINARY:{}", b.len()),
            RegValue::ExpandSz(v) => format!("EXPAND_SZ:{v}"),
            RegValue::MultiSz(v) => format!("MULTI_SZ:{v:?}"),
            RegValue::Absent => "ABSENT".into(),
        }
    }
    pub fn reg_type(&self) -> Option<RegType> {
        match self {
            RegValue::Dword(_) => Some(RegType::Dword),
            RegValue::Qword(_) => Some(RegType::Qword),
            RegValue::Sz(_) => Some(RegType::Sz),
            RegValue::Binary(_) => Some(RegType::Binary),
            RegValue::ExpandSz(_) => Some(RegType::ExpandSz),
            RegValue::MultiSz(_) => Some(RegType::MultiSz),
            RegValue::Absent => None,
        }
    }
}

/// An AC / DC value pair (power settings have both).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcDc<T> {
    pub ac: T,
    pub dc: T,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PowerPlan {
    pub guid: String,
    pub name: String,
    pub active: bool,
}

/// One physical core and the logical processors it exposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreGroup {
    pub physical_id: u32,
    pub logical_ids: Vec<u32>,
    pub is_efficiency: bool,
    pub ccd: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuTopology {
    pub cores: Vec<CoreGroup>,
    pub smt_enabled: bool,
    pub group_count: u32,
}

impl CpuTopology {
    /// Logical processor ids belonging to physical core 0 (both SMT siblings
    /// when SMT is on; just one when it is off).
    pub fn physical_core0_logical_ids(&self) -> Vec<u32> {
        self.cores
            .iter()
            .find(|c| c.physical_id == 0)
            .map(|c| c.logical_ids.clone())
            .unwrap_or_default()
    }
    /// Every logical processor id, sorted.
    pub fn all_logical_ids(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self
            .cores
            .iter()
            .flat_map(|c| c.logical_ids.clone())
            .collect();
        v.sort_unstable();
        v
    }
}

/// Identifies the run + scenario + step a mutation belongs to; recorded in the
/// journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationCtx {
    pub run_id: String,
    pub scenario_id: String,
    pub step_index: u32,
}

/// Whether Steam is in the state pre-flight requires: running, and — if
/// running — not itself elevated (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §2.1a: an elevated Steam produces an
/// elevated CS2, which VOIDFRAME never wants and never causes on its own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteamStatus {
    pub running: bool,
    /// Meaningless when `running` is `false`.
    pub elevated: bool,
}

/// Everything `launch_cs2` needs. The URI itself carries no information
/// Steam actually reads (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §2.1a — confirmed inline args are ignored);
/// `app_id` is still a real parameter so the call site stays explicit about
/// what's being launched, and so a future non-CS2 use (unlikely, but the
/// type shouldn't hardcode 730) isn't precluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cs2LaunchSpec {
    pub app_id: u32,
}

/// A process this call discovered — enough to act on it later without a
/// second lookup racing a PID-reuse window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcHandle {
    pub pid: u32,
}

/// One point-in-time read of HWiNFO's Shared Memory Support sensors — at
/// minimum a CPU package/die temperature (always present, or the read
/// itself fails) and, when a GPU temperature reading was found, a GPU
/// temperature. Binary layout and the sensor labels these are matched by
/// are confirmed live against a real running HWiNFO instance; see
/// `system::windows::hwinfo`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HwinfoSensorSnapshot {
    pub cpu_temp_celsius: f64,
    pub gpu_temp_celsius: Option<f64>,
}

/// Facts read from Steam's `appmanifest_<app_id>.acf`: the installed
/// build (compared at every scenario boundary against the run-start value,
/// `docs/03-functional-spec.md` §3.1) and `StateFlags` (`4` = fully
/// installed; anything else = not installed or mid-update, a pre-flight
/// block). Either field is `None` when the manifest lacks it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppManifest {
    pub build_id: Option<String>,
    pub state_flags: Option<u32>,
}

#[async_trait]
pub trait SystemController: Send + Sync {
    // registry
    async fn read_registry(&self, k: &RegKey) -> Result<RegValue>;
    async fn write_registry(&self, k: &RegKey, v: &RegValue, c: &MutationCtx) -> Result<()>;
    async fn delete_registry_value(&self, k: &RegKey, c: &MutationCtx) -> Result<()>;

    // power settings
    async fn read_powercfg(&self, sub: &str, setting: &str) -> Result<AcDc<u32>>;
    async fn write_powercfg(&self, sub: &str, setting: &str, v: u32, c: &MutationCtx)
    -> Result<()>;

    // power plans
    async fn list_power_plans(&self) -> Result<Vec<PowerPlan>>;
    async fn active_power_plan(&self) -> Result<PowerPlan>;
    async fn set_active_power_plan(&self, guid: &str, c: &MutationCtx) -> Result<()>;
    async fn duplicate_power_plan(&self, template_guid: &str, c: &MutationCtx)
    -> Result<PowerPlan>;
    async fn delete_power_plan(&self, guid: &str, c: &MutationCtx) -> Result<()>;

    // cpu
    async fn cpu_topology(&self) -> Result<CpuTopology>;
    async fn set_process_affinity(&self, pid: u32, mask: u64) -> Result<()>;

    // gpu
    async fn gpu_model(&self) -> Result<String>;

    // process control (see docs/superpowers/plans/2026-09-01-m1-phase-3b-cs2-control-run-loop.md)
    async fn steam_status(&self) -> Result<SteamStatus>;
    async fn read_cs2_launch_options(&self) -> Result<String>;
    async fn write_cs2_launch_options(&self, args: &str) -> Result<()>;
    /// Steam client install dir. Windows: HKCU SteamPath or Program Files
    /// guesses; errors if `<dir>\steam.exe` does not exist.
    async fn steam_install_path(&self) -> Result<PathBuf>;
    /// Steam Library Folder that has `app_id` installed (often a different
    /// drive from the client).
    async fn app_library_path(&self, app_id: u32) -> Result<PathBuf>;
    /// `appmanifest_<app_id>.acf` facts; `Ok(None)` when the file does not
    /// exist (not installed).
    async fn app_manifest(&self, app_id: u32) -> Result<Option<AppManifest>>;
    /// `<steam>\steamapps\workshop\content\<app_id>\<item_id>\` exists.
    async fn workshop_item_installed(&self, app_id: u32, item_id: &str) -> Result<bool>;
    /// Free bytes on the drive hosting the data root.
    async fn free_disk_bytes(&self) -> Result<u64>;
    async fn launch_cs2(&self, spec: &Cs2LaunchSpec) -> Result<()>;
    /// Launches `program` with `args` de-elevated, by driving the
    /// already-running `explorer.exe` over COM
    /// (`IShellDispatch2.ShellExecute` via the `ExecInExplorer` technique —
    /// verified against `steam.exe`'s full process tree,
    /// `de-elevation-findings.md` §4-§6). Fire-and-forget: no child PID or
    /// exit code is available (explorer owns the launch); the caller polls
    /// `find_process` for the expected process by name. Fails if there is
    /// no interactive desktop / `explorer.exe` to drive. Spec §2.1a rev 6.
    async fn launch_deelevated(&self, program: &str, args: &str) -> Result<()>;
    async fn find_process(&self, name: &str) -> Result<Option<ProcHandle>>;
    async fn suspend_process_tree(&self, pid: u32) -> Result<()>;
    async fn resume_process_tree(&self, pid: u32) -> Result<()>;
    /// Terminates every process in `pid`'s tree (`pid` itself plus every
    /// descendant, transitively). Added alongside `run_scenario`
    /// (`docs/superpowers/plans/2026-09-01-m1-phase-3b-cs2-control-run-loop.md`):
    /// REVERT_MODULES and the watchdog's kill-and-relaunch retry
    /// (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.3 step 6) both need CS2
    /// actually terminated, and nothing on this trait provided that before
    /// (the earlier process-control subset was find/suspend/resume only).
    async fn kill_process_tree(&self, pid: u32) -> Result<()>;
    async fn reissue_map(&self, map_command: &str) -> Result<()>;
    /// Closes CS2's console (`Escape`, Source engine's standard dismiss
    /// key — not the `F9`/`toggleconsole` bind `reissue_map` uses to open
    /// it) — call this once the caller has confirmed (via a real
    /// `console.log` signal) that whatever `reissue_map` triggered has
    /// actually finished, not on a fixed delay. See `cs2::keybind_cfg`'s
    /// doc comment on `CFG_CONTENTS` for why closing was moved off that
    /// toggle keybind entirely.
    async fn hide_console(&self) -> Result<()>;
    /// Asks CS2 to quit via its own console `quit` command, rather than
    /// external termination. Best-effort: `Err` means the attempt itself
    /// couldn't be made (e.g. cs2.exe not running) — it does not mean CS2
    /// refused to quit. Callers needing to know CS2 actually exited must
    /// poll `find_process` afterward; see `run::execute::graceful_kill_cs2`
    /// for why this exists ahead of `kill_process_tree`.
    async fn quit_cs2_gracefully(&self) -> Result<()>;
    /// A process's creation time, as Windows itself reports it (`None` if
    /// the pid is already gone, or the read otherwise fails) — purely
    /// diagnostic, for telling "the same OS process, still alive" apart
    /// from "a different process that happened to reuse the same numeric
    /// pid." Not a `Result`: a caller logging this for the record treats
    /// "couldn't read it" the same as "don't know," never as fatal.
    async fn process_started_at(&self, pid: u32) -> Option<std::time::SystemTime>;
    /// Sends `WM_CLOSE` to Steam's main window (simulating the user
    /// clicking its window's X button) -- NOT `SW_MINIMIZE`, and NOT
    /// killing the process. Whether this hides Steam to the tray (leaving
    /// `steam.exe`/`steamwebhelper.exe` running) or fully exits the client
    /// depends on Steam's own "Close button minimizes Steam instead of
    /// exiting" setting -- callers that just launched Steam themselves
    /// should re-check `find_process` afterward rather than assume it held.
    /// A no-op, not an error, if Steam has no visible window right now
    /// (e.g. already minimized to tray).
    async fn close_steam_window(&self) -> Result<()>;
    /// Whether HWiNFO's own process is already running -- `find_process`
    /// against its confirmed real process name. Spec §A2: this decides,
    /// once per run, whether VOIDFRAME ever calls [`Self::start_hwinfo`] /
    /// [`Self::close_hwinfo`] at all ("only touch what we started" — the
    /// same precedent as `close_steam_window`).
    async fn hwinfo_already_running(&self) -> Result<bool>;
    /// Launches HWiNFO from `path`, plain (never de-elevated -- unlike
    /// Steam/CS2, HWiNFO has no integrity-level inheritance requirement to
    /// preserve). `path` is the caller's responsibility to supply (mirrors
    /// how `PresentMonController` takes its own exe path explicitly rather
    /// than guessing one internally) -- a future `hwinfo_path` Settings
    /// field only ever changes what value callers pass in here, never this
    /// method's own signature or body. Only ever called when
    /// [`Self::hwinfo_already_running`] just reported `false`.
    ///
    /// Does not return until HWiNFO's sensors are actually confirmed
    /// readable (up to an internal, implementation-defined timeout) -- a
    /// process merely having started is not sufficient on its own, since a
    /// cold-started HWiNFO's shared-memory section reliably isn't valid to
    /// read yet for a few seconds after launch. A timeout is a real `Err`,
    /// and on that `Err` the process this call just started is terminated
    /// before returning, so `Err` always means nothing was left running --
    /// every caller relies on this "sensors are readable right after this
    /// returns, or nothing is left running" contract as this method's
    /// whole point, rather than polling readiness themselves.
    async fn start_hwinfo(&self, path: &Path) -> Result<()>;
    /// Terminates HWiNFO's process tree -- a plain `kill_process_tree`, not
    /// a graceful window close (no anti-cheat-style termination-blocking
    /// concern here, and no "user was already looking at it" state to
    /// preserve, since this is only ever called on an instance VOIDFRAME
    /// itself started). A no-op, not an error, if HWiNFO isn't running.
    async fn close_hwinfo(&self) -> Result<()>;
    /// Reads HWiNFO's Shared Memory Support (a memory-mapped structure
    /// HWiNFO itself publishes once that setting is enabled in its own
    /// UI -- VOIDFRAME never enables it on the user's behalf). Binary
    /// layout and sensor-label text confirmed live against a real running
    /// HWiNFO instance; see `system::windows::hwinfo`.
    async fn read_hwinfo_sensors(&self) -> Result<HwinfoSensorSnapshot>;
}

// Lets an already-`Arc`'d controller -- most notably `Arc<dyn
// SystemController>`, which is what `AppState.sys` holds -- be wrapped by
// another `SystemController` decorator such as `DryRunController<C>`
// (whose `new` requires `C: SystemController`, not `Arc<dyn
// SystemController>` on its own). Every call just delegates through the
// `Arc`'s `Deref`, so this adds no behavior of its own.
#[async_trait]
impl<T: SystemController + ?Sized> SystemController for Arc<T> {
    async fn read_registry(&self, k: &RegKey) -> Result<RegValue> {
        (**self).read_registry(k).await
    }
    async fn write_registry(&self, k: &RegKey, v: &RegValue, c: &MutationCtx) -> Result<()> {
        (**self).write_registry(k, v, c).await
    }
    async fn delete_registry_value(&self, k: &RegKey, c: &MutationCtx) -> Result<()> {
        (**self).delete_registry_value(k, c).await
    }
    async fn read_powercfg(&self, sub: &str, setting: &str) -> Result<AcDc<u32>> {
        (**self).read_powercfg(sub, setting).await
    }
    async fn write_powercfg(
        &self,
        sub: &str,
        setting: &str,
        v: u32,
        c: &MutationCtx,
    ) -> Result<()> {
        (**self).write_powercfg(sub, setting, v, c).await
    }
    async fn list_power_plans(&self) -> Result<Vec<PowerPlan>> {
        (**self).list_power_plans().await
    }
    async fn active_power_plan(&self) -> Result<PowerPlan> {
        (**self).active_power_plan().await
    }
    async fn set_active_power_plan(&self, guid: &str, c: &MutationCtx) -> Result<()> {
        (**self).set_active_power_plan(guid, c).await
    }
    async fn duplicate_power_plan(
        &self,
        template_guid: &str,
        c: &MutationCtx,
    ) -> Result<PowerPlan> {
        (**self).duplicate_power_plan(template_guid, c).await
    }
    async fn delete_power_plan(&self, guid: &str, c: &MutationCtx) -> Result<()> {
        (**self).delete_power_plan(guid, c).await
    }
    async fn cpu_topology(&self) -> Result<CpuTopology> {
        (**self).cpu_topology().await
    }
    async fn set_process_affinity(&self, pid: u32, mask: u64) -> Result<()> {
        (**self).set_process_affinity(pid, mask).await
    }
    async fn gpu_model(&self) -> Result<String> {
        (**self).gpu_model().await
    }
    async fn steam_status(&self) -> Result<SteamStatus> {
        (**self).steam_status().await
    }
    async fn read_cs2_launch_options(&self) -> Result<String> {
        (**self).read_cs2_launch_options().await
    }
    async fn write_cs2_launch_options(&self, args: &str) -> Result<()> {
        (**self).write_cs2_launch_options(args).await
    }
    async fn steam_install_path(&self) -> Result<PathBuf> {
        (**self).steam_install_path().await
    }
    async fn app_library_path(&self, app_id: u32) -> Result<PathBuf> {
        (**self).app_library_path(app_id).await
    }
    async fn app_manifest(&self, app_id: u32) -> Result<Option<AppManifest>> {
        (**self).app_manifest(app_id).await
    }
    async fn workshop_item_installed(&self, app_id: u32, item_id: &str) -> Result<bool> {
        (**self).workshop_item_installed(app_id, item_id).await
    }
    async fn free_disk_bytes(&self) -> Result<u64> {
        (**self).free_disk_bytes().await
    }
    async fn launch_cs2(&self, spec: &Cs2LaunchSpec) -> Result<()> {
        (**self).launch_cs2(spec).await
    }
    async fn launch_deelevated(&self, program: &str, args: &str) -> Result<()> {
        (**self).launch_deelevated(program, args).await
    }
    async fn find_process(&self, name: &str) -> Result<Option<ProcHandle>> {
        (**self).find_process(name).await
    }
    async fn suspend_process_tree(&self, pid: u32) -> Result<()> {
        (**self).suspend_process_tree(pid).await
    }
    async fn resume_process_tree(&self, pid: u32) -> Result<()> {
        (**self).resume_process_tree(pid).await
    }
    async fn kill_process_tree(&self, pid: u32) -> Result<()> {
        (**self).kill_process_tree(pid).await
    }
    async fn reissue_map(&self, map_command: &str) -> Result<()> {
        (**self).reissue_map(map_command).await
    }
    async fn hide_console(&self) -> Result<()> {
        (**self).hide_console().await
    }
    async fn quit_cs2_gracefully(&self) -> Result<()> {
        (**self).quit_cs2_gracefully().await
    }
    async fn process_started_at(&self, pid: u32) -> Option<std::time::SystemTime> {
        (**self).process_started_at(pid).await
    }
    async fn close_steam_window(&self) -> Result<()> {
        (**self).close_steam_window().await
    }
    async fn hwinfo_already_running(&self) -> Result<bool> {
        (**self).hwinfo_already_running().await
    }
    async fn start_hwinfo(&self, path: &Path) -> Result<()> {
        (**self).start_hwinfo(path).await
    }
    async fn close_hwinfo(&self) -> Result<()> {
        (**self).close_hwinfo().await
    }
    async fn read_hwinfo_sensors(&self) -> Result<HwinfoSensorSnapshot> {
        (**self).read_hwinfo_sensors().await
    }
}

/// Picks the controller a run executes against: `real` as-is, or wrapped in
/// [`DryRunController`] (real reads, logged-but-never-applied writes).
pub fn select(dry_run: bool, real: Arc<dyn SystemController>) -> Arc<dyn SystemController> {
    if dry_run {
        Arc::new(DryRunController::new(real))
    } else {
        real
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regkey_and_regvalue_are_constructible() {
        let k = RegKey {
            hive: Hive::Hklm,
            subkey: "SYSTEM\\Foo".into(),
            value_name: "Bar".into(),
        };
        assert_eq!(k.value_name, "Bar");
        assert!(matches!(RegValue::Dword(2), RegValue::Dword(_)));
        assert!(matches!(RegValue::Absent, RegValue::Absent));
        assert_eq!(RegValue::Sz("x".into()).reg_type(), Some(RegType::Sz));
    }

    #[tokio::test]
    async fn select_dry_run_leaves_mock_state_unchanged_but_real_mutates() {
        // Regression test for C1: `start_run` used to hand `execute()` the
        // real `state.sys` regardless of `dry_run`, so a "Dry Run" checked
        // in the UI still performed real elevated registry/powercfg/
        // power-plan writes. `select` is the fix -- this proves its actual
        // behavior, not just that it compiles.
        let ctx = MutationCtx {
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
        };

        // dry_run: true -- a write through the selected controller must
        // never reach the underlying `MockController`.
        let dry_backing = MockController::new();
        let dry = select(true, Arc::new(dry_backing.clone()));
        let snapshot_before = dry_backing.snapshot();
        dry.write_powercfg("sub_processor", "IDLEDISABLE", 1, &ctx)
            .await
            .unwrap();
        assert_eq!(
            dry_backing.snapshot(),
            snapshot_before,
            "dry_run: true must not mutate the underlying system state"
        );

        // dry_run: false -- the exact same controller instance is
        // returned (not a copy), so the identical write really does land.
        let real_backing = MockController::new();
        let not_dry = select(false, Arc::new(real_backing.clone()));
        let snapshot_before_real = real_backing.snapshot();
        not_dry
            .write_powercfg("sub_processor", "IDLEDISABLE", 1, &ctx)
            .await
            .unwrap();
        assert_ne!(
            real_backing.snapshot(),
            snapshot_before_real,
            "dry_run: false must apply the write for real, exactly as before this fix"
        );
    }

    #[test]
    fn topology_helper_reports_physical_core0_siblings() {
        let topo = CpuTopology {
            smt_enabled: true,
            group_count: 1,
            cores: vec![
                CoreGroup {
                    physical_id: 0,
                    logical_ids: vec![0, 1],
                    is_efficiency: false,
                    ccd: Some(0),
                },
                CoreGroup {
                    physical_id: 1,
                    logical_ids: vec![2, 3],
                    is_efficiency: false,
                    ccd: Some(0),
                },
            ],
        };
        assert_eq!(topo.physical_core0_logical_ids(), vec![0, 1]);
        assert_eq!(topo.all_logical_ids(), vec![0, 1, 2, 3]);
    }
}
