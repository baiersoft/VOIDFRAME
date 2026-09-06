//! `Project`, `Scenario`, `Baseline` and schema-checked load/save.

use crate::error::{Error, Result};
use crate::model::{Module, SCHEMA_VERSION, Settings};
use crate::paths::atomic_write;
use serde::{Deserialize, Serialize};
use std::path::Path;

fn def_true() -> bool {
    true
}

/// Compile-time-embedded starter project (docs/superpowers/specs/2026-09-05-alpha-test-seed-project-design.md),
/// same reasoning as `model::catalog::EMBEDDED_CATALOG_JSON` and
/// `run::execute::signatures::EMBEDDED_SIGNATURES_JSON`: the repo-root
/// `data/` seed isn't reachable from a deployed binary with no build step
/// to copy it there.
const EMBEDDED_ALPHA_TEST_PROJECT_JSON: &str =
    include_str!("../../../../data/alpha-test-project.json");

/// The baseline "scenario" — no modules applied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Baseline {
    pub name: String,
    pub description: String,
}

/// One test scenario: a named set of modules applied against the baseline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Scenario {
    pub id: String,
    pub name: String,
    pub description: String,
    #[serde(default = "def_true")]
    pub enabled: bool,
    #[serde(default)]
    pub modules: Vec<Module>,
}

/// A benchmark project: settings, a baseline, and 1..N scenarios.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Project {
    pub schema_version: String,
    pub id: String,
    pub name: String,
    pub description: String,
    pub created_at: String,
    pub settings: Settings,
    pub baseline: Baseline,
    #[serde(default)]
    pub scenarios: Vec<Scenario>,
}

impl Project {
    /// Read + schema-check + validate a project from a JSON file.
    pub fn load(path: &Path) -> Result<Project> {
        let bytes = std::fs::read(path)?;
        let p: Project = serde_json::from_slice(&bytes)?;
        if p.schema_version != SCHEMA_VERSION {
            return Err(Error::schema_version(
                SCHEMA_VERSION.to_string(),
                p.schema_version.clone(),
            ));
        }
        // Structural/safety checks only -- NOT the full `validate()` (which
        // also requires at least one enabled scenario). A brand-new project
        // is legitimately saved with zero scenarios (see `save_project_impl`
        // in the Tauri shell, which explicitly skips the "ready to run"
        // check for exactly this reason) and must still be loadable as a
        // draft -- otherwise `list_projects_impl`'s "skip whatever fails to
        // load" behavior silently erases every fresh project from the UI
        // the moment it's created, before the user can add a single
        // scenario to it.
        p.validate_structure()?;
        Ok(p)
    }

    /// Pretty-print + atomic-write the project.
    pub fn save(&self, path: &Path) -> Result<()> {
        atomic_write(path, &serde_json::to_vec_pretty(self)?)
    }

    /// `settings` valid, every scenario id a safe path segment, every module
    /// M1-supported. Safety-critical checks that must hold for ANY
    /// persisted project, including an in-progress draft with no enabled
    /// scenarios yet -- see `validate()` for the additional "ready to run"
    /// check.
    fn validate_structure(&self) -> Result<()> {
        self.settings.validate()?;
        for sc in &self.scenarios {
            // `Scenario.id` is user-authored (the project builder lets the
            // user name a scenario anything) and `run::execute` joins it
            // straight into `runs\<run_id>\journal-{id}.jsonl`, which it
            // then opens for writing from an elevated process. Validating
            // here — inside a check both `Project::load` and the Tauri
            // `save_project`/`validate_scenario` commands run — closes that
            // at the source, for every front end, rather than relying on
            // each consumer re-checking it at read time.
            // `enabled` is deliberately NOT considered: a disabled scenario
            // with a traversal id is still persisted to disk and is one
            // checkbox away from being executed.
            crate::model::validate_id_segment(&sc.id)?;
            for m in &sc.modules {
                m.require_m1_supported()?;
            }
        }
        Ok(())
    }

    /// `validate_structure()`'s checks, plus: at least one enabled scenario.
    /// This is the "ready to run" check — called by the `validate_scenario`
    /// Tauri command before a matrix can launch, deliberately NOT by
    /// `Project::load` (a work-in-progress draft with zero or all-disabled
    /// scenarios must still be loadable and editable).
    pub fn validate(&self) -> Result<()> {
        self.validate_structure()?;
        if !self.scenarios.iter().any(|s| s.enabled) {
            return Err(Error::msg("project has no enabled scenarios".into()));
        }
        Ok(())
    }

    pub fn enabled_scenarios(&self) -> impl Iterator<Item = &Scenario> {
        self.scenarios.iter().filter(|s| s.enabled)
    }
}

/// Writes the bundled "Alpha Test -- Exclude Core 0" starter project into
/// `projects_dir` the first time VOIDFRAME runs against it, so a fresh
/// alpha install has something runnable without the tester building a
/// project from scratch first. Idempotent: if the file already exists --
/// whether from a previous launch, or because the tester edited or
/// deleted-then-it-was-recreated -- this does nothing and never
/// overwrites, so tester edits are never silently clobbered.
/// `SCHEMA_VERSION` is stamped in at write time, not trusted from the
/// embedded template, so this can never ship a stale schema version.
pub fn seed_alpha_test_project(projects_dir: &Path) -> Result<bool> {
    let path = projects_dir.join("alpha-test-exclude-core0.json");
    if path.exists() {
        return Ok(false);
    }
    let mut value: serde_json::Value = serde_json::from_str(EMBEDDED_ALPHA_TEST_PROJECT_JSON)?;
    value["schema_version"] = serde_json::Value::String(SCHEMA_VERSION.to_string());
    let project: Project = serde_json::from_value(value)?;
    project.validate_structure()?;
    project.save(&path)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
      "schema_version":"2.0.0","id":"p1","name":"Test","description":"d",
      "created_at":"2026-09-01T00:00:00Z",
      "settings":{},
      "baseline":{"name":"Stock","description":"clean"},
      "scenarios":[{"id":"s1","name":"Core Parking","description":"d","modules":[
        {"type":"powercfg","sub":"sub_processor","setting":"CPMINCORES","value":100}
      ]}]
    }"#;

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p1.json");
        std::fs::write(&p, GOOD).unwrap();
        let proj = Project::load(&p).unwrap();
        assert_eq!(proj.scenarios.len(), 1);
        let p2 = dir.path().join("out.json");
        proj.save(&p2).unwrap();
        let reloaded = Project::load(&p2).unwrap();
        assert_eq!(proj, reloaded);
    }

    #[test]
    fn rejects_wrong_schema_version() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.json");
        std::fs::write(&p, GOOD.replace("2.0.0", "9.9.9")).unwrap();
        assert!(Project::load(&p).is_err_and(|e| e.is_schema_version()));
    }

    #[test]
    fn rejects_a_scenario_id_that_would_escape_the_run_directory() {
        // `run::execute` joins `Scenario.id` into
        // `runs\<run_id>\journal-{id}.jsonl` and opens it for WRITING, from
        // a process running as administrator. A traversal id must never
        // survive validation — not at save time, not at load time.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.json");
        std::fs::write(
            &p,
            GOOD.replace(r#""id":"s1""#, r#""id":"..\\..\\..\\Windows\\Temp\\evil""#),
        )
        .unwrap();
        let err = Project::load(&p).unwrap_err();
        assert!(err.to_string().contains("invalid id"), "{err}");
    }

    #[test]
    fn rejects_a_scenario_id_that_is_a_bare_parent_traversal_even_when_disabled() {
        // A disabled scenario is still persisted and is one checkbox away
        // from running, so `validate` must not skip it.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.json");
        let mut proj: Project = serde_json::from_str(GOOD).unwrap();
        proj.scenarios.push(Scenario {
            id: "../evil".into(),
            name: "Sneaky".into(),
            description: "d".into(),
            enabled: false,
            modules: vec![],
        });
        proj.save(&p).unwrap();
        let err = Project::load(&p).unwrap_err();
        assert!(err.to_string().contains("invalid id"), "{err}");
    }

    #[test]
    fn accepts_ordinary_scenario_ids() {
        assert!(crate::model::validate_id_segment("baseline").is_ok());
        assert!(crate::model::validate_id_segment("core-parking_v2").is_ok());
        assert!(crate::model::validate_id_segment("").is_err());
        assert!(crate::model::validate_id_segment("a/b").is_err());
        assert!(crate::model::validate_id_segment("a\\b").is_err());
        assert!(crate::model::validate_id_segment("..").is_err());
        assert!(crate::model::validate_id_segment("C:").is_err());
        assert!(crate::model::validate_id_segment(&"x".repeat(129)).is_err());
    }

    #[test]
    fn a_fresh_draft_project_with_zero_scenarios_still_loads() {
        // Regression test: a brand-new project (as `NewProjectModal.tsx`
        // creates one) has `scenarios: []`. `save_project_impl` explicitly
        // allows saving that as a draft; `Project::load` must accept it
        // back too, or `list_projects_impl` (which silently skips whatever
        // fails to load) makes every fresh project vanish from the UI the
        // instant it's created.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("draft.json");
        let draft = GOOD.replace(
            r#""scenarios":[{"id":"s1","name":"Core Parking","description":"d","modules":[
        {"type":"powercfg","sub":"sub_processor","setting":"CPMINCORES","value":100}
      ]}]"#,
            r#""scenarios":[]"#,
        );
        std::fs::write(&p, &draft).unwrap();
        let proj = Project::load(&p).unwrap();
        assert!(proj.scenarios.is_empty());
        // The "ready to run" check must still reject it, just not at load
        // time -- this is exactly what `validate_scenario` calls before a
        // matrix is allowed to launch.
        assert!(proj.validate().is_err());
    }

    #[test]
    fn seed_alpha_test_project_writes_a_valid_loadable_project() {
        let dir = tempfile::tempdir().unwrap();
        let wrote = seed_alpha_test_project(dir.path()).unwrap();
        assert!(wrote, "should write on a fresh, empty projects dir");

        let seeded_path = dir.path().join("alpha-test-exclude-core0.json");
        let proj = Project::load(&seeded_path).unwrap();
        assert_eq!(proj.id, "alpha-test-exclude-core0");
        assert_eq!(proj.schema_version, SCHEMA_VERSION);
        assert_eq!(proj.scenarios.len(), 1);
        assert_eq!(proj.scenarios[0].modules.len(), 1);
        proj.validate().unwrap(); // ready to run out of the box -- 1 enabled scenario
    }

    #[test]
    fn seed_alpha_test_project_never_overwrites_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let seeded_path = dir.path().join("alpha-test-exclude-core0.json");
        std::fs::write(&seeded_path, r#"{"tester":"edited this file"}"#).unwrap();

        let wrote = seed_alpha_test_project(dir.path()).unwrap();
        assert!(!wrote, "must not overwrite a file that already exists");

        let contents = std::fs::read_to_string(&seeded_path).unwrap();
        assert_eq!(contents, r#"{"tester":"edited this file"}"#);
    }

    #[test]
    fn rejects_project_with_unsupported_module() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.json");
        let bad = GOOD.replace(
            r#"{"type":"powercfg","sub":"sub_processor","setting":"CPMINCORES","value":100}"#,
            r#"{"type":"driver_install","package":"x.exe"}"#,
        );
        std::fs::write(&p, bad).unwrap();
        assert!(Project::load(&p).is_err_and(|e| e.is_unsupported_module()));
    }
}
