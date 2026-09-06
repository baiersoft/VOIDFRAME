use voidframe_engine::model::project::{Project, Scenario};
use voidframe_engine::preflight::{PreflightReport, evaluate, gather_facts};
use voidframe_engine::system::SystemController;

const MIN_FREE_DISK_BYTES: u64 = 2_000_000_000;

pub(crate) async fn preflight_impl(
    sys: &dyn SystemController,
    last_known_build_id: Option<&str>,
    min_free_bytes: u64,
    scenarios: &[Scenario],
) -> Result<PreflightReport, String> {
    let facts = gather_facts(sys, last_known_build_id, min_free_bytes, scenarios)
        .await
        .map_err(|e| e.to_string())?;
    Ok(evaluate(&facts))
}

#[specta::specta]
#[tauri::command]
pub async fn preflight(
    state: tauri::State<'_, crate::state::AppState>,
    project_id: Option<String>,
) -> Result<PreflightReport, String> {
    let last_known = crate::commands::config::get_config_from_mutex(&state.config)?
        .last_known_cs2_build_id
        .clone();
    // Best-effort: pre-flight is reachable with no project selected (the
    // header bar opens it unconditionally), and a project that fails to
    // load must not block the rest of pre-flight's genuinely useful
    // system-state checks -- it just means the power-plan-noop check has
    // nothing to compare against.
    let scenarios = match project_id {
        Some(id) => Project::load(&state.data_root.projects_dir().join(format!("{id}.json")))
            .map(|p| p.scenarios)
            .unwrap_or_default(),
        None => Vec::new(),
    };
    preflight_impl(
        state.sys.as_ref(),
        last_known.as_deref(),
        MIN_FREE_DISK_BYTES,
        &scenarios,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::system::MockController;

    #[tokio::test]
    async fn all_ok_mock_controller_produces_no_blocking_checks() {
        let sys = MockController::new().with_steam_status(voidframe_engine::system::SteamStatus {
            running: true,
            elevated: false,
        });
        let report = preflight_impl(&sys, None, 2_000_000_000, &[])
            .await
            .unwrap();
        // Every blocking check `gather_facts` runs (CS2 install, workshop
        // map, disk space, Steam-running) now reads through
        // `SystemController` (`sys.app_manifest`,
        // `sys.workshop_item_installed`, `sys.free_disk_bytes`) instead of
        // hitting this machine's real state -- `MockController::new()`'s
        // all-ok defaults (state_flags: Some(4), workshop installed, 50 GB
        // free, Steam running set above) mean this call has nothing left
        // to fail on. Assert that end state directly, not just that the
        // call succeeds.
        assert!(!report.checks.is_empty());
        assert!(report.blocking().is_empty());
    }

    #[tokio::test]
    async fn scenarios_whose_power_plan_matches_the_active_plan_produce_a_warning() {
        use voidframe_engine::model::module::{Module, PowerPlanPayload};

        let sys = MockController::new().with_steam_status(voidframe_engine::system::SteamStatus {
            running: true,
            elevated: false,
        });
        // MockController's default active plan is "Balanced",
        // 381b4222-f694-41f0-9685-ff5bb260df2e.
        let scenarios = vec![Scenario {
            id: "s1".into(),
            name: "Same As Active".into(),
            description: "d".into(),
            enabled: true,
            modules: vec![Module::PowerPlan(PowerPlanPayload {
                plan_guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
                friendly_name: None,
                create_if_missing: false,
            })],
        }];
        let report = preflight_impl(&sys, None, 2_000_000_000, &scenarios)
            .await
            .unwrap();
        let check = report
            .checks
            .iter()
            .find(|c| c.id == "power_plan_noop_vs_active")
            .unwrap();
        assert_eq!(check.status, voidframe_engine::preflight::CheckStatus::Warn);
    }
}
