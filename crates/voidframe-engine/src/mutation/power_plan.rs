//! `power_plan` module apply / revert — activate a plan by GUID, optionally
//! duplicating a template if it isn't installed (e.g. Ultimate Performance).

use crate::error::{Error, Result};
use crate::journal::{Journal, JournalRecord, Op};
use crate::model::module::PowerPlanPayload;
use crate::system::{MutationCtx, SystemController};
use serde_json::json;

pub async fn apply(
    p: &PowerPlanPayload,
    sys: &dyn SystemController,
    journal: &mut Journal,
    ctx: &MutationCtx,
) -> Result<()> {
    let prev_active = sys.active_power_plan().await?.guid;
    let plans = sys.list_power_plans().await?;
    let exists = plans.iter().any(|pp| pp.guid == p.plan_guid);

    let target_guid = if exists {
        p.plan_guid.clone()
    } else if p.create_if_missing {
        let created = sys.duplicate_power_plan(&p.plan_guid, ctx).await?;
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
        )
        .await
        .unwrap();
        assert_eq!(mock.active_power_plan().await.unwrap().guid, target);

        for rec in Journal::load_pending(j.path()).unwrap().iter().rev() {
            revert_record(rec, &mock, &ctx()).await.unwrap();
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
        )
        .await
        .unwrap();
        assert_eq!(
            mock.list_power_plans().await.unwrap().len(),
            before_count + 1
        );

        for rec in Journal::load_pending(j.path()).unwrap().iter().rev() {
            revert_record(rec, &mock, &ctx()).await.unwrap();
        }
        assert_eq!(mock.list_power_plans().await.unwrap().len(), before_count);
    }
}
