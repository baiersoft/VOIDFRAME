//! `read_registry_value` -- a live registry read for the builder's "already
//! your baseline" hint (spec §3.1). Goes through `Module::require_m1_supported`
//! (the same M1-subset gate every other registry-touching path in this app
//! uses) rather than reading the registry unconditionally, so this command
//! can't be used to probe an `\Enum\` key M1 has no SYSTEM token for.

use voidframe_engine::model::module::{Hive, Module, RegType, RegistryPayload};
use voidframe_engine::system::{RegKey, RegValue, SystemController};

#[derive(Debug, serde::Serialize, specta::Type)]
pub struct RegistryValueView {
    pub present: bool,
    /// `{type, value}`, same shape as the journal's own registry records
    /// (`voidframe_engine::mutation::registry::value_to_json`) -- untyped
    /// for the same reason `RegistryPayload::value` is (its shape depends
    /// on the value's registry type).
    #[specta(type = Option<specta_typescript::Unknown>)]
    pub value: Option<serde_json::Value>,
}

pub(crate) async fn read_registry_value_impl(
    sys: &dyn SystemController,
    hive: Hive,
    subkey: &str,
    value_name: &str,
) -> Result<RegistryValueView, String> {
    Module::Registry(RegistryPayload {
        hive,
        subkey: subkey.into(),
        value_name: value_name.into(),
        value_type: RegType::Dword,
        value: serde_json::Value::Null,
        requires_reboot: false,
    })
    .require_m1_supported()
    .map_err(|e| e.to_string())?;
    let v = sys
        .read_registry(&RegKey {
            hive,
            subkey: subkey.into(),
            value_name: value_name.into(),
        })
        .await
        .map_err(|e| e.to_string())?;
    Ok(match v {
        RegValue::Absent => RegistryValueView {
            present: false,
            value: None,
        },
        other => RegistryValueView {
            present: true,
            value: Some(voidframe_engine::mutation::registry::value_to_json(&other)),
        },
    })
}

#[specta::specta]
#[tauri::command]
pub async fn read_registry_value(
    state: tauri::State<'_, crate::state::AppState>,
    hive: Hive,
    subkey: String,
    value_name: String,
) -> Result<RegistryValueView, String> {
    read_registry_value_impl(state.sys.as_ref(), hive, &subkey, &value_name).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::system::MockController;

    fn hwschmode_key() -> RegKey {
        RegKey {
            hive: Hive::Hklm,
            subkey: r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers".into(),
            value_name: "HwSchMode".into(),
        }
    }

    #[tokio::test]
    async fn read_registry_value_reports_present_and_the_journal_shaped_value() {
        let sys = MockController::new().with_registry(&hwschmode_key(), RegValue::Dword(2));
        let view = read_registry_value_impl(
            &sys,
            Hive::Hklm,
            r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers",
            "HwSchMode",
        )
        .await
        .unwrap();
        assert!(view.present);
        assert_eq!(
            view.value,
            Some(serde_json::json!({"type": "DWORD", "value": 2}))
        );
    }

    #[tokio::test]
    async fn read_registry_value_reports_absent_when_the_value_does_not_exist() {
        let sys = MockController::new();
        let view = read_registry_value_impl(
            &sys,
            Hive::Hklm,
            r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers",
            "HwSchMode",
        )
        .await
        .unwrap();
        assert!(!view.present);
        assert_eq!(view.value, None);
    }

    #[tokio::test]
    async fn read_registry_value_rejects_a_traversal_subkey() {
        let sys = MockController::new();
        let err = read_registry_value_impl(&sys, Hive::Hklm, "..\\..\\evil", "V")
            .await
            .unwrap_err();
        assert!(err.contains(".."), "{err}");
    }
}
