//! Spec §5.2: the deadman path. Headless, no store actor, no UI. Reverts
//! a stranded run's journals and clears its scheduled tasks and markers.

use crate::error::Result;
use crate::model::progress::RunProgress;
use crate::system::{DEADMAN_TASK_NAME, MutationCtx, RESUME_TASK_NAME, SystemController};
use std::path::Path;
use std::time::{Duration, SystemTime};

pub const HEARTBEAT_FILE: &str = "heartbeat";
pub const RECOVERED_MARKER: &str = "recovered.json";
pub const HEARTBEAT_FRESH: Duration = Duration::from_secs(120);

pub fn touch_heartbeat(run_dir: &Path) -> Result<()> {
    std::fs::create_dir_all(run_dir)?;
    std::fs::write(
        run_dir.join(HEARTBEAT_FILE),
        crate::model::results::utc_timestamp_now(),
    )?;
    Ok(())
}

pub fn heartbeat_is_fresh(run_dir: &Path, now: SystemTime) -> bool {
    std::fs::metadata(run_dir.join(HEARTBEAT_FILE))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| now.duration_since(t).ok())
        .is_some_and(|age| age < HEARTBEAT_FRESH)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverReport {
    pub run_id: String,
    pub reverted: u32,
    pub verify_failures: Vec<String>,
    pub skipped_fresh_heartbeat: bool,
}

fn run_id_from_state(data_root: &Path) -> Option<String> {
    let bytes = std::fs::read(data_root.join("state").join("current.json")).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v["run_id"].as_str().map(str::to_string)
}

fn most_recent_run_with_progress(data_root: &Path) -> Option<String> {
    let mut best: Option<(SystemTime, String)> = None;
    for e in std::fs::read_dir(data_root.join("runs")).ok()?.flatten() {
        let p = e.path();
        if !p.join(crate::model::progress::PROGRESS_FILE).exists() {
            continue;
        }
        let m = e.metadata().and_then(|m| m.modified()).ok()?;
        let id = e.file_name().to_string_lossy().into_owned();
        if best.as_ref().is_none_or(|(t, _)| m > *t) {
            best = Some((m, id));
        }
    }
    best.map(|(_, id)| id)
}

/// The project a run directory's journals belong to -- needed to resolve a
/// `custom_script` record's `scripts/` directory at revert time -- read
/// from whichever of the run's own files is still there: `progress.json`
/// (a run in flight or stranded), else `results.json` (a run that finished
/// and had its `progress.json` removed). `None` when neither is readable.
/// Run directories are named by run uuid, never by project id, so the
/// directory name itself is never a usable fallback.
///
/// Validated as a path segment before it is returned: both files sit under
/// `%LOCALAPPDATA%` and are writable by the unelevated user, while every
/// caller joins the result into a path an elevated (or SYSTEM) process then
/// reads scripts from.
pub fn project_id_for_run(run_dir: &Path) -> Option<String> {
    let id = RunProgress::load(run_dir)
        .map(|p| p.project.id)
        .or_else(|_| {
            crate::model::results::RunResults::load(&run_dir.join("results.json"))
                .map(|r| r.project_id)
        })
        .ok()?;
    crate::model::validate_id_segment(&id).ok()?;
    Some(id)
}

pub async fn recover(
    sys: &dyn SystemController,
    data_root: &Path,
    now: SystemTime,
) -> Result<RecoverReport> {
    let run_id = run_id_from_state(data_root)
        .or_else(|| most_recent_run_with_progress(data_root))
        .ok_or_else(|| crate::error::Error::msg("no run to recover".into()))?;
    let run_dir = data_root.join("runs").join(&run_id);
    if heartbeat_is_fresh(&run_dir, now) {
        return Ok(RecoverReport {
            run_id,
            reverted: 0,
            verify_failures: vec![],
            skipped_fresh_heartbeat: true,
        });
    }
    // Loaded up front (not just inside the restore block below): the
    // cursor decides which journals are still live, and the project id
    // resolves a `custom_script` module's `scripts/` directory. With no
    // resolvable project id at all, a `custom_script` revert lands its
    // failure in `verify_failures` (via `revert_all`'s own error handling
    // below) rather than panicking.
    let progress = RunProgress::load(&run_dir).ok();
    let project_dir = match project_id_for_run(&run_dir) {
        Some(id) => data_root.join("projects").join(id),
        None => {
            tracing::warn!(
                run_id,
                "no readable progress.json or results.json names this run's project -- \
                 custom_script reverts in its journals cannot be resolved"
            );
            data_root.join("projects").join("<unknown>")
        }
    };

    let mut reverted = 0;
    let mut verify_failures = Vec::new();
    // Only the journals whose mutations are still live per the persisted
    // cursor -- a scenario that completed its own revert before the reboot
    // keeps its journal on disk, and replaying it here would re-run its
    // `custom_script` revert as SYSTEM and re-delete an already-deleted
    // power plan (see `journal::live`). Every journal when the cursor is
    // unreadable.
    let journals = match crate::journal::live::live_journals(&run_dir, progress.as_ref()) {
        Ok(j) => j,
        Err(e) => {
            verify_failures.push(format!("listing journals failed: {e}"));
            vec![]
        }
    };
    for path in journals.into_iter().rev() {
        match crate::journal::replay::revert_all(&project_dir, &path, sys).await {
            Ok(report) => {
                reverted += report.reverted;
                verify_failures.extend(report.verify_failures);
            }
            Err(e) => {
                verify_failures.push(format!("{}: replay failed: {e}", path.display()));
            }
        }
    }
    if let Some(progress) = &progress {
        let ctx = MutationCtx {
            run_id: run_id.clone(),
            scenario_id: "recover".into(),
            step_index: 0,
        };
        if let Err(e) = sys
            .set_active_power_plan(&progress.start_power_plan.guid, &ctx)
            .await
        {
            verify_failures.push(format!("power plan restore failed: {e}"));
        }
        if let Err(e) = sys
            .write_cs2_launch_options(&progress.start_launch_args_raw)
            .await
        {
            verify_failures.push(format!("launch options restore failed: {e}"));
        }
    }
    if let Err(e) = sys.inhibit_sleep(false).await {
        verify_failures.push(format!("inhibit_sleep(false) failed: {e}"));
    }
    for name in [RESUME_TASK_NAME, DEADMAN_TASK_NAME] {
        if let Err(e) = sys.deregister_task(name).await {
            verify_failures.push(format!("deregister {name} failed: {e}"));
        }
    }
    let marker = serde_json::json!({
        "at": crate::model::results::utc_timestamp_now(),
        "reason": "deadman",
        "reverted": reverted,
        "verify_failures": verify_failures,
    });
    crate::paths::atomic_write(
        &run_dir.join(RECOVERED_MARKER),
        &serde_json::to_vec_pretty(&marker)?,
    )?;
    let _ = std::fs::remove_file(RunProgress::path(&run_dir));
    crate::journal::restore_script::remove_all(data_root);
    let _ = std::fs::remove_file(data_root.join("state").join("current.json"));
    Ok(RecoverReport {
        run_id,
        reverted,
        verify_failures,
        skipped_fresh_heartbeat: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{Journal, Op};
    use crate::system::{MockController, RegKey, RegValue, TaskPrincipal, TaskSpec, TaskTrigger};
    use serde_json::json;
    use std::path::PathBuf;

    fn stranded_run(dir: &Path) -> (MockController, PathBuf) {
        let run_dir = dir.join("runs").join("r1");
        std::fs::create_dir_all(dir.join("state")).unwrap();
        std::fs::write(
            dir.join("state").join("current.json"),
            r#"{"run_id":"r1","phase":{"kind":"reboot_pending","reason":"apply_next"}}"#,
        )
        .unwrap();
        let key = RegKey {
            hive: crate::model::module::Hive::Hklm,
            subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers".into(),
            value_name: "HwSchMode".into(),
        };
        let mock = MockController::new().with_registry(&key, RegValue::Dword(2));
        let ctx = MutationCtx {
            run_id: "r1".into(),
            scenario_id: "hags".into(),
            step_index: 0,
        };
        let mut j = Journal::open(&run_dir.join("journal-hags.jsonl")).unwrap();
        let seq = j
            .record(
                Op::RegistryWrite,
                &ctx,
                json!({"hive":"HKLM","subkey":"SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers","value_name":"HwSchMode"}),
                json!({"type":"DWORD","value":2}),
                json!({"kind":"delete","target":{"hive":"HKLM","subkey":"SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers","value_name":"HwSchMode"}}),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
        (mock, run_dir)
    }

    #[tokio::test]
    async fn recover_reverts_journals_removes_tasks_and_writes_the_marker() {
        let dir = tempfile::tempdir().unwrap();
        let (mock, run_dir) = stranded_run(dir.path());
        let spec = |name: &str| TaskSpec {
            name: name.into(),
            description: "r1".into(),
            trigger: TaskTrigger::AtLogonOfCurrentUser,
            principal: TaskPrincipal::CurrentUserHighest,
            exe: PathBuf::from("v.exe"),
            args: String::new(),
        };
        mock.register_task(&spec(RESUME_TASK_NAME)).await.unwrap();
        mock.register_task(&spec(DEADMAN_TASK_NAME)).await.unwrap();

        let report = recover(&mock, dir.path(), SystemTime::now()).await.unwrap();

        assert_eq!(report.run_id, "r1");
        assert_eq!(report.reverted, 1);
        assert!(!report.skipped_fresh_heartbeat);
        let key = RegKey {
            hive: crate::model::module::Hive::Hklm,
            subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers".into(),
            value_name: "HwSchMode".into(),
        };
        assert_eq!(mock.read_registry(&key).await.unwrap(), RegValue::Absent);
        assert!(mock.registered_tasks().is_empty());
        assert!(run_dir.join(RECOVERED_MARKER).exists());
        assert!(!dir.path().join("state").join("current.json").exists());
    }

    #[tokio::test]
    async fn recover_continues_past_a_corrupt_journal_and_still_reverts_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let (mock, run_dir) = stranded_run(dir.path());
        // Lexically after "journal-hags.jsonl", so `journals_in`'s ascending
        // sort plus the caller's `.rev()` reverts this one first.
        std::fs::write(run_dir.join("journal-zzz.jsonl"), [0xFF, 0xFE, 0x00, 0x01]).unwrap();

        let report = recover(&mock, dir.path(), SystemTime::now()).await.unwrap();

        assert_eq!(report.reverted, 1);
        let key = RegKey {
            hive: crate::model::module::Hive::Hklm,
            subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers".into(),
            value_name: "HwSchMode".into(),
        };
        assert_eq!(mock.read_registry(&key).await.unwrap(), RegValue::Absent);
        assert_eq!(
            report.verify_failures.len(),
            1,
            "{:?}",
            report.verify_failures
        );
        assert!(
            report.verify_failures[0].contains("journal-zzz.jsonl"),
            "{}",
            report.verify_failures[0]
        );
        assert!(run_dir.join(RECOVERED_MARKER).exists());
    }

    #[tokio::test]
    async fn recover_records_a_failed_task_deregistration_instead_of_discarding_it() {
        let dir = tempfile::tempdir().unwrap();
        let (mock, _run_dir) = stranded_run(dir.path());
        let mock = mock.with_deregister_task_failing("access denied");

        let report = recover(&mock, dir.path(), SystemTime::now()).await.unwrap();

        assert!(
            report
                .verify_failures
                .iter()
                .any(|f| f.contains("deregister")),
            "{:?}",
            report.verify_failures
        );
    }

    #[tokio::test]
    async fn recover_does_nothing_while_the_heartbeat_is_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let (mock, run_dir) = stranded_run(dir.path());
        touch_heartbeat(&run_dir).unwrap();
        let report = recover(&mock, dir.path(), SystemTime::now()).await.unwrap();
        assert!(report.skipped_fresh_heartbeat);
        assert_eq!(report.reverted, 0);
        assert!(!run_dir.join(RECOVERED_MARKER).exists());
    }

    #[test]
    fn heartbeat_freshness_uses_the_file_mtime() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!heartbeat_is_fresh(dir.path(), SystemTime::now()));
        touch_heartbeat(dir.path()).unwrap();
        assert!(heartbeat_is_fresh(dir.path(), SystemTime::now()));
        assert!(!heartbeat_is_fresh(
            dir.path(),
            SystemTime::now() + Duration::from_secs(600)
        ));
    }
}

#[cfg(test)]
mod review_2026_09_10_tests {
    use super::*;
    use crate::journal::{Journal, Op};
    use crate::model::progress::{Cursor, Stage};
    use crate::model::project::{Baseline, Project, Scenario};
    use crate::system::{MockController, PowerPlan};
    use serde_json::json;

    fn project(id: &str) -> Project {
        let scenario = |id: &str| Scenario {
            id: id.into(),
            name: id.into(),
            description: "d".into(),
            enabled: true,
            modules: vec![],
        };
        Project {
            schema_version: crate::model::SCHEMA_VERSION.to_string(),
            id: id.into(),
            name: "P".into(),
            description: "d".into(),
            created_at: "2026-09-10T00:00:00Z".into(),
            settings: serde_json::from_str("{}").unwrap(),
            baseline: Baseline {
                name: "Stock".into(),
                description: "d".into(),
            },
            scenarios: vec![scenario("a"), scenario("b")],
        }
    }

    fn progress(project: Project, cursor: Cursor) -> RunProgress {
        RunProgress {
            schema_version: crate::model::SCHEMA_VERSION.to_string(),
            run_id: "r1".into(),
            project,
            start_build_id: None,
            start_launch_args: String::new(),
            start_launch_args_raw: String::new(),
            start_power_plan: PowerPlan {
                guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
                name: "Balanced".into(),
                active: true,
            },
            thermal_baseline: None,
            completed: vec![],
            unstable: vec![],
            cursor,
            reboot: None,
            shutdown_when_complete: false,
            skip_revert_once: false,
            abort_requested: false,
        }
    }

    fn powercfg_journal(run_dir: &Path, scenario_id: &str, setting: &str) {
        let mut j = Journal::open(&run_dir.join(format!("journal-{scenario_id}.jsonl"))).unwrap();
        let seq = j
            .record(
                Op::PowercfgWrite,
                &MutationCtx {
                    run_id: "r1".into(),
                    scenario_id: scenario_id.into(),
                    step_index: 0,
                },
                json!({}),
                json!(1),
                json!({"sub":"sub_processor","setting":setting,"value":0}),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
    }

    /// The deadman's real case: `a` applied, measured and reverted cleanly,
    /// `b` applied and the machine rebooted, nobody logged in. Only `b`'s
    /// journal is replayed -- `a`'s revert already ran.
    #[tokio::test]
    async fn recover_replays_only_the_journals_still_live_per_the_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(dir.path().join("state")).unwrap();
        std::fs::write(
            dir.path().join("state").join("current.json"),
            r#"{"run_id":"r1","phase":{"kind":"reboot_pending","reason":"apply_next"}}"#,
        )
        .unwrap();
        powercfg_journal(&run_dir, "a", "SETTING_A");
        powercfg_journal(&run_dir, "b", "SETTING_B");
        progress(
            project("p1"),
            Cursor {
                index: 2,
                stage: Stage::Measure,
            },
        )
        .save(&run_dir)
        .unwrap();
        let mock = MockController::new()
            .with_powercfg(
                "sub_processor",
                "SETTING_A",
                crate::system::AcDc { ac: 7, dc: 7 },
            )
            .with_powercfg(
                "sub_processor",
                "SETTING_B",
                crate::system::AcDc { ac: 1, dc: 1 },
            );

        let report = recover(&mock, dir.path(), SystemTime::now()).await.unwrap();

        assert_eq!(report.reverted, 1, "{:?}", report.verify_failures);
        assert!(
            report.verify_failures.is_empty(),
            "{:?}",
            report.verify_failures
        );
        assert_eq!(
            mock.read_powercfg("sub_processor", "SETTING_B")
                .await
                .unwrap()
                .ac,
            0,
            "the in-flight scenario's mutation is reverted"
        );
        assert_eq!(
            mock.read_powercfg("sub_processor", "SETTING_A")
                .await
                .unwrap()
                .ac,
            7,
            "a scenario that already completed its own revert is left alone"
        );
    }

    /// Run directories are named by run uuid; the project id has to come
    /// from the run's own files -- `results.json` once a completed run's
    /// `progress.json` is gone.
    #[test]
    fn project_id_for_run_falls_back_to_results_json() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("6f1c2d3e");
        std::fs::create_dir_all(&run_dir).unwrap();
        assert_eq!(project_id_for_run(&run_dir), None);

        std::fs::write(
            run_dir.join("results.json"),
            json!({
                "schema_version": crate::model::SCHEMA_VERSION,
                "run_id": "6f1c2d3e",
                "project_id": "my-project",
                "completed_at": "2026-09-10T00:00:00Z",
                "detection_tier": "log_tail",
                "baseline": {
                    "scenario_id": "baseline", "name": "Stock", "is_baseline": true,
                    "aggregated": crate::mock_harness::mock_metrics(),
                    "per_iteration": [], "metric_deltas": [], "wcps": 0.0,
                    "verdict": "confirmed_same"
                },
                "scenarios": []
            })
            .to_string(),
        )
        .unwrap();
        assert_eq!(project_id_for_run(&run_dir).as_deref(), Some("my-project"));

        progress(
            project("from-progress"),
            Cursor {
                index: 0,
                stage: Stage::Apply,
            },
        )
        .save(&run_dir)
        .unwrap();
        assert_eq!(
            project_id_for_run(&run_dir).as_deref(),
            Some("from-progress")
        );
    }

    /// A JSON-sourced id that is not a plain path segment is never joined
    /// into a scripts path.
    #[test]
    fn project_id_for_run_rejects_a_traversal_id() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        progress(
            project(r"..\..\Users\Public\x"),
            Cursor {
                index: 0,
                stage: Stage::Apply,
            },
        )
        .save(&run_dir)
        .unwrap();
        assert_eq!(project_id_for_run(&run_dir), None);
    }
}
