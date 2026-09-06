use std::path::Path;

pub(crate) fn reveal_args(data_root: &Path) -> Vec<String> {
    vec![
        "/select,".to_string(),
        data_root
            .join("recovery")
            .join("VOIDFRAME_RESTORE.bat")
            .to_string_lossy()
            .into_owned(),
    ]
}

#[specta::specta]
#[tauri::command]
pub async fn open_data_dir(state: tauri::State<'_, crate::state::AppState>) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .arg(state.data_root.path())
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[specta::specta]
#[tauri::command]
pub async fn reveal_restore_bat(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    std::process::Command::new("explorer.exe")
        .args(reveal_args(state.data_root.path()))
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_args_select_the_restore_bat_not_just_open_the_folder() {
        let dir = std::path::PathBuf::from(r"C:\Users\test\AppData\Local\baiersoft\VOIDFRAME");
        let args = reveal_args(&dir);
        assert_eq!(args[0], "/select,");
        assert!(args[1].ends_with("VOIDFRAME_RESTORE.bat"));
    }
}
