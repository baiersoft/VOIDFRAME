//! In-memory, scriptable [`SystemController`] used by unit and integration
//! tests. Not exposed outside the crate for anything except testing.

use crate::error::{Error, Result};
use crate::system::*;
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// A point-in-time copy of a [`MockController`]'s state, for asserting a run
/// left the (simulated) system unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct MockSnapshot {
    pub registry: BTreeMap<String, RegValue>,
    pub powercfg: BTreeMap<String, AcDc<u32>>,
    pub active_plan: String,
}

#[derive(Debug)]
struct MockState {
    registry: BTreeMap<String, RegValue>,
    powercfg: BTreeMap<String, AcDc<u32>>,
    cs2_video_config_path: std::path::PathBuf,
    cs2_video_config: String,
    plans: Vec<PowerPlan>,
    /// When `Some(d)`, `duplicate_power_plan` sleeps `d` before it
    /// registers the new plan -- a real `.await` point inside that call,
    /// so a test can observe state that only holds *while* the call is in
    /// flight (`mutation::power_plan`'s `no_return` shield). Without it
    /// the whole call resolves synchronously on its first poll and no
    /// other task ever gets a chance to look. `None` (the default) is the
    /// instant behavior every other test relies on.
    duplicate_power_plan_delay: Option<std::time::Duration>,
    /// When `Some(d)`, `write_powercfg` sleeps `d` before it applies the
    /// value -- a real `.await` point inside the one call a powercfg
    /// journal record's *revert* makes, so a test can observe state that
    /// only holds *while* a reverse replay is in flight (`run::execute::
    /// scenario::revert_stage`'s `no_return` shield). Without it the whole
    /// replay resolves synchronously and no other task ever gets a chance
    /// to look. `None` (the default) is the instant behavior every other
    /// test relies on. Mirrors `duplicate_power_plan_delay`.
    write_powercfg_delay: Option<std::time::Duration>,
    /// Optional artificial delay inside every `read_registry` call -- the
    /// first await of `mutation::registry::apply`, before its journal
    /// record exists -- so a test can park a run between one module's
    /// confirmed apply and the next module's first journal line, the exact
    /// window an operator Abort dropping the run body must still recover
    /// from. `None` (the default) is the instant behavior every other test
    /// relies on. Mirrors `write_powercfg_delay`.
    read_registry_delay: Option<std::time::Duration>,
    topology: CpuTopology,
    gpu_model: String,
    fail_next_write: Option<String>,
    steam_status: SteamStatus,
    launch_options: String,
    /// How many times `write_cs2_launch_options` has been called so far --
    /// lets a test prove a launch-options write did NOT happen (e.g.
    /// ROLLBACK finding `live == desired` and skipping its own restore),
    /// which the final value alone can't distinguish from "wrote the same
    /// value again".
    write_launch_options_calls: u32,
    processes: BTreeMap<String, ProcHandle>, // name -> discovered handle
    suspended: std::collections::BTreeSet<u32>,
    send_console_command_calls: Vec<String>,
    hide_console_calls: u32,
    quit_cs2_calls: u32,
    /// When `true`, `quit_cs2_gracefully` also removes the tracked cs2.exe
    /// process (simulating a real graceful quit actually working) — lets a
    /// test exercise `graceful_kill_cs2`'s "no force-kill needed" path
    /// without waiting out its full poll timeout. Defaults to `false`
    /// (matches every test written before this existed: `quit_cs2_gracefully`
    /// is recorded but has no effect on `processes`, so `graceful_kill_cs2`
    /// falls through to its `kill_process_tree` fallback exactly as if this
    /// method didn't exist).
    quit_removes_process: bool,
    launch_cs2_calls: u32,
    /// When set, `launch_cs2` registers this (name, pid) into `processes`
    /// as part of the call — lets a test simulate "CS2 becomes discoverable
    /// once launched" without pre-registering it (which would make the
    /// pre-launch stale-process check in `run_scenario` see it as a
    /// leftover from a previous run and kill it before ever launching).
    spawn_on_launch: Option<(String, u32)>,
    killed_pids: Vec<u32>,
    /// `(name, pid, remaining_skips)` entries, one per independently-delayed
    /// process name: `find_process(name)` returns `None` for that entry's
    /// next `remaining_skips` calls, then `Some(pid)` forever after — lets a
    /// test deterministically exercise "a process becomes discoverable
    /// exactly N calls from now" without depending on real timing or a
    /// genuine race to land it. A `Vec` (not a single `Option`) so two
    /// different process names can each have their own independent delay in
    /// the same test (e.g. `steam.exe` immediate, `steamwebhelper.exe`
    /// delayed a few polls, for `ensure_steam_running`'s dual-condition
    /// wait).
    visible_after: Vec<(String, u32, u32)>,
    /// Every `find_process(name)` call so far, counted per name — lets a
    /// test prove a process was actually polled multiple times (genuine
    /// waiting), not just checked once.
    find_process_calls: BTreeMap<String, u32>,
    /// When `Some(msg)`, every `launch_deelevated` call returns
    /// `Err(Error::mock(msg))` — simulates "no interactive desktop" or a
    /// COM failure. Unlike `fail_next_write`'s one-shot semantics, this
    /// persists across calls: a test exercising `ensure_steam_running`'s
    /// fallback wants every attempt to keep failing, not succeed on a
    /// retry. Defaults to `None` (every other MockController
    /// process-control call defaults to success).
    deelevate_failure: Option<String>,
    deelevate_calls: u32,
    /// When set, a successful `launch_deelevated` call registers this
    /// (name, pid) into `processes` — mirrors `spawn_on_launch` for
    /// `launch_cs2`, letting a test simulate "steam.exe becomes
    /// discoverable once launch_deelevated succeeds" for
    /// `ensure_steam_running`'s polling.
    spawn_on_deelevate: Option<(String, u32)>,
    /// The (virtual, `tokio::time::pause`-aware) instant of the most recent
    /// `set_process_affinity` call, if any -- lets a test assert ordering
    /// against another timestamped event (e.g. a console.log line landing),
    /// not just that the call eventually happened.
    affinity_applied_at: Option<tokio::time::Instant>,
    close_steam_window_calls: u32,
    start_hwinfo_calls: u32,
    close_hwinfo_calls: u32,
    /// When `Some(msg)`, `start_hwinfo` returns `Err(Error::mock(msg))`
    /// instead of registering the process -- lets a test exercise a
    /// `start_hwinfo` failure (e.g. `wait_for_thermal_cooldown`'s
    /// fixed-time fallback on that path) without a real HWiNFO launch.
    /// `None` (the default) means `start_hwinfo` always succeeds.
    start_hwinfo_failure: Option<String>,
    /// When `Some(d)`, `start_hwinfo` sleeps `d` before it registers the
    /// process -- a real `.await` point inside that call, mirroring the real
    /// controller's spawn-plus-readiness-poll `spawn_blocking`, so a test can
    /// observe state that only holds *while* the start is in flight (the
    /// `hwinfo_started` flag being set before the await, not after). Without
    /// it the whole call resolves on its first poll and that window never
    /// exists. `None` (the default) is the instant behavior every other test
    /// relies on. Mirrors `duplicate_power_plan_delay`.
    start_hwinfo_delay: Option<std::time::Duration>,
    hwinfo_reading: HwinfoSensorSnapshot,
    /// A scripted sequence of successive `read_hwinfo_sensors` results,
    /// consumed front-to-back one per call -- lets a test assert on a real
    /// average across distinct readings (e.g. `collect_thermal_sample`)
    /// rather than the same fixed value every time. Empty by default: falls
    /// back to `hwinfo_reading` for every call, matching pre-existing
    /// behavior for every test written before this existed. Once exhausted,
    /// also falls back to `hwinfo_reading` rather than panicking -- no
    /// existing or brief-specified test calls `read_hwinfo_sensors` more
    /// times than it scripts, so this is a safety net, not a behavior
    /// depended on.
    hwinfo_readings: std::collections::VecDeque<HwinfoSensorSnapshot>,
    /// When `Some(n)`, `read_hwinfo_sensors` returns `Err` starting on its
    /// `(n + 1)`th call (1-indexed) -- lets a test exercise a sensor read
    /// failing partway through a sample loop (e.g.
    /// `collect_thermal_sample`'s cleanup-on-error path). Calls up to and
    /// including the `n`th still succeed via the normal
    /// `hwinfo_readings`/`hwinfo_reading` path. `None` (the default) means
    /// every call succeeds, matching every test written before this
    /// existed.
    hwinfo_read_fail_after: Option<u32>,
    hwinfo_read_calls: u32,
    /// When `true`, `close_steam_window` also removes `steam.exe` and
    /// `steamwebhelper.exe` from `processes` -- simulates Steam's "close
    /// button minimizes instead of exiting" setting NOT being enabled, so a
    /// window-close actually exits the client. Lets a test exercise
    /// `ensure_steam_running`'s post-close safety-net check. Defaults to
    /// `false` (every other test's `close_steam_window` is a pure no-op
    /// beyond the call counter, matching real behavior when that setting
    /// *is* enabled).
    close_exits_steam_client: bool,
    steam_install_path: std::path::PathBuf,
    app_manifest: Option<AppManifest>,
    workshop_item_installed: bool,
    free_disk_bytes: u64,
    reboot_calls: u32,
    shutdown_calls: u32,
    shutdown_failure: Option<String>,
    cancel_shutdown_calls: u32,
    reboot_failure: Option<String>,
    deregister_task_failure: Option<String>,
    tasks: Vec<TaskSpec>,
    boot_report: BootReport,
    bitlocker: BitlockerStatus,
    inhibit_sleep_calls: Vec<bool>,
    run_script_calls: Vec<std::path::PathBuf>,
    run_script_result: Option<Result<ScriptOutput>>,
}

fn key_str(k: &RegKey) -> String {
    format!("{}\\{}\\{}", k.hive.as_str(), k.subkey, k.value_name)
}
fn pc_str(sub: &str, setting: &str) -> String {
    format!("{sub}/{setting}")
}

fn default_topology() -> CpuTopology {
    let cores = (0..8u32)
        .map(|i| CoreGroup {
            physical_id: i,
            logical_ids: vec![i * 2, i * 2 + 1],
            is_efficiency: false,
            ccd: Some(0),
        })
        .collect();
    CpuTopology {
        cores,
        smt_enabled: true,
        group_count: 1,
    }
}

fn default_plans() -> Vec<PowerPlan> {
    vec![
        PowerPlan {
            guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
            name: "Balanced".into(),
            active: true,
        },
        PowerPlan {
            guid: "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c".into(),
            name: "High performance".into(),
            active: false,
        },
        PowerPlan {
            guid: "a1841308-3541-4fab-bc81-f71556f20b4a".into(),
            name: "Power saver".into(),
            active: false,
        },
    ]
}

#[derive(Clone)]
pub struct MockController(Arc<Mutex<MockState>>);

impl Default for MockController {
    fn default() -> Self {
        Self::new()
    }
}

impl MockController {
    pub fn new() -> MockController {
        MockController(Arc::new(Mutex::new(MockState {
            registry: BTreeMap::new(),
            powercfg: BTreeMap::new(),
            cs2_video_config_path: std::path::PathBuf::from(
                r"C:\Program Files (x86)\Steam\userdata\12345678\730\local\cfg\cs2_video.txt",
            ),
            cs2_video_config: String::new(),
            plans: default_plans(),
            duplicate_power_plan_delay: None,
            write_powercfg_delay: None,
            read_registry_delay: None,
            topology: default_topology(),
            gpu_model: "Mock GPU".to_string(),
            fail_next_write: None,
            steam_status: SteamStatus {
                running: true,
                elevated: false,
            },
            launch_options: String::new(),
            write_launch_options_calls: 0,
            processes: BTreeMap::new(),
            suspended: std::collections::BTreeSet::new(),
            send_console_command_calls: Vec::new(),
            hide_console_calls: 0,
            quit_cs2_calls: 0,
            quit_removes_process: false,
            launch_cs2_calls: 0,
            spawn_on_launch: None,
            killed_pids: Vec::new(),
            visible_after: Vec::new(),
            find_process_calls: BTreeMap::new(),
            deelevate_failure: None,
            deelevate_calls: 0,
            spawn_on_deelevate: None,
            affinity_applied_at: None,
            close_steam_window_calls: 0,
            close_exits_steam_client: false,
            start_hwinfo_calls: 0,
            close_hwinfo_calls: 0,
            start_hwinfo_failure: None,
            start_hwinfo_delay: None,
            hwinfo_reading: HwinfoSensorSnapshot {
                cpu_temp_celsius: 50.0,
                gpu_temp_celsius: None,
            },
            hwinfo_readings: std::collections::VecDeque::new(),
            hwinfo_read_fail_after: None,
            hwinfo_read_calls: 0,
            steam_install_path: std::path::PathBuf::from(r"C:\Program Files (x86)\Steam"),
            app_manifest: Some(AppManifest {
                build_id: Some("199".into()),
                state_flags: Some(4),
            }),
            workshop_item_installed: true,
            free_disk_bytes: 50_000_000_000,
            reboot_calls: 0,
            shutdown_calls: 0,
            shutdown_failure: None,
            cancel_shutdown_calls: 0,
            reboot_failure: None,
            deregister_task_failure: None,
            tasks: Vec::new(),
            boot_report: BootReport::default(),
            bitlocker: BitlockerStatus::Off,
            inhibit_sleep_calls: Vec::new(),
            run_script_calls: Vec::new(),
            run_script_result: None,
        })))
    }

    pub fn with_registry(self, k: &RegKey, v: RegValue) -> Self {
        self.0.lock().unwrap().registry.insert(key_str(k), v);
        self
    }
    pub fn with_powercfg(self, sub: &str, setting: &str, v: AcDc<u32>) -> Self {
        self.0
            .lock()
            .unwrap()
            .powercfg
            .insert(pc_str(sub, setting), v);
        self
    }
    pub fn with_cs2_video_config(self, text: &str) -> Self {
        self.0.lock().unwrap().cs2_video_config = text.to_string();
        self
    }
    pub fn with_cs2_video_config_path(self, path: &str) -> Self {
        self.0.lock().unwrap().cs2_video_config_path = std::path::PathBuf::from(path);
        self
    }
    pub fn with_run_script_result(self, r: Result<ScriptOutput>) -> Self {
        self.0.lock().unwrap().run_script_result = Some(r);
        self
    }
    pub fn run_script_calls(&self) -> Vec<std::path::PathBuf> {
        self.0.lock().unwrap().run_script_calls.clone()
    }
    pub fn with_topology(self, t: CpuTopology) -> Self {
        self.0.lock().unwrap().topology = t;
        self
    }
    /// See `MockState::duplicate_power_plan_delay`.
    pub fn with_duplicate_power_plan_delay(self, d: std::time::Duration) -> Self {
        self.0.lock().unwrap().duplicate_power_plan_delay = Some(d);
        self
    }
    /// See `MockState::write_powercfg_delay`.
    pub fn with_write_powercfg_delay(self, d: std::time::Duration) -> Self {
        self.0.lock().unwrap().write_powercfg_delay = Some(d);
        self
    }
    /// See `MockState::read_registry_delay`.
    pub fn with_read_registry_delay(self, d: std::time::Duration) -> Self {
        self.0.lock().unwrap().read_registry_delay = Some(d);
        self
    }
    pub fn with_power_plans(self, p: Vec<PowerPlan>) -> Self {
        self.0.lock().unwrap().plans = p;
        self
    }
    pub fn with_gpu_model(self, name: &str) -> Self {
        self.0.lock().unwrap().gpu_model = name.to_string();
        self
    }
    pub fn with_steam_status(self, s: SteamStatus) -> Self {
        self.0.lock().unwrap().steam_status = s;
        self
    }
    pub fn with_process(self, name: &str, pid: u32) -> Self {
        self.0
            .lock()
            .unwrap()
            .processes
            .insert(name.into(), ProcHandle { pid });
        self
    }
    /// `launch_cs2` registers `(name, pid)` as discoverable the moment it's
    /// called, rather than immediately (see the `spawn_on_launch` field's
    /// doc comment). Distinct from [`Self::with_process`], which registers
    /// it right away.
    pub fn with_process_on_launch(self, name: &str, pid: u32) -> Self {
        self.0.lock().unwrap().spawn_on_launch = Some((name.to_string(), pid));
        self
    }
    pub fn with_launch_options(self, args: &str) -> Self {
        self.0.lock().unwrap().launch_options = args.to_string();
        self
    }
    /// How many times `write_cs2_launch_options` has been called so far.
    pub fn write_launch_options_calls(&self) -> u32 {
        self.0.lock().unwrap().write_launch_options_calls
    }
    /// Makes `quit_cs2_gracefully` also remove the tracked cs2.exe process,
    /// simulating a real graceful quit actually working — lets a test
    /// exercise `graceful_kill_cs2`'s "no force-kill fallback needed" path.
    pub fn with_graceful_quit_working(self) -> Self {
        self.0.lock().unwrap().quit_removes_process = true;
        self
    }
    /// `find_process(name)` returns `None` for the first `skip` calls, then
    /// `Some(ProcHandle{pid})` on every call after that. Can be called
    /// multiple times with different `name`s for independently-delayed
    /// processes in the same test.
    pub fn with_process_visible_after(self, name: &str, pid: u32, skip: u32) -> Self {
        self.0
            .lock()
            .unwrap()
            .visible_after
            .push((name.to_string(), pid, skip));
        self
    }
    /// Makes `close_steam_window` also remove `steam.exe`/`steamwebhelper.exe`
    /// from the tracked processes -- simulates Steam's "close button
    /// minimizes instead of exiting" setting not being enabled.
    pub fn with_close_exits_steam_client(self) -> Self {
        self.0.lock().unwrap().close_exits_steam_client = true;
        self
    }
    /// Scripts the value `read_hwinfo_sensors` returns -- defaults to a
    /// fixed sane value (50.0°C, no GPU reading) if never called.
    pub fn with_hwinfo_reading(self, cpu_temp_celsius: f64, gpu_temp_celsius: Option<f64>) -> Self {
        self.0.lock().unwrap().hwinfo_reading = HwinfoSensorSnapshot {
            cpu_temp_celsius,
            gpu_temp_celsius,
        };
        self
    }
    /// Scripts a sequence of `read_hwinfo_sensors` results, one consumed per
    /// call in order -- for a test that needs successive distinct readings
    /// (e.g. averaging across samples), not the same fixed value repeated.
    /// Falls back to [`Self::with_hwinfo_reading`]'s fixed value once the
    /// sequence is exhausted.
    pub fn with_hwinfo_readings(self, readings: Vec<(f64, Option<f64>)>) -> Self {
        self.0.lock().unwrap().hwinfo_readings = readings
            .into_iter()
            .map(
                |(cpu_temp_celsius, gpu_temp_celsius)| HwinfoSensorSnapshot {
                    cpu_temp_celsius,
                    gpu_temp_celsius,
                },
            )
            .collect();
        self
    }
    /// Makes `read_hwinfo_sensors` return `Err` starting on its
    /// `(fail_after + 1)`th call (1-indexed) -- lets a test exercise a
    /// sensor read failing partway through a sample loop.
    pub fn with_hwinfo_read_failing_after(self, fail_after: u32) -> Self {
        self.0.lock().unwrap().hwinfo_read_fail_after = Some(fail_after);
        self
    }
    /// Makes `start_hwinfo` return `Err(Error::mock(msg))` on every call --
    /// lets a test exercise a failed HWiNFO launch without a real process.
    pub fn with_start_hwinfo_failing(self, msg: &str) -> Self {
        self.0.lock().unwrap().start_hwinfo_failure = Some(msg.to_string());
        self
    }
    /// See `MockState::start_hwinfo_delay`.
    pub fn with_start_hwinfo_delay(self, d: std::time::Duration) -> Self {
        self.0.lock().unwrap().start_hwinfo_delay = Some(d);
        self
    }
    /// The next mutation call (`write_registry` / `write_powercfg` /
    /// `set_active_power_plan`) returns `Error::mock(msg)` once, then clears.
    pub fn fail_next_write(&self, msg: &str) {
        self.0.lock().unwrap().fail_next_write = Some(msg.to_string());
    }
    fn take_fail(&self) -> Option<String> {
        self.0.lock().unwrap().fail_next_write.take()
    }
    pub fn snapshot(&self) -> MockSnapshot {
        let s = self.0.lock().unwrap();
        MockSnapshot {
            registry: s.registry.clone(),
            powercfg: s.powercfg.clone(),
            active_plan: s
                .plans
                .iter()
                .find(|p| p.active)
                .map(|p| p.guid.clone())
                .unwrap_or_default(),
        }
    }
    /// Every `send_console_command` call so far, in call order.
    pub fn send_console_command_calls(&self) -> Vec<String> {
        self.0.lock().unwrap().send_console_command_calls.clone()
    }
    /// How many times `hide_console` has been called so far.
    pub fn hide_console_calls(&self) -> u32 {
        self.0.lock().unwrap().hide_console_calls
    }
    /// How many times `quit_cs2_gracefully` has been called so far.
    pub fn quit_cs2_calls(&self) -> u32 {
        self.0.lock().unwrap().quit_cs2_calls
    }
    pub fn launch_cs2_calls(&self) -> u32 {
        self.0.lock().unwrap().launch_cs2_calls
    }
    /// Makes `launch_deelevated` return `Err(Error::mock(msg))` on every
    /// call — simulates "no interactive desktop" / COM failure.
    pub fn with_deelevate_failing(self, msg: &str) -> Self {
        self.0.lock().unwrap().deelevate_failure = Some(msg.to_string());
        self
    }
    /// `launch_deelevated` registers `(name, pid)` as discoverable once it
    /// succeeds — mirrors [`Self::with_process_on_launch`] for `launch_cs2`.
    pub fn with_process_on_deelevate(self, name: &str, pid: u32) -> Self {
        self.0.lock().unwrap().spawn_on_deelevate = Some((name.to_string(), pid));
        self
    }
    /// The (virtual) instant `set_process_affinity` was last called, if at
    /// all -- `None` if it was never called.
    pub fn affinity_applied_at(&self) -> Option<tokio::time::Instant> {
        self.0.lock().unwrap().affinity_applied_at
    }
    /// How many times `launch_deelevated` has been called so far.
    pub fn deelevate_calls(&self) -> u32 {
        self.0.lock().unwrap().deelevate_calls
    }
    /// Every pid passed to `kill_process_tree` so far, in call order.
    pub fn killed_pids(&self) -> Vec<u32> {
        self.0.lock().unwrap().killed_pids.clone()
    }
    /// Every pid currently suspended (i.e. `suspend_process_tree` called
    /// and not yet followed by a matching `resume_process_tree`).
    pub fn suspended_pids(&self) -> Vec<u32> {
        self.0.lock().unwrap().suspended.iter().copied().collect()
    }
    /// How many times `find_process(name)` has been called so far, for this
    /// specific `name` -- lets a test prove a process was actually polled
    /// repeatedly (genuine waiting), not just checked once.
    pub fn find_process_calls(&self, name: &str) -> u32 {
        self.0
            .lock()
            .unwrap()
            .find_process_calls
            .get(name)
            .copied()
            .unwrap_or(0)
    }
    /// How many times `close_steam_window` has been called so far.
    pub fn close_steam_window_calls(&self) -> u32 {
        self.0.lock().unwrap().close_steam_window_calls
    }
    /// How many times `start_hwinfo` has been called so far.
    pub fn start_hwinfo_calls(&self) -> u32 {
        self.0.lock().unwrap().start_hwinfo_calls
    }
    /// How many times `close_hwinfo` has been called so far.
    pub fn close_hwinfo_calls(&self) -> u32 {
        self.0.lock().unwrap().close_hwinfo_calls
    }
    /// How many times `read_hwinfo_sensors` has been called so far -- lets a
    /// test prove a sample loop is genuinely partway through its
    /// `sample_count` reads (not just started or finished).
    pub fn hwinfo_read_calls(&self) -> u32 {
        self.0.lock().unwrap().hwinfo_read_calls
    }
    pub fn with_steam_install_path(self, path: &str) -> Self {
        self.0.lock().unwrap().steam_install_path = std::path::PathBuf::from(path);
        self
    }
    pub fn with_app_manifest(self, manifest: Option<AppManifest>) -> Self {
        self.0.lock().unwrap().app_manifest = manifest;
        self
    }
    pub fn with_workshop_item_installed(self, installed: bool) -> Self {
        self.0.lock().unwrap().workshop_item_installed = installed;
        self
    }
    pub fn with_free_disk_bytes(self, bytes: u64) -> Self {
        self.0.lock().unwrap().free_disk_bytes = bytes;
        self
    }
    /// The next `reboot` call returns `Error::mock(msg)`.
    pub fn with_reboot_failing(self, msg: &str) -> Self {
        self.0.lock().unwrap().reboot_failure = Some(msg.to_string());
        self
    }
    pub fn reboot_calls(&self) -> u32 {
        self.0.lock().unwrap().reboot_calls
    }
    /// Every `deregister_task` call returns `Error::mock(msg)`, persisting
    /// across calls (mirrors `with_reboot_failing`'s one-field, always-on
    /// semantics rather than `fail_next_write`'s one-shot behavior).
    pub fn with_deregister_task_failing(self, msg: &str) -> Self {
        self.0.lock().unwrap().deregister_task_failure = Some(msg.to_string());
        self
    }
    pub fn shutdown_calls(&self) -> u32 {
        self.0.lock().unwrap().shutdown_calls
    }
    /// Every `shutdown` call returns `Error::mock(msg)` (mirrors
    /// `with_reboot_failing`).
    pub fn with_shutdown_failing(self, msg: &str) -> Self {
        self.0.lock().unwrap().shutdown_failure = Some(msg.to_string());
        self
    }
    pub fn cancel_shutdown_calls(&self) -> u32 {
        self.0.lock().unwrap().cancel_shutdown_calls
    }
    /// Every scheduled task currently registered.
    pub fn registered_tasks(&self) -> Vec<TaskSpec> {
        self.0.lock().unwrap().tasks.clone()
    }
    pub fn with_boot_report(self, r: BootReport) -> Self {
        self.0.lock().unwrap().boot_report = r;
        self
    }
    pub fn with_bitlocker(self, s: BitlockerStatus) -> Self {
        self.0.lock().unwrap().bitlocker = s;
        self
    }
    /// Every `on` value passed to `inhibit_sleep` so far, in call order.
    pub fn inhibit_sleep_calls(&self) -> Vec<bool> {
        self.0.lock().unwrap().inhibit_sleep_calls.clone()
    }
}

#[async_trait]
impl SystemController for MockController {
    async fn read_registry(&self, k: &RegKey) -> Result<RegValue> {
        // Read and release the lock before any `.await` -- a `std::sync`
        // guard must never be held across one.
        let delay = self.0.lock().unwrap().read_registry_delay;
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        Ok(self
            .0
            .lock()
            .unwrap()
            .registry
            .get(&key_str(k))
            .cloned()
            .unwrap_or(RegValue::Absent))
    }
    async fn write_registry(&self, k: &RegKey, v: &RegValue, _c: &MutationCtx) -> Result<()> {
        if let Some(m) = self.take_fail() {
            return Err(Error::mock(m));
        }
        self.0
            .lock()
            .unwrap()
            .registry
            .insert(key_str(k), v.clone());
        Ok(())
    }
    async fn delete_registry_value(&self, k: &RegKey, _c: &MutationCtx) -> Result<()> {
        self.0.lock().unwrap().registry.remove(&key_str(k));
        Ok(())
    }
    async fn read_powercfg(&self, sub: &str, setting: &str) -> Result<AcDc<u32>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .powercfg
            .get(&pc_str(sub, setting))
            .copied()
            .unwrap_or(AcDc { ac: 0, dc: 0 }))
    }
    async fn write_powercfg(
        &self,
        sub: &str,
        setting: &str,
        v: u32,
        _c: &MutationCtx,
    ) -> Result<()> {
        if let Some(m) = self.take_fail() {
            return Err(Error::mock(m));
        }
        // Read and release the lock before any `.await` -- a `std::sync`
        // guard must never be held across one.
        let delay = self.0.lock().unwrap().write_powercfg_delay;
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        self.0
            .lock()
            .unwrap()
            .powercfg
            .insert(pc_str(sub, setting), AcDc { ac: v, dc: v });
        Ok(())
    }
    async fn find_cs2_video_config_path(&self) -> Result<std::path::PathBuf> {
        Ok(self.0.lock().unwrap().cs2_video_config_path.clone())
    }
    async fn read_cs2_video_config(&self) -> Result<String> {
        Ok(self.0.lock().unwrap().cs2_video_config.clone())
    }
    async fn write_cs2_video_config(&self, text: &str) -> Result<()> {
        self.0.lock().unwrap().cs2_video_config = text.to_string();
        Ok(())
    }
    async fn run_script(
        &self,
        path: &std::path::Path,
        _timeout: std::time::Duration,
    ) -> Result<ScriptOutput> {
        let mut state = self.0.lock().unwrap();
        state.run_script_calls.push(path.to_path_buf());
        if let Some(scripted) = state.run_script_result.take() {
            return scripted;
        }
        Ok(ScriptOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
    async fn list_power_plans(&self) -> Result<Vec<PowerPlan>> {
        Ok(self.0.lock().unwrap().plans.clone())
    }
    async fn active_power_plan(&self) -> Result<PowerPlan> {
        self.0
            .lock()
            .unwrap()
            .plans
            .iter()
            .find(|p| p.active)
            .cloned()
            .ok_or_else(|| Error::mock("no active plan".into()))
    }
    async fn set_active_power_plan(&self, guid: &str, _c: &MutationCtx) -> Result<()> {
        if let Some(m) = self.take_fail() {
            return Err(Error::mock(m));
        }
        let mut s = self.0.lock().unwrap();
        if !s.plans.iter().any(|p| p.guid == guid) {
            return Err(Error::mock(format!("no such plan {guid}")));
        }
        for p in &mut s.plans {
            p.active = p.guid == guid;
        }
        Ok(())
    }
    async fn duplicate_power_plan(
        &self,
        template_guid: &str,
        _c: &MutationCtx,
    ) -> Result<PowerPlan> {
        // Read and release the lock before any `.await` -- a `std::sync`
        // guard must never be held across one.
        let delay = self.0.lock().unwrap().duplicate_power_plan_delay;
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        let mut s = self.0.lock().unwrap();
        let name = s
            .plans
            .iter()
            .find(|p| p.guid == template_guid)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "Copy".into());
        let new = PowerPlan {
            guid: format!("dup-{template_guid}"),
            name: format!("{name} (copy)"),
            active: false,
        };
        s.plans.push(new.clone());
        Ok(new)
    }
    async fn delete_power_plan(&self, guid: &str, _c: &MutationCtx) -> Result<()> {
        self.0.lock().unwrap().plans.retain(|p| p.guid != guid);
        Ok(())
    }
    async fn cpu_topology(&self) -> Result<CpuTopology> {
        Ok(self.0.lock().unwrap().topology.clone())
    }
    async fn set_process_affinity(&self, _pid: u32, _mask: u64) -> Result<()> {
        self.0.lock().unwrap().affinity_applied_at = Some(tokio::time::Instant::now());
        Ok(())
    }
    async fn gpu_model(&self) -> Result<String> {
        Ok(self.0.lock().unwrap().gpu_model.clone())
    }
    async fn steam_status(&self) -> Result<SteamStatus> {
        Ok(self.0.lock().unwrap().steam_status)
    }
    async fn read_cs2_launch_options(&self) -> Result<String> {
        Ok(self.0.lock().unwrap().launch_options.clone())
    }
    async fn write_cs2_launch_options(&self, args: &str) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        s.launch_options = args.to_string();
        s.write_launch_options_calls += 1;
        Ok(())
    }
    async fn launch_cs2(&self, _spec: &Cs2LaunchSpec) -> Result<()> {
        if let Some(m) = self.take_fail() {
            return Err(Error::mock(m));
        }
        let mut s = self.0.lock().unwrap();
        s.launch_cs2_calls += 1;
        if let Some((name, pid)) = s.spawn_on_launch.clone() {
            s.processes.insert(name, ProcHandle { pid });
        }
        Ok(())
    }
    async fn launch_deelevated(&self, _program: &str, _args: &str) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        s.deelevate_calls += 1;
        if let Some(msg) = s.deelevate_failure.clone() {
            return Err(Error::mock(msg));
        }
        if let Some((name, pid)) = s.spawn_on_deelevate.clone() {
            // Symmetric with `kill_process_tree`'s own `steam_status.running
            // = false` flip above: both derive from the same live snapshot
            // in the real `WindowsController`, so a scripted successful
            // relaunch of steam.exe must flip `running` back to `true` here
            // too -- otherwise `find_process("steam.exe")` would succeed
            // while `steam_status().running` still incorrectly reported
            // `false`, a state the real controller can never produce.
            if name == "steam.exe" {
                s.steam_status.running = true;
            }
            s.processes.insert(name, ProcHandle { pid });
        }
        Ok(())
    }
    async fn find_process(&self, name: &str) -> Result<Option<ProcHandle>> {
        let mut s = self.0.lock().unwrap();
        *s.find_process_calls.entry(name.to_string()).or_insert(0) += 1;
        if let Some(idx) = s.visible_after.iter().position(|(n, _, _)| n == name) {
            let (n, pid, skip) = s.visible_after[idx].clone();
            if skip > 0 {
                s.visible_after[idx] = (n, pid, skip - 1);
                return Ok(None);
            }
            return Ok(Some(ProcHandle { pid }));
        }
        Ok(s.processes.get(name).copied())
    }
    async fn suspend_process_tree(&self, pid: u32) -> Result<()> {
        self.0.lock().unwrap().suspended.insert(pid);
        Ok(())
    }
    async fn resume_process_tree(&self, pid: u32) -> Result<()> {
        self.0.lock().unwrap().suspended.remove(&pid);
        Ok(())
    }
    async fn kill_process_tree(&self, pid: u32) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        // A killed process is no longer discoverable — mirrors real
        // behavior, and matters for `run_scenario`'s own stale-process
        // check on the *next* scenario, which relies on `find_process`
        // genuinely reflecting "is this still running".
        // Killing the tracked steam.exe process also flips `steam_status`
        // to not-running -- mirrors real behavior and matters for
        // `wait_until_steam_closed`'s poll in the launch_args-differ
        // automated close/edit/relaunch flow, which relies on
        // `steam_status` genuinely reflecting "is Steam still running".
        if s.processes.get("steam.exe").map(|h| h.pid) == Some(pid) {
            s.steam_status.running = false;
        }
        s.processes.retain(|_, h| h.pid != pid);
        s.killed_pids.push(pid);
        Ok(())
    }
    async fn send_console_command(&self, command: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap()
            .send_console_command_calls
            .push(command.to_string());
        Ok(())
    }
    async fn hide_console(&self) -> Result<()> {
        self.0.lock().unwrap().hide_console_calls += 1;
        Ok(())
    }
    async fn quit_cs2_gracefully(&self) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        s.quit_cs2_calls += 1;
        if s.quit_removes_process {
            s.processes.remove("cs2.exe");
        }
        Ok(())
    }
    async fn process_started_at(&self, _pid: u32) -> Option<std::time::SystemTime> {
        // A real Windows concept (GetProcessTimes) with nothing to fake
        // meaningfully in-memory — no test relies on this today.
        None
    }
    async fn close_steam_window(&self) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        s.close_steam_window_calls += 1;
        if s.close_exits_steam_client {
            s.processes.remove("steam.exe");
            s.processes.remove("steamwebhelper.exe");
            s.steam_status.running = false;
        }
        Ok(())
    }
    async fn hwinfo_already_running(&self) -> Result<bool> {
        Ok(self.find_process("HWiNFO64.exe").await?.is_some())
    }
    async fn start_hwinfo(&self, _path: &std::path::Path) -> Result<()> {
        // Counted, then the lock released before any `.await` -- a
        // `std::sync` guard must never be held across one.
        let delay = {
            let mut s = self.0.lock().unwrap();
            s.start_hwinfo_calls += 1;
            s.start_hwinfo_delay
        };
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        let mut s = self.0.lock().unwrap();
        if let Some(msg) = s.start_hwinfo_failure.clone() {
            return Err(Error::mock(msg));
        }
        // Registers the process, mirroring the real contract (`start_hwinfo`
        // never returns `Ok` unless sensors are actually confirmed readable,
        // which real HWiNFO can only be while running) and `close_hwinfo`'s
        // own already-correct lookup-by-name below -- otherwise a
        // mock-driven test could never observe `hwinfo_already_running() ==
        // true` right after a successful `start_hwinfo()` call. A fixed
        // synthetic pid not used by any other test in this file.
        s.processes
            .insert("HWiNFO64.exe".to_string(), ProcHandle { pid: 42_424_242 });
        Ok(())
    }
    async fn close_hwinfo(&self) -> Result<()> {
        self.0.lock().unwrap().close_hwinfo_calls += 1;
        if let Some(handle) = self.find_process("HWiNFO64.exe").await? {
            self.kill_process_tree(handle.pid).await?;
        }
        Ok(())
    }
    async fn read_hwinfo_sensors(&self) -> Result<HwinfoSensorSnapshot> {
        let mut s = self.0.lock().unwrap();
        s.hwinfo_read_calls += 1;
        if let Some(fail_after) = s.hwinfo_read_fail_after
            && s.hwinfo_read_calls > fail_after
        {
            return Err(Error::mock("hwinfo sensor read failed".to_string()));
        }
        Ok(s.hwinfo_readings.pop_front().unwrap_or(s.hwinfo_reading))
    }
    async fn steam_install_path(&self) -> Result<std::path::PathBuf> {
        Ok(self.0.lock().unwrap().steam_install_path.clone())
    }
    async fn app_library_path(&self, _app_id: u32) -> Result<std::path::PathBuf> {
        Ok(self.0.lock().unwrap().steam_install_path.clone())
    }
    async fn app_manifest(&self, _app_id: u32) -> Result<Option<AppManifest>> {
        Ok(self.0.lock().unwrap().app_manifest.clone())
    }
    async fn workshop_item_installed(&self, _app_id: u32, _item_id: &str) -> Result<bool> {
        Ok(self.0.lock().unwrap().workshop_item_installed)
    }
    async fn free_disk_bytes(&self) -> Result<u64> {
        Ok(self.0.lock().unwrap().free_disk_bytes)
    }
    async fn reboot(&self, _delay_secs: u32, _message: &str) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        s.reboot_calls += 1;
        if let Some(msg) = s.reboot_failure.clone() {
            return Err(Error::mock(msg));
        }
        Ok(())
    }
    async fn shutdown(&self, _delay_secs: u32, _message: &str) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        s.shutdown_calls += 1;
        if let Some(msg) = s.shutdown_failure.clone() {
            return Err(Error::mock(msg));
        }
        Ok(())
    }
    async fn cancel_shutdown(&self) -> Result<()> {
        self.0.lock().unwrap().cancel_shutdown_calls += 1;
        Ok(())
    }
    async fn register_task(&self, spec: &TaskSpec) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        s.tasks.retain(|t| t.name != spec.name);
        s.tasks.push(spec.clone());
        Ok(())
    }
    async fn deregister_task(&self, name: &str) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        if let Some(msg) = s.deregister_task_failure.clone() {
            return Err(Error::mock(msg));
        }
        s.tasks.retain(|t| t.name != name);
        Ok(())
    }
    async fn task_exists(&self, name: &str) -> Result<bool> {
        Ok(self.0.lock().unwrap().tasks.iter().any(|t| t.name == name))
    }
    async fn boot_report(&self, _since: std::time::SystemTime) -> Result<BootReport> {
        Ok(self.0.lock().unwrap().boot_report.clone())
    }
    async fn bitlocker_protection(&self) -> Result<BitlockerStatus> {
        Ok(self.0.lock().unwrap().bitlocker)
    }
    async fn inhibit_sleep(&self, on: bool) -> Result<()> {
        self.0.lock().unwrap().inhibit_sleep_calls.push(on);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::module::Hive;
    use std::time::Duration;

    fn ctx() -> MutationCtx {
        MutationCtx {
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
        }
    }
    fn key() -> RegKey {
        RegKey {
            hive: Hive::Hklm,
            subkey: "SYSTEM\\X".into(),
            value_name: "V".into(),
        }
    }

    #[tokio::test]
    async fn absent_value_reads_as_absent_then_written_then_deleted() {
        let m = MockController::new();
        assert_eq!(m.read_registry(&key()).await.unwrap(), RegValue::Absent);
        m.write_registry(&key(), &RegValue::Dword(2), &ctx())
            .await
            .unwrap();
        assert_eq!(m.read_registry(&key()).await.unwrap(), RegValue::Dword(2));
        m.delete_registry_value(&key(), &ctx()).await.unwrap();
        assert_eq!(m.read_registry(&key()).await.unwrap(), RegValue::Absent);
    }

    #[tokio::test]
    async fn fail_next_write_triggers_once() {
        let m = MockController::new();
        m.fail_next_write("boom");
        let e = m
            .write_registry(&key(), &RegValue::Dword(1), &ctx())
            .await
            .unwrap_err();
        assert!(e.to_string().contains("boom"));
        m.write_registry(&key(), &RegValue::Dword(1), &ctx())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn power_plan_activate_changes_active() {
        let m = MockController::new();
        let plans = m.list_power_plans().await.unwrap();
        let other = plans.iter().find(|p| !p.active).unwrap().guid.clone();
        m.set_active_power_plan(&other, &ctx()).await.unwrap();
        assert_eq!(m.active_power_plan().await.unwrap().guid, other);
    }

    #[tokio::test]
    async fn snapshot_is_stable_across_write_then_revert() {
        // Seed a starting value first: on a real machine a powercfg setting
        // always has *some* current value, so this mirrors real usage (read
        // current -> journal it as the inverse -> write -> revert to it) —
        // unlike the registry, "absent" isn't a state powercfg settings have.
        let m = MockController::new().with_powercfg(
            "sub_processor",
            "IDLEDISABLE",
            AcDc { ac: 0, dc: 0 },
        );
        let before = m.snapshot();
        m.write_powercfg("sub_processor", "IDLEDISABLE", 1, &ctx())
            .await
            .unwrap();
        m.write_powercfg("sub_processor", "IDLEDISABLE", 0, &ctx())
            .await
            .unwrap();
        assert_eq!(before, m.snapshot());
    }

    #[tokio::test]
    async fn steam_status_reads_back_configured_value() {
        let m = MockController::new().with_steam_status(SteamStatus {
            running: true,
            elevated: true,
        });
        assert_eq!(
            m.steam_status().await.unwrap(),
            SteamStatus {
                running: true,
                elevated: true,
            }
        );
    }

    #[tokio::test]
    async fn launch_options_round_trip() {
        let m = MockController::new();
        assert_eq!(m.read_cs2_launch_options().await.unwrap(), "");
        m.write_cs2_launch_options("-novid -tickrate 128")
            .await
            .unwrap();
        assert_eq!(
            m.read_cs2_launch_options().await.unwrap(),
            "-novid -tickrate 128"
        );
    }

    #[tokio::test]
    async fn read_cs2_video_config_returns_the_seeded_text() {
        let m = MockController::new().with_cs2_video_config("setting.defaultres 1920\n");
        assert_eq!(
            m.read_cs2_video_config().await.unwrap(),
            "setting.defaultres 1920\n"
        );
    }

    #[tokio::test]
    async fn find_cs2_video_config_path_returns_the_seeded_path() {
        let m = MockController::new()
            .with_cs2_video_config_path(r"D:\Steam\userdata\1\730\local\cfg\cs2_video.txt");
        assert_eq!(
            m.find_cs2_video_config_path().await.unwrap(),
            std::path::PathBuf::from(r"D:\Steam\userdata\1\730\local\cfg\cs2_video.txt")
        );
    }

    #[tokio::test]
    async fn find_cs2_video_config_path_has_a_sensible_default() {
        let m = MockController::new();
        let path = m.find_cs2_video_config_path().await.unwrap();
        assert!(path.to_string_lossy().ends_with("cs2_video.txt"));
    }

    #[tokio::test]
    async fn write_cs2_video_config_updates_what_read_returns() {
        let m = MockController::new().with_cs2_video_config("old\n");
        m.write_cs2_video_config("new\n").await.unwrap();
        assert_eq!(m.read_cs2_video_config().await.unwrap(), "new\n");
    }

    #[tokio::test]
    async fn find_process_returns_none_when_not_registered() {
        let m = MockController::new().with_process("cs2.exe", 4242);
        assert_eq!(
            m.find_process("cs2.exe").await.unwrap(),
            Some(ProcHandle { pid: 4242 })
        );
        assert_eq!(m.find_process("steam.exe").await.unwrap(), None);
    }

    #[tokio::test]
    async fn send_console_command_records_every_call_in_order() {
        let m = MockController::new();
        m.send_console_command("map de_dust2").await.unwrap();
        m.send_console_command("map de_mirage").await.unwrap();
        assert_eq!(
            m.send_console_command_calls(),
            vec!["map de_dust2".to_string(), "map de_mirage".to_string()]
        );
    }

    #[tokio::test]
    async fn suspend_then_resume_process_tree_is_idempotent() {
        let m = MockController::new();
        m.suspend_process_tree(100).await.unwrap();
        m.suspend_process_tree(100).await.unwrap();
        m.resume_process_tree(100).await.unwrap();
        m.resume_process_tree(100).await.unwrap();
    }

    #[tokio::test]
    async fn launch_deelevated_succeeds_by_default_and_counts_calls() {
        let m = MockController::new();
        m.launch_deelevated("steam.exe", "").await.unwrap();
        m.launch_deelevated("steam.exe", "").await.unwrap();
        assert_eq!(m.deelevate_calls(), 2);
    }

    #[tokio::test]
    async fn launch_deelevated_can_be_scripted_to_fail() {
        let m = MockController::new().with_deelevate_failing("no interactive desktop");
        let err = m.launch_deelevated("steam.exe", "").await.unwrap_err();
        assert!(err.to_string().contains("no interactive desktop"));
        // Persists across calls, unlike `fail_next_write`'s one-shot semantics.
        let err2 = m.launch_deelevated("steam.exe", "").await.unwrap_err();
        assert!(err2.to_string().contains("no interactive desktop"));
    }

    #[tokio::test]
    async fn hwinfo_already_running_reflects_a_scripted_process() {
        let sys = MockController::new();
        assert!(!sys.hwinfo_already_running().await.unwrap());
        let sys = sys.with_process("HWiNFO64.exe", 4242);
        assert!(sys.hwinfo_already_running().await.unwrap());
    }

    #[tokio::test]
    async fn start_and_close_hwinfo_count_calls() {
        let m = MockController::new();
        let dummy_path = std::path::Path::new("HWiNFO64.exe");
        m.start_hwinfo(dummy_path).await.unwrap();
        m.start_hwinfo(dummy_path).await.unwrap();
        m.close_hwinfo().await.unwrap();
        assert_eq!(m.start_hwinfo_calls(), 2);
        assert_eq!(m.close_hwinfo_calls(), 1);
    }

    #[tokio::test]
    async fn close_hwinfo_removes_a_process_it_finds() {
        let m = MockController::new().with_process("HWiNFO64.exe", 4242);
        m.close_hwinfo().await.unwrap();
        assert!(!m.hwinfo_already_running().await.unwrap());
    }

    #[tokio::test]
    async fn read_hwinfo_sensors_defaults_to_a_fixed_sane_value() {
        let m = MockController::new();
        let snapshot = m.read_hwinfo_sensors().await.unwrap();
        assert_eq!(snapshot.cpu_temp_celsius, 50.0);
        assert_eq!(snapshot.gpu_temp_celsius, None);
    }

    #[tokio::test]
    async fn read_hwinfo_sensors_can_be_scripted() {
        let m = MockController::new().with_hwinfo_reading(61.5, Some(32.0));
        let snapshot = m.read_hwinfo_sensors().await.unwrap();
        assert_eq!(snapshot.cpu_temp_celsius, 61.5);
        assert_eq!(snapshot.gpu_temp_celsius, Some(32.0));
    }

    #[tokio::test]
    async fn launch_deelevated_registers_the_process_when_scripted() {
        let m = MockController::new().with_process_on_deelevate("steam.exe", 4242);
        assert_eq!(m.find_process("steam.exe").await.unwrap(), None);
        m.launch_deelevated("steam.exe", "").await.unwrap();
        assert_eq!(
            m.find_process("steam.exe").await.unwrap(),
            Some(ProcHandle { pid: 4242 })
        );
    }

    #[tokio::test]
    async fn install_facts_have_all_ok_defaults_and_are_scriptable() {
        let m = MockController::new();
        assert_eq!(
            m.steam_install_path().await.unwrap(),
            std::path::PathBuf::from(r"C:\Program Files (x86)\Steam")
        );
        assert_eq!(
            m.app_library_path(730).await.unwrap(),
            m.steam_install_path().await.unwrap()
        );
        assert_eq!(
            m.app_manifest(730).await.unwrap(),
            Some(AppManifest {
                build_id: Some("199".into()),
                state_flags: Some(4)
            })
        );
        assert!(m.workshop_item_installed(730, "3240880604").await.unwrap());
        assert_eq!(m.free_disk_bytes().await.unwrap(), 50_000_000_000);

        let m = MockController::new()
            .with_steam_install_path(r"D:\Steam")
            .with_app_manifest(None)
            .with_workshop_item_installed(false)
            .with_free_disk_bytes(1);
        assert_eq!(
            m.steam_install_path().await.unwrap(),
            std::path::PathBuf::from(r"D:\Steam")
        );
        assert_eq!(m.app_manifest(730).await.unwrap(), None);
        assert!(!m.workshop_item_installed(730, "3240880604").await.unwrap());
        assert_eq!(m.free_disk_bytes().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn reboot_and_shutdown_are_counted_and_never_executed() {
        let m = MockController::new();
        m.reboot(10, "restarting for HAGS").await.unwrap();
        m.shutdown(60, "run complete").await.unwrap();
        m.cancel_shutdown().await.unwrap();
        assert_eq!(m.reboot_calls(), 1);
        assert_eq!(m.shutdown_calls(), 1);
        assert_eq!(m.cancel_shutdown_calls(), 1);
    }

    #[tokio::test]
    async fn reboot_can_be_scripted_to_fail() {
        let m = MockController::new().with_reboot_failing("no shutdown privilege");
        let err = m.reboot(10, "x").await.unwrap_err();
        assert!(err.to_string().contains("no shutdown privilege"));
    }

    #[tokio::test]
    async fn boot_report_defaults_to_a_clean_boot_and_can_be_scripted() {
        let m = MockController::new();
        let r = m
            .boot_report(std::time::SystemTime::UNIX_EPOCH)
            .await
            .unwrap();
        assert_eq!(r, BootReport::default());
        let scripted = BootReport {
            kernel_power_41: true,
            bugcheck_1001: true,
            new_minidumps: vec![std::path::PathBuf::from(r"C:\Windows\Minidump\a.dmp")],
            safeboot_option: None,
        };
        let m = MockController::new().with_boot_report(scripted.clone());
        assert_eq!(
            m.boot_report(std::time::SystemTime::UNIX_EPOCH)
                .await
                .unwrap(),
            scripted
        );
    }

    #[tokio::test]
    async fn bitlocker_defaults_off_and_sleep_inhibition_is_recorded() {
        let m = MockController::new();
        assert_eq!(
            m.bitlocker_protection().await.unwrap(),
            BitlockerStatus::Off
        );
        let m = MockController::new().with_bitlocker(BitlockerStatus::On);
        assert_eq!(m.bitlocker_protection().await.unwrap(), BitlockerStatus::On);
        m.inhibit_sleep(true).await.unwrap();
        m.inhibit_sleep(false).await.unwrap();
        assert_eq!(m.inhibit_sleep_calls(), vec![true, false]);
    }

    #[tokio::test]
    async fn tasks_register_deregister_and_report_existence() {
        let m = MockController::new();
        let spec = TaskSpec {
            name: RESUME_TASK_NAME.into(),
            description: "run r1".into(),
            trigger: TaskTrigger::AtLogonOfCurrentUser,
            principal: TaskPrincipal::CurrentUserHighest,
            exe: std::path::PathBuf::from(r"C:\VOIDFRAME\voidframe.exe"),
            args: "--resume".into(),
        };
        assert!(!m.task_exists(RESUME_TASK_NAME).await.unwrap());
        m.register_task(&spec).await.unwrap();
        assert!(m.task_exists(RESUME_TASK_NAME).await.unwrap());
        assert_eq!(m.registered_tasks(), vec![spec.clone()]);
        // Re-registering the same name replaces, never duplicates.
        m.register_task(&spec).await.unwrap();
        assert_eq!(m.registered_tasks().len(), 1);
        m.deregister_task(RESUME_TASK_NAME).await.unwrap();
        assert!(!m.task_exists(RESUME_TASK_NAME).await.unwrap());
        // Deregistering an absent task is Ok (idempotent cleanup).
        m.deregister_task(RESUME_TASK_NAME).await.unwrap();
    }

    #[tokio::test]
    async fn run_script_succeeds_by_default_and_records_the_call() {
        let m = MockController::new();
        let out = m
            .run_script(
                std::path::Path::new("C:\\scripts\\apply.bat"),
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        assert_eq!(out.exit_code, Some(0));
        assert_eq!(
            m.run_script_calls(),
            vec![std::path::PathBuf::from("C:\\scripts\\apply.bat")]
        );
    }

    #[tokio::test]
    async fn run_script_can_be_scripted_to_fail() {
        let m =
            MockController::new().with_run_script_result(Err(Error::mock("nonzero exit".into())));
        let err = m
            .run_script(
                std::path::Path::new("C:\\scripts\\apply.bat"),
                Duration::from_secs(5),
            )
            .await
            .unwrap_err();
        assert!(err.to_string().contains("nonzero exit"));
    }
}
