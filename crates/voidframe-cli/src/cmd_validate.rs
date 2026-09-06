//! `voidframe-cli validate <project.json>`

use std::path::Path;
use voidframe_engine::model::project::Project;

pub fn run(path: &Path) -> anyhow::Result<String> {
    let project = Project::load(path)?;
    let scenarios = project.enabled_scenarios().count();
    let modules: usize = project.enabled_scenarios().map(|s| s.modules.len()).sum();
    Ok(format!(
        "OK: {} — {} enabled scenario(s), {} module(s)",
        project.name, scenarios, modules
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_ok_project_reports_counts() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.json");
        std::fs::write(
            &p,
            r#"{
          "schema_version":"2.0.0","id":"p","name":"P","description":"d",
          "created_at":"2026-09-01T00:00:00Z","settings":{},
          "baseline":{"name":"b","description":"d"},
          "scenarios":[{"id":"s","name":"s","description":"d","modules":[
            {"type":"powercfg","sub":"sub_processor","setting":"CPMINCORES","value":100}]}]
        }"#,
        )
        .unwrap();
        let msg = run(&p).unwrap();
        assert!(msg.contains("1 enabled scenario"));
        assert!(msg.contains("1 module"));
    }

    #[test]
    fn validate_rejects_unsupported_module() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.json");
        std::fs::write(
            &p,
            r#"{
          "schema_version":"2.0.0","id":"p","name":"P","description":"d",
          "created_at":"2026-09-01T00:00:00Z","settings":{},
          "baseline":{"name":"b","description":"d"},
          "scenarios":[{"id":"s","name":"s","description":"d","modules":[
            {"type":"driver_install","package":"x.exe"}]}]
        }"#,
        )
        .unwrap();
        assert!(run(&p).is_err());
    }
}
