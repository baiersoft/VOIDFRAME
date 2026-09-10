use std::collections::BTreeMap;
use voidframe_engine::mutation::cs2_config::parse_video_settings;
use voidframe_engine::system::SystemController;

pub(crate) async fn read_cs2_video_config_impl(
    sys: &dyn SystemController,
) -> Result<BTreeMap<String, String>, String> {
    let raw = sys
        .read_cs2_video_config()
        .await
        .map_err(|e| e.to_string())?;
    Ok(parse_video_settings(&raw))
}

#[specta::specta]
#[tauri::command]
pub async fn read_cs2_video_config(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<BTreeMap<String, String>, String> {
    read_cs2_video_config_impl(state.sys.as_ref()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::system::MockController;

    #[tokio::test]
    async fn reads_the_live_settings_into_a_map() {
        let sys = MockController::new().with_cs2_video_config(
            "\"VideoConfig\"\n{\n\t\"setting.defaultres\"\t\t\"1920\"\n\t\"setting.fullscreen\"\t\t\"1\"\n}\n",
        );
        let map = read_cs2_video_config_impl(&sys).await.unwrap();
        assert_eq!(
            map.get("setting.defaultres").map(String::as_str),
            Some("1920")
        );
        assert_eq!(map.get("setting.fullscreen").map(String::as_str), Some("1"));
    }

    #[tokio::test]
    async fn empty_live_value_reads_as_an_empty_map() {
        let sys = MockController::new();
        let map = read_cs2_video_config_impl(&sys).await.unwrap();
        assert!(map.is_empty());
    }
}
