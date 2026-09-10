//! BitLocker protection status of the system volume via WMI
//! `Win32_EncryptableVolume` (`ProtectionStatus`: 0 off, 1 on, 2 unknown).
//! Same COM/WMI discipline as `src-tauri/src/webview_orphan_cleanup.rs`.

use crate::error::{Error, Result};
use crate::system::BitlockerStatus;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    CoSetProxyBlanket, CoUninitialize, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
    RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Variant::{VARIANT, VariantToInt32};
use windows::Win32::System::Wmi::{
    IWbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY, WbemLocator,
};
use windows::core::{BSTR, w};

fn query_sync() -> Result<BitlockerStatus> {
    // SAFETY: MTA init on this blocking thread, balanced by CoUninitialize below.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|e| Error::msg(format!("CoInitializeEx: {e}")))?;
    let result = (|| -> Result<BitlockerStatus> {
        // SAFETY: CLSID/IID from the windows crate; COM initialised above.
        let locator: IWbemLocator =
            unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER) }
                .map_err(|e| Error::msg(format!("CoCreateInstance(WbemLocator): {e}")))?;
        // SAFETY: all BSTRs live for the call; empty BSTRs = current identity/locale.
        let services = unsafe {
            locator.ConnectServer(
                &BSTR::from(r"ROOT\CIMV2\Security\MicrosoftVolumeEncryption"),
                &BSTR::new(),
                &BSTR::new(),
                &BSTR::new(),
                0,
                &BSTR::new(),
                None,
            )
        }
        .map_err(|e| Error::msg(format!("ConnectServer(MicrosoftVolumeEncryption): {e}")))?;
        // SAFETY: `services` is the just-obtained, valid `IWbemServices` proxy; `None`
        // for `pServerPrincName`/`pAuthInfo` (use the process's own default identity)
        // and `EOAC_NONE` (no extra capabilities) are documented-valid. Without this
        // call, `ExecQuery` below fails with `WBEM_E_ACCESS_DENIED` (`0x80041003`) even
        // though `ConnectServer` itself succeeded -- confirmed live against this exact
        // `IWbemLocator` -> `ConnectServer` -> `ExecQuery` sequence in
        // `src-tauri/src/webview_orphan_cleanup.rs`; this is the same standard,
        // universally-documented step every WMI C++ consumer performs on the proxy
        // before issuing any query.
        unsafe {
            CoSetProxyBlanket(
                &services,
                RPC_C_AUTHN_WINNT,
                RPC_C_AUTHZ_NONE,
                None,
                RPC_C_AUTHN_LEVEL_CALL,
                RPC_C_IMP_LEVEL_IMPERSONATE,
                None,
                EOAC_NONE,
            )
        }
        .map_err(|e| Error::msg(format!("CoSetProxyBlanket: {e}")))?;
        let system_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let wql = format!(
            "SELECT ProtectionStatus FROM Win32_EncryptableVolume WHERE DriveLetter = '{system_drive}'"
        );
        // SAFETY: live services proxy; BSTRs alive for the call.
        let enumerator = unsafe {
            services.ExecQuery(
                &BSTR::from("WQL"),
                &BSTR::from(wql.as_str()),
                WBEM_FLAG_FORWARD_ONLY | WBEM_FLAG_RETURN_IMMEDIATELY,
                None,
            )
        }
        .map_err(|e| Error::msg(format!("ExecQuery(Win32_EncryptableVolume): {e}")))?;
        let mut objs = [None; 1];
        let mut returned = 0u32;
        // SAFETY: `objs` has room for the one object requested; `returned` is a valid out-slot.
        unsafe { enumerator.Next(-1, &mut objs, &mut returned) }
            .ok()
            .map_err(|e| Error::msg(format!("IEnumWbemClassObject::Next: {e}")))?;
        let Some(obj) = objs[0].take() else {
            return Ok(BitlockerStatus::Unknown);
        };
        let mut value = VARIANT::default();
        // SAFETY: property name is a literal wide string; `value` is a valid out VARIANT.
        unsafe { obj.Get(w!("ProtectionStatus"), 0, &mut value, None, None) }
            .map_err(|e| Error::msg(format!("Get(ProtectionStatus): {e}")))?;
        // SAFETY: `value` was written by a successful Get.
        let status = unsafe { VariantToInt32(&value) }
            .map_err(|e| Error::msg(format!("VariantToInt32(ProtectionStatus): {e}")))?;
        Ok(match status {
            0 => BitlockerStatus::Off,
            1 => BitlockerStatus::On,
            _ => BitlockerStatus::Unknown,
        })
    })();
    // SAFETY: balances the CoInitializeEx above on the same thread.
    unsafe { CoUninitialize() };
    result
}

pub async fn protection_status() -> Result<BitlockerStatus> {
    tokio::task::spawn_blocking(query_sync)
        .await
        .map_err(|e| Error::msg(format!("bitlocker query panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "real WMI query against this machine's BitLocker state -- run manually: `cargo test -p voidframe-engine --lib system::windows::bitlocker::tests::live_status_is_a_known_variant -- --ignored --exact --nocapture`"]
    async fn live_status_is_a_known_variant() {
        let s = protection_status().await.unwrap();
        println!("{s:?}");
    }
}
