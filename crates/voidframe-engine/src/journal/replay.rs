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
/// Only `RegistryWrite`/`RegistryDelete` have a meaningful apply-side guard
/// today (`require_m1_supported`'s NUL/length/`..`-segment/`\Enum\` checks
/// on `subkey`/`value_name`); the `Powercfg`/`PowerPlan*` branches of
/// `require_m1_supported` are unconditional `Ok(())`, so there is nothing to
/// replicate for those ops without inventing a check the apply side doesn't
/// itself have.
fn validate_inverse(rec: &JournalRecord) -> std::result::Result<(), String> {
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
            };
            crate::model::module::Module::Registry(payload)
                .require_m1_supported()
                .map_err(|e| format!("seq {}: inverse rejected: {e}", rec.seq))
        }
        Op::PowercfgWrite | Op::PowerPlanActivate | Op::PowerPlanCreate | Op::PowerPlanDelete => {
            Ok(())
        }
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
/// `inverse` is first checked by `validate_inverse` (private to this module) against the same
/// guards the apply side enforces; a record that fails validation is never
/// reverted at all. A validation failure, revert failure, or verify failure
/// is pushed to `verify_failures` — never thrown — so one bad record does
/// not abort the rest of the rollback. Records with `applied: false` are
/// still reverted defensively (the mutation may or may not have landed).
pub async fn revert_all(journal_path: &Path, sys: &dyn SystemController) -> Result<RevertReport> {
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
        match revert_record(rec, sys, &ctx).await {
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
        Op::PowerPlanCreate | Op::PowerPlanDelete => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Journal;
    use crate::model::module::{Hive, RegType, RegistryPayload};
    use crate::mutation::apply_module;
    use crate::system::{MockController, RegValue, SystemController};

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
            &mock,
            &mut j,
            &ctx(),
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
            }),
            &mock,
            &mut j,
            &ctx(),
        )
        .await
        .unwrap();
        drop(j);

        let report = revert_all(&jp, &mock).await.unwrap();
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
            &mock,
            &mut j,
            &ctx(),
        )
        .await
        .unwrap();
        drop(j);
        // Sabotage: make the revert write silently fail once so the value stays at 1.
        mock.fail_next_write("sabotage");
        let report = revert_all(&jp, &mock).await.unwrap();
        assert_eq!(report.verify_failures.len(), 1);
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

        let report = revert_all(&jp, &mock).await.unwrap();
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
}
