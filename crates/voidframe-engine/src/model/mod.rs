//! Serde data model for projects, scenarios, modules, and results.
//!
//! Every persisted struct carries a `schema_version` field equal to
//! [`SCHEMA_VERSION`]; [`crate::model::project::Project::load`] rejects a
//! mismatch.

pub mod catalog;
pub mod config;
pub mod module;
pub mod progress;
pub mod project;
pub mod results;
pub mod settings;
pub mod thermal;

/// The one schema version `docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md` reads and writes.
///
/// Bumped `1.0.0` -> `2.0.0` for WCPS v3 (see
/// `docs/superpowers/plans/2026-09-04-wcps-v3-pipeline-replace-v2.md`):
/// `ScenarioResult` lost `comparisons`/`wcps_v2` in favor of
/// `metric_deltas`/`wcps`/`verdict`, a breaking change with no migration --
/// a `results.json` written by a pre-v3 build will fail to deserialize
/// under this new shape. Deliberate and accepted: WCPS v2 is fully removed,
/// not migrated; old run data can still be inspected manually as raw JSON.
pub const SCHEMA_VERSION: &str = "2.0.0";

/// Rejects anything that isn't a safe, single filesystem path segment — no
/// path separators, no `..`, no drive letters, no null bytes, no empty
/// string, and a sane length cap.
///
/// Every user-authorable id that later gets joined into a filesystem path
/// goes through this. That is not hypothetical: `run::execute` builds
/// `runs\<run_id>\journal-{scenario.id}.jsonl` from a project's own
/// `Scenario.id` and opens it for **writing**, and the desktop app that
/// drives it runs as `requireAdministrator` — an id like
/// `..\..\..\..\Windows\Temp\evil` would write an attacker-named file
/// outside the data root with administrator rights. This check therefore
/// lives in the engine, called from [`crate::model::project::Project::validate`],
/// so it protects `voidframe-cli` and the Tauri app alike rather than only
/// whichever front end remembers to re-check.
///
/// The character class is deliberately a strict allowlist (ASCII
/// alphanumerics plus `-` and `_`) rather than a denylist of known-bad
/// sequences: a denylist has to anticipate every Windows path oddity
/// (alternate data streams, `\\?\` prefixes, trailing dots/spaces, reserved
/// device names like `CON`/`NUL`), whereas nothing in this allowlist can
/// form any of them.
pub fn validate_id_segment(id: &str) -> crate::error::Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(crate::error::Error::msg(format!(
            "invalid id {id:?}: ids must be 1-128 characters of ASCII letters, digits, '-' or '_'"
        )))
    }
}

pub use catalog::CatalogEntry;
pub use config::Config;
pub use module::{
    AffinityCpuPayload, AffinityMode, Hive, Module, PowerPlanPayload, RegType, RegistryPayload,
};
pub use project::{Baseline, Project, Scenario, seed_alpha_test_project};
pub use results::{MetricDelta, Metrics, RunResults, ScenarioResult, Verdict};
pub use settings::{BenchmarkKind, Settings};
pub use thermal::ThermalLog;
