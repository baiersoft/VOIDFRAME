//! ROLLBACK's own real work (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1): restoring the run-start active
//! power plan, a defensive sweep for any journal file this run left with
//! unconfirmed entries, and (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes.md
//! task 7) a best-effort restore of the run-start CS2 launch options if the
//! last executed scenario left them changed.

use super::RunContext;
use super::launch::write_launch_options_with_steam_closed;
use super::steam::ensure_steam_running;
use crate::error::Result;
use crate::journal::Journal;
use crate::run::EngineEvent;
use crate::system::{MutationCtx, SystemController};
use tokio::sync::mpsc;

/// ROLLBACK's real work (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1). `ctx.start_power_plan` is the plan
/// active when this run entered SNAPSHOT.
///
/// The best-effort launch-options restore below must run regardless of
/// whether the power-plan restore or the journal sweep themselves failed
/// (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes/final-fix-brief.md
/// Important 3) -- it is deliberately the last of the three to be decided,
/// but not gated on the other two's own `Result`s the way a plain sequence
/// of `?`s would. Both of those results are still propagated at the end,
/// once the restore has had its chance to run.
pub(super) async fn rollback(
    sys: &dyn SystemController,
    ctx: &RunContext<'_>,
    events: &mpsc::Sender<EngineEvent>,
) -> Result<()> {
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

    let sweep = sweep_journals(sys, ctx, events).await;

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

    plan?;
    sweep?;
    Ok(())
}

/// Defensive sweep: every scenario's own `run_scenario` call already
/// reverted its journal file -- a no-op in the normal case. Real recovery
/// only if an earlier revert was itself interrupted (e.g. the process
/// crashing between `run_scenario`'s revert call and returning), leaving
/// some records still unconfirmed (`applied: false` -- the apply/revert
/// flow never rewrites a record's `applied` flag on revert, only on the
/// original apply's confirmation, so an unconfirmed record this late means
/// its *apply*, not just its revert, never finished cleanly). Extracted out
/// of `rollback` (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes/final-fix-brief.md
/// Important 3) so its own `Result` can be held aside there rather than
/// short-circuiting the launch-options restore that follows it.
async fn sweep_journals(
    sys: &dyn SystemController,
    ctx: &RunContext<'_>,
    events: &mpsc::Sender<EngineEvent>,
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
        let report = crate::journal::replay::revert_all(&path, sys).await?;
        events
            .send(EngineEvent::RollbackProgress {
                reverted: report.reverted,
            })
            .await
            .ok();
    }
    Ok(())
}

/// `desired` is exactly what a scenario with no `Module::LaunchArgs`
/// reconciles to (`ctx.start_launch_args` is the run-start options with
/// VOIDFRAME's reserved tokens already stripped) -- so a run whose last
/// scenario carried no such module finds `live == desired` here and never
/// touches Steam at all. Also short-circuits when `live ==
/// ctx.start_launch_args_raw` (the exact, un-reconciled string read at
/// SNAPSHOT, before `strip_reserved`) -- a run that never got as far as
/// BASELINE's own write (an Abort, or a failure inside
/// `prepare_cs2_session` before it) leaves `live` at exactly that
/// un-reconciled value, which on a first run against un-reconciled options
/// never equals `desired` on its own; without this second check, a run
/// where nothing here actually changed would still make this function kill
/// Steam and write reserved tokens into the user's config
/// (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes/final-fix-brief.md
/// Important 1). When they differ, mirrors `prepare_cs2_session`'s
/// own "Differ" branch (kill `steam.exe` if found -> `wait_until_steam_closed`
/// -> `write_cs2_launch_options` -> `ensure_steam_running`), except
/// `rollback()` has no control channel to run the manual `OperatorPrompt`
/// fallback against: a failed relaunch here is only ever logged, never
/// blocks on an operator ack.
async fn restore_launch_options(
    sys: &dyn SystemController,
    ctx: &RunContext<'_>,
    events: &mpsc::Sender<EngineEvent>,
) -> Result<()> {
    let live = sys.read_cs2_launch_options().await?;
    let desired = crate::cs2::keybind_cfg::reconcile(&ctx.start_launch_args);
    if live == desired || live == ctx.start_launch_args_raw {
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
