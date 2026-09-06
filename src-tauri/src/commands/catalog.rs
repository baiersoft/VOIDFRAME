use voidframe_engine::model::catalog::{CatalogEntry, load_catalog};

/// Alpha-release scope (docs/superpowers/specs/2026-09-05-bundled-tools-alpha-install-design.md
/// §5): only these three catalog entries have been confirmed working end-to-end on real tester
/// machines. The other three M1 entries stay fully functional in the engine -- this list only
/// narrows what the builder UI offers, deliberately NOT applied inside `load_catalog()` itself
/// (which `mutation::conflict_check` also uses, for the full catalog's singleton-kind set).
/// Revisit (widen or remove) once more of the M1 catalog is alpha-confirmed.
const ALPHA_CATALOG_IDS: &[&str] = &["launch-options", "power-plan", "affinity-exclude-core0"];

#[specta::specta]
#[tauri::command]
pub async fn list_catalog_tweaks() -> Result<Vec<CatalogEntry>, String> {
    let entries = load_catalog().map_err(|e| e.to_string())?;
    Ok(entries
        .into_iter()
        .filter(|e| ALPHA_CATALOG_IDS.contains(&e.id.as_str()))
        .collect())
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
            3,
            "expected exactly the 3 alpha-scoped entries, got {ids:?}"
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
}
