//! `registry` module apply / revert.

use crate::error::{Error, Result};
use crate::journal::{Journal, JournalRecord, Op};
use crate::model::module::{Hive, RegType, RegistryPayload};
use crate::system::{MutationCtx, RegKey, RegValue, SystemController};
use serde_json::json;

pub fn regkey(p: &RegistryPayload) -> RegKey {
    RegKey {
        hive: p.hive,
        subkey: p.subkey.clone(),
        value_name: p.value_name.clone(),
    }
}

/// Convert the payload's untyped JSON `value` into a `RegValue`, checked
/// against `value_type` (per `docs/08-security-model.md` §5: "`value`
/// type-checked to match").
pub fn regvalue_from_payload(p: &RegistryPayload) -> Result<RegValue> {
    Ok(match p.value_type {
        RegType::Dword => RegValue::Dword(
            p.value
                .as_u64()
                .ok_or_else(|| Error::msg("registry DWORD value must be an integer".into()))?
                as u32,
        ),
        RegType::Qword => RegValue::Qword(
            p.value
                .as_u64()
                .ok_or_else(|| Error::msg("registry QWORD value must be an integer".into()))?,
        ),
        RegType::Sz => RegValue::Sz(
            p.value
                .as_str()
                .ok_or_else(|| Error::msg("registry SZ value must be a string".into()))?
                .to_string(),
        ),
        RegType::Binary => {
            let s = p
                .value
                .as_str()
                .ok_or_else(|| Error::msg("registry BINARY value must be a hex string".into()))?;
            if s.len() % 2 != 0 {
                return Err(Error::msg(
                    "registry BINARY value must have an even number of hex digits".into(),
                ));
            }
            let bytes = (0..s.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
                .collect::<std::result::Result<Vec<u8>, _>>()
                .map_err(|_| Error::msg("registry BINARY value is not valid hex".into()))?;
            RegValue::Binary(bytes)
        }
        RegType::ExpandSz => RegValue::ExpandSz(
            p.value
                .as_str()
                .ok_or_else(|| Error::msg("registry EXPAND_SZ value must be a string".into()))?
                .to_string(),
        ),
        RegType::MultiSz => {
            let arr = p.value.as_array().ok_or_else(|| {
                Error::msg("registry MULTI_SZ value must be an array of strings".into())
            })?;
            let strings = arr
                .iter()
                .map(|v| {
                    v.as_str().map(str::to_string).ok_or_else(|| {
                        Error::msg("registry MULTI_SZ value must be an array of strings".into())
                    })
                })
                .collect::<Result<Vec<String>>>()?;
            RegValue::MultiSz(strings)
        }
    })
}

fn value_to_json(v: &RegValue) -> serde_json::Value {
    match v {
        RegValue::Dword(n) => json!({ "type": "DWORD", "value": n }),
        RegValue::Qword(n) => json!({ "type": "QWORD", "value": n }),
        RegValue::Sz(s) => json!({ "type": "SZ", "value": s }),
        RegValue::Binary(b) => json!({ "type": "BINARY", "value": hex(b) }),
        RegValue::ExpandSz(s) => json!({ "type": "EXPAND_SZ", "value": s }),
        RegValue::MultiSz(v) => json!({ "type": "MULTI_SZ", "value": v }),
        RegValue::Absent => json!({ "type": "ABSENT" }),
    }
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn json_to_value(v: &serde_json::Value) -> Result<RegValue> {
    match v.get("type").and_then(|t| t.as_str()) {
        Some("DWORD") => Ok(RegValue::Dword(v["value"].as_u64().unwrap_or(0) as u32)),
        Some("QWORD") => Ok(RegValue::Qword(v["value"].as_u64().unwrap_or(0))),
        Some("SZ") => Ok(RegValue::Sz(
            v["value"].as_str().unwrap_or_default().to_string(),
        )),
        Some("BINARY") => {
            let s = v["value"].as_str().unwrap_or_default();
            let bytes = (0..s.len())
                .step_by(2)
                .filter_map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()))
                .collect();
            Ok(RegValue::Binary(bytes))
        }
        Some("EXPAND_SZ") => Ok(RegValue::ExpandSz(
            v["value"].as_str().unwrap_or_default().to_string(),
        )),
        Some("MULTI_SZ") => Ok(RegValue::MultiSz(
            v["value"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        )),
        Some("ABSENT") => Ok(RegValue::Absent),
        _ => Err(Error::msg(
            "journal registry value has no recognised type".into(),
        )),
    }
}

pub async fn apply(
    p: &RegistryPayload,
    sys: &dyn SystemController,
    journal: &mut Journal,
    ctx: &MutationCtx,
) -> Result<()> {
    let key = regkey(p);
    let new = regvalue_from_payload(p)?;
    let current = sys.read_registry(&key).await?;

    let target =
        json!({ "hive": key.hive.as_str(), "subkey": key.subkey, "value_name": key.value_name });
    let inverse = if current == RegValue::Absent {
        json!({ "kind": "delete", "target": target })
    } else {
        json!({ "kind": "write", "target": target, "value": value_to_json(&current) })
    };

    let seq = journal.record(
        Op::RegistryWrite,
        ctx,
        target.clone(),
        value_to_json(&new),
        inverse,
    )?;
    sys.write_registry(&key, &new, ctx).await?;
    journal.mark_applied(seq)?;
    Ok(())
}

pub async fn revert(
    rec: &JournalRecord,
    sys: &dyn SystemController,
    ctx: &MutationCtx,
) -> Result<()> {
    let inv = &rec.inverse;
    let t = &inv["target"];
    let hive = match t["hive"].as_str() {
        Some("HKLM") => Hive::Hklm,
        Some("HKCU") => Hive::Hkcu,
        _ => return Err(Error::msg("journal inverse target has no hive".into())),
    };
    let key = RegKey {
        hive,
        subkey: t["subkey"].as_str().unwrap_or_default().to_string(),
        value_name: t["value_name"].as_str().unwrap_or_default().to_string(),
    };
    match inv["kind"].as_str() {
        Some("delete") => sys.delete_registry_value(&key, ctx).await,
        Some("write") => {
            let v = json_to_value(&inv["value"])?;
            sys.write_registry(&key, &v, ctx).await
        }
        _ => Err(Error::msg("journal inverse has no kind".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::Journal;
    use crate::mutation::{apply_module, conflict_check};
    use crate::system::MockController;

    fn ctx() -> MutationCtx {
        MutationCtx {
            run_id: "r".into(),
            scenario_id: "s".into(),
            step_index: 0,
        }
    }

    fn reg_module(v: i64) -> crate::model::Module {
        crate::model::Module::Registry(RegistryPayload {
            hive: Hive::Hklm,
            subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers".into(),
            value_name: "HwSchMode".into(),
            value_type: RegType::Dword,
            value: serde_json::json!(v),
        })
    }

    #[tokio::test]
    async fn apply_registry_writes_and_journals_inverse_absent() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new();
        apply_module(&reg_module(2), &mock, &mut j, &ctx())
            .await
            .unwrap();

        let k = RegKey {
            hive: Hive::Hklm,
            subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers".into(),
            value_name: "HwSchMode".into(),
        };
        assert_eq!(mock.read_registry(&k).await.unwrap(), RegValue::Dword(2));

        let recs = Journal::load_pending(j.path()).unwrap();
        assert_eq!(recs.len(), 1);
        assert!(recs[0].applied);
        assert_eq!(recs[0].inverse["kind"], "delete");
    }

    #[tokio::test]
    async fn apply_registry_writes_and_journals_inverse_prior_value() {
        let dir = tempfile::tempdir().unwrap();
        let mut j = Journal::open(&dir.path().join("j.jsonl")).unwrap();
        let mock = MockController::new().with_registry(
            Hive::Hklm,
            "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
            "HwSchMode",
            RegValue::Dword(1),
        );
        apply_module(&reg_module(2), &mock, &mut j, &ctx())
            .await
            .unwrap();
        let recs = Journal::load_pending(j.path()).unwrap();
        assert_eq!(recs[0].inverse["kind"], "write");
        assert_eq!(recs[0].inverse["value"]["value"], 1);
    }

    #[test]
    fn conflict_check_rejects_two_modules_same_value() {
        let e = conflict_check(&[reg_module(1), reg_module(2)]).unwrap_err();
        assert!(e.to_string().to_lowercase().contains("conflict"));
    }

    #[test]
    fn regvalue_from_payload_rejects_type_mismatch() {
        let p = RegistryPayload {
            hive: Hive::Hklm,
            subkey: "X".into(),
            value_name: "V".into(),
            value_type: RegType::Dword,
            value: serde_json::json!("not a number"),
        };
        assert!(regvalue_from_payload(&p).is_err());
    }
}
