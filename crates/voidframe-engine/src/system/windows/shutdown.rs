//! Reboot / shutdown / cancel via `InitiateSystemShutdownExW`. The elevated
//! process holds `SeShutdownPrivilege` but it is disabled by default in the
//! token; `enable_shutdown_privilege` turns it on for this process once.

use crate::error::{Error, Result};
use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Security::{
    AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED,
    TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY,
};
use windows::Win32::System::Shutdown::{
    AbortSystemShutdownW, InitiateSystemShutdownExW, SHTDN_REASON_FLAG_PLANNED,
    SHTDN_REASON_MAJOR_APPLICATION, SHTDN_REASON_MINOR_MAINTENANCE,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{PCWSTR, w};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn enable_shutdown_privilege() -> Result<()> {
    let mut token = HANDLE::default();
    // SAFETY: `GetCurrentProcess` returns a pseudo-handle that needs no
    // closing; `token` is a valid out-slot for the duration of the call.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        )
    }
    .map_err(|e| Error::msg(format!("OpenProcessToken: {e}")))?;
    let mut luid = LUID::default();
    // SAFETY: `w!` yields a NUL-terminated wide string that outlives the
    // call; `luid` is a valid out-slot.
    let looked_up =
        unsafe { LookupPrivilegeValueW(PCWSTR::null(), w!("SeShutdownPrivilege"), &mut luid) };
    if let Err(e) = looked_up {
        // SAFETY: `token` was returned by a successful OpenProcessToken.
        let _ = unsafe { CloseHandle(token) };
        return Err(Error::msg(format!("LookupPrivilegeValueW: {e}")));
    }
    let privs = TOKEN_PRIVILEGES {
        PrivilegeCount: 1,
        Privileges: [LUID_AND_ATTRIBUTES {
            Luid: luid,
            Attributes: SE_PRIVILEGE_ENABLED,
        }],
    };
    // SAFETY: `privs` is a fully initialised, correctly sized struct; the
    // optional out-parameters are documented as ignorable when None.
    let adjusted = unsafe { AdjustTokenPrivileges(token, false, Some(&privs), 0, None, None) };
    // SAFETY: `token` was returned by a successful OpenProcessToken.
    let _ = unsafe { CloseHandle(token) };
    adjusted.map_err(|e| Error::msg(format!("AdjustTokenPrivileges: {e}")))
}

fn initiate(reboot: bool, delay_secs: u32, message: &str) -> Result<()> {
    enable_shutdown_privilege()?;
    let msg = wide(message);
    let reason =
        SHTDN_REASON_MAJOR_APPLICATION | SHTDN_REASON_MINOR_MAINTENANCE | SHTDN_REASON_FLAG_PLANNED;
    // SAFETY: `msg` is a NUL-terminated wide buffer alive for the call;
    // machine name None means the local machine; `force = true` closes
    // apps without waiting (CS2 is already closed by the run loop).
    unsafe {
        InitiateSystemShutdownExW(
            PCWSTR::null(),
            PCWSTR(msg.as_ptr()),
            delay_secs,
            true,
            reboot,
            reason,
        )
    }
    .map_err(|e| Error::msg(format!("InitiateSystemShutdownExW(reboot={reboot}): {e}")))
}

pub async fn reboot(delay_secs: u32, message: &str) -> Result<()> {
    let message = message.to_string();
    tokio::task::spawn_blocking(move || initiate(true, delay_secs, &message))
        .await
        .map_err(|e| Error::msg(format!("reboot task panicked: {e}")))?
}

pub async fn shutdown(delay_secs: u32, message: &str) -> Result<()> {
    let message = message.to_string();
    tokio::task::spawn_blocking(move || initiate(false, delay_secs, &message))
        .await
        .map_err(|e| Error::msg(format!("shutdown task panicked: {e}")))?
}

pub async fn cancel() -> Result<()> {
    tokio::task::spawn_blocking(|| {
        enable_shutdown_privilege()?;
        // SAFETY: machine name None = local machine; no pointers involved.
        unsafe { AbortSystemShutdownW(PCWSTR::null()) }
            .map_err(|e| Error::msg(format!("AbortSystemShutdownW: {e}")))
    })
    .await
    .map_err(|e| Error::msg(format!("cancel_shutdown task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabling_the_shutdown_privilege_succeeds_on_an_elevated_test_run() {
        // Fails only if the test process is not elevated; the workspace's
        // own tests already assume an elevated developer shell (registry
        // HKLM round-trips).
        enable_shutdown_privilege().unwrap();
    }

    // Deliberately no live reboot/shutdown test: a mistake would reboot the
    // developer's machine. `cancel()` with nothing pending returns an
    // ERROR_NO_SHUTDOWN_IN_PROGRESS error, which is a usable liveness probe.
    #[tokio::test]
    async fn cancel_with_nothing_pending_reports_no_shutdown_in_progress() {
        let err = cancel().await.unwrap_err();
        assert!(err.to_string().contains("AbortSystemShutdownW"), "{err}");
    }
}
