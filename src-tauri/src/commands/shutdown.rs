//! `cancel_shutdown` -- lets the user abort a pending
//! `shutdown_when_complete` shutdown (spec §3.4) before it fires.

#[specta::specta]
#[tauri::command]
pub async fn cancel_shutdown(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<(), String> {
    state.sys.cancel_shutdown().await.map_err(|e| e.to_string())
}
