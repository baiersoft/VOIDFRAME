//! `voidframe-cli dry-run <project.json>` — simulate applying every enabled
//! scenario, print what would have run, then revert and report.
//!
//! Two backends are available: the default path runs over a fresh
//! `DryRunController<MockController>`, where reads see an empty in-memory
//! system; the `--windows` path runs over `DryRunController<WindowsController>`,
//! where reads hit this machine's real registry/powercfg/topology state while
//! every mutation is still only logged, never actually applied.

use std::path::Path;
use voidframe_engine::journal::Journal;
use voidframe_engine::model::project::Project;
use voidframe_engine::mutation::{apply_scenario, revert_scenario};
use voidframe_engine::system::{DryRunController, MockController};

pub struct ScenarioOutcome {
    pub id: String,
    pub planned: Vec<String>,
    pub reverted: u32,
    pub verify_failures: Vec<String>,
}

pub struct DryRunOutcome {
    pub scenarios: Vec<ScenarioOutcome>,
}

impl DryRunOutcome {
    pub fn all_clean(&self) -> bool {
        self.scenarios.iter().all(|s| s.verify_failures.is_empty())
    }
}

pub async fn run_collect(project_path: &Path) -> anyhow::Result<DryRunOutcome> {
    let project = Project::load(project_path)?;
    let tmp = tempfile::tempdir()?;
    let mut scenarios = Vec::new();
    // A dry run has no live abort race to shield against (`apply_scenario`'s
    // `no_return` only matters inside a real `execute()` run) -- a throwaway
    // channel satisfies the parameter. `_no_return_rx` is bound so the
    // channel stays open for the loop's `send`s.
    let (no_return_tx, _no_return_rx) = tokio::sync::watch::channel(false);

    // Mirrors real project layout (`<project_dir>/project.json` +
    // `<project_dir>/scripts/`, see `run/execute/scenario.rs::project_dir_for`):
    // a `custom_script` module's `apply_script`/`revert_script` resolve
    // against `project_path`'s own parent directory.
    let project_dir = project_path.parent().unwrap_or_else(|| Path::new(""));
    for sc in project.enabled_scenarios() {
        // A fresh DryRun over a fresh Mock: reads see an empty system, writes
        // are logged only.
        let dry = DryRunController::new(MockController::new());
        let jp = tmp.path().join(format!("journal-{}.jsonl", sc.id));
        let mut j = Journal::open(&jp)?;
        apply_scenario(sc, "dry-run", project_dir, &dry, &mut j, &no_return_tx).await?;
        drop(j);
        let report = revert_scenario(&sc.id, project_dir, &jp, &dry).await?;
        scenarios.push(ScenarioOutcome {
            id: sc.id.clone(),
            planned: dry.planned_mutations(),
            reverted: report.reverted,
            verify_failures: report.verify_failures,
        });
    }
    Ok(DryRunOutcome { scenarios })
}

#[cfg(windows)]
pub async fn run_collect_windows(project_path: &Path) -> anyhow::Result<DryRunOutcome> {
    use voidframe_engine::system::WindowsController;

    let project = Project::load(project_path)?;
    let tmp = tempfile::tempdir()?;
    let mut scenarios = Vec::new();
    // A dry run has no live abort race to shield against (`apply_scenario`'s
    // `no_return` only matters inside a real `execute()` run) -- a throwaway
    // channel satisfies the parameter. `_no_return_rx` is bound so the
    // channel stays open for the loop's `send`s.
    let (no_return_tx, _no_return_rx) = tokio::sync::watch::channel(false);

    // See `run_collect`'s identical comment above.
    let project_dir = project_path.parent().unwrap_or_else(|| Path::new(""));
    for sc in project.enabled_scenarios() {
        // A fresh DryRun over the real WindowsController: reads see this
        // machine's actual current state, writes are logged only.
        let dry = DryRunController::new(WindowsController::new());
        let jp = tmp.path().join(format!("journal-{}.jsonl", sc.id));
        let mut j = Journal::open(&jp)?;
        apply_scenario(
            sc,
            "dry-run-windows",
            project_dir,
            &dry,
            &mut j,
            &no_return_tx,
        )
        .await?;
        drop(j);
        let report = revert_scenario(&sc.id, project_dir, &jp, &dry).await?;
        scenarios.push(ScenarioOutcome {
            id: sc.id.clone(),
            planned: dry.planned_mutations(),
            reverted: report.reverted,
            verify_failures: report.verify_failures,
        });
    }
    Ok(DryRunOutcome { scenarios })
}

#[cfg(not(windows))]
pub async fn run_collect_windows(_project_path: &Path) -> anyhow::Result<DryRunOutcome> {
    anyhow::bail!("--windows is only available on Windows builds")
}

/// Run the dry-run flow, dispatching to the Mock backend by default or the
/// real `WindowsController` backend when `windows` is set.
pub async fn run(project_path: &Path, windows: bool) -> anyhow::Result<()> {
    let outcome = if windows {
        run_collect_windows(project_path).await?
    } else {
        run_collect(project_path).await?
    };
    for sc in &outcome.scenarios {
        println!("\n== scenario {} ==", sc.id);
        for line in &sc.planned {
            println!("  would: {line}");
        }
        println!("  reverted {} record(s)", sc.reverted);
        for f in &sc.verify_failures {
            println!("  VERIFY FAIL: {f}");
        }
    }
    if outcome.all_clean() {
        println!("\ndry-run OK — every scenario applied (simulated) and reverted cleanly");
        Ok(())
    } else {
        eprintln!("\ndry-run had verify failures");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dry_run_lists_planned_mutations_and_reverts_clean() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.json");
        std::fs::write(
            &p,
            r#"{
          "schema_version":"2.0.0","id":"p","name":"P","description":"d",
          "created_at":"2026-09-01T00:00:00Z","settings":{},
          "baseline":{"name":"b","description":"d"},
          "scenarios":[{"id":"s1","name":"CP","description":"d","modules":[
            {"type":"powercfg","sub":"sub_processor","setting":"CPMINCORES","value":100},
            {"type":"registry","hive":"HKCU","subkey":"System\\GameConfigStore","value_name":"GameDVR_FSEBehavior","value_type":"DWORD","value":2}
          ]}]
        }"#,
        )
        .unwrap();

        let outcome = run_collect(&p).await.unwrap();
        assert_eq!(outcome.scenarios.len(), 1);
        assert!(
            outcome.scenarios[0]
                .planned
                .iter()
                .any(|l| l.contains("CPMINCORES"))
        );
        assert!(outcome.scenarios[0].verify_failures.is_empty());
        assert!(outcome.all_clean());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_dry_run_reads_real_state_but_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("p.json");
        std::fs::write(
            &p,
            r#"{
          "schema_version":"2.0.0","id":"p","name":"P","description":"d",
          "created_at":"2026-09-01T00:00:00Z","settings":{},
          "baseline":{"name":"b","description":"d"},
          "scenarios":[{"id":"s1","name":"CP","description":"d","modules":[
            {"type":"powercfg","sub":"sub_processor","setting":"CPMINCORES","value":100}
          ]}]
        }"#,
        )
        .unwrap();

        let outcome = run_collect_windows(&p).await.unwrap();
        assert_eq!(outcome.scenarios.len(), 1);
        assert!(
            outcome.scenarios[0]
                .planned
                .iter()
                .any(|l| l.contains("CPMINCORES"))
        );
        assert!(outcome.scenarios[0].verify_failures.is_empty());
        assert!(outcome.all_clean());
    }
}
