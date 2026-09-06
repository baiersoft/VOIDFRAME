//! `powercfg` module apply / revert.

use crate::error::Result;
use crate::journal::{Journal, JournalRecord, Op};
use crate::system::{MutationCtx, SystemController};
use serde_json::json;

pub async fn apply(
    sub: &str,
    setting: &str,
    value: u32,
    sys: &dyn SystemController,
    journal: &mut Journal,
    ctx: &MutationCtx,
) -> Result<()> {
    let current = sys.read_powercfg(sub, setting).await?;
    let target = json!({ "sub": sub, "setting": setting });
    let inverse = json!({ "sub": sub, "setting": setting, "value": current.ac });
    let seq = journal.record(Op::PowercfgWrite, ctx, target, json!(value), inverse)?;
    sys.write_powercfg(sub, setting, value, ctx).await?;
    journal.mark_applied(seq)?;
    Ok(())
}

pub async fn revert(
    rec: &JournalRecord,
    sys: &dyn SystemController,
    ctx: &MutationCtx,
) -> Result<()> {
    let inv = &rec.inverse;
    let sub = inv["sub"].as_str().unwrap_or_default();
    let setting = inv["setting"].as_str().unwrap_or_default();
    let value = inv["value"].as_u64().unwrap_or(0) as u32;
    sys.write_powercfg(sub, setting, value, ctx).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Journal;
    use crate::mutation::revert_record;
    use crate::system::{AcDc, MockController};

    fn ctx() -> MutationCtx {
        MutationCtx {
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
        }
    }

    #[tokio::test]
    async fn apply_then_revert_restores_prior_value() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new().with_powercfg(
            "sub_processor",
            "IDLEDISABLE",
            AcDc { ac: 0, dc: 0 },
        );

        apply("sub_processor", "IDLEDISABLE", 1, &mock, &mut j, &ctx())
            .await
            .unwrap();
        assert_eq!(
            mock.read_powercfg("sub_processor", "IDLEDISABLE")
                .await
                .unwrap(),
            AcDc { ac: 1, dc: 1 }
        );

        let recs = Journal::load_pending(j.path()).unwrap();
        revert_record(&recs[0], &mock, &ctx()).await.unwrap();
        assert_eq!(
            mock.read_powercfg("sub_processor", "IDLEDISABLE")
                .await
                .unwrap(),
            AcDc { ac: 0, dc: 0 }
        );
    }
}
