use std::path::Path;
use voidframe_engine::model::project::Project;

/// Rejects anything that isn't a safe, single path segment — no path
/// separators, no `..`, no null bytes, no empty string, and a sane length
/// cap. Every project id used to build a filesystem path goes through
/// this first; a caller-controlled id joined into a path unvalidated is a
/// path-traversal / arbitrary-file-access risk, worse in this app
/// specifically because it runs elevated (requireAdministrator).
///
/// A thin wrapper over [`voidframe_engine::model::validate_id_segment`],
/// which owns the actual character-class check so this crate and the engine
/// (which applies the identical rule to every `Scenario.id` inside
/// `Project::validate`, protecting `voidframe-cli` too) cannot drift apart.
/// Only the error *message* is specialized here, so a caller sees "project
/// id" rather than a generic "id".
pub(crate) fn validate_project_id(id: &str) -> Result<(), String> {
    voidframe_engine::model::validate_id_segment(id)
        .map_err(|_| format!("invalid project id: {id:?}"))
}

pub(crate) fn list_projects_impl(projects_dir: &Path) -> Result<Vec<Project>, String> {
    let mut out = Vec::new();
    let entries = std::fs::read_dir(projects_dir).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        // A corrupt/unrelated .json in this directory must not hide every
        // other valid project — skip it, don't error the whole list.
        if let Ok(p) = Project::load(&entry.path()) {
            out.push(p);
        }
    }
    out.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(out)
}

pub(crate) fn get_project_impl(projects_dir: &Path, id: &str) -> Result<Project, String> {
    validate_project_id(id)?;
    let path = projects_dir.join(format!("{id}.json"));
    Project::load(&path).map_err(|e| e.to_string())
}

pub(crate) fn save_project_impl(projects_dir: &Path, project: &Project) -> Result<(), String> {
    validate_project_id(&project.id)?;
    // `Project::save` is a plain atomic write — it does NOT run
    // `Project::validate`, so the engine-side scenario-id guard would only
    // fire later, at `Project::load` time, silently leaving an unloadable
    // project on disk in the meantime. Check it here so the builder gets a
    // real error at save time. Deliberately only the id check, not full
    // `Project::validate()`: that also requires at least one *enabled*
    // scenario, which would stop the user saving a work-in-progress draft.
    for sc in &project.scenarios {
        voidframe_engine::model::validate_id_segment(&sc.id)
            .map_err(|_| format!("invalid scenario id: {:?}", sc.id))?;
    }
    std::fs::create_dir_all(projects_dir).map_err(|e| e.to_string())?;
    let path = projects_dir.join(format!("{}.json", project.id));
    project.save(&path).map_err(|e| e.to_string())
}

pub(crate) fn delete_project_impl(projects_dir: &Path, id: &str) -> Result<(), String> {
    validate_project_id(id)?;
    let path = projects_dir.join(format!("{id}.json"));
    std::fs::remove_file(&path).map_err(|e| e.to_string())
}

/// `Ok(vec![])` for a fully valid project; `Err` only for `Project::validate`'s
/// own hard failures (unsupported module, no enabled scenario, bad settings).
/// M1 has no *soft* warning tier here yet (no conflict-but-not-fatal case
/// exists in the current `Project::validate`) — the `Vec<String>` return
/// shape is kept anyway so a future soft-warning addition (e.g. "this
/// scenario looks identical to another one") doesn't need a signature
/// change, matching docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §9.1's framing ("errors/warnings for the builder").
pub(crate) fn validate_scenario_impl(project: &Project) -> Result<Vec<String>, String> {
    project.validate().map_err(|e| e.to_string())?;
    Ok(vec![])
}

#[specta::specta]
#[tauri::command]
pub async fn list_projects(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<Vec<Project>, String> {
    let projects_dir = state.data_root.projects_dir();
    tokio::task::spawn_blocking(move || list_projects_impl(&projects_dir))
        .await
        .map_err(|e| super::join_error_to_string("list_projects", e))?
}

#[specta::specta]
#[tauri::command]
pub async fn get_project(
    state: tauri::State<'_, crate::state::AppState>,
    id: String,
) -> Result<Project, String> {
    let projects_dir = state.data_root.projects_dir();
    tokio::task::spawn_blocking(move || get_project_impl(&projects_dir, &id))
        .await
        .map_err(|e| super::join_error_to_string("get_project", e))?
}

#[specta::specta]
#[tauri::command]
pub async fn save_project(
    state: tauri::State<'_, crate::state::AppState>,
    project: Project,
) -> Result<(), String> {
    let projects_dir = state.data_root.projects_dir();
    tokio::task::spawn_blocking(move || save_project_impl(&projects_dir, &project))
        .await
        .map_err(|e| super::join_error_to_string("save_project", e))?
}

#[specta::specta]
#[tauri::command]
pub async fn delete_project(
    state: tauri::State<'_, crate::state::AppState>,
    id: String,
) -> Result<(), String> {
    let projects_dir = state.data_root.projects_dir();
    tokio::task::spawn_blocking(move || delete_project_impl(&projects_dir, &id))
        .await
        .map_err(|e| super::join_error_to_string("delete_project", e))?
}

#[specta::specta]
#[tauri::command]
pub async fn validate_scenario(project: Project) -> Result<Vec<String>, String> {
    validate_scenario_impl(&project)
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::model::project::{Baseline, Project};

    fn sample_project(id: &str) -> Project {
        Project {
            schema_version: voidframe_engine::model::SCHEMA_VERSION.into(),
            id: id.into(),
            name: format!("Project {id}"),
            description: "d".into(),
            created_at: "2026-09-01T00:00:00Z".into(),
            settings: serde_json::from_str("{}").unwrap(),
            baseline: Baseline {
                name: "Stock".into(),
                description: "d".into(),
            },
            scenarios: vec![voidframe_engine::model::project::Scenario {
                id: "s1".into(),
                name: "S1".into(),
                description: "d".into(),
                enabled: true,
                modules: vec![],
            }],
        }
    }

    #[test]
    fn save_then_list_then_get_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().to_path_buf();
        let p = sample_project("p1");
        save_project_impl(&projects_dir, &p).unwrap();

        let listed = list_projects_impl(&projects_dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "p1");

        let fetched = get_project_impl(&projects_dir, "p1").unwrap();
        assert_eq!(fetched, p);
    }

    #[test]
    fn get_of_missing_project_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(get_project_impl(dir.path(), "nope").is_err());
    }

    #[test]
    fn delete_removes_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().to_path_buf();
        save_project_impl(&projects_dir, &sample_project("p1")).unwrap();
        delete_project_impl(&projects_dir, "p1").unwrap();
        assert!(list_projects_impl(&projects_dir).unwrap().is_empty());
    }

    #[test]
    fn list_skips_files_that_fail_to_parse_rather_than_erroring_the_whole_list() {
        // One corrupt project.json (e.g. hand-edited, now invalid) must not
        // hide every OTHER valid project from the dashboard.
        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().to_path_buf();
        save_project_impl(&projects_dir, &sample_project("good")).unwrap();
        std::fs::write(projects_dir.join("corrupt.json"), "{not valid json").unwrap();
        let listed = list_projects_impl(&projects_dir).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "good");
    }

    #[test]
    fn validate_scenario_flags_no_enabled_scenarios_as_a_hard_error() {
        let mut p = sample_project("p1");
        p.scenarios[0].enabled = false;
        assert!(validate_scenario_impl(&p).is_err());
    }

    #[test]
    fn validate_scenario_passes_a_well_formed_project() {
        let p = sample_project("p1");
        assert_eq!(validate_scenario_impl(&p).unwrap(), Vec::<String>::new());
    }

    #[test]
    fn get_project_rejects_a_traversal_id() {
        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().to_path_buf();
        let err = get_project_impl(&projects_dir, "../../../../etc/passwd").unwrap_err();
        assert!(err.contains("invalid project id"));
    }

    #[test]
    fn delete_project_rejects_a_traversal_id() {
        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().to_path_buf();
        let err =
            delete_project_impl(&projects_dir, "..\\..\\Windows\\System32\\evil").unwrap_err();
        assert!(err.contains("invalid project id"));
    }

    #[test]
    fn save_project_rejects_a_traversal_scenario_id() {
        // The producing end of the path-traversal class: `Scenario.id` is
        // user-authored and `run::execute` later joins it into
        // `runs\<run_id>\journal-{id}.jsonl`, opened for writing by an
        // elevated process. Rejecting it at save time (as well as in the
        // engine's own `Project::validate`) means such a project never
        // reaches disk at all.
        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().to_path_buf();
        let mut p = sample_project("valid");
        p.scenarios[0].id = "..\\..\\..\\..\\Windows\\Temp\\evil".to_string();
        let err = save_project_impl(&projects_dir, &p).unwrap_err();
        assert!(err.contains("invalid scenario id"), "{err}");
        assert!(!projects_dir.join("valid.json").exists());
    }

    #[test]
    fn validate_scenario_rejects_a_traversal_scenario_id() {
        // Same guard, reached through the engine's own `Project::validate`
        // — which is what `voidframe-cli` and `Project::load` also go
        // through, so the protection is not Tauri-only.
        let mut p = sample_project("p1");
        p.scenarios[0].id = "../evil".to_string();
        let err = validate_scenario_impl(&p).unwrap_err();
        assert!(err.contains("invalid id"), "{err}");
    }

    #[test]
    fn save_project_rejects_a_project_with_a_traversal_id() {
        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().to_path_buf();
        let mut p = sample_project("valid");
        p.id = "../evil".to_string();
        assert!(save_project_impl(&projects_dir, &p).is_err());
    }
}
