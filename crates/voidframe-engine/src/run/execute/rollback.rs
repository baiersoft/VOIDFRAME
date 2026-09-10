//! ROLLBACK's own real work (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1): restoring the run-start active
//! power plan, a forced revert of the scenario that was in flight when the
//! run was interrupted, a defensive sweep for any journal file this run left
//! with unconfirmed entries, and (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes.md
//! task 7) a best-effort restore of the run-start CS2 launch options if the
//! last executed scenario left them changed.

use super::RunContext;
use super::launch::write_launch_options_with_steam_closed;
use super::scenario::{journal_path_for, project_dir_for};
use super::steam::ensure_steam_running;
use crate::error::Result;
use crate::journal::Journal;
use crate::journal::replay::RevertReport;
use crate::model::progress::{RunProgress, Stage};
use crate::run::EngineEvent;
use crate::system::{MutationCtx, SystemController};
use std::path::{Path, PathBuf};
use tokio::sync::mpsc;

/// ROLLBACK's real work (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1). `ctx.start_power_plan` is the plan
/// active when this run entered SNAPSHOT.
///
/// The best-effort launch-options restore below must run regardless of
/// whether the forced in-flight revert, the power-plan restore, or the
/// journal sweep themselves failed
/// (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes/final-fix-brief.md
/// Important 3) -- it is deliberately the last of the four to be decided,
/// but not gated on the other three's own `Result`s the way a plain sequence
/// of `?`s would. All three of those results are still propagated at the
/// end, once the restore has had its chance to run.
pub(super) async fn rollback(
    sys: &dyn SystemController,
    ctx: &RunContext<'_>,
    progress: &RunProgress,
    events: &mpsc::Sender<EngineEvent>,
) -> Result<()> {
    // The scenario that was in flight when this run was interrupted never
    // reached its own `revert_stage`, so its mutations are still live --
    // revert them first, in exactly the position `revert_stage` itself
    // would have occupied (before the belt-and-braces power-plan restore
    // below), rather than leaving them to a sweep that only looks at
    // unconfirmed records. See [`in_flight_journal`].
    let project_dir = project_dir_for(ctx);
    let in_flight = in_flight_journal(ctx, progress);
    let forced = match &in_flight {
        Some(path) => force_revert_in_flight_scenario(sys, &project_dir, path, events).await,
        None => Ok(()),
    };

    // Restore the run-start active power plan. NOT itself journaled: this
    // is the belt-and-braces step *after* every per-scenario journal has
    // already been reverted (by each `run_scenario` call above), not a new
    // mutation that needs its own inverse recorded.
    let mutation_ctx = MutationCtx {
        run_id: ctx.config.run_id.clone(),
        scenario_id: "rollback".into(),
        step_index: 0,
    };
    let plan = sys
        .set_active_power_plan(&ctx.start_power_plan.guid, &mutation_ctx)
        .await;

    let sweep = sweep_journals(sys, ctx, &project_dir, events, in_flight.as_deref()).await;

    // Restore the run-start launch options if the last executed scenario
    // (or one an Abort interrupted mid-way) left them changed (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes.md task 7).
    // Best-effort -- unlike the power-plan restore and the journal sweep
    // above, a failure here must not fail `rollback()` itself: leaving the
    // user's Steam launch options unrestored is a real but recoverable
    // inconvenience, not something worth losing the rest of ROLLBACK's own
    // work over. Deliberately NOT skipped when `plan`/`sweep` above already
    // failed -- see this function's own doc comment.
    if let Err(e) = restore_launch_options(sys, ctx, events).await {
        tracing::warn!(
            error = %e,
            "failed to restore run-start launch options at ROLLBACK"
        );
        events
            .send(EngineEvent::LogLine {
                text: format!("Failed to restore run-start launch options: {e}"),
            })
            .await
            .ok();
    }

    forced?;
    plan?;
    sweep?;
    Ok(())
}

/// The journal file of the scenario that was in flight when the run was
/// interrupted, but *only* when the interruption landed strictly BEFORE
/// that scenario's own `revert_stage` was ever invoked -- i.e. with the
/// cursor at [`Stage::Apply`] (mid-apply: some modules confirmed, some
/// perhaps not) or [`Stage::Measure`] (every module confirmed, CS2 about
/// to run or running). Those are exactly the two stages whose mutations
/// are guaranteed still live on the machine with nothing else in the run
/// scheduled to take them back off. `None` if the file does not exist
/// (nothing was ever journaled for it).
///
/// Deliberately `None` for [`Stage::Revert`]: `revert_stage` may already
/// have run for that scenario, in whole or in part, and a revert does NOT
/// clear a record's `applied` flag (only `Journal::record`/`mark_applied`
/// touch it), so this function has no way to tell an already-reverted
/// journal from an untouched one. A second reverse replay over one that was
/// already reverted would re-attempt reverts that already succeeded, which
/// is not a clean no-op for every op: `Op::PowerPlanCreate`'s revert calls
/// `delete_power_plan`, which has nothing left to delete on a second pass
/// (`Op::PowerPlanDelete`'s own revert, by contrast, is an unconditional
/// `Ok(())` -- see `mutation::power_plan::revert`). That second-pass failure
/// would not hard-fail ROLLBACK -- `revert_all` collects it into
/// `RevertReport::verify_failures`, which [`report_verify_failures`] then
/// surfaces -- so the real cost is noise plus an unreliable "was it
/// restored?" signal, not a broken rollback. The scoping decision stands on
/// the ambiguity itself rather than on that hazard: recovering from an
/// interruption DURING `revert_stage` is a genuinely separate case that has
/// not been designed yet, and is explicitly out of scope here.
/// [`sweep_journals`] still covers the part of it that leaves an unconfirmed
/// record behind, and `scenario::revert_stage` now shields its ENTIRE body
/// (not just the replay call -- widened 2026-09-08 after a re-review found
/// the progress LogLine send just before it was an equally-reachable, equally
/// unrecoverable door into the same state) from the abort race (I1, 2026-09-07
/// follow-up review), so an operator Abort can no longer open this window in
/// the first place -- only a crash or a hard process kill can.
///
/// Also `None` for [`Stage::Done`] (nothing of this scenario's is left
/// applied at that point).
///
/// Scope, precisely: this only ever names the scenario at
/// `progress.cursor.index`, and only for those two stages -- so it covers
/// exactly one scenario, never a sweep of "everything except the current
/// index". Every earlier index already completed its own `apply` ->
/// `measure` -> `revert` cycle before the loop advanced past it, which is
/// why one scenario is enough. That relies on `scenario_loop` keeping the
/// cursor honest about which scenario is actually applied: its
/// `Transition::ApplyNextThenReboot` arm applies index `i + 1` while the
/// stage still reads `Revert`, so it persists `progress.cursor` as
/// `{i + 1, Stage::Apply}` BEFORE that apply starts (C1, 2026-09-07
/// follow-up review; widened 2026-09-10 so the abort race's from-disk
/// reload sees it too) and advances it to `{i + 1, Stage::Measure}` once
/// the apply is confirmed, rather than only inside `reboot_sequence`.
/// Still genuinely uncovered: an interruption landing inside `revert_stage`
/// itself, per the paragraph above.
fn in_flight_journal(ctx: &RunContext<'_>, progress: &RunProgress) -> Option<PathBuf> {
    if !matches!(progress.cursor.stage, Stage::Apply | Stage::Measure) {
        return None;
    }
    let scenarios = super::ordered_scenarios(&progress.project);
    let scenario = scenarios.get(progress.cursor.index as usize)?;
    let path = journal_path_for(ctx, &scenario.id);
    path.exists().then_some(path)
}

/// Reverse-replays `path` unconditionally -- every record, confirmed or
/// not. This is the fix for the gap [`sweep_journals`] cannot close on its
/// own: a scenario whose modules all applied *successfully* leaves a journal
/// with no unconfirmed record at all, so the sweep skips it entirely and its
/// registry/powercfg/power-plan mutations stay live on the machine. Reached
/// on every mid-run failure path, not just Abort -- [`rollback`]'s one
/// caller is `handle_body_failure`, which `scenario_loop`'s own `Err` exit
/// (an ordinary `measure_stage` failure propagating via `?`, which
/// short-circuits past `revert_stage` the same way) reaches too.
async fn force_revert_in_flight_scenario(
    sys: &dyn SystemController,
    project_dir: &Path,
    path: &Path,
    events: &mpsc::Sender<EngineEvent>,
) -> Result<()> {
    tracing::warn!(
        path = %path.display(),
        "the scenario in flight when this run was interrupted never reached its own revert -- \
         reverting its journal at ROLLBACK"
    );
    events
        .send(EngineEvent::LogLine {
            text: format!(
                "The scenario interrupted mid-run never reverted its own modules -- reverting \
                 {} at ROLLBACK",
                path.display()
            ),
        })
        .await
        .ok();
    let report = crate::journal::replay::revert_all(project_dir, path, sys).await?;
    events
        .send(EngineEvent::RollbackProgress {
            reverted: report.reverted,
        })
        .await
        .ok();
    report_verify_failures(&report, path, events).await;
    Ok(())
}

/// `revert_all` never throws on a record it could not actually put back --
/// a validation failure, a revert call that errored, or a verify-readback
/// mismatch all land in [`RevertReport::verify_failures`] so one bad record
/// cannot abort the rest of the replay. Without surfacing them, a forced
/// revert that RAN but did not RESTORE is indistinguishable in the event
/// stream and `run.log` from one that fully succeeded (I2, 2026-09-07
/// follow-up review). Mirrors `scenario::revert_stage`'s own handling of the
/// same report shape: warn + a `LogLine`, never a hard failure -- ROLLBACK's
/// remaining work (the power-plan and launch-options restores) is still
/// worth doing, and the journal survives `prune_run_dir` for Emergency
/// Restore either way.
async fn report_verify_failures(
    report: &RevertReport,
    path: &Path,
    events: &mpsc::Sender<EngineEvent>,
) {
    if report.verify_failures.is_empty() {
        return;
    }
    tracing::warn!(
        path = %path.display(),
        failures = ?report.verify_failures,
        "rollback verify mismatch -- a reverted record did not read back as restored"
    );
    events
        .send(EngineEvent::LogLine {
            text: format!(
                "Rollback verify mismatch reverting {}: {:?}",
                path.display(),
                report.verify_failures
            ),
        })
        .await
        .ok();
}

/// Defensive sweep: every scenario's own `run_scenario` call already
/// reverted its journal file -- a no-op in the normal case. Real recovery
/// only if an earlier apply or revert was itself interrupted (e.g. the
/// process crashing between `run_scenario`'s revert call and returning),
/// leaving some records still unconfirmed (`applied: false` -- the
/// apply/revert flow never rewrites a record's `applied` flag on revert,
/// only on the original apply's confirmation, so an unconfirmed record this
/// late means its *apply*, not just its revert, never finished cleanly).
/// Extracted out of `rollback` (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes/final-fix-brief.md
/// Important 3) so its own `Result` can be held aside there rather than
/// short-circuiting the launch-options restore that follows it.
///
/// Deliberately still unconfirmed-record-gated: a journal whose records are
/// all `applied: true` is skipped here. That used to leave a real gap -- an
/// interrupted scenario whose modules had all applied cleanly kept its
/// mutations live -- which is now closed by
/// [`force_revert_in_flight_scenario`], not by widening this sweep (a blanket
/// re-revert of every journal would re-revert scenarios that already
/// completed their own `revert_stage` normally). `skip` is that forced
/// revert's own journal, if any: it was just reverse-replayed in full, and a
/// lone unconfirmed record inside it must not trigger a second pass over the
/// whole file.
async fn sweep_journals(
    sys: &dyn SystemController,
    ctx: &RunContext<'_>,
    project_dir: &Path,
    events: &mpsc::Sender<EngineEvent>,
    skip: Option<&Path>,
) -> Result<()> {
    let run_dir = ctx.config.data_root.join("runs").join(&ctx.config.run_id);
    let mut entries = match tokio::fs::read_dir(&run_dir).await {
        Ok(e) => e,
        // The run directory not existing at all this late would itself be
        // surprising (every `run_scenario` call creates it via
        // `Journal::open`), but there is nothing to sweep either way.
        Err(_) => return Ok(()),
    };
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let is_journal = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("journal-") && n.ends_with(".jsonl"));
        if !is_journal {
            continue;
        }
        if skip == Some(path.as_path()) {
            continue; // already reverse-replayed in full just above
        }
        let records = Journal::load_pending(&path)?;
        if !records.iter().any(|r| !r.applied) {
            continue; // every entry already confirmed applied -- nothing to do
        }
        tracing::warn!(
            path = %path.display(),
            "journal file has unconfirmed entries at ROLLBACK -- this indicates an earlier \
             apply or revert was itself interrupted, not a normal-path occurrence; running a \
             defensive revert now"
        );
        events
            .send(EngineEvent::LogLine {
                text: format!(
                    "Found unconfirmed entries in {} at ROLLBACK -- running a defensive revert",
                    path.display()
                ),
            })
            .await
            .ok();
        let report = crate::journal::replay::revert_all(project_dir, &path, sys).await?;
        events
            .send(EngineEvent::RollbackProgress {
                reverted: report.reverted,
            })
            .await
            .ok();
        report_verify_failures(&report, &path, events).await;
    }
    Ok(())
}

/// Restores the true pristine pre-run launch options
/// (`ctx.start_launch_args_raw`, the exact string read at SNAPSHOT before
/// any scenario touched it) whenever the live value has diverged from it --
/// for every run, not just a failed one, matching `recover.rs`'s own
/// crash-recovery semantics (`write_cs2_launch_options(&progress.
/// start_launch_args_raw)`, unconditional). Previously computed its target
/// via `reconcile(ctx.start_launch_args)`, which still contains VOIDFRAME's
/// own reserved tokens -- exactly what BASELINE itself would have written,
/// so the short-circuit below fired even when nothing was ever restored to
/// the user's real original string. Confirmed as the cause of a real
/// alpha-tester's unrestored launch options after a failed run:
/// study/pamuk/a85ddb56-.../run.log (BASELINE's write succeeded, CS2 then
/// failed to launch, and this function silently no-op'd).
async fn restore_launch_options(
    sys: &dyn SystemController,
    ctx: &RunContext<'_>,
    events: &mpsc::Sender<EngineEvent>,
) -> Result<()> {
    let live = sys.read_cs2_launch_options().await?;
    let desired = ctx.start_launch_args_raw.clone();
    if live == desired {
        return Ok(());
    }
    events
        .send(EngineEvent::LogLine {
            text: format!(
                "Restoring launch options at ROLLBACK: before: \"{}\" after: \"{}\"",
                crate::cs2::keybind_cfg::redact(&live),
                crate::cs2::keybind_cfg::redact(&desired)
            ),
        })
        .await
        .ok();
    write_launch_options_with_steam_closed(sys, &desired).await?;
    if let Err(e) = ensure_steam_running(sys).await {
        events
            .send(EngineEvent::LogLine {
                text: format!(
                    "Could not relaunch Steam automatically after restoring launch options \
                     ({e}) -- Steam must be reopened manually."
                ),
            })
            .await
            .ok();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::progress::Cursor;
    use crate::model::project::{Baseline, Project, Scenario};
    use crate::run::execute::RunConfig;
    use crate::system::PowerPlan;
    use std::time::Duration;

    fn project() -> Project {
        Project {
            schema_version: crate::model::SCHEMA_VERSION.to_string(),
            id: "p1".into(),
            name: "P".into(),
            description: "d".into(),
            created_at: "2026-09-07T00:00:00Z".into(),
            settings: serde_json::from_str("{}").unwrap(),
            baseline: Baseline {
                name: "Stock".into(),
                description: "d".into(),
            },
            scenarios: vec![Scenario {
                id: "sc1".into(),
                name: "Scenario 1".into(),
                description: "d".into(),
                enabled: true,
                modules: vec![],
            }],
        }
    }

    fn config(data_root: &std::path::Path) -> RunConfig {
        RunConfig {
            project: project(),
            run_id: "r1".into(),
            dry_run: false,
            data_root: data_root.to_path_buf(),
            webview_root_pid: None,
            console_log_override: None,
            mock_cs2_log: None,
            thermal_sample_override: None,
            inter_scenario_break_seconds: 0,
            hwinfo_path: None,
            shutdown_when_complete: false,
            post_boot_settle: Duration::from_secs(0),
            exe_path: std::path::PathBuf::from("voidframe.exe"),
        }
    }

    fn ctx_for(config: &RunConfig) -> RunContext<'_> {
        RunContext {
            config,
            start_build_id: None,
            start_launch_args: String::new(),
            start_launch_args_raw: String::new(),
            start_power_plan: PowerPlan {
                guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
                name: "Balanced".into(),
                active: true,
            },
            abort: tokio::sync::watch::channel(false).1,
            no_return: tokio::sync::watch::channel(false).0,
        }
    }

    /// `ordered_scenarios` prepends the baseline dummy, so `sc1` is index 1.
    fn progress_at(config: &RunConfig, stage: Stage) -> RunProgress {
        RunProgress {
            schema_version: crate::model::SCHEMA_VERSION.to_string(),
            run_id: config.run_id.clone(),
            project: config.project.clone(),
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
            cursor: Cursor { index: 1, stage },
            reboot: None,
            shutdown_when_complete: false,
            skip_revert_once: false,
            abort_requested: false,
        }
    }

    /// Creates `journal-sc1.jsonl` so the existence check isn't what decides
    /// these cases.
    fn touch_journal(config: &RunConfig) -> PathBuf {
        let dir = config.data_root.join("runs").join(&config.run_id);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("journal-sc1.jsonl");
        std::fs::write(&path, "").unwrap();
        path
    }

    /// Interrupted mid-apply: some of this scenario's modules may already be
    /// live and nothing else in the run will take them back off.
    #[test]
    fn stage_apply_is_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let expected = touch_journal(&config);
        assert_eq!(
            in_flight_journal(&ctx_for(&config), &progress_at(&config, Stage::Apply)),
            Some(expected)
        );
    }

    /// Interrupted with every module confirmed applied, CS2 about to run --
    /// the exact case `sweep_journals` cannot see, since no record is
    /// unconfirmed.
    #[test]
    fn stage_measure_is_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let expected = touch_journal(&config);
        assert_eq!(
            in_flight_journal(&ctx_for(&config), &progress_at(&config, Stage::Measure)),
            Some(expected)
        );
    }

    /// The scoping decision this fix deliberately makes: `Stage::Revert`
    /// means `revert_stage` may already have run for this scenario, wholly
    /// or partly, and a revert leaves the records' `applied` flags untouched
    /// -- so a second reverse replay could re-attempt reverts that already
    /// succeeded. Recovering from an interruption during the revert itself
    /// is a separate, not-yet-designed case; the forced revert must stay out
    /// of it.
    #[test]
    fn stage_revert_is_never_force_reverted() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        touch_journal(&config);
        assert_eq!(
            in_flight_journal(&ctx_for(&config), &progress_at(&config, Stage::Revert)),
            None,
            "an interruption during revert_stage is explicitly out of scope"
        );
    }

    /// Nothing of this scenario's is left applied once the cursor is `Done`.
    #[test]
    fn stage_done_is_never_force_reverted() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        touch_journal(&config);
        assert_eq!(
            in_flight_journal(&ctx_for(&config), &progress_at(&config, Stage::Done)),
            None
        );
    }

    /// A scenario that never journaled anything (e.g. the baseline, or an
    /// abort landing before the first `Journal::open`) has no file to
    /// replay.
    #[test]
    fn a_missing_journal_is_not_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        assert_eq!(
            in_flight_journal(&ctx_for(&config), &progress_at(&config, Stage::Measure)),
            None
        );
    }

    /// I2 (2026-09-07 follow-up review): `revert_all` never throws on a
    /// record it could not actually put back -- it collects the failure into
    /// `RevertReport::verify_failures` so one bad record cannot abort the
    /// rest of the replay. `force_revert_in_flight_scenario` used to read
    /// only `report.reverted`, which made a forced revert that RAN but did
    /// NOT RESTORE indistinguishable, in both the event stream and
    /// `run.log`, from one that fully succeeded.
    #[tokio::test]
    async fn a_forced_revert_that_could_not_restore_the_machine_is_surfaced_not_swallowed() {
        const SUB: &str = "sub_processor";
        const SETTING: &str = "IDLEDISABLE";

        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        let run_dir = config.data_root.join("runs").join(&config.run_id);
        std::fs::create_dir_all(&run_dir).unwrap();
        let path = run_dir.join("journal-sc1.jsonl");

        let mock = crate::system::MockController::new().with_powercfg(
            SUB,
            SETTING,
            crate::system::AcDc { ac: 0, dc: 0 },
        );
        let mut journal = crate::journal::Journal::open(&path).unwrap();
        let scenario = crate::model::project::Scenario {
            id: "sc1".into(),
            name: "Scenario 1".into(),
            description: "d".into(),
            enabled: true,
            modules: vec![crate::model::Module::Powercfg {
                sub: SUB.into(),
                setting: SETTING.into(),
                value: 1,
            }],
        };
        crate::mutation::apply_scenario(
            &scenario,
            &config.run_id,
            dir.path(),
            &mock,
            &mut journal,
            &tokio::sync::watch::channel(false).0,
        )
        .await
        .unwrap();
        drop(journal);

        // Sabotage the one write the revert makes, so the replay runs but
        // the machine is left exactly as the scenario set it.
        mock.fail_next_write("sabotage");
        let (events_tx, mut events_rx) = mpsc::channel(16);
        force_revert_in_flight_scenario(&mock, dir.path(), &path, &events_tx)
            .await
            .expect("a verify failure is reported, never a hard rollback failure");

        assert_eq!(
            mock.read_powercfg(SUB, SETTING).await.unwrap().ac,
            1,
            "sanity: the sabotage must genuinely have left the mutation live"
        );
        let mut saw_mismatch = false;
        while let Ok(ev) = events_rx.try_recv() {
            if let EngineEvent::LogLine { text } = ev
                && text.contains("Rollback verify mismatch")
            {
                saw_mismatch = true;
            }
        }
        assert!(
            saw_mismatch,
            "a forced revert that did not restore the machine must say so in the event stream"
        );
    }

    /// A cursor index past the end of `[baseline] ++ enabled scenarios`
    /// (a corrupt `progress.json`) must not panic -- ROLLBACK still has the
    /// power-plan and launch-options restores to do.
    #[test]
    fn an_out_of_range_cursor_is_not_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path());
        touch_journal(&config);
        let mut progress = progress_at(&config, Stage::Measure);
        progress.cursor.index = 99;
        assert_eq!(in_flight_journal(&ctx_for(&config), &progress), None);
    }
}
