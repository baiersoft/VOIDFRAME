use std::path::Path;
use voidframe_engine::model::results::{RunResults, RunSummary};
use voidframe_engine::store::RunState;

use super::projects::validate_project_id;

pub(crate) fn get_results_impl(run_dir: &Path, run_id: &str) -> Result<RunResults, String> {
    // Validate run_id as a safe path segment. validate_project_id's name is
    // project-id-specific, but its check is a generic "safe path segment"
    // validation (no path separators, no `..`, no empty string, length-capped),
    // reused here for the same path-traversal defense reason.
    validate_project_id(run_id)?;
    let path = run_dir.join(run_id).join("results.json");
    RunResults::load(&path).map_err(|e| e.to_string())
}

#[specta::specta]
#[tauri::command]
pub async fn get_results(
    state: tauri::State<'_, crate::state::AppState>,
    run_id: String,
) -> Result<RunResults, String> {
    let runs_dir = state.data_root.runs_dir();
    tokio::task::spawn_blocking(move || get_results_impl(&runs_dir, &run_id))
        .await
        .map_err(|e| super::join_error_to_string("get_results", e))?
}

pub(crate) fn list_results_impl(
    runs_dir: &Path,
    project_id: &str,
) -> Result<Vec<RunSummary>, String> {
    // Not a path-traversal concern here (project_id is only ever compared
    // against a loaded results.json's own field, never joined into a path)
    // -- reused purely as a "well-formed id" sanity check, for the same
    // reason every other project-id-shaped input in this codebase goes
    // through it.
    validate_project_id(project_id)?;

    let mut summaries = Vec::new();
    let entries = match std::fs::read_dir(runs_dir) {
        Ok(entries) => entries,
        // A brand-new install (or one that has never completed a run) has
        // no `runs/` directory at all yet -- an empty history, not an error.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(summaries),
        Err(e) => return Err(e.to_string()),
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.path().is_dir() {
            continue;
        }
        // A run directory with no results.json (crashed/interrupted before
        // REPORT) must not hide every other run -- skip it, don't error the
        // whole list, matching list_projects_impl's own convention.
        let Ok(rr) = RunResults::load(&entry.path().join("results.json")) else {
            continue;
        };
        if rr.project_id != project_id {
            continue;
        }
        summaries.push(rr.summarize());
    }
    summaries.sort_by(|a, b| b.completed_at.cmp(&a.completed_at));
    Ok(summaries)
}

#[specta::specta]
#[tauri::command]
pub async fn list_results(
    state: tauri::State<'_, crate::state::AppState>,
    project_id: String,
) -> Result<Vec<RunSummary>, String> {
    let runs_dir = state.data_root.runs_dir();
    tokio::task::spawn_blocking(move || list_results_impl(&runs_dir, &project_id))
        .await
        .map_err(|e| super::join_error_to_string("list_results", e))?
}

#[specta::specta]
#[tauri::command]
pub async fn get_run_snapshot(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<Option<RunState>, String> {
    state.store.snapshot().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::model::results::{Metrics, ScenarioResult, Verdict};
    use voidframe_engine::run::DetectionTier;

    #[test]
    fn get_results_rejects_a_traversal_run_id() {
        let dir = tempfile::tempdir().unwrap();
        let err = get_results_impl(dir.path(), "../../../etc/passwd").unwrap_err();
        assert!(err.contains("invalid project id")); // validate_project_id's own error message
    }

    fn dummy_metrics() -> Metrics {
        Metrics {
            avg_fps: 400.0,
            median_fps: 400.0,
            p1_fps: 250.0,
            p01_fps: 180.0,
            frame_time_mean_ms: 2.5,
            frame_time_stddev_ms: 0.2,
            frame_time_cv: 0.08,
            adaptive_frame_time_cv: 0.08,
            stutter_count_pct: 0.0,
            mean_abs_animation_error_ms: None,
            gpu_busy_ms: None,
            bottleneck_ratio: None,
            render_latency_ms: None,
            dominant_present_mode: "Hardware: Independent Flip".into(),
            present_mode_consistent: true,
            present_mode_warning: None,
        }
    }

    fn dummy_results(run_id: &str, project_id: &str, completed_at: &str) -> RunResults {
        let baseline = ScenarioResult {
            scenario_id: "baseline".into(),
            name: "Stock".into(),
            is_baseline: true,
            aggregated: dummy_metrics(),
            per_iteration: vec![],
            metric_deltas: vec![],
            wcps: 0.0,
            verdict: Verdict::ConfirmedSame,
        };
        RunResults {
            schema_version: "1.0.0".into(),
            run_id: run_id.into(),
            project_id: project_id.into(),
            completed_at: completed_at.into(),
            detection_tier: DetectionTier::LogTail,
            baseline,
            scenarios: vec![],
        }
    }

    #[test]
    fn list_results_rejects_a_traversal_project_id() {
        let dir = tempfile::tempdir().unwrap();
        let err = list_results_impl(dir.path(), "../../../etc/passwd").unwrap_err();
        assert!(err.contains("invalid project id"));
    }

    #[test]
    fn list_results_returns_empty_when_runs_dir_does_not_exist_yet() {
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().join("runs"); // deliberately never created
        let summaries = list_results_impl(&runs_dir, "p1").unwrap();
        assert!(summaries.is_empty());
    }

    #[test]
    fn list_results_only_returns_runs_for_the_given_project_sorted_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path();

        let r1 = dummy_results("r1", "p1", "2026-09-04T00:00:00Z");
        let r2 = dummy_results("r2", "p1", "2026-09-05T00:00:00Z"); // newer
        let other = dummy_results("r3", "p2", "2026-09-04T12:00:00Z"); // different project

        for (run_id, rr) in [("r1", &r1), ("r2", &r2), ("r3", &other)] {
            let run_dir = runs_dir.join(run_id);
            std::fs::create_dir_all(&run_dir).unwrap();
            rr.save(&run_dir.join("results.json")).unwrap();
        }

        let summaries = list_results_impl(runs_dir, "p1").unwrap();
        let ids: Vec<&str> = summaries.iter().map(|s| s.run_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["r2", "r1"],
            "p2's run must be excluded, newest p1 run first"
        );
    }

    #[test]
    fn list_results_skips_a_run_directory_with_no_results_json() {
        // An incomplete/crashed run has no results.json -- must not error
        // the whole listing, matching list_projects_impl's own convention.
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path();
        std::fs::create_dir_all(runs_dir.join("crashed-run")).unwrap();

        let good = dummy_results("r1", "p1", "2026-09-04T00:00:00Z");
        let run_dir = runs_dir.join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        good.save(&run_dir.join("results.json")).unwrap();

        let summaries = list_results_impl(runs_dir, "p1").unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].run_id, "r1");
    }
}
