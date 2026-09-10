//! Which of a run's journal files still describe mutations that are live on
//! the machine -- the shared selection rule behind the out-of-process
//! recovery paths (`recover::recover`'s deadman replay and
//! `restore_script::write_all`'s rendered `.bat`).
//!
//! A clean `revert_stage` neither clears a record's `applied` flag nor
//! deletes the journal file (`run::spawn::prune_run_dir` keeps journals for
//! forensics), so "every `journal-*.jsonl` in the run directory" is NOT the
//! set of mutations still applied: it also contains every scenario that
//! already completed its own apply -> measure -> revert cycle. Replaying
//! those a second time re-runs `custom_script` reverts (harmful for any
//! non-idempotent script), calls `delete_power_plan` on an already-deleted
//! GUID, and reports the resulting failures against a scenario that was never
//! stranded at all. `run::execute::rollback::in_flight_journal` documents the
//! same hazard for the in-process ROLLBACK; this module is that rule,
//! applied from the persisted cursor instead of the live one.

use crate::error::Result;
use crate::journal::Journal;
use crate::model::progress::{RunProgress, Stage};
use std::path::{Path, PathBuf};

/// Every `journal-*.jsonl` directly inside `run_dir`, sorted by file name.
pub fn journal_paths(run_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(run_dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("journal-") && n.ends_with(".jsonl"))
        })
        .collect();
    v.sort();
    Ok(v)
}

/// The subset of [`journal_paths`] whose mutations are still live on the
/// machine according to `progress`, in the same sorted order:
///
/// * the journal of the scenario `progress.cursor` names, when its stage is
///   `Apply` or `Measure` -- the two stages whose mutations are guaranteed
///   still applied with nothing else in the run scheduled to take them back
///   off (the exact rule `in_flight_journal` uses in-process; `Revert` is
///   deliberately excluded there and here, see its doc comment);
/// * plus any journal with an unconfirmed (`applied: false`) record -- an
///   apply interrupted between its journal line and its `mark_applied`, the
///   in-process `sweep_journals` case.
///
/// `progress: None` (no readable `progress.json`) returns every journal:
/// with no cursor to narrow by, over-reverting is the conservative choice.
pub fn live_journals(run_dir: &Path, progress: Option<&RunProgress>) -> Result<Vec<PathBuf>> {
    let all = journal_paths(run_dir)?;
    let Some(progress) = progress else {
        return Ok(all);
    };
    let in_flight = matches!(progress.cursor.stage, Stage::Apply | Stage::Measure)
        .then(|| {
            crate::run::execute::ordered_scenarios(&progress.project)
                .get(progress.cursor.index as usize)
                .map(|s| run_dir.join(format!("journal-{}.jsonl", s.id)))
        })
        .flatten();
    let mut live = Vec::new();
    for path in all {
        let is_in_flight = in_flight.as_deref() == Some(path.as_path());
        if is_in_flight || Journal::load_pending(&path)?.iter().any(|r| !r.applied) {
            live.push(path);
        }
    }
    Ok(live)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Op;
    use crate::model::progress::Cursor;
    use crate::model::project::{Baseline, Project, Scenario};
    use crate::system::{MutationCtx, PowerPlan};
    use serde_json::json;

    fn scenario(id: &str) -> Scenario {
        Scenario {
            id: id.into(),
            name: id.into(),
            description: "d".into(),
            enabled: true,
            modules: vec![],
        }
    }

    /// `ordered_scenarios` prepends the baseline dummy, so `a` is index 1
    /// and `b` is index 2.
    fn progress_at(index: u32, stage: Stage) -> RunProgress {
        RunProgress {
            schema_version: crate::model::SCHEMA_VERSION.to_string(),
            run_id: "r1".into(),
            project: Project {
                schema_version: crate::model::SCHEMA_VERSION.to_string(),
                id: "p1".into(),
                name: "P".into(),
                description: "d".into(),
                created_at: "2026-09-10T00:00:00Z".into(),
                settings: serde_json::from_str("{}").unwrap(),
                baseline: Baseline {
                    name: "Stock".into(),
                    description: "d".into(),
                },
                scenarios: vec![scenario("a"), scenario("b")],
            },
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
            cursor: Cursor { index, stage },
            reboot: None,
            shutdown_when_complete: false,
            skip_revert_once: false,
            abort_requested: false,
        }
    }

    /// One `PowercfgWrite` record per journal, confirmed or not.
    fn write_journal(run_dir: &Path, scenario_id: &str, applied: bool) -> PathBuf {
        let path = run_dir.join(format!("journal-{scenario_id}.jsonl"));
        let mut j = Journal::open(&path).unwrap();
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
                json!({"sub":"sub_processor","setting":"IDLEDISABLE","value":0}),
            )
            .unwrap();
        if applied {
            j.mark_applied(seq).unwrap();
        }
        path
    }

    #[test]
    fn without_progress_every_journal_is_live() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_journal(dir.path(), "a", true);
        let b = write_journal(dir.path(), "b", true);
        assert_eq!(live_journals(dir.path(), None).unwrap(), vec![a, b]);
    }

    /// The deadman's real case: scenario `a` completed its own revert, `b`
    /// was applied and the machine rebooted -- only `b` is still live.
    #[test]
    fn a_completed_scenarios_journal_is_not_live_while_the_cursor_scenarios_is() {
        let dir = tempfile::tempdir().unwrap();
        write_journal(dir.path(), "a", true);
        let b = write_journal(dir.path(), "b", true);
        assert_eq!(
            live_journals(dir.path(), Some(&progress_at(2, Stage::Measure))).unwrap(),
            vec![b]
        );
    }

    #[test]
    fn stage_apply_counts_as_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_journal(dir.path(), "a", true);
        assert_eq!(
            live_journals(dir.path(), Some(&progress_at(1, Stage::Apply))).unwrap(),
            vec![a]
        );
    }

    /// Same scoping decision as `in_flight_journal`: an interruption during
    /// `revert_stage` is ambiguous and out of scope.
    #[test]
    fn stage_revert_and_done_are_not_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        write_journal(dir.path(), "a", true);
        for stage in [Stage::Revert, Stage::Done] {
            assert!(
                live_journals(dir.path(), Some(&progress_at(1, stage)))
                    .unwrap()
                    .is_empty(),
                "{stage:?}"
            );
        }
    }

    /// An unconfirmed record marks its journal live regardless of the
    /// cursor -- the `sweep_journals` case.
    #[test]
    fn an_unconfirmed_record_makes_any_journal_live() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_journal(dir.path(), "a", false);
        write_journal(dir.path(), "b", true);
        assert_eq!(
            live_journals(dir.path(), Some(&progress_at(2, Stage::Done))).unwrap(),
            vec![a]
        );
    }
}
