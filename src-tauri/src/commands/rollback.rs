//! Crash recovery: `rollback_now` (targets whatever the store's `RunState`
//! snapshot says was active) and `emergency_rollback` (the always-available
//! panic button — scans disk for the most-recently-modified run directory
//! and reverts every journal in it, without needing any live store state).
//!
//! Both commands must also resolve the crash-recovery `RunState` itself once
//! they're done — `run_store.rs`'s `RunComplete`/`RunFailed` handling is the
//! *only* other place this store ever gets cleared, so a run that instead
//! ends by being killed (no terminal event ever fires) has no other path
//! back to a clean state. Confirmed live 2026-09-05: without this, rolling
//! back a crashed run — even successfully — left `refuse_if_unresolved_run_state`
//! (`commands/run.rs`) permanently refusing every subsequent `start_run`,
//! with no way out except editing `state/current.json` by hand.

use std::path::Path;
use tokio::sync::Mutex as AsyncMutex;
use voidframe_engine::journal::replay::{RevertReport, revert_all};
use voidframe_engine::store::RunState;
use voidframe_engine::system::SystemController;

use super::projects::validate_project_id;
use crate::state::ActiveRun;

/// Shared by both `rollback_now` and `emergency_rollback`: replaying a
/// journal while a run is still applying or reverting mutations races two
/// writers against the same journal/state (the run's own executor and this
/// rollback), so both commands refuse outright rather than proceeding
/// concurrently.
///
/// Returns the held `MutexGuard` rather than dropping it after the check --
/// the caller must keep it alive for the entire `revert_all` call, not just
/// this initial check. A check-then-release-then-act version of this
/// (this function's original shape) left two real windows open: a
/// `start_run` call landing after the check but before/during `revert_all`
/// could apply real mutations while this rollback is reading/reverting the
/// same `SystemController`/journal state, and two overlapping
/// `rollback_now`/`emergency_rollback` calls (e.g. a double-click before the
/// UI disables the button) could both pass the check and both call
/// `revert_all` on the same journal file concurrently. Holding the lock for
/// the whole operation closes both: `start_run`'s own atomic
/// check-and-reserve (`commands/run.rs`) needs this exact same lock, so it
/// now blocks for as long as a rollback is in flight instead of racing it,
/// and a second overlapping rollback call blocks on this same lock instead
/// of running its own `revert_all` concurrently with the first.
async fn reject_if_run_active(
    active_run: &AsyncMutex<Option<ActiveRun>>,
) -> Result<tokio::sync::MutexGuard<'_, Option<ActiveRun>>, String> {
    let guard = active_run.lock().await;
    if guard.is_some() {
        return Err(
            "a run is currently active -- cannot roll back while it is applying or reverting \
             mutations. Stop or wait for the run to finish first."
                .to_string(),
        );
    }
    Ok(guard)
}

pub(crate) async fn rollback_now_impl(
    sys: &dyn SystemController,
    snapshot: Option<RunState>,
    data_root: &Path,
    active_run: &AsyncMutex<Option<ActiveRun>>,
) -> Result<RevertReport, String> {
    // Held until this function returns -- see `reject_if_run_active`'s doc
    // comment for why the guard, not just the initial check, must span the
    // whole operation.
    let _active_run_guard = reject_if_run_active(active_run).await?;
    let snapshot = snapshot.ok_or_else(|| "no run state to roll back".to_string())?;
    // Hard error, NOT a `.unwrap_or("baseline")` fallback. That fallback
    // used to fire on literally every call (nothing ever wrote
    // `current_scenario` -- see `commands::run_store::update_store_from_event`), so
    // `rollback_now` always reverted `journal-baseline.jsonl` regardless of
    // which scenario actually crashed, and reported `reverted: 0` -- which
    // reads as success. Now that `current_scenario` is genuinely populated
    // at every scenario boundary, a `None` here can only mean the run
    // crashed before BASELINE ever started (PREFLIGHT/SNAPSHOT), where no
    // journal exists to revert anyway. Failing loudly means any FUTURE
    // regression in the write path surfaces as an error instead of silently
    // reverting the wrong file again.
    let scenario_id = snapshot.current_scenario.as_deref().ok_or_else(|| {
        "run state records no current scenario -- the run did not reach its baseline phase, so \
         there are no applied mutations to roll back. Use Emergency Restore if you believe \
         otherwise."
            .to_string()
    })?;
    // Both of these are disk-sourced (read back out of `state/current.json`,
    // which a non-elevated process can rewrite) and both get joined into a
    // filesystem path this elevated process then opens. `scenario_id`
    // ultimately traces back to `Scenario.id` in a project.json -- now also
    // validated at the producing end, in `Project::validate` -- and
    // `run_id` to `start_run`'s own uuid. Validate both here regardless:
    // this is the read side, and it must not depend on the writer having
    // been well-behaved.
    validate_project_id(scenario_id)?;
    validate_project_id(&snapshot.run_id)?;
    let journal_path = data_root
        .join("runs")
        .join(&snapshot.run_id)
        .join(format!("journal-{scenario_id}.jsonl"));
    revert_all(&journal_path, sys)
        .await
        .map_err(|e| e.to_string())
}

/// A journal file name looks like `journal-<scenario_id>.jsonl`.
fn is_journal_name(name: &str) -> bool {
    name.starts_with("journal-") && name.ends_with(".jsonl")
}

/// Lists every `journal-*.jsonl` file directly inside `dir`.
fn journals_in(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut journals = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        let is_journal = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(is_journal_name)
            .unwrap_or(false);
        if is_journal {
            journals.push(path);
        }
    }
    Ok(journals)
}

/// The purely synchronous half of [`emergency_rollback_impl`]: scans
/// `runs_dir` for the most-recently-modified run directory that contains at
/// least one `journal-*.jsonl` file, then lists every journal inside it.
/// Run directories with no journal (a preflight-blocked run, or an aborted
/// run -- both create only `run.log`) are skipped rather than trusted just
/// for being newest. Split out so the whole filesystem scan -- two
/// `std::fs::read_dir` passes plus a `metadata()` per entry -- can run as a
/// single `spawn_blocking` closure, off the async command's own task.
fn find_most_recent_run_journals(runs_dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let entries = std::fs::read_dir(runs_dir).map_err(|e| e.to_string())?;
    let mut qualifying_dirs: Vec<(std::path::PathBuf, std::time::SystemTime)> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            let path = entry.path();
            // An unreadable candidate (a transient sharing violation, or a
            // run directory removed concurrently with this scan) does not
            // qualify -- it must not abort the whole scan, or the panic
            // button restores nothing (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes/final-fix-brief.md Important 2).
            if journals_in(&path).map(|j| j.is_empty()).unwrap_or(true) {
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            qualifying_dirs.push((path, modified));
        }
    }
    let (most_recent, _) = qualifying_dirs
        .into_iter()
        .max_by_key(|(_, m)| *m)
        .ok_or_else(|| "no runs found to roll back".to_string())?;

    journals_in(&most_recent)
}

pub(crate) async fn emergency_rollback_impl(
    sys: &dyn SystemController,
    runs_dir: &Path,
    active_run: &AsyncMutex<Option<ActiveRun>>,
) -> Result<Vec<RevertReport>, String> {
    // Held until this function returns -- see `reject_if_run_active`'s doc
    // comment for why the guard, not just the initial check, must span the
    // whole operation (including every `revert_all` call in the loop below).
    let _active_run_guard = reject_if_run_active(active_run).await?;

    let runs_dir = runs_dir.to_path_buf();
    let journal_paths =
        tokio::task::spawn_blocking(move || find_most_recent_run_journals(&runs_dir))
            .await
            .map_err(|e| super::join_error_to_string("emergency_rollback scan", e))??;

    let mut reports = Vec::new();
    for path in journal_paths {
        reports.push(revert_all(&path, sys).await.map_err(|e| e.to_string())?);
    }
    Ok(reports)
}

/// Whether a rollback's report(s) are clean enough to also resolve the
/// crash-recovery `RunState` — every entry actually reverted (or had
/// nothing to revert) with no verify failures. Split out so this decision
/// is unit-testable without a real `StoreHandle`.
///
/// Deliberately conservative: if reverting genuinely failed to verify some
/// entry, the system may still be sitting in a modified state, and clearing
/// the crash-recovery flag anyway would let a new run start over that
/// unacknowledged — leaving the flag in place forces the user (or another
/// Emergency Restore attempt) to notice, matching this app's own
/// journaled-for-rollback safety posture rather than silently papering over
/// it.
fn revert_was_clean(reports: &[RevertReport]) -> bool {
    reports.iter().all(|r| r.verify_failures.is_empty())
}

#[specta::specta]
#[tauri::command]
pub async fn rollback_now(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<RevertReport, String> {
    let snapshot = state.store.snapshot().await.map_err(|e| e.to_string())?;
    let report = rollback_now_impl(
        state.sys.as_ref(),
        snapshot,
        state.data_root.path(),
        &state.active_run,
    )
    .await?;
    // See this module's own doc comment: a rollback is how a crashed run's
    // recovery state gets resolved, mirroring `run_store.rs`'s
    // RunComplete/RunFailed handling -- the only other place this ever
    // happens.
    if revert_was_clean(std::slice::from_ref(&report)) {
        let _ = state.store.clear().await;
    }
    Ok(report)
}

#[specta::specta]
#[tauri::command]
pub async fn emergency_rollback(
    state: tauri::State<'_, crate::state::AppState>,
) -> Result<Vec<RevertReport>, String> {
    let reports = emergency_rollback_impl(
        state.sys.as_ref(),
        &state.data_root.runs_dir(),
        &state.active_run,
    )
    .await?;
    if revert_was_clean(&reports) {
        let _ = state.store.clear().await;
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::run::Phase;
    use voidframe_engine::system::MockController;

    /// A fresh, never-armed `active_run` -- the common case for every test
    /// in this module that isn't specifically exercising the active-run
    /// refusal itself.
    fn no_active_run() -> AsyncMutex<Option<ActiveRun>> {
        AsyncMutex::new(None)
    }

    #[test]
    fn revert_was_clean_is_true_for_an_empty_report_list() {
        assert!(revert_was_clean(&[]));
    }

    #[test]
    fn revert_was_clean_is_true_when_every_report_has_no_verify_failures() {
        let reports = vec![
            RevertReport {
                reverted: 0,
                verify_failures: vec![],
            },
            RevertReport {
                reverted: 3,
                verify_failures: vec![],
            },
        ];
        assert!(revert_was_clean(&reports));
    }

    #[test]
    fn revert_was_clean_is_false_when_any_report_has_a_verify_failure() {
        let reports = vec![
            RevertReport {
                reverted: 2,
                verify_failures: vec![],
            },
            RevertReport {
                reverted: 1,
                verify_failures: vec![
                    "registry value did not read back the expected inverse".into(),
                ],
            },
        ];
        assert!(!revert_was_clean(&reports));
    }

    #[tokio::test]
    async fn rollback_now_with_no_active_run_state_is_an_error() {
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        assert!(
            rollback_now_impl(&sys, None, dir.path(), &no_active_run())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn rollback_now_reverts_the_snapshotted_runs_scenario_journal() {
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let journal_path = run_dir.join("journal-s1.jsonl");
        std::fs::write(&journal_path, "").unwrap(); // an empty, already-reverted journal is a valid input -- revert_all on empty input is a no-op success, already proven by journal::replay's own tests

        let snapshot = voidframe_engine::store::RunState {
            schema_version: "1.1.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            phase: Phase::Scenario { id: "s1".into() },
            current_scenario: Some("s1".into()),
            completed_scenarios: vec![],
            revision: 3,
        };
        let report = rollback_now_impl(&sys, Some(snapshot), dir.path(), &no_active_run())
            .await
            .unwrap();
        assert_eq!(report.reverted, 0); // empty journal, nothing to revert -- still Ok, not an error
    }

    #[tokio::test]
    async fn rollback_now_rejects_a_traversal_scenario_id_in_the_snapshot() {
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let snapshot = voidframe_engine::store::RunState {
            schema_version: "1.1.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            phase: Phase::Scenario { id: "s1".into() },
            current_scenario: Some("../../evil".into()),
            completed_scenarios: vec![],
            revision: 1,
        };
        let err = rollback_now_impl(&sys, Some(snapshot), dir.path(), &no_active_run())
            .await
            .unwrap_err();
        assert!(err.contains("invalid project id")); // validate_project_id's own error message
    }

    #[tokio::test]
    async fn rollback_now_targets_the_scenario_the_snapshot_names_not_always_baseline() {
        // Regression: `current_scenario` was never written, so this used to
        // fall back to "baseline" every time and revert the WRONG journal,
        // reporting `reverted: 0` (which reads as success). Both journals
        // exist here; only the named one may be opened, so a regression
        // that silently retargets baseline still finds a file and would go
        // unnoticed without asserting on the record count.
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        // An empty (valid, already-reverted) baseline journal, and a
        // scenario journal with one real revertible record. If the wrong
        // file is picked, `reverted` is 0 instead of 1.
        std::fs::write(run_dir.join("journal-baseline.jsonl"), "").unwrap();
        let mut journal =
            voidframe_engine::journal::Journal::open(&run_dir.join("journal-s2.jsonl")).unwrap();
        voidframe_engine::mutation::apply_module(
            &voidframe_engine::model::Module::Powercfg {
                sub: "sub_processor".into(),
                setting: "IDLEDISABLE".into(),
                value: 1,
            },
            &sys,
            &mut journal,
            &voidframe_engine::system::MutationCtx {
                run_id: "r1".into(),
                scenario_id: "s2".into(),
                step_index: 0,
            },
        )
        .await
        .unwrap();
        drop(journal);

        let snapshot = voidframe_engine::store::RunState {
            schema_version: "1.1.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            phase: Phase::Scenario { id: "s2".into() },
            current_scenario: Some("s2".into()),
            completed_scenarios: vec![],
            revision: 3,
        };
        let report = rollback_now_impl(&sys, Some(snapshot), dir.path(), &no_active_run())
            .await
            .unwrap();
        assert_eq!(
            report.reverted, 1,
            "rolled back the wrong journal -- s2's record was not reverted"
        );
    }

    #[tokio::test]
    async fn rollback_now_with_no_current_scenario_is_a_hard_error_not_a_silent_baseline_revert() {
        // `current_scenario == None` now means only one thing: the run
        // crashed before BASELINE began, so nothing was ever applied. The
        // old `.unwrap_or("baseline")` fallback turned that into a silent
        // revert of an unrelated journal.
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("journal-baseline.jsonl"), "").unwrap();

        let snapshot = voidframe_engine::store::RunState {
            schema_version: "1.1.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            phase: Phase::Preflight,
            current_scenario: None,
            completed_scenarios: vec![],
            revision: 1,
        };
        let err = rollback_now_impl(&sys, Some(snapshot), dir.path(), &no_active_run())
            .await
            .unwrap_err();
        assert!(err.contains("no current scenario"), "{err}");
    }

    #[tokio::test]
    async fn rollback_now_rejects_a_traversal_run_id_in_the_snapshot() {
        // `run_id` is read back out of `state/current.json`, which lives
        // under `%LOCALAPPDATA%` and is writable by the *unelevated* user --
        // so it gets the same treatment as `scenario_id`, not a pass on the
        // grounds that `start_run` generates a uuid.
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let snapshot = voidframe_engine::store::RunState {
            schema_version: "1.1.0".into(),
            run_id: "..\\..\\Windows\\Temp".into(),
            project_id: "p1".into(),
            phase: Phase::Baseline,
            current_scenario: Some("baseline".into()),
            completed_scenarios: vec![],
            revision: 1,
        };
        let err = rollback_now_impl(&sys, Some(snapshot), dir.path(), &no_active_run())
            .await
            .unwrap_err();
        assert!(err.contains("invalid project id"), "{err}");
    }

    #[tokio::test]
    async fn emergency_rollback_reverts_every_journal_in_the_most_recent_run_dir() {
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().join("runs");
        let run_dir = runs_dir.join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("journal-baseline.jsonl"), "").unwrap();
        std::fs::write(run_dir.join("journal-s1.jsonl"), "").unwrap();
        std::fs::write(run_dir.join("results.json"), "{}").unwrap(); // a non-journal file present must not break the scan

        let reports = emergency_rollback_impl(&sys, &runs_dir, &no_active_run())
            .await
            .unwrap();
        assert_eq!(reports.len(), 2);
    }

    #[tokio::test]
    async fn emergency_rollback_skips_a_newer_run_dir_that_holds_no_journal() {
        // Regression: since every run now writes `runs/<run_id>/run.log` at
        // its very first event, a preflight-blocked or aborted run leaves
        // behind a run directory with no journal at all -- and it can
        // easily be the newest by mtime. Picking it blindly used to make
        // `emergency_rollback` return `Ok(vec![])` while an older run's
        // journals stayed unreverted.
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().join("runs");

        let old_dir = runs_dir.join("old");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("journal-s1.jsonl"), "").unwrap();

        // Directory mtime on Windows NTFS updates when an entry is added
        // inside it, so create `new`'s contents strictly after `old`'s --
        // with no `filetime` dependency in `src-tauri` to set mtimes
        // explicitly, a short sleep keeps the ordering unambiguous across
        // mtime-granularity filesystems.
        std::thread::sleep(std::time::Duration::from_millis(20));

        let new_dir = runs_dir.join("new");
        std::fs::create_dir_all(&new_dir).unwrap();
        std::fs::write(new_dir.join("run.log"), "").unwrap(); // no journal -- must be skipped

        let reports = emergency_rollback_impl(&sys, &runs_dir, &no_active_run())
            .await
            .unwrap();
        assert_eq!(
            reports.len(),
            1,
            "expected the one report from `old`'s journal, not an empty vec from picking `new`"
        );
    }

    /// Important 2 (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes/final-fix-brief.md):
    /// a candidate run directory that exists but can't be read (a transient
    /// sharing violation, or one removed concurrently with this scan) must
    /// not abort the whole scan -- it must simply not qualify, leaving an
    /// older, genuinely readable run dir to be picked instead. A dangling
    /// directory symlink (its target never exists) reproduces this: Windows
    /// reports it as a directory (so it passes the `is_dir()` check), but
    /// `read_dir` on it fails.
    #[tokio::test]
    async fn emergency_rollback_tolerates_an_unreadable_candidate_dir() {
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().join("runs");
        std::fs::create_dir_all(&runs_dir).unwrap();

        let old_dir = runs_dir.join("old");
        std::fs::create_dir_all(&old_dir).unwrap();
        std::fs::write(old_dir.join("journal-s1.jsonl"), "").unwrap();

        // If this environment can't create directory symlinks at all (no
        // Developer Mode / no privilege), skip rather than fail on an
        // environment limitation unrelated to the behavior under test --
        // the two-line fix itself was verified by reading, per the brief.
        let broken = runs_dir.join("broken");
        if std::os::windows::fs::symlink_dir(runs_dir.join("does-not-exist"), &broken).is_err() {
            eprintln!(
                "skipping emergency_rollback_tolerates_an_unreadable_candidate_dir: this \
                 environment cannot create directory symlinks"
            );
            return;
        }

        let reports = emergency_rollback_impl(&sys, &runs_dir, &no_active_run())
            .await
            .unwrap();
        assert_eq!(
            reports.len(),
            1,
            "the unreadable 'broken' candidate must be skipped, not abort the whole scan -- \
             'old's journal should still be found"
        );
    }

    #[tokio::test]
    async fn emergency_rollback_with_only_journal_less_run_dirs_is_an_error() {
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().join("runs");
        let run_dir = runs_dir.join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        std::fs::write(run_dir.join("run.log"), "").unwrap();

        let err = emergency_rollback_impl(&sys, &runs_dir, &no_active_run())
            .await
            .unwrap_err();
        assert!(err.contains("no runs found to roll back"), "{err}");
    }

    #[tokio::test]
    async fn emergency_rollback_with_no_runs_at_all_is_an_error_not_an_empty_vec() {
        // Distinguishing "nothing to roll back" (silently fine) from "the
        // runs directory itself is missing/unreadable" (worth surfacing)
        // matters for a panic-button command specifically -- a user who
        // hits Emergency Restore wants to KNOW if it found nothing to do,
        // not see it silently report success.
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().join("runs"); // never created
        assert!(
            emergency_rollback_impl(&sys, &runs_dir, &no_active_run())
                .await
                .is_err()
        );
    }

    /// H1 (active-run guard): replaying a journal while a run is still
    /// applying/reverting mutations races two writers against the same
    /// journal/state. Both rollback commands must refuse outright rather
    /// than proceed concurrently -- mirrors `send_control`'s own
    /// `AsyncMutex<Option<ActiveRun>>` fixture pattern in `commands/run.rs`.
    #[tokio::test]
    async fn rollback_now_refuses_while_a_run_is_active() {
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let (control_tx, _control_rx) = tokio::sync::mpsc::channel(1);
        let active_run = AsyncMutex::new(Some(ActiveRun {
            run_id: "r1".into(),
            control_tx,
        }));

        let snapshot = voidframe_engine::store::RunState {
            schema_version: "1.1.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            phase: Phase::Baseline,
            current_scenario: Some("baseline".into()),
            completed_scenarios: vec![],
            revision: 1,
        };
        let err = rollback_now_impl(&sys, Some(snapshot), dir.path(), &active_run)
            .await
            .unwrap_err();
        assert!(err.contains("currently active"), "{err}");
    }

    #[tokio::test]
    async fn emergency_rollback_refuses_while_a_run_is_active() {
        let sys = MockController::new();
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().join("runs"); // never created -- must not even be read
        let (control_tx, _control_rx) = tokio::sync::mpsc::channel(1);
        let active_run = AsyncMutex::new(Some(ActiveRun {
            run_id: "r1".into(),
            control_tx,
        }));

        let err = emergency_rollback_impl(&sys, &runs_dir, &active_run)
            .await
            .unwrap_err();
        assert!(err.contains("currently active"), "{err}");
    }

    /// Wraps a `MockController`, delaying `write_powercfg` by `delay` before
    /// delegating -- the one call `revert_all` makes while reverting the
    /// single-record Powercfg journals the two exclusivity tests below
    /// build. Makes `revert_all`'s in-flight duration a real, measurable
    /// sleep the tests can race against, rather than something assumed to
    /// take nonzero time. Every other method is a plain pass-through,
    /// mirroring `SystemController`'s own blanket `Arc<T>` impl in
    /// `voidframe_engine::system`.
    #[derive(Clone)]
    struct SlowController {
        inner: MockController,
        delay: std::time::Duration,
    }

    #[async_trait::async_trait]
    impl SystemController for SlowController {
        async fn read_registry(
            &self,
            k: &voidframe_engine::system::RegKey,
        ) -> voidframe_engine::error::Result<voidframe_engine::system::RegValue> {
            self.inner.read_registry(k).await
        }
        async fn write_registry(
            &self,
            k: &voidframe_engine::system::RegKey,
            v: &voidframe_engine::system::RegValue,
            c: &voidframe_engine::system::MutationCtx,
        ) -> voidframe_engine::error::Result<()> {
            self.inner.write_registry(k, v, c).await
        }
        async fn delete_registry_value(
            &self,
            k: &voidframe_engine::system::RegKey,
            c: &voidframe_engine::system::MutationCtx,
        ) -> voidframe_engine::error::Result<()> {
            self.inner.delete_registry_value(k, c).await
        }
        async fn read_powercfg(
            &self,
            sub: &str,
            setting: &str,
        ) -> voidframe_engine::error::Result<voidframe_engine::system::AcDc<u32>> {
            self.inner.read_powercfg(sub, setting).await
        }
        async fn write_powercfg(
            &self,
            sub: &str,
            setting: &str,
            v: u32,
            c: &voidframe_engine::system::MutationCtx,
        ) -> voidframe_engine::error::Result<()> {
            tokio::time::sleep(self.delay).await;
            self.inner.write_powercfg(sub, setting, v, c).await
        }
        async fn list_power_plans(
            &self,
        ) -> voidframe_engine::error::Result<Vec<voidframe_engine::system::PowerPlan>> {
            self.inner.list_power_plans().await
        }
        async fn active_power_plan(
            &self,
        ) -> voidframe_engine::error::Result<voidframe_engine::system::PowerPlan> {
            self.inner.active_power_plan().await
        }
        async fn set_active_power_plan(
            &self,
            guid: &str,
            c: &voidframe_engine::system::MutationCtx,
        ) -> voidframe_engine::error::Result<()> {
            self.inner.set_active_power_plan(guid, c).await
        }
        async fn duplicate_power_plan(
            &self,
            template_guid: &str,
            c: &voidframe_engine::system::MutationCtx,
        ) -> voidframe_engine::error::Result<voidframe_engine::system::PowerPlan> {
            self.inner.duplicate_power_plan(template_guid, c).await
        }
        async fn delete_power_plan(
            &self,
            guid: &str,
            c: &voidframe_engine::system::MutationCtx,
        ) -> voidframe_engine::error::Result<()> {
            self.inner.delete_power_plan(guid, c).await
        }
        async fn cpu_topology(
            &self,
        ) -> voidframe_engine::error::Result<voidframe_engine::system::CpuTopology> {
            self.inner.cpu_topology().await
        }
        async fn set_process_affinity(
            &self,
            pid: u32,
            mask: u64,
        ) -> voidframe_engine::error::Result<()> {
            self.inner.set_process_affinity(pid, mask).await
        }
        async fn gpu_model(&self) -> voidframe_engine::error::Result<String> {
            self.inner.gpu_model().await
        }
        async fn steam_status(
            &self,
        ) -> voidframe_engine::error::Result<voidframe_engine::system::SteamStatus> {
            self.inner.steam_status().await
        }
        async fn read_cs2_launch_options(&self) -> voidframe_engine::error::Result<String> {
            self.inner.read_cs2_launch_options().await
        }
        async fn write_cs2_launch_options(
            &self,
            args: &str,
        ) -> voidframe_engine::error::Result<()> {
            self.inner.write_cs2_launch_options(args).await
        }
        async fn steam_install_path(&self) -> voidframe_engine::error::Result<std::path::PathBuf> {
            self.inner.steam_install_path().await
        }
        async fn app_library_path(
            &self,
            app_id: u32,
        ) -> voidframe_engine::error::Result<std::path::PathBuf> {
            self.inner.app_library_path(app_id).await
        }
        async fn app_manifest(
            &self,
            app_id: u32,
        ) -> voidframe_engine::error::Result<Option<voidframe_engine::system::AppManifest>>
        {
            self.inner.app_manifest(app_id).await
        }
        async fn workshop_item_installed(
            &self,
            app_id: u32,
            item_id: &str,
        ) -> voidframe_engine::error::Result<bool> {
            self.inner.workshop_item_installed(app_id, item_id).await
        }
        async fn free_disk_bytes(&self) -> voidframe_engine::error::Result<u64> {
            self.inner.free_disk_bytes().await
        }
        async fn launch_cs2(
            &self,
            spec: &voidframe_engine::system::Cs2LaunchSpec,
        ) -> voidframe_engine::error::Result<()> {
            self.inner.launch_cs2(spec).await
        }
        async fn launch_deelevated(
            &self,
            program: &str,
            args: &str,
        ) -> voidframe_engine::error::Result<()> {
            self.inner.launch_deelevated(program, args).await
        }
        async fn find_process(
            &self,
            name: &str,
        ) -> voidframe_engine::error::Result<Option<voidframe_engine::system::ProcHandle>> {
            self.inner.find_process(name).await
        }
        async fn suspend_process_tree(&self, pid: u32) -> voidframe_engine::error::Result<()> {
            self.inner.suspend_process_tree(pid).await
        }
        async fn resume_process_tree(&self, pid: u32) -> voidframe_engine::error::Result<()> {
            self.inner.resume_process_tree(pid).await
        }
        async fn kill_process_tree(&self, pid: u32) -> voidframe_engine::error::Result<()> {
            self.inner.kill_process_tree(pid).await
        }
        async fn reissue_map(&self, map_command: &str) -> voidframe_engine::error::Result<()> {
            self.inner.reissue_map(map_command).await
        }
        async fn hide_console(&self) -> voidframe_engine::error::Result<()> {
            self.inner.hide_console().await
        }
        async fn quit_cs2_gracefully(&self) -> voidframe_engine::error::Result<()> {
            self.inner.quit_cs2_gracefully().await
        }
        async fn process_started_at(&self, pid: u32) -> Option<std::time::SystemTime> {
            self.inner.process_started_at(pid).await
        }
        async fn close_steam_window(&self) -> voidframe_engine::error::Result<()> {
            self.inner.close_steam_window().await
        }
        async fn hwinfo_already_running(&self) -> voidframe_engine::error::Result<bool> {
            self.inner.hwinfo_already_running().await
        }
        async fn start_hwinfo(&self, path: &Path) -> voidframe_engine::error::Result<()> {
            self.inner.start_hwinfo(path).await
        }
        async fn close_hwinfo(&self) -> voidframe_engine::error::Result<()> {
            self.inner.close_hwinfo().await
        }
        async fn read_hwinfo_sensors(
            &self,
        ) -> voidframe_engine::error::Result<voidframe_engine::system::HwinfoSensorSnapshot>
        {
            self.inner.read_hwinfo_sensors().await
        }
    }

    /// Builds a one-record Powercfg journal at `runs/r1/journal-s1.jsonl`
    /// under `dir`, against `mock` -- the fixture both exclusivity tests
    /// below share.
    async fn seed_one_record_journal(dir: &Path, mock: &MockController) {
        let run_dir = dir.join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let journal_path = run_dir.join("journal-s1.jsonl");
        let mut journal = voidframe_engine::journal::Journal::open(&journal_path).unwrap();
        voidframe_engine::mutation::apply_module(
            &voidframe_engine::model::Module::Powercfg {
                sub: "sub_processor".into(),
                setting: "IDLEDISABLE".into(),
                value: 1,
            },
            mock,
            &mut journal,
            &voidframe_engine::system::MutationCtx {
                run_id: "r1".into(),
                scenario_id: "s1".into(),
                step_index: 0,
            },
        )
        .await
        .unwrap();
    }

    /// Regression proof for the Stage C review's residual gap: M3's guard
    /// only checked `active_run` once at the top of each function, then
    /// released it before calling `revert_all` -- leaving a real window for
    /// a `start_run` call to land mid-revert and apply real mutations while
    /// this rollback is still reading/reverting the same
    /// `SystemController`/journal state. Multi-threaded so the spawned
    /// rollback genuinely runs concurrently with this task rather than
    /// depending on single-threaded cooperative scheduling order.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rollback_now_holds_active_run_for_the_entire_revert_blocking_a_concurrent_start_run_reservation()
     {
        let dir = tempfile::tempdir().unwrap();
        let mock = MockController::new().with_powercfg(
            "sub_processor",
            "IDLEDISABLE",
            voidframe_engine::system::AcDc { ac: 0, dc: 0 },
        );
        seed_one_record_journal(dir.path(), &mock).await;

        let sys = SlowController {
            inner: mock,
            delay: std::time::Duration::from_millis(200),
        };
        let snapshot = voidframe_engine::store::RunState {
            schema_version: "1.1.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            phase: Phase::Baseline,
            current_scenario: Some("s1".into()),
            completed_scenarios: vec![],
            revision: 1,
        };

        let active_run = std::sync::Arc::new(AsyncMutex::new(None));
        let active_for_rollback = active_run.clone();
        let data_root = dir.path().to_path_buf();
        let rollback_task = tokio::spawn(async move {
            rollback_now_impl(&sys, Some(snapshot), &data_root, &active_for_rollback).await
        });

        // Give the spawned rollback time to pass the initial check and
        // reach the deliberately slow `write_powercfg` inside `revert_all`.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Simulate `start_run`'s own atomic check-and-reserve
        // (`commands/run.rs`), which needs this exact same lock. With the
        // fix, it must still be blocked here -- the rollback is genuinely
        // still in flight, not just past its initial check.
        let reservation_attempt =
            tokio::time::timeout(std::time::Duration::from_millis(50), active_run.lock()).await;
        assert!(
            reservation_attempt.is_err(),
            "a concurrent start_run reservation acquired active_run's lock while rollback_now \
             was still reverting -- the guard is not held for the whole operation"
        );

        let report = rollback_task.await.unwrap().unwrap();
        assert_eq!(report.reverted, 1);

        // Once the revert has genuinely finished, the lock must be free
        // immediately -- proving this isn't a permanent hold, but one
        // that's released as soon as the operation actually completes.
        let reservation_after =
            tokio::time::timeout(std::time::Duration::from_millis(50), active_run.lock()).await;
        assert!(
            reservation_after.is_ok(),
            "active_run's lock was not released once rollback_now finished"
        );
    }

    /// Regression proof for the second race the Stage C review found: two
    /// overlapping rollback calls (e.g. a double-click before the UI
    /// disables the button) used to both pass `reject_if_run_active`'s
    /// check and both call `revert_all` concurrently on the same journal.
    /// With the fix, the second call blocks on the same `active_run` lock
    /// until the first's `revert_all` has genuinely finished, then runs
    /// (and succeeds) itself -- proven here by timing: if the two ran
    /// concurrently, the whole test would take about one `delay`'s worth of
    /// wall-clock time; serialized behind the same lock, it takes noticeably
    /// more than that.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn two_concurrent_emergency_rollback_calls_do_not_revert_the_same_journal_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let runs_dir = dir.path().join("runs");
        let mock = MockController::new().with_powercfg(
            "sub_processor",
            "IDLEDISABLE",
            voidframe_engine::system::AcDc { ac: 0, dc: 0 },
        );
        seed_one_record_journal(dir.path(), &mock).await;

        let delay = std::time::Duration::from_millis(150);
        let active_run = std::sync::Arc::new(AsyncMutex::new(None));

        let sys_a = SlowController {
            inner: mock.clone(),
            delay,
        };
        let active_a = active_run.clone();
        let runs_dir_a = runs_dir.clone();
        let task_a =
            tokio::spawn(
                async move { emergency_rollback_impl(&sys_a, &runs_dir_a, &active_a).await },
            );

        // Give task A time to pass the initial check and start its slow
        // `write_powercfg`, so task B genuinely arrives while A is in
        // flight rather than racing it at t=0.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        let sys_b = SlowController {
            inner: mock.clone(),
            delay,
        };
        let active_b = active_run.clone();
        let runs_dir_b = runs_dir.clone();
        let start = tokio::time::Instant::now();
        let task_b =
            tokio::spawn(
                async move { emergency_rollback_impl(&sys_b, &runs_dir_b, &active_b).await },
            );

        let (result_a, result_b) = tokio::join!(task_a, task_b);
        let elapsed = start.elapsed();
        result_a.unwrap().unwrap();
        result_b.unwrap().unwrap();

        // Concurrent execution (the bug) would finish close to task B's own
        // spawn-to-completion time alone (~150ms, since it never had to
        // wait for A). Serialized behind `active_run`'s lock, task B only
        // starts its own revert after A's ~150ms-minus-the-30ms-head-start
        // remainder, then takes another ~150ms itself -- roughly 270ms
        // total from `start`. 200ms cleanly separates the two outcomes with
        // margin on both sides.
        assert!(
            elapsed >= std::time::Duration::from_millis(200),
            "two overlapping rollback calls finished too quickly to have been serialized \
             (elapsed {elapsed:?})"
        );
    }
}
