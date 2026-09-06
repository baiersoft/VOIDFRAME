//! `WindowsController` — the real [`crate::system::SystemController`] for
//! Windows. Registry and CPU-topology reads/writes are raw Win32 FFI via the
//! `windows` crate; powercfg and power-plan operations shell out to
//! `powercfg.exe` (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §4.2's own explicit choice, not a fallback).
//!
//! Every blocking OS call runs inside [`tokio::task::spawn_blocking`] — the
//! trait's methods are `async`, but registry/topology/affinity calls are
//! synchronous Win32 APIs with no async variant.

mod affinity;
mod deelevate;
mod dxgi;
mod hwinfo;
mod input;
mod power_plan;
mod powercfg;
mod process;
mod registry;
mod steam;
mod topology;
pub(crate) mod vdf;

use crate::error::{Error, Result};
use crate::system::*;
use async_trait::async_trait;
use std::path::PathBuf;

/// Stateless — every method talks to the live OS on each call. No fields
/// needed (registry/powercfg/topology have no connection or handle to hold
/// open between calls).
#[derive(Debug, Default, Clone, Copy)]
pub struct WindowsController;

impl WindowsController {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl SystemController for WindowsController {
    async fn read_registry(&self, k: &RegKey) -> Result<RegValue> {
        registry::read(k).await
    }
    async fn write_registry(&self, k: &RegKey, v: &RegValue, _c: &MutationCtx) -> Result<()> {
        registry::write(k, v).await
    }
    async fn delete_registry_value(&self, k: &RegKey, _c: &MutationCtx) -> Result<()> {
        registry::delete_value(k).await
    }

    async fn read_powercfg(&self, sub: &str, setting: &str) -> Result<AcDc<u32>> {
        powercfg::read(sub, setting).await
    }
    async fn write_powercfg(
        &self,
        sub: &str,
        setting: &str,
        v: u32,
        _c: &MutationCtx,
    ) -> Result<()> {
        powercfg::write(sub, setting, v).await
    }

    async fn list_power_plans(&self) -> Result<Vec<PowerPlan>> {
        power_plan::list().await
    }
    async fn active_power_plan(&self) -> Result<PowerPlan> {
        power_plan::active().await
    }
    async fn set_active_power_plan(&self, guid: &str, _c: &MutationCtx) -> Result<()> {
        power_plan::set_active(guid).await
    }
    async fn duplicate_power_plan(
        &self,
        template_guid: &str,
        _c: &MutationCtx,
    ) -> Result<PowerPlan> {
        power_plan::duplicate(template_guid).await
    }
    async fn delete_power_plan(&self, guid: &str, _c: &MutationCtx) -> Result<()> {
        power_plan::delete(guid).await
    }

    async fn cpu_topology(&self) -> Result<CpuTopology> {
        topology::read().await
    }
    async fn set_process_affinity(&self, pid: u32, mask: u64) -> Result<()> {
        affinity::set(pid, mask).await
    }

    async fn gpu_model(&self) -> Result<String> {
        tokio::task::spawn_blocking(dxgi::primary_adapter_description)
            .await
            .map_err(|e| Error::msg(format!("gpu_model task panicked: {e}")))?
    }

    async fn read_cs2_launch_options(&self) -> Result<String> {
        vdf::read(730).await
    }
    async fn write_cs2_launch_options(&self, args: &str) -> Result<()> {
        vdf::write(730, args).await
    }

    async fn steam_install_path(&self) -> Result<PathBuf> {
        let dir = vdf::find_steam_path().await?;
        // A stale HKCU SteamPath would otherwise surface much later as an
        // opaque ShellExecute failure -- fail here, naming what's missing.
        let exe = dir.join("steam.exe");
        if !tokio::fs::try_exists(&exe).await.unwrap_or(false) {
            return Err(Error::msg(format!(
                "Steam executable not found at {}",
                exe.display()
            )));
        }
        Ok(dir)
    }
    async fn app_library_path(&self, app_id: u32) -> Result<PathBuf> {
        vdf::find_app_library_path(app_id).await
    }
    async fn app_manifest(&self, app_id: u32) -> Result<Option<AppManifest>> {
        let path = vdf::find_app_library_path(app_id)
            .await?
            .join("steamapps")
            .join(format!("appmanifest_{app_id}.acf"));
        tokio::task::spawn_blocking(move || {
            let text = match std::fs::read_to_string(&path) {
                Ok(t) => t,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(Error::msg(format!("reading {}: {e}", path.display()))),
            };
            vdf::parse_app_manifest(&text).map(Some)
        })
        .await
        .map_err(|e| Error::msg(format!("appmanifest read task panicked: {e}")))?
    }
    async fn workshop_item_installed(&self, app_id: u32, item_id: &str) -> Result<bool> {
        let dir = vdf::find_steam_path()
            .await?
            .join("steamapps")
            .join("workshop")
            .join("content")
            .join(app_id.to_string())
            .join(item_id);
        Ok(tokio::fs::try_exists(&dir).await?)
    }
    async fn free_disk_bytes(&self) -> Result<u64> {
        tokio::task::spawn_blocking(crate::paths::free_disk_bytes_for_data_root)
            .await
            .map_err(|e| Error::msg(format!("free_disk_bytes task panicked: {e}")))?
    }

    async fn find_process(&self, name: &str) -> Result<Option<ProcHandle>> {
        process::find_by_name(name).await
    }
    async fn suspend_process_tree(&self, pid: u32) -> Result<()> {
        process::suspend_tree(pid).await
    }
    async fn resume_process_tree(&self, pid: u32) -> Result<()> {
        process::resume_tree(pid).await
    }
    async fn kill_process_tree(&self, pid: u32) -> Result<()> {
        process::kill_tree(pid).await
    }

    async fn steam_status(&self) -> Result<SteamStatus> {
        steam::status().await
    }
    async fn launch_cs2(&self, spec: &Cs2LaunchSpec) -> Result<()> {
        steam::launch(spec).await
    }
    async fn launch_deelevated(&self, program: &str, args: &str) -> Result<()> {
        deelevate::launch(program, args).await
    }

    async fn reissue_map(&self, map_command: &str) -> Result<()> {
        input::reissue_map(map_command).await
    }
    async fn hide_console(&self) -> Result<()> {
        input::hide_console().await
    }
    async fn quit_cs2_gracefully(&self) -> Result<()> {
        input::quit_cs2().await
    }
    async fn process_started_at(&self, pid: u32) -> Option<std::time::SystemTime> {
        process::process_started_at(pid).await
    }
    async fn close_steam_window(&self) -> Result<()> {
        steam::close_window().await
    }

    async fn hwinfo_already_running(&self) -> Result<bool> {
        hwinfo::already_running().await
    }
    async fn start_hwinfo(&self, path: &std::path::Path) -> Result<()> {
        hwinfo::start(path).await
    }
    async fn close_hwinfo(&self) -> Result<()> {
        hwinfo::close().await
    }
    async fn read_hwinfo_sensors(&self) -> Result<HwinfoSensorSnapshot> {
        hwinfo::read_sensors().await
    }
}
