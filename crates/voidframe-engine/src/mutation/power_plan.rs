//! `power_plan` module apply / revert — activate a plan by GUID, optionally
//! duplicating a template if it isn't installed (e.g. Ultimate Performance).

use crate::error::{Error, Result};
use crate::journal::{Journal, JournalRecord, Op};
use crate::model::module::PowerPlanPayload;
use crate::no_return::NoReturnGuard;
use crate::system::{MutationCtx, SystemController};
use serde_json::json;

/// `no_return` shields the one `.await` in this function that resolves
/// before its journal record can exist -- see [`NoReturnGuard`]. Closes the
/// 2026-09-07 audit finding: `duplicate_power_plan` was the one mutation
/// apply-path await that happens *before* its journal record exists (the
/// guid isn't known until it returns), so a cancellation landing mid-call
/// could create a power plan with zero journal trace. This does not reorder
/// the journal write -- it removes the cancellation window around the await
/// instead. Every other step here journals first, so nothing else needs it.
pub async fn apply(
    p: &PowerPlanPayload,
    sys: &dyn SystemController,
    journal: &mut Journal,
    ctx: &MutationCtx,
    no_return: &tokio::sync::watch::Sender<bool>,
) -> Result<()> {
    let prev_active = sys.active_power_plan().await?.guid;
    let plans = sys.list_power_plans().await?;
    let exists = plans.iter().any(|pp| pp.guid == p.plan_guid);

    let target_guid = if exists {
        p.plan_guid.clone()
    } else if p.create_if_missing {
        // `Journal::record`/`mark_applied` below are plain synchronous `fn`s,
        // and a future can only be dropped by the executor at an `.await`
        // point -- so shielding just this one await is enough to make the
        // create-then-journal pair uninterruptible as a whole.
        let created = {
            let _shield = NoReturnGuard::engage(no_return);
            sys.duplicate_power_plan(&p.plan_guid, ctx).await?
        };
        let seq = journal.record(
            Op::PowerPlanCreate,
            ctx,
            json!({ "template": p.plan_guid }),
            json!({ "guid": created.guid }),
            json!({ "guid": created.guid }),
        )?;
        journal.mark_applied(seq)?;
        created.guid
    } else {
        return Err(Error::msg(format!(
            "power plan {} is not installed",
            p.plan_guid
        )));
    };

    let seq = journal.record(
        Op::PowerPlanActivate,
        ctx,
        json!({ "guid": target_guid }),
        json!(target_guid),
        json!({ "guid": prev_active }),
    )?;
    sys.set_active_power_plan(&target_guid, ctx).await?;
    journal.mark_applied(seq)?;
    Ok(())
}

pub async fn revert(
    rec: &JournalRecord,
    sys: &dyn SystemController,
    ctx: &MutationCtx,
) -> Result<()> {
    match rec.op {
        Op::PowerPlanActivate => {
            let guid = rec.inverse["guid"].as_str().unwrap_or_default();
            sys.set_active_power_plan(guid, ctx).await
        }
        Op::PowerPlanCreate => {
            let guid = rec.inverse["guid"].as_str().unwrap_or_default();
            sys.delete_power_plan(guid, ctx).await
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Journal;
    use crate::mutation::revert_record;
    use crate::system::MockController;
    use std::time::Duration;

    /// A throwaway `no_return` shield for the tests that don't exercise it.
    /// The dedicated shield test below builds its own channel so it can
    /// observe the flag.
    fn no_return() -> tokio::sync::watch::Sender<bool> {
        tokio::sync::watch::channel(false).0
    }

    fn ctx() -> MutationCtx {
        MutationCtx {
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
        }
    }

    #[tokio::test]
    async fn activate_then_revert_restores_previous_active_plan() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new();
        let plans = mock.list_power_plans().await.unwrap();
        let start = mock.active_power_plan().await.unwrap().guid;
        let target = plans.iter().find(|p| p.guid != start).unwrap().guid.clone();

        apply(
            &PowerPlanPayload {
                plan_guid: target.clone(),
                friendly_name: None,
                create_if_missing: false,
            },
            &mock,
            &mut j,
            &ctx(),
            &no_return(),
        )
        .await
        .unwrap();
        assert_eq!(mock.active_power_plan().await.unwrap().guid, target);

        for rec in Journal::load_pending(j.path()).unwrap().iter().rev() {
            revert_record(rec, dir.path(), &mock, &ctx()).await.unwrap();
        }
        assert_eq!(mock.active_power_plan().await.unwrap().guid, start);
    }

    #[tokio::test]
    async fn missing_plan_without_create_flag_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new();
        let e = apply(
            &PowerPlanPayload {
                plan_guid: "nope".into(),
                friendly_name: None,
                create_if_missing: false,
            },
            &mock,
            &mut j,
            &ctx(),
            &no_return(),
        )
        .await
        .unwrap_err();
        assert!(e.to_string().contains("nope"));
    }

    #[tokio::test]
    async fn missing_plan_with_create_flag_duplicates_and_reverts_by_deleting() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new();
        let before_count = mock.list_power_plans().await.unwrap().len();

        apply(
            &PowerPlanPayload {
                plan_guid: "template-guid".into(),
                friendly_name: None,
                create_if_missing: true,
            },
            &mock,
            &mut j,
            &ctx(),
            &no_return(),
        )
        .await
        .unwrap();
        assert_eq!(
            mock.list_power_plans().await.unwrap().len(),
            before_count + 1
        );

        for rec in Journal::load_pending(j.path()).unwrap().iter().rev() {
            revert_record(rec, dir.path(), &mock, &ctx()).await.unwrap();
        }
        assert_eq!(mock.list_power_plans().await.unwrap().len(), before_count);
    }

    /// The 2026-09-07 audit finding's actual fix: `duplicate_power_plan` is
    /// the one mutation `.await` that resolves *before* its journal record
    /// can exist (the guid isn't known until it returns), so the whole-body
    /// abort race must not be allowed to drop the run there -- it would
    /// leave a real power plan created with zero journal trace. The shield
    /// is engaged for exactly that call and released again immediately
    /// after, so a deferred Abort is never lost for the rest of the run.
    ///
    /// Observes the real transition, not just "didn't panic": the mock is
    /// given a genuine `.await` inside `duplicate_power_plan`, so this task
    /// gets scheduled while the call is still in flight and can read the
    /// shield as `true` at that moment, then `false` once `apply` returns.
    /// `tokio::time::pause()` makes that ordering deterministic rather than
    /// wall-clock dependent -- the mock's sleep parks, this task observes,
    /// and the timer only auto-advances once this task parks in turn.
    #[tokio::test]
    async fn create_if_missing_shields_the_duplicate_call_and_releases_it_afterwards() {
        tokio::time::pause();
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&dir.path().join("j.jsonl")).unwrap();
        let mock =
            MockController::new().with_duplicate_power_plan_delay(Duration::from_millis(150));
        let (no_return_tx, mut no_return_rx) = tokio::sync::watch::channel(false);

        let handle = tokio::spawn(async move {
            apply(
                &PowerPlanPayload {
                    plan_guid: "template-guid".into(),
                    friendly_name: None,
                    create_if_missing: true,
                },
                &mock,
                &mut j,
                &ctx(),
                &no_return_tx,
            )
            .await
            .unwrap();
            // Held until the task ends so the receiver below never sees a
            // closed channel instead of the real `false`.
            no_return_tx
        });

        // Bounded so a regression (the shield removed) fails here instead of
        // hanging: the sender is parked in the spawned task until
        // `handle.await` below, so a never-engaged shield would otherwise
        // leave `changed()` pending forever. Under paused time this budget
        // costs nothing when the shield is present -- the `send(true)`
        // happens before the mock's own `.await`, so `changed()` resolves
        // with no time advanced at all.
        tokio::time::timeout(Duration::from_secs(5), no_return_rx.changed())
            .await
            .expect("the shield must engage while the duplicate call is in flight")
            .expect("the shield's sender must still be alive");
        assert!(
            *no_return_rx.borrow_and_update(),
            "the shield must read `true` while `duplicate_power_plan` is still awaiting"
        );

        handle.await.unwrap();
        assert!(
            !*no_return_rx.borrow(),
            "the shield must be released again once `apply` returns -- a permanently-set \
             shield would make Abort dead for the rest of the run"
        );
    }
}
