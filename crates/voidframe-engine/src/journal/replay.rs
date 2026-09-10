//! Reverse-replay a journal back to its pre-run state, verifying each revert
//! by reading the system back.

use crate::error::Result;
use crate::journal::{Journal, JournalRecord, Op};
use crate::mutation::revert_record;
use crate::system::{MutationCtx, RegKey, RegValue, SystemController};
use serde::Serialize;
use std::path::Path;

/// Validate a record's `inverse` against the exact same guards the apply
/// side (`Module::require_m1_supported`, via `apply_scenario`) already runs
/// against every mutation before it lands. Journal files live under
/// `%LOCALAPPDATA%`, writable by the *unelevated* user, and Emergency
/// Restore is the button VOIDFRAME's own UI recommends for any problem — so
/// a crafted `inverse.target`/`inverse.value` must be rejected here exactly
/// as it would have been rejected on the way in, not merely trusted because
/// it came from a journal file.
///
/// `RegistryWrite`/`RegistryDelete` and `CustomScriptApply` have a
/// meaningful apply-side guard today (`require_m1_supported`'s
/// NUL/length/`..`-segment/`\Enum\` checks on `subkey`/`value_name` for the
/// registry ops, `validate_script_path`'s character-allowlist/traversal/
/// extension checks for `CustomScriptApply`'s `revert_path`); the
/// `Powercfg`/`PowerPlan*`/`Cs2ConfigApply` branches of
/// `require_m1_supported` are unconditional `Ok(())` (or, for
/// `Cs2ConfigApply`, the inverse is a whole-file text snapshot with no path
/// to validate at all), so there is nothing to replicate for those ops
/// without inventing a check the apply side doesn't itself have.
pub(crate) fn validate_inverse(rec: &JournalRecord) -> std::result::Result<(), String> {
    match rec.op {
        Op::RegistryWrite | Op::RegistryDelete => {
            let t = &rec.inverse["target"];
            let hive = match t["hive"].as_str() {
                Some("HKLM") => crate::model::module::Hive::Hklm,
                Some("HKCU") => crate::model::module::Hive::Hkcu,
                _ => return Err(format!("seq {}: inverse has no hive", rec.seq)),
            };
            // `value_type`/`value` are irrelevant to `require_m1_supported`'s
            // registry branch (it only inspects `subkey`/`value_name`), so a
            // placeholder here is fine — this is a validation-only `Module`,
            // never applied.
            let payload = crate::model::module::RegistryPayload {
                hive,
                subkey: t["subkey"].as_str().unwrap_or_default().to_string(),
                value_name: t["value_name"].as_str().unwrap_or_default().to_string(),
                value_type: crate::model::module::RegType::Dword,
                value: serde_json::Value::Null,
                requires_reboot: false,
            };
            crate::model::module::Module::Registry(payload)
                .require_m1_supported()
                .map_err(|e| format!("seq {}: inverse rejected: {e}", rec.seq))
        }
        Op::CustomScriptApply => {
            // Only `revert_path` is attacker-controlled input worth
            // validating here — `apply_script` is a placeholder never used
            // for anything (this `Module` is validation-only, never
            // applied), but it still has to satisfy `validate_script_path`
            // itself or it would spuriously fail validation regardless of
            // `revert_path`.
            let payload = crate::model::module::CustomScriptPayload {
                apply_script: "placeholder.bat".into(),
                revert_script: rec.inverse["revert_path"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                requires_reboot: false,
                description: String::new(),
            };
            crate::model::module::Module::CustomScript(payload)
                .require_m1_supported()
                .map_err(|e| format!("seq {}: inverse rejected: {e}", rec.seq))
        }
        Op::PowercfgWrite
        | Op::PowerPlanActivate
        | Op::PowerPlanCreate
        | Op::PowerPlanDelete
        | Op::Cs2ConfigApply => Ok(()),
    }
}

/// `Serialize` is required here (not just for internal engine use) because
/// this type crosses the Tauri IPC boundary as a command return value
/// (`rollback_now`/`emergency_rollback` in `src-tauri`). `specta::Type` is
/// required for the same reason — `tauri-specta`'s export needs it to
/// generate this type's real TypeScript shape, not just accept it as an
/// opaque return value.
#[derive(Debug, Default, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RevertReport {
    pub reverted: u32,
    pub verify_failures: Vec<String>,
}

/// Reverse-replay every record in `journal_path` (newest first) through
/// `sys`, verifying each one by reading the value back. Each record's
/// `inverse` is first checked by `validate_inverse` (`pub(crate)` -- also called by
/// `restore_script::render_record` for the same reason) against the same
/// guards the apply side enforces; a record that fails validation is never
/// reverted at all. A validation failure, revert failure, or verify failure
/// is pushed to `verify_failures` — never thrown — so one bad record does
/// not abort the rest of the rollback. Records with `applied: false` are
/// still reverted defensively (the mutation may or may not have landed).
pub async fn revert_all(
    project_dir: &Path,
    journal_path: &Path,
    sys: &dyn SystemController,
) -> Result<RevertReport> {
    let mut records = Journal::load_pending(journal_path)?;
    records.sort_by_key(|r| std::cmp::Reverse(r.seq));
    let mut report = RevertReport::default();
    let ctx = MutationCtx {
        run_id: "revert".into(),
        scenario_id: "revert".into(),
        step_index: 0,
    };

    for rec in &records {
        if let Err(msg) = validate_inverse(rec) {
            report.verify_failures.push(msg);
            continue;
        }
        match revert_record(rec, project_dir, sys, &ctx).await {
            Ok(()) => {
                report.reverted += 1;
                if let Err(msg) = verify(rec, sys).await {
                    report.verify_failures.push(msg);
                }
            }
            Err(e) => {
                report
                    .verify_failures
                    .push(format!("seq {}: revert failed: {e}", rec.seq));
            }
        }
    }
    Ok(report)
}

async fn verify(
    rec: &JournalRecord,
    sys: &dyn SystemController,
) -> std::result::Result<(), String> {
    match rec.op {
        Op::RegistryWrite | Op::RegistryDelete => {
            let t = &rec.inverse["target"];
            let hive = match t["hive"].as_str() {
                Some("HKLM") => crate::model::module::Hive::Hklm,
                Some("HKCU") => crate::model::module::Hive::Hkcu,
                _ => return Err(format!("seq {}: inverse has no hive", rec.seq)),
            };
            let key = RegKey {
                hive,
                subkey: t["subkey"].as_str().unwrap_or_default().to_string(),
                value_name: t["value_name"].as_str().unwrap_or_default().to_string(),
            };
            let got = sys.read_registry(&key).await.map_err(|e| e.to_string())?;
            let want_absent = rec.inverse["kind"].as_str() == Some("delete");
            if want_absent && got != RegValue::Absent {
                return Err(format!(
                    "seq {}: expected absent, found {}",
                    rec.seq,
                    got.describe()
                ));
            }
            Ok(())
        }
        Op::PowercfgWrite => {
            let sub = rec.inverse["sub"].as_str().unwrap_or_default();
            let setting = rec.inverse["setting"].as_str().unwrap_or_default();
            let want = rec.inverse["value"].as_u64().unwrap_or(0) as u32;
            let got = sys
                .read_powercfg(sub, setting)
                .await
                .map_err(|e| e.to_string())?;
            if got.ac != want {
                return Err(format!(
                    "seq {}: {sub}/{setting} expected {want}, found {}",
                    rec.seq, got.ac
                ));
            }
            Ok(())
        }
        Op::PowerPlanActivate => {
            let want = rec.inverse["guid"].as_str().unwrap_or_default();
            let got = sys.active_power_plan().await.map_err(|e| e.to_string())?;
            if got.guid != want {
                return Err(format!(
                    "seq {}: active plan expected {want}, found {}",
                    rec.seq, got.guid
                ));
            }
            Ok(())
        }
        Op::Cs2ConfigApply => {
            let want = rec.inverse["original_text"].as_str().unwrap_or_default();
            let got = sys
                .read_cs2_video_config()
                .await
                .map_err(|e| e.to_string())?;
            if got != want {
                return Err(format!(
                    "seq {}: video.txt expected to match the pre-run snapshot, found a different value",
                    rec.seq
                ));
            }
            Ok(())
        }
        Op::PowerPlanCreate | Op::PowerPlanDelete | Op::CustomScriptApply => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Journal;
    use crate::model::module::{Cs2ConfigPayload, Hive, RegType, RegistryPayload};
    use crate::mutation::apply_module;
    use crate::system::{MockController, RegValue, SystemController};

    /// A throwaway `no_return` shield for `apply_module`/`apply_scenario`:
    /// these tests have no live run whose abort race could need shielding, and
    /// nothing here observes the flag. The receiver is dropped immediately --
    /// the only sender is `power_plan::apply`'s guard, which ignores send
    /// errors.
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
    async fn revert_all_returns_system_to_initial_state() {
        let dir = tempfile::tempdir().unwrap();
        let jp = dir.path().join("j.jsonl");
        let mock = MockController::new().with_powercfg(
            "sub_processor",
            "IDLEDISABLE",
            crate::system::AcDc { ac: 0, dc: 0 },
        );
        let before = mock.snapshot();

        let mut j = Journal::open(&jp).unwrap();
        apply_module(
            &crate::model::Module::Powercfg {
                sub: "sub_processor".into(),
                setting: "IDLEDISABLE".into(),
                value: 1,
            },
            dir.path(),
            &mock,
            &mut j,
            &ctx(),
            &no_return(),
        )
        .await
        .unwrap();
        apply_module(
            &crate::model::Module::Registry(RegistryPayload {
                hive: Hive::Hklm,
                subkey: "SYSTEM\\Control\\GraphicsDrivers".into(),
                value_name: "HwSchMode".into(),
                value_type: RegType::Dword,
                value: serde_json::json!(2),
                requires_reboot: false,
            }),
            dir.path(),
            &mock,
            &mut j,
            &ctx(),
            &no_return(),
        )
        .await
        .unwrap();
        drop(j);

        let report = revert_all(dir.path(), &jp, &mock).await.unwrap();
        assert_eq!(report.reverted, 2);
        assert!(report.verify_failures.is_empty());
        assert_eq!(before, mock.snapshot());
        let k = RegKey {
            hive: Hive::Hklm,
            subkey: "SYSTEM\\Control\\GraphicsDrivers".into(),
            value_name: "HwSchMode".into(),
        };
        assert_eq!(mock.read_registry(&k).await.unwrap(), RegValue::Absent);
    }

    #[tokio::test]
    async fn verify_failure_is_reported_not_thrown() {
        let dir = tempfile::tempdir().unwrap();
        let jp = dir.path().join("j.jsonl");
        let mock = MockController::new();
        let mut j = Journal::open(&jp).unwrap();
        apply_module(
            &crate::model::Module::Powercfg {
                sub: "sub_processor".into(),
                setting: "IDLEDISABLE".into(),
                value: 1,
            },
            dir.path(),
            &mock,
            &mut j,
            &ctx(),
            &no_return(),
        )
        .await
        .unwrap();
        drop(j);
        // Sabotage: make the revert write silently fail once so the value stays at 1.
        mock.fail_next_write("sabotage");
        let report = revert_all(dir.path(), &jp, &mock).await.unwrap();
        assert_eq!(report.verify_failures.len(), 1);
    }

    /// Unlike `CustomScriptApply` (genuinely unverifiable -- no readable
    /// state), `Cs2ConfigApply`'s revert effect is directly checkable: read
    /// `video.txt` back and compare it to the journaled snapshot.
    #[tokio::test]
    async fn verify_succeeds_for_cs2_config_when_video_txt_matches_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let jp = dir.path().join("j.jsonl");
        let mock = MockController::new()
            .with_cs2_video_config("setting.defaultres 1920\nsetting.fullscreen 1\n");
        let mut j = Journal::open(&jp).unwrap();
        let mut settings = std::collections::BTreeMap::new();
        settings.insert("setting.fullscreen".to_string(), "0".to_string());
        apply_module(
            &crate::model::Module::Cs2Config(Cs2ConfigPayload { settings }),
            dir.path(),
            &mock,
            &mut j,
            &ctx(),
            &no_return(),
        )
        .await
        .unwrap();
        drop(j);

        let recs = Journal::load_pending(&jp).unwrap();
        revert_record(&recs[0], dir.path(), &mock, &ctx())
            .await
            .unwrap();

        assert!(verify(&recs[0], &mock).await.is_ok());
    }

    #[tokio::test]
    async fn verify_reports_a_failure_for_cs2_config_when_video_txt_does_not_match() {
        let dir = tempfile::tempdir().unwrap();
        let jp = dir.path().join("j.jsonl");
        let mock = MockController::new()
            .with_cs2_video_config("setting.defaultres 1920\nsetting.fullscreen 1\n");
        let mut j = Journal::open(&jp).unwrap();
        let mut settings = std::collections::BTreeMap::new();
        settings.insert("setting.fullscreen".to_string(), "0".to_string());
        apply_module(
            &crate::model::Module::Cs2Config(Cs2ConfigPayload { settings }),
            dir.path(),
            &mock,
            &mut j,
            &ctx(),
            &no_return(),
        )
        .await
        .unwrap();
        drop(j);

        let recs = Journal::load_pending(&jp).unwrap();
        // Simulate a failed/wrong revert: video.txt ends up with something
        // other than the journaled pre-run snapshot.
        mock.write_cs2_video_config("setting.defaultres 1920\nsetting.fullscreen 1\ncorrupted\n")
            .await
            .unwrap();

        let err = verify(&recs[0], &mock).await.unwrap_err();
        assert!(err.contains("video.txt"), "{err}");
    }

    /// H1 (replay-side validation): a journal file lives under
    /// `%LOCALAPPDATA%`, writable by the unelevated user, and
    /// `rollback_now`/`emergency_rollback` replay it as this elevated
    /// process. A crafted record whose `inverse.target` points at a
    /// `\Enum\` registry subkey — exactly what `require_m1_supported`
    /// already rejects on the apply side — must be rejected here too,
    /// not trusted just because it came from a journal file. This
    /// record is written directly with `Journal::record` (never through
    /// `apply_module`, which would itself have refused to journal it) to
    /// simulate a planted/malformed entry.
    #[tokio::test]
    async fn revert_all_rejects_a_crafted_inverse_targeting_an_enum_subkey() {
        let dir = tempfile::tempdir().unwrap();
        let jp = dir.path().join("j.jsonl");
        let mock = MockController::new();

        let mut j = Journal::open(&jp).unwrap();
        let seq = j
            .record(
                Op::RegistryWrite,
                &ctx(),
                serde_json::json!({}),
                serde_json::json!({}),
                serde_json::json!({
                    "kind": "write",
                    "target": {
                        "hive": "HKLM",
                        "subkey": "SYSTEM\\CurrentControlSet\\Enum\\PCI\\VEN_10DE",
                        "value_name": "DevicePolicy",
                    },
                    "value": { "type": "DWORD", "value": 4 },
                }),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
        drop(j);

        let report = revert_all(dir.path(), &jp, &mock).await.unwrap();
        assert_eq!(report.reverted, 0, "the crafted record must not be applied");
        assert_eq!(report.verify_failures.len(), 1);
        assert!(
            report.verify_failures[0].contains("M3"),
            "{}",
            report.verify_failures[0]
        );

        // Prove it was genuinely never applied, not just under-counted.
        let k = RegKey {
            hive: Hive::Hklm,
            subkey: "SYSTEM\\CurrentControlSet\\Enum\\PCI\\VEN_10DE".into(),
            value_name: "DevicePolicy".into(),
        };
        assert_eq!(mock.read_registry(&k).await.unwrap(), RegValue::Absent);
    }

    /// H1 for `CustomScriptApply` (Finding 3): a journal file lives under
    /// `%LOCALAPPDATA%`, writable by the unelevated user, and `revert`
    /// eventually runs `revert_path` as Administrator. A crafted inverse
    /// whose `revert_path` contains a `..` traversal segment — exactly what
    /// `model/module.rs`'s own `validate_script_path` tests already reject
    /// on the apply side — must be rejected here too, not trusted just
    /// because it came from a journal file.
    #[tokio::test]
    async fn revert_all_rejects_a_crafted_custom_script_inverse_with_a_traversal_path() {
        let dir = tempfile::tempdir().unwrap();
        let jp = dir.path().join("j.jsonl");
        let mock = MockController::new();

        let mut j = Journal::open(&jp).unwrap();
        let seq = j
            .record(
                Op::CustomScriptApply,
                &ctx(),
                serde_json::json!({}),
                serde_json::json!({}),
                serde_json::json!({
                    "apply_sha256": "a",
                    "revert_sha256": "b",
                    "revert_path": "..\\..\\evil.bat",
                }),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
        drop(j);

        let report = revert_all(dir.path(), &jp, &mock).await.unwrap();
        assert_eq!(report.reverted, 0, "the crafted record must not be applied");
        assert_eq!(report.verify_failures.len(), 1);
        assert!(
            report.verify_failures[0].contains("inverse rejected"),
            "{}",
            report.verify_failures[0]
        );
    }

    /// Same H1 guard, but for a `revert_path` carrying a shell
    /// metacharacter (`&`) rather than a traversal segment — the other
    /// rejection case `model/module.rs`'s own tests cover for
    /// `validate_script_path`.
    #[tokio::test]
    async fn revert_all_rejects_a_crafted_custom_script_inverse_with_a_shell_metacharacter() {
        let dir = tempfile::tempdir().unwrap();
        let jp = dir.path().join("j.jsonl");
        let mock = MockController::new();

        let mut j = Journal::open(&jp).unwrap();
        let seq = j
            .record(
                Op::CustomScriptApply,
                &ctx(),
                serde_json::json!({}),
                serde_json::json!({}),
                serde_json::json!({
                    "apply_sha256": "a",
                    "revert_sha256": "b",
                    "revert_path": "evil & calc.exe.bat",
                }),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
        drop(j);

        let report = revert_all(dir.path(), &jp, &mock).await.unwrap();
        assert_eq!(report.reverted, 0, "the crafted record must not be applied");
        assert_eq!(report.verify_failures.len(), 1);
        assert!(
            report.verify_failures[0].contains("inverse rejected"),
            "{}",
            report.verify_failures[0]
        );
    }
}
