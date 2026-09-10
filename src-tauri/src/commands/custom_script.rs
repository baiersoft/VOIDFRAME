use std::path::{Path, PathBuf};
use voidframe_engine::model::Module;
use voidframe_engine::model::module::CustomScriptPayload;

use super::projects::validate_project_id;

const ALLOWED_EXTENSIONS: &[&str] = &["bat", "cmd", "ps1"];

fn scripts_dir(data_root: &Path, project_id: &str) -> PathBuf {
    data_root.join("projects").join(project_id).join("scripts")
}

fn has_allowed_extension(name: &str) -> bool {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| {
            ALLOWED_EXTENSIONS
                .iter()
                .any(|allowed| e.eq_ignore_ascii_case(allowed))
        })
}

/// Copies `source_path` into `<project>/scripts/<basename>`, re-validating
/// through the same public gate the engine applies at deserialize time
/// (`Module::require_m1_supported`, not the private `validate_script_path`
/// -- see this plan's Global Constraints) by constructing a throwaway
/// `Module::CustomScript` naming the candidate filename as both
/// `apply_script` and `revert_script` (only one is under test here; the
/// validation is per-field and symmetric, so reusing the same filename for
/// both sides of the throwaway payload exercises the identical check either
/// field would get).
pub(crate) fn import_custom_script_impl(
    data_root: &Path,
    project_id: &str,
    source_path: &str,
) -> Result<String, String> {
    validate_project_id(project_id)?;
    let source = Path::new(source_path);
    let filename = source
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("source path {source_path:?} has no file name"))?
        .to_string();

    if !has_allowed_extension(&filename) {
        return Err(format!("{filename:?} must end in .bat, .cmd, or .ps1"));
    }

    let candidate = Module::CustomScript(CustomScriptPayload {
        apply_script: filename.clone(),
        revert_script: filename.clone(),
        requires_reboot: false,
        description: String::new(),
    });
    candidate
        .require_m1_supported()
        .map_err(|e| e.to_string())?;

    let dest_dir = scripts_dir(data_root, project_id);
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let dest = dest_dir.join(&filename);
    std::fs::copy(source, &dest).map_err(|e| format!("failed to copy {source_path:?}: {e}"))?;

    Ok(filename)
}

#[specta::specta]
#[tauri::command]
pub async fn import_custom_script(
    state: tauri::State<'_, crate::state::AppState>,
    project_id: String,
    source_path: String,
) -> Result<String, String> {
    let data_root = state.data_root.path().to_path_buf();
    tokio::task::spawn_blocking(move || {
        import_custom_script_impl(&data_root, &project_id, &source_path)
    })
    .await
    .map_err(|e| super::join_error_to_string("import_custom_script", e))?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_dir(root: &std::path::Path, project_id: &str) -> std::path::PathBuf {
        root.join("projects").join(project_id)
    }

    #[test]
    fn import_custom_script_copies_a_valid_script_into_the_scripts_dir() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("apply.bat");
        std::fs::write(&source, "echo hi").unwrap();

        let filename =
            import_custom_script_impl(root.path(), "proj1", source.to_str().unwrap()).unwrap();

        assert_eq!(filename, "apply.bat");
        let copied = project_dir(root.path(), "proj1")
            .join("scripts")
            .join("apply.bat");
        assert_eq!(std::fs::read_to_string(copied).unwrap(), "echo hi");
    }

    #[test]
    fn import_custom_script_rejects_an_unsupported_extension() {
        let root = tempfile::tempdir().unwrap();
        let source_dir = tempfile::tempdir().unwrap();
        let source = source_dir.path().join("apply.exe");
        std::fs::write(&source, "x").unwrap();

        let err =
            import_custom_script_impl(root.path(), "proj1", source.to_str().unwrap()).unwrap_err();
        assert!(
            err.contains("apply.exe") || err.to_lowercase().contains("extension"),
            "{err}"
        );
    }
}
