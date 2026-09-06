//! The M1-scoped tweak catalog `list_catalog_tweaks` serves — a small,
//! curated seed (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §9.1), not the full prose catalog in
//! `docs/05-tweak-catalog.md` (that whole doc's own conversion to
//! structured data is a post-milestone follow-up, docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §15). Every entry
//! here maps to one of M1's five real `Module` kinds.

use crate::error::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct CatalogEntry {
    pub id: String,
    pub name: String,
    pub category: String,
    pub kind: String,
    pub description: String,
    pub available_in: String,
    /// A ready-to-use `Module` JSON value (matches `Module`'s own
    /// `#[serde(tag = "type")]` shape) — the builder can drop this
    /// straight into a scenario's `modules` array and let the user tweak
    /// specific fields (e.g. `affinityMask` for `explicit_mask`) from
    /// there, rather than re-deriving the whole shape client-side.
    ///
    /// Exported to TypeScript as `unknown` (specta-typescript's own documented
    /// per-field escape hatch for opaque/dynamic JSON, `specta_typescript::Unknown`
    /// -- deliberately not narrowing it further: this field is intentionally
    /// untyped, and specta's own `serde_json::Value` structural export hits
    /// its BigInt-forbidden guard on `serde_json::Number`'s internal i64/u64
    /// variant, which nothing in this crate's own types can fix directly).
    #[cfg_attr(feature = "specta", specta(type = specta_typescript::Unknown))]
    pub module_template: serde_json::Value,
    /// `true` if a scenario may contain at most one module of this entry's
    /// `kind` (e.g. `power_plan` -- a scenario targets exactly one power
    /// plan, so a second one is never meaningful). Enforced by
    /// `mutation::conflict_check`. Defaults to `false` so every other,
    /// already-shipped catalog entry needs no change.
    #[serde(default)]
    pub singleton: bool,
}

/// Compile-time-embedded fallback, same reasoning as
/// `run::execute::EMBEDDED_SIGNATURES_JSON`: `data/catalog.json` next to a
/// dev checkout is not reachable from a deployed binary with no build
/// step to copy it there.
const EMBEDDED_CATALOG_JSON: &str = include_str!("../../../../data/catalog.json");

pub fn load_catalog() -> Result<Vec<CatalogEntry>> {
    Ok(serde_json::from_str(EMBEDDED_CATALOG_JSON)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_seed_parses_and_is_non_empty() {
        let entries = load_catalog().unwrap();
        assert!(!entries.is_empty());
        assert!(entries.iter().all(|e| e.available_in == "M1"));
    }

    #[test]
    fn every_entry_kind_is_m1_supported() {
        let m1_kinds = [
            "registry",
            "powercfg",
            "power_plan",
            "affinity_cpu",
            "launch_args",
        ];
        for e in load_catalog().unwrap() {
            assert!(
                m1_kinds.contains(&e.kind.as_str()),
                "catalog entry {:?} has kind {:?}, not an M1-supported module kind",
                e.id,
                e.kind
            );
        }
    }

    #[test]
    fn ids_are_unique() {
        let entries = load_catalog().unwrap();
        let mut ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), before, "duplicate catalog entry id");
    }

    #[test]
    fn singleton_field_defaults_to_false_when_absent_from_json() {
        let e: CatalogEntry = serde_json::from_str(
            r#"{"id":"x","name":"X","category":"cpu","kind":"powercfg","description":"d","available_in":"M1","module_template":{}}"#,
        )
        .unwrap();
        assert!(!e.singleton);
    }

    #[test]
    fn power_plan_catalog_entry_is_marked_singleton() {
        let entries = load_catalog().unwrap();
        let power_plan = entries
            .iter()
            .find(|e| e.kind == "power_plan")
            .expect("catalog should have a power_plan entry");
        assert!(
            power_plan.singleton,
            "a scenario can only meaningfully target one power plan at a time"
        );
    }

    #[test]
    fn launch_args_catalog_entry_is_marked_singleton() {
        let entries = load_catalog().unwrap();
        let launch_args = entries
            .iter()
            .find(|e| e.kind == "launch_args")
            .expect("catalog should have a launch_args entry");
        assert!(
            launch_args.singleton,
            "a scenario edits one free-text launch-options field, not several"
        );
    }
}
