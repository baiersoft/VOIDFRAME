use voidframe_engine::model::catalog::{CatalogEntry, load_catalog};

/// Alpha-release scope (docs/superpowers/specs/2026-09-05-bundled-tools-alpha-install-design.md
/// §5): only these catalog entries have been confirmed working end-to-end on real tester
/// machines -- `hags` is M3's first reboot-required tweak, added here once its scenario-reboot
/// path landed; `game-mode` and `win32-priority-separation` are further M3 registry tweaks,
/// added alongside it. The remaining M1 entries stay fully functional in the engine -- this
/// list only narrows what the builder UI offers, deliberately NOT applied inside `load_catalog()`
/// itself (which `mutation::conflict_check` also uses, for the full catalog's singleton-kind set).
/// Revisit (widen or remove) once more of the catalog is alpha-confirmed.
const ALPHA_CATALOG_IDS: &[&str] = &[
    "launch-options",
    "power-plan",
    "affinity-exclude-core0",
    "hags",
    "game-mode",
    "win32-priority-separation",
    "cs2-config",
    "custom-script",
];

fn list_catalog_tweaks_impl() -> Result<Vec<CatalogEntry>, String> {
    let entries = load_catalog().map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .filter(|e| ALPHA_CATALOG_IDS.contains(&e.id.as_str()))
        .collect())
}

#[specta::specta]
#[tauri::command]
pub async fn list_catalog_tweaks() -> Result<Vec<CatalogEntry>, String> {
    list_catalog_tweaks_impl()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn list_catalog_tweaks_returns_only_the_alpha_confirmed_entries() {
        let entries = list_catalog_tweaks().await.unwrap();
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids.len(),
            8,
            "expected exactly the 8 alpha-scoped entries, got {ids:?}"
        );
        for id in ALPHA_CATALOG_IDS {
            assert!(ids.contains(id), "missing alpha catalog entry {id:?}");
        }
    }

    #[tokio::test]
    async fn list_catalog_tweaks_excludes_not_yet_confirmed_entries() {
        let entries = list_catalog_tweaks().await.unwrap();
        for hidden in [
            "core-parking-disable",
            "cpu-idle-disable",
            "affinity-pcore-only",
        ] {
            assert!(
                !entries.iter().any(|e| e.id == hidden),
                "{hidden} should be hidden from the alpha catalog"
            );
        }
    }

    #[tokio::test]
    async fn the_alpha_picker_includes_hags_as_a_reboot_required_registry_tweak() {
        let entries = list_catalog_tweaks_impl().unwrap();
        let hags = entries
            .iter()
            .find(|e| e.id == "hags")
            .expect("hags in alpha picker");
        assert_eq!(hags.kind, "registry");
        assert_eq!(hags.available_in, "M3");
        assert_eq!(
            hags.module_template["requires_reboot"],
            serde_json::json!(true)
        );
        assert_eq!(
            hags.module_template["value_name"],
            serde_json::json!("HwSchMode")
        );
    }

    #[tokio::test]
    async fn the_alpha_picker_includes_game_mode_as_a_no_reboot_registry_tweak() {
        let entries = list_catalog_tweaks_impl().unwrap();
        let game_mode = entries
            .iter()
            .find(|e| e.id == "game-mode")
            .expect("game-mode in alpha picker");
        assert_eq!(game_mode.kind, "registry");
        assert_eq!(game_mode.available_in, "M3");
        assert_eq!(
            game_mode.module_template["value_name"],
            serde_json::json!("AutoGameModeEnabled")
        );
        assert_eq!(game_mode.off_value, Some(serde_json::json!(0)));
    }

    #[tokio::test]
    async fn the_alpha_picker_includes_win32_priority_separation_with_twelve_choices() {
        let entries = list_catalog_tweaks_impl().unwrap();
        let entry = entries
            .iter()
            .find(|e| e.id == "win32-priority-separation")
            .expect("win32-priority-separation in alpha picker");
        assert_eq!(entry.kind, "registry");
        assert_eq!(entry.available_in, "M3");
        assert_eq!(
            entry.module_template["value_name"],
            serde_json::json!("Win32PrioritySeparation")
        );
        assert_eq!(entry.value_choices.as_ref().map(Vec::len), Some(12));
    }
}
