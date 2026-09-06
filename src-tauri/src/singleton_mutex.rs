//! The real single-instance guard `crates/voidframe-engine/src/paths.rs`'s
//! `acquire_instance_lock` doc comment already anticipated ("The Tauri layer
//! layers a `Global\VOIDFRAME_SINGLETON` named mutex on top in a later
//! phase") but was never actually built -- confirmed needed live 2026-09-05:
//! under the process churn from testing this alpha's other fixes, the
//! engine-level file lock's recorded pid was reused by an unrelated, still-
//! alive process (invisible to a non-elevated diagnostic session, but
//! visible to this app's own elevated `OpenProcess` check), permanently
//! false-positiving every subsequent launch with "another instance is
//! already running" even though the real previous VOIDFRAME instance had
//! long since exited. `paths.rs`'s own doc comment already names this exact
//! race as accepted-for-now, pending this real guard.
//!
//! A named kernel mutex has no such ambiguity: it identifies the actual
//! owning process, not a recyclable numeric pid, and the OS releases it
//! unconditionally the instant that process's last handle to it closes --
//! including on a crash, a hard kill, or (as this same alpha's testing hit)
//! an internal `std::process::exit()` that skips every `Drop` impl in the
//! process, which is exactly the case the file-lock approach can't recover
//! from cleanly.

use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::w;

/// Acquires the process-wide singleton mutex. `Ok(Some(handle))` means this
/// process now owns it (the common case); `Ok(None)` means another live
/// process already does. The returned handle must be kept alive (bound to a
/// variable, never closed) for as long as this process wants to hold the
/// mutex — letting it go out of scope does NOT close it (`HANDLE` has no
/// `Drop`), so simply keeping it bound in `run()`'s own scope for the
/// process's entire lifetime is sufficient; the OS closes it automatically,
/// unconditionally, on process exit regardless of how that exit happens.
///
/// `Global\` (not a per-session name): this app runs elevated, and the
/// intent is one VOIDFRAME instance system-wide, not one per login session.
pub fn acquire() -> anyhow::Result<Option<HANDLE>> {
    // SAFETY: `None` (no security attributes -- default), `true` (this
    // process requests initial ownership), and the `w!()` literal name are
    // all plain, always-valid arguments. The returned handle (on the `Ok`
    // path) is intentionally never closed by this function — see this
    // module's own doc comment for why that's the correct, not leaked,
    // behavior here.
    let handle = unsafe { CreateMutexW(None, true, w!("Global\\VOIDFRAME_SINGLETON")) }
        .map_err(|e| anyhow::anyhow!("CreateMutexW(Global\\VOIDFRAME_SINGLETON) failed: {e}"))?;
    // A non-null handle is returned even when the mutex already existed;
    // `GetLastError() == ERROR_ALREADY_EXISTS` (checked immediately after,
    // per this API's own documented contract) is the only way to tell the
    // two cases apart.
    // SAFETY: `GetLastError` takes no arguments and has no preconditions;
    // called immediately after `CreateMutexW` above, before any other API
    // call could overwrite the calling thread's last-error value.
    if unsafe { windows::Win32::Foundation::GetLastError() } == ERROR_ALREADY_EXISTS {
        // SAFETY: `handle` is the exact handle `CreateMutexW` just returned;
        // this process does not own the mutex (another live process does),
        // so it must not keep this handle open or leave it treated as owned.
        let _ = unsafe { windows::Win32::Foundation::CloseHandle(handle) };
        return Ok(None);
    }
    Ok(Some(handle))
}
