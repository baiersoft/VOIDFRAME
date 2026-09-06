use voidframe_engine::system::{PowerPlan, SystemController};

pub(crate) async fn list_power_plans_impl(
    sys: &dyn SystemController,
) -> Result<Vec<PowerPlan>, String> {
    sys.list_power_plans().await.map_err(|e| e.to_string())
}

#[specta::specta]
#[tauri::command]
pub async fn list_power_plans(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<Vec<PowerPlan>, String> {
    list_power_plans_impl(state.sys.as_ref()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::system::MockController;

    #[tokio::test]
    async fn lists_the_mock_controllers_power_plans() {
        let sys = MockController::new();
        let plans = list_power_plans_impl(&sys).await.unwrap();
        assert!(!plans.is_empty());
        assert_eq!(plans.iter().filter(|p| p.active).count(), 1);
    }
}
