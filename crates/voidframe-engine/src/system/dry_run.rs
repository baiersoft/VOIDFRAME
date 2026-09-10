//! Wraps any [`SystemController`] and turns every mutation into a no-op that
//! is logged instead of applied. Reads delegate to the inner controller.

use crate::error::Result;
use crate::system::*;
use async_trait::async_trait;
use std::sync::{Arc, Mutex};

pub struct DryRunController<C: SystemController> {
    inner: C,
    log: Arc<Mutex<Vec<String>>>,
}

impl<C: SystemController> DryRunController<C> {
    pub fn new(inner: C) -> Self {
        Self {
            inner,
            log: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The human-readable log of every mutation that *would* have run.
    pub fn planned_mutations(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn note(&self, line: String) {
        tracing::info!(target: "voidframe::dryrun", "{line}");
        self.log.lock().unwrap().push(line);
    }
}

#[async_trait]
impl<C: SystemController> SystemController for DryRunController<C> {
    async fn read_registry(&self, k: &RegKey) -> Result<RegValue> {
        self.inner.read_registry(k).await
    }
    async fn write_registry(&self, k: &RegKey, v: &RegValue, _c: &MutationCtx) -> Result<()> {
        self.note(format!(
            "registry.write {}\\{}\\{} = {}",
            k.hive.as_str(),
            k.subkey,
            k.value_name,
            v.describe()
        ));
        Ok(())
    }
    async fn delete_registry_value(&self, k: &RegKey, _c: &MutationCtx) -> Result<()> {
        self.note(format!(
            "registry.delete {}\\{}\\{}",
            k.hive.as_str(),
            k.subkey,
            k.value_name
        ));
        Ok(())
    }
    async fn read_powercfg(&self, sub: &str, setting: &str) -> Result<AcDc<u32>> {
        self.inner.read_powercfg(sub, setting).await
    }
    async fn write_powercfg(
        &self,
        sub: &str,
        setting: &str,
        v: u32,
        _c: &MutationCtx,
    ) -> Result<()> {
        self.note(format!("powercfg.write {sub}/{setting} = {v}"));
        Ok(())
    }
    async fn find_cs2_video_config_path(&self) -> Result<std::path::PathBuf> {
        self.inner.find_cs2_video_config_path().await
    }
    async fn read_cs2_video_config(&self) -> Result<String> {
        self.inner.read_cs2_video_config().await
    }
    async fn write_cs2_video_config(&self, text: &str) -> Result<()> {
        self.note(format!("cs2_video_config.write ({} bytes)", text.len()));
        Ok(())
    }
    async fn run_script(
        &self,
        path: &std::path::Path,
        _timeout: std::time::Duration,
    ) -> Result<ScriptOutput> {
        self.note(format!("run_script {}", path.display()));
        Ok(ScriptOutput {
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
        })
    }
    async fn list_power_plans(&self) -> Result<Vec<PowerPlan>> {
        self.inner.list_power_plans().await
    }
    async fn active_power_plan(&self) -> Result<PowerPlan> {
        self.inner.active_power_plan().await
    }
    async fn set_active_power_plan(&self, guid: &str, _c: &MutationCtx) -> Result<()> {
        self.note(format!("power_plan.activate {guid}"));
        Ok(())
    }
    async fn duplicate_power_plan(
        &self,
        template_guid: &str,
        _c: &MutationCtx,
    ) -> Result<PowerPlan> {
        self.note(format!("power_plan.duplicate {template_guid}"));
        Ok(PowerPlan {
            guid: "dryrun-dup".into(),
            name: "Dry Run Duplicate".into(),
            active: false,
        })
    }
    async fn delete_power_plan(&self, guid: &str, _c: &MutationCtx) -> Result<()> {
        self.note(format!("power_plan.delete {guid}"));
        Ok(())
    }
    async fn cpu_topology(&self) -> Result<CpuTopology> {
        self.inner.cpu_topology().await
    }
    async fn set_process_affinity(&self, pid: u32, mask: u64) -> Result<()> {
        self.note(format!("set_process_affinity pid={pid} mask={mask:#x}"));
        Ok(())
    }
    async fn gpu_model(&self) -> Result<String> {
        self.inner.gpu_model().await
    }
    async fn steam_status(&self) -> Result<SteamStatus> {
        self.inner.steam_status().await
    }
    async fn read_cs2_launch_options(&self) -> Result<String> {
        self.inner.read_cs2_launch_options().await
    }
    async fn write_cs2_launch_options(&self, args: &str) -> Result<()> {
        self.note(format!("cs2_launch_options.write {args}"));
        Ok(())
    }
    async fn steam_install_path(&self) -> Result<std::path::PathBuf> {
        self.inner.steam_install_path().await
    }
    async fn app_library_path(&self, app_id: u32) -> Result<std::path::PathBuf> {
        self.inner.app_library_path(app_id).await
    }
    async fn app_manifest(&self, app_id: u32) -> Result<Option<AppManifest>> {
        self.inner.app_manifest(app_id).await
    }
    async fn workshop_item_installed(&self, app_id: u32, item_id: &str) -> Result<bool> {
        self.inner.workshop_item_installed(app_id, item_id).await
    }
    async fn free_disk_bytes(&self) -> Result<u64> {
        self.inner.free_disk_bytes().await
    }
    async fn launch_cs2(&self, spec: &Cs2LaunchSpec) -> Result<()> {
        self.note(format!("launch_cs2 app_id={}", spec.app_id));
        Ok(())
    }
    async fn launch_deelevated(&self, program: &str, args: &str) -> Result<()> {
        self.note(format!("launch_deelevated {program} {args}"));
        Ok(())
    }
    async fn find_process(&self, name: &str) -> Result<Option<ProcHandle>> {
        self.inner.find_process(name).await
    }
    async fn suspend_process_tree(&self, pid: u32) -> Result<()> {
        self.note(format!("suspend_process_tree pid={pid}"));
        Ok(())
    }
    async fn resume_process_tree(&self, pid: u32) -> Result<()> {
        self.note(format!("resume_process_tree pid={pid}"));
        Ok(())
    }
    async fn kill_process_tree(&self, pid: u32) -> Result<()> {
        self.note(format!("kill_process_tree pid={pid}"));
        Ok(())
    }
    async fn send_console_command(&self, command: &str) -> Result<()> {
        self.note(format!("send_console_command {command}"));
        Ok(())
    }
    async fn hide_console(&self) -> Result<()> {
        self.note("hide_console".to_string());
        Ok(())
    }
    async fn quit_cs2_gracefully(&self) -> Result<()> {
        self.note("quit_cs2_gracefully".to_string());
        Ok(())
    }
    async fn process_started_at(&self, pid: u32) -> Option<std::time::SystemTime> {
        self.inner.process_started_at(pid).await
    }
    async fn close_steam_window(&self) -> Result<()> {
        self.note("close_steam_window".to_string());
        Ok(())
    }
    async fn hwinfo_already_running(&self) -> Result<bool> {
        self.inner.hwinfo_already_running().await
    }
    async fn start_hwinfo(&self, path: &std::path::Path) -> Result<()> {
        self.note(format!("start_hwinfo {}", path.display()));
        Ok(())
    }
    async fn close_hwinfo(&self) -> Result<()> {
        self.note("close_hwinfo".to_string());
        Ok(())
    }
    async fn read_hwinfo_sensors(&self) -> Result<HwinfoSensorSnapshot> {
        self.inner.read_hwinfo_sensors().await
    }
    async fn reboot(&self, delay_secs: u32, message: &str) -> Result<()> {
        self.note(format!("reboot in {delay_secs}s: {message}"));
        Ok(())
    }
    async fn shutdown(&self, delay_secs: u32, message: &str) -> Result<()> {
        self.note(format!("shutdown in {delay_secs}s: {message}"));
        Ok(())
    }
    async fn cancel_shutdown(&self) -> Result<()> {
        self.note("cancel_shutdown".to_string());
        Ok(())
    }
    async fn register_task(&self, spec: &TaskSpec) -> Result<()> {
        self.note(format!("register_task {} ({:?})", spec.name, spec.trigger));
        Ok(())
    }
    async fn deregister_task(&self, name: &str) -> Result<()> {
        self.note(format!("deregister_task {name}"));
        Ok(())
    }
    async fn task_exists(&self, name: &str) -> Result<bool> {
        self.inner.task_exists(name).await
    }
    async fn boot_report(&self, since: std::time::SystemTime) -> Result<BootReport> {
        self.inner.boot_report(since).await
    }
    async fn bitlocker_protection(&self) -> Result<BitlockerStatus> {
        self.inner.bitlocker_protection().await
    }
    async fn inhibit_sleep(&self, on: bool) -> Result<()> {
        self.note(format!("inhibit_sleep {on}"));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::module::Hive;

    fn ctx() -> MutationCtx {
        MutationCtx {
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
        }
    }

    #[tokio::test]
    async fn reads_pass_through_but_writes_are_recorded_not_applied() {
        let mock = MockController::new().with_powercfg(
            "sub_processor",
            "IDLEDISABLE",
            AcDc { ac: 0, dc: 0 },
        );
        let dry = DryRunController::new(mock.clone());

        assert_eq!(
            dry.read_powercfg("sub_processor", "IDLEDISABLE")
                .await
                .unwrap(),
            AcDc { ac: 0, dc: 0 }
        );
        dry.write_powercfg("sub_processor", "IDLEDISABLE", 1, &ctx())
            .await
            .unwrap();

        // inner is untouched
        assert_eq!(
            mock.read_powercfg("sub_processor", "IDLEDISABLE")
                .await
                .unwrap(),
            AcDc { ac: 0, dc: 0 }
        );
        // but it was logged
        assert!(
            dry.planned_mutations()
                .iter()
                .any(|l| l.contains("IDLEDISABLE") && l.contains("1"))
        );
    }

    #[tokio::test]
    async fn registry_write_delegates_reads_only() {
        let mock = MockController::new();
        let dry = DryRunController::new(mock.clone());
        let k = RegKey {
            hive: Hive::Hklm,
            subkey: "S".into(),
            value_name: "V".into(),
        };
        dry.write_registry(&k, &RegValue::Dword(2), &ctx())
            .await
            .unwrap();
        assert_eq!(mock.read_registry(&k).await.unwrap(), RegValue::Absent);
    }

    #[tokio::test]
    async fn process_control_writes_are_logged_not_applied() {
        let mock = MockController::new();
        let dry = DryRunController::new(mock.clone());

        dry.write_cs2_launch_options("-novid").await.unwrap();
        dry.launch_cs2(&Cs2LaunchSpec { app_id: 730 })
            .await
            .unwrap();
        dry.send_console_command("map de_dust2").await.unwrap();

        // inner is untouched
        assert_eq!(mock.read_cs2_launch_options().await.unwrap(), "");
        assert!(mock.send_console_command_calls().is_empty());

        // but every call was logged
        let log = dry.planned_mutations();
        assert!(log.iter().any(|l| l.contains("-novid")));
        assert!(log.iter().any(|l| l.contains("launch_cs2")));
        assert!(log.iter().any(|l| l.contains("de_dust2")));
    }

    #[tokio::test]
    async fn launch_deelevated_is_logged_not_executed() {
        let inner = MockController::new();
        let dry = DryRunController::new(inner.clone());
        dry.launch_deelevated("steam.exe", "-novid").await.unwrap();
        assert_eq!(
            inner.deelevate_calls(),
            0,
            "dry run must never call through to the inner controller"
        );
        let log = dry.planned_mutations();
        assert!(log.iter().any(|l| l.contains("launch_deelevated")));
    }

    #[tokio::test]
    async fn hwinfo_reads_delegate_but_mutations_are_logged_not_executed() {
        let inner = MockController::new()
            .with_process("HWiNFO64.exe", 4242)
            .with_hwinfo_reading(61.5, Some(32.0));
        let dry = DryRunController::new(inner.clone());

        // Reads delegate to the inner controller, same as every other read
        // in this file (steam_status, gpu_model, find_process, ...) --
        // hwinfo_already_running/read_hwinfo_sensors change nothing, so
        // there's nothing to fake or no-op.
        assert!(dry.hwinfo_already_running().await.unwrap());
        let snapshot = dry.read_hwinfo_sensors().await.unwrap();
        assert_eq!(snapshot.cpu_temp_celsius, 61.5);
        assert_eq!(snapshot.gpu_temp_celsius, Some(32.0));

        // start_hwinfo/close_hwinfo are real mutations (they actually
        // start/stop a process) -- logged, not applied, same as
        // close_steam_window's own established DryRun treatment.
        dry.start_hwinfo(std::path::Path::new(r"C:\HWiNFO64\HWiNFO64.exe"))
            .await
            .unwrap();
        dry.close_hwinfo().await.unwrap();
        assert_eq!(
            inner.start_hwinfo_calls(),
            0,
            "dry run must never call through to the inner controller for a mutation"
        );
        assert_eq!(inner.close_hwinfo_calls(), 0);

        let log = dry.planned_mutations();
        assert!(log.iter().any(|l| l.contains("start_hwinfo")));
        assert!(log.iter().any(|l| l.contains("close_hwinfo")));
    }
}
