use voidframe_engine::cs2::keybind_cfg::strip_reserved;
use voidframe_engine::system::SystemController;

pub(crate) async fn read_cs2_launch_options_impl(
    sys: &dyn SystemController,
) -> Result<String, String> {
    let raw = sys
        .read_cs2_launch_options()
        .await
        .map_err(|e| e.to_string())?;
    Ok(strip_reserved(&raw))
}

#[specta::specta]
#[tauri::command]
pub async fn read_cs2_launch_options(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<String, String> {
    read_cs2_launch_options_impl(state.sys.as_ref()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::system::MockController;

    #[tokio::test]
    async fn strips_voidframes_own_reserved_tokens_from_the_live_value() {
        let sys = MockController::new()
            .with_launch_options("-condebug -conclearlog +exec voidframe_keybind.cfg -novid");
        let args = read_cs2_launch_options_impl(&sys).await.unwrap();
        assert_eq!(args, "-novid");
    }

    #[tokio::test]
    async fn empty_live_value_reads_as_empty() {
        let sys = MockController::new();
        let args = read_cs2_launch_options_impl(&sys).await.unwrap();
        assert_eq!(args, "");
    }
}
