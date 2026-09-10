//! `custom_script` module apply / revert.

use crate::error::{Error, Result};
use crate::journal::{Journal, JournalRecord, Op};
use crate::model::module::CustomScriptPayload;
use crate::system::{MutationCtx, SystemController};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

fn sha256_hex(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| {
        Error::msg(format!(
            "failed to read {} for hashing: {e}",
            path.display()
        ))
    })?;
    let digest = Sha256::digest(&bytes);
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(300);

pub async fn apply(
    p: &CustomScriptPayload,
    project_dir: &Path,
    sys: &dyn SystemController,
    journal: &mut Journal,
    ctx: &MutationCtx,
) -> Result<()> {
    let scripts_dir = project_dir.join("scripts");
    let apply_path = scripts_dir.join(&p.apply_script);
    let revert_path = scripts_dir.join(&p.revert_script);

    let apply_sha256 = sha256_hex(&apply_path)?;
    let revert_sha256 = sha256_hex(&revert_path)?;

    let target = json!({ "apply_script": p.apply_script, "revert_script": p.revert_script });
    let inverse = json!({
        "apply_sha256": apply_sha256,
        "revert_sha256": revert_sha256,
        "revert_path": p.revert_script,
    });
    let seq = journal.record(
        Op::CustomScriptApply,
        ctx,
        target,
        json!({ "apply_sha256": apply_sha256 }),
        inverse,
    )?;

    let output = sys.run_script(&apply_path, SCRIPT_TIMEOUT).await?;
    // D6: this is the only diagnostic trail for a script VOIDFRAME cannot
    // verify the effect of -- logged regardless of exit code, not just on
    // the error path.
    tracing::info!(
        script = %p.apply_script,
        stdout = %output.stdout,
        stderr = %output.stderr,
        exit_code = ?output.exit_code,
        "custom_script apply ran"
    );
    if output.exit_code != Some(0) {
        return Err(Error::msg(format!(
            "custom_script apply ({}) exited with {:?}: {}",
            p.apply_script, output.exit_code, output.stderr
        )));
    }
    journal.mark_applied(seq)?;
    Ok(())
}

pub async fn revert(
    rec: &JournalRecord,
    project_dir: &Path,
    sys: &dyn SystemController,
    ctx: &MutationCtx,
) -> Result<()> {
    let inv = &rec.inverse;
    let revert_rel = inv["revert_path"].as_str().ok_or_else(|| {
        Error::msg(format!(
            "seq {}: custom_script revert inverse is missing a string revert_path -- refusing to guess which script to run",
            rec.seq
        ))
    })?;
    let expected_sha256 = inv["revert_sha256"].as_str().ok_or_else(|| {
        Error::msg(format!(
            "seq {}: custom_script revert inverse is missing a string revert_sha256 -- refusing to run an unverified script",
            rec.seq
        ))
    })?;
    let revert_path = project_dir.join("scripts").join(revert_rel);

    let actual_sha256 = sha256_hex(&revert_path)?;
    if actual_sha256 != expected_sha256 {
        return Err(Error::msg(format!(
            "revert script {revert_rel} changed since apply (expected sha256 {expected_sha256}, found {actual_sha256}) -- refusing to run it"
        )));
    }

    let output = sys.run_script(&revert_path, SCRIPT_TIMEOUT).await?;
    // D6: this is the only diagnostic trail for a script VOIDFRAME cannot
    // verify the effect of -- logged regardless of exit code, not just on
    // the error path.
    tracing::info!(
        script = revert_rel,
        stdout = %output.stdout,
        stderr = %output.stderr,
        exit_code = ?output.exit_code,
        "custom_script revert ran"
    );
    if output.exit_code != Some(0) {
        return Err(Error::msg(format!(
            "custom_script revert ({revert_rel}) exited with {:?}: {}",
            output.exit_code, output.stderr
        )));
    }
    let _ = ctx;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::MockController;

    fn ctx() -> MutationCtx {
        MutationCtx {
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
        }
    }

    fn write_script(dir: &Path, name: &str, contents: &str) {
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join("scripts").join(name), contents).unwrap();
    }

    #[tokio::test]
    async fn apply_journals_both_hashes_and_runs_the_apply_script() {
        let project_dir = tempfile::tempdir().unwrap();
        write_script(project_dir.path(), "apply.bat", "@echo off\r\n");
        write_script(project_dir.path(), "revert.bat", "@echo off\r\n");
        let journal_dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&journal_dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new();

        let p = CustomScriptPayload {
            apply_script: "apply.bat".into(),
            revert_script: "revert.bat".into(),
            requires_reboot: false,
            description: "d".into(),
        };
        apply(&p, project_dir.path(), &mock, &mut j, &ctx())
            .await
            .unwrap();

        assert_eq!(mock.run_script_calls().len(), 1);
        assert!(mock.run_script_calls()[0].ends_with("apply.bat"));

        let recs = Journal::load_pending(j.path()).unwrap();
        assert!(recs[0].applied);
        assert!(recs[0].inverse["revert_sha256"].as_str().unwrap().len() == 64);
        assert_eq!(recs[0].inverse["revert_path"], "revert.bat");
    }

    #[tokio::test]
    async fn apply_fails_the_scenario_when_the_script_exits_nonzero() {
        let project_dir = tempfile::tempdir().unwrap();
        write_script(project_dir.path(), "apply.bat", "@echo off\r\n");
        write_script(project_dir.path(), "revert.bat", "@echo off\r\n");
        let journal_dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&journal_dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new().with_run_script_result(Ok(crate::system::ScriptOutput {
            exit_code: Some(1),
            stdout: String::new(),
            stderr: "boom".into(),
        }));

        let p = CustomScriptPayload {
            apply_script: "apply.bat".into(),
            revert_script: "revert.bat".into(),
            requires_reboot: false,
            description: "d".into(),
        };
        let err = apply(&p, project_dir.path(), &mock, &mut j, &ctx())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("boom"));

        let recs = Journal::load_pending(j.path()).unwrap();
        assert!(
            !recs[0].applied,
            "a failed apply must not be marked applied"
        );
    }

    #[tokio::test]
    async fn revert_runs_the_recorded_revert_script_when_the_hash_matches() {
        let project_dir = tempfile::tempdir().unwrap();
        write_script(project_dir.path(), "apply.bat", "@echo off\r\n");
        write_script(project_dir.path(), "revert.bat", "@echo off\r\n");
        let journal_dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&journal_dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new();

        let p = CustomScriptPayload {
            apply_script: "apply.bat".into(),
            revert_script: "revert.bat".into(),
            requires_reboot: false,
            description: "d".into(),
        };
        apply(&p, project_dir.path(), &mock, &mut j, &ctx())
            .await
            .unwrap();
        let recs = Journal::load_pending(j.path()).unwrap();

        revert(&recs[0], project_dir.path(), &mock, &ctx())
            .await
            .unwrap();
        assert_eq!(mock.run_script_calls().len(), 2, "apply + revert");
        assert!(mock.run_script_calls()[1].ends_with("revert.bat"));
    }

    #[tokio::test]
    async fn revert_refuses_to_run_when_the_script_changed_since_apply() {
        let project_dir = tempfile::tempdir().unwrap();
        write_script(project_dir.path(), "apply.bat", "@echo off\r\n");
        write_script(project_dir.path(), "revert.bat", "@echo off\r\n");
        let journal_dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&journal_dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new();

        let p = CustomScriptPayload {
            apply_script: "apply.bat".into(),
            revert_script: "revert.bat".into(),
            requires_reboot: false,
            description: "d".into(),
        };
        apply(&p, project_dir.path(), &mock, &mut j, &ctx())
            .await
            .unwrap();
        let recs = Journal::load_pending(j.path()).unwrap();

        // Tamper with the revert script after apply.
        write_script(
            project_dir.path(),
            "revert.bat",
            "@echo off\r\necho tampered\r\n",
        );

        let err = revert(&recs[0], project_dir.path(), &mock, &ctx())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("changed since apply"), "{err}");
        assert_eq!(
            mock.run_script_calls().len(),
            1,
            "revert must never have run"
        );
    }

    /// Finding 7: a corrupted or planted journal line whose `inverse` has no
    /// `revert_path` field must not silently resolve to an empty string and
    /// try to run whatever `project_dir/scripts/` happens to contain --
    /// `revert` must refuse outright, mirroring `cs2_config::revert`'s own
    /// `ok_or_else` guard on `original_text`.
    #[tokio::test]
    async fn revert_refuses_when_revert_path_is_missing_from_the_inverse() {
        let project_dir = tempfile::tempdir().unwrap();
        let mock = MockController::new();
        let rec = JournalRecord {
            seq: 1,
            ts_unix_ms: 0,
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
            op: Op::CustomScriptApply,
            target: serde_json::json!({}),
            new: serde_json::json!({}),
            inverse: serde_json::json!({ "revert_sha256": "aaaa" }),
            applied: true,
        };

        let err = revert(&rec, project_dir.path(), &mock, &ctx())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("revert_path"), "{err}");
        assert!(
            mock.run_script_calls().is_empty(),
            "revert must never have run"
        );
    }

    /// Same guard, but for a missing `revert_sha256` -- without it, revert
    /// would have nothing to verify the (attacker- or corruption-controlled)
    /// script against before running it as Administrator.
    #[tokio::test]
    async fn revert_refuses_when_revert_sha256_is_missing_from_the_inverse() {
        let project_dir = tempfile::tempdir().unwrap();
        write_script(project_dir.path(), "revert.bat", "@echo off\r\n");
        let mock = MockController::new();
        let rec = JournalRecord {
            seq: 1,
            ts_unix_ms: 0,
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
            op: Op::CustomScriptApply,
            target: serde_json::json!({}),
            new: serde_json::json!({}),
            inverse: serde_json::json!({ "revert_path": "revert.bat" }),
            applied: true,
        };

        let err = revert(&rec, project_dir.path(), &mock, &ctx())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("revert_sha256"), "{err}");
        assert!(
            mock.run_script_calls().is_empty(),
            "revert must never have run"
        );
    }
}
