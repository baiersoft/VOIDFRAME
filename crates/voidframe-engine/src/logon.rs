//! AutoLogon and Secure Boot facts composed from registry reads (spec §8,
//! planning simplification (a)): no new `SystemController` methods.

use crate::error::Result;
use crate::model::module::Hive;
use crate::system::{RegKey, RegValue, SystemController};

const WINLOGON: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon";
const LOGON_UI: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Authentication\LogonUI";
const SECURE_BOOT_STATE: &str = r"SYSTEM\CurrentControlSet\Control\SecureBoot\State";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogonConfig {
    /// `AutoAdminLogon` is "1".
    pub auto_admin_logon: bool,
    pub default_user_name: Option<String>,
    /// A plain-text `DefaultPassword` value exists (Sysinternals AutoLogon
    /// stores it as an LSA secret instead, which leaves this false).
    pub default_password_present: bool,
    /// `LastLoggedOnUser` starts with `MicrosoftAccount\`.
    pub microsoft_account: bool,
}

fn key(subkey: &str, value_name: &str) -> RegKey {
    RegKey {
        hive: Hive::Hklm,
        subkey: subkey.into(),
        value_name: value_name.into(),
    }
}

fn as_string(v: RegValue) -> Option<String> {
    match v {
        RegValue::Sz(s) | RegValue::ExpandSz(s) => Some(s),
        _ => None,
    }
}

pub async fn read_logon_config(sys: &dyn SystemController) -> Result<LogonConfig> {
    let auto = as_string(sys.read_registry(&key(WINLOGON, "AutoAdminLogon")).await?)
        .is_some_and(|s| s.trim() == "1");
    let user = as_string(sys.read_registry(&key(WINLOGON, "DefaultUserName")).await?)
        .filter(|s| !s.is_empty());
    let pw = sys.read_registry(&key(WINLOGON, "DefaultPassword")).await? != RegValue::Absent;
    let last = as_string(
        sys.read_registry(&key(LOGON_UI, "LastLoggedOnUser"))
            .await?,
    )
    .unwrap_or_default();
    Ok(LogonConfig {
        auto_admin_logon: auto,
        default_user_name: user,
        default_password_present: pw,
        microsoft_account: last.to_ascii_lowercase().starts_with("microsoftaccount\\"),
    })
}

pub async fn secure_boot_enabled(sys: &dyn SystemController) -> Result<Option<bool>> {
    Ok(
        match sys
            .read_registry(&key(SECURE_BOOT_STATE, "UEFISecureBootEnabled"))
            .await?
        {
            RegValue::Dword(v) => Some(v == 1),
            _ => None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::MockController;

    #[tokio::test]
    async fn reads_a_sysinternals_style_autologon_as_safe() {
        let sys = MockController::new()
            .with_registry(&key(WINLOGON, "AutoAdminLogon"), RegValue::Sz("1".into()))
            .with_registry(
                &key(WINLOGON, "DefaultUserName"),
                RegValue::Sz("bench".into()),
            )
            .with_registry(
                &key(LOGON_UI, "LastLoggedOnUser"),
                RegValue::Sz(".\\bench".into()),
            );
        let c = read_logon_config(&sys).await.unwrap();
        assert_eq!(
            c,
            LogonConfig {
                auto_admin_logon: true,
                default_user_name: Some("bench".into()),
                default_password_present: false,
                microsoft_account: false,
            }
        );
    }

    #[tokio::test]
    async fn flags_plain_text_password_and_microsoft_account() {
        let sys = MockController::new()
            .with_registry(&key(WINLOGON, "AutoAdminLogon"), RegValue::Sz("1".into()))
            .with_registry(
                &key(WINLOGON, "DefaultPassword"),
                RegValue::Sz("hunter2".into()),
            )
            .with_registry(
                &key(LOGON_UI, "LastLoggedOnUser"),
                RegValue::Sz("MicrosoftAccount\\a@b.c".into()),
            );
        let c = read_logon_config(&sys).await.unwrap();
        assert!(c.default_password_present && c.microsoft_account);
    }

    #[tokio::test]
    async fn secure_boot_is_none_when_the_state_key_is_absent() {
        assert_eq!(
            secure_boot_enabled(&MockController::new()).await.unwrap(),
            None
        );
        let sys = MockController::new().with_registry(
            &key(SECURE_BOOT_STATE, "UEFISecureBootEnabled"),
            RegValue::Dword(1),
        );
        assert_eq!(secure_boot_enabled(&sys).await.unwrap(), Some(true));
    }
}
