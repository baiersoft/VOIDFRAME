//! Steam status + CS2 launch. CS2 always inherits Steam's own integrity
//! level (spike/findings.md §5, §8, §9) — VOIDFRAME never launches or
//! restarts Steam, and performs no token duplication.

use crate::error::{Error, Result};
use crate::system::{Cs2LaunchSpec, SteamStatus};
use std::time::Duration;
use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, WPARAM};
use windows::Win32::Security::{
    GetTokenInformation, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenIntegrityLevel,
};
use windows::Win32::System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_INFORMATION};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SW_SHOWNORMAL, WM_CLOSE};
use windows::core::PCWSTR;

/// Well-known RIDs for the mandatory-integrity SIDs Windows defines
/// (`S-1-16-<RID>`); anything at or above High (0x3000) counts as elevated
/// for this check's purposes (System, 0x4000, is elevated a fortiori).
const SECURITY_MANDATORY_HIGH_RID: u32 = 0x3000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Pure comparison extracted for testability: `true` iff `rid` is at or
/// above High (0x3000) — Low (0x1000) and Medium (0x2000) are not
/// elevated, High (0x3000) and System (0x4000) are.
fn rid_is_elevated(rid: u32) -> bool {
    rid >= SECURITY_MANDATORY_HIGH_RID
}

/// Reads a process's integrity level (`true` = High or above). Returns
/// `Ok(None)` if the process can't be opened (e.g. it exited between
/// discovery and this call, or access is denied — best-effort, matching
/// the spike's own established pattern of degrading to "unknown" rather
/// than failing the whole check).
fn process_is_elevated(pid: u32) -> Result<Option<bool>> {
    // SAFETY: `pid` is a plain integer; `process` is owned by this function
    // and closed exactly once on every path below (on the `OpenProcessToken`
    // failure path, and again after `GetTokenInformation` runs), same for
    // `token` (closed once, right after its last use). `token`/`len`
    // out-pointers are `&mut` locals. The `GetTokenInformation`
    // sizing call (`None` buffer, `&mut len`) is the documented two-call
    // pattern; the following fill call writes into `byte_ptr`, which points
    // into `buf`, a `Vec<u64>` of at least `len` bytes (8-byte aligned,
    // sufficient for `TOKEN_MANDATORY_LABEL`) that outlives the call. The
    // `&*(byte_ptr as *const TOKEN_MANDATORY_LABEL)` cast only runs once
    // `ok` confirms that fill call succeeded. `GetSidSubAuthorityCount`/
    // `GetSidSubAuthority` dereference a `sid` pointer taken from that same
    // populated buffer; the `count == 0` guard above the second call
    // prevents the `count - 1` index from underflowing.
    unsafe {
        let process = match OpenProcess(PROCESS_QUERY_INFORMATION, false, pid) {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let mut token = HANDLE::default();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
            let _ = CloseHandle(process);
            return Ok(None);
        }
        let mut len = 0u32;
        // Sizing call — same two-call pattern established in Plan A.
        let _ = GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut len);
        // `Vec<u64>` instead of `Vec<u8>`: guarantees 8-byte alignment for
        // the `TOKEN_MANDATORY_LABEL` (which itself contains a pointer,
        // requiring 8-byte alignment) we're about to cast this buffer's
        // bytes into — a `Vec<u8>` only guarantees 1-byte alignment, which
        // is technically insufficient (UB by the language rules) even
        // though it works today via the Windows heap's actual
        // over-alignment. Same pattern as `topology.rs`'s own identical
        // fix.
        let word_len = (len as usize).div_ceil(8);
        let mut buf: Vec<u64> = vec![0u64; word_len];
        let byte_ptr = buf.as_mut_ptr() as *mut u8;
        let ok = GetTokenInformation(
            token,
            TokenIntegrityLevel,
            Some(byte_ptr as *mut _),
            len,
            &mut len,
        )
        .is_ok();
        let _ = CloseHandle(token);
        let _ = CloseHandle(process);
        if !ok {
            return Ok(None);
        }
        let label = &*(byte_ptr as *const TOKEN_MANDATORY_LABEL);
        // The SID's sub-authority count tells us where the last RID lives;
        // the integrity RID is always the SID's final sub-authority.
        let sid = label.Label.Sid;
        let count = *windows::Win32::Security::GetSidSubAuthorityCount(sid) as u32;
        if count == 0 {
            // Never happens for a real integrity SID, but costless to
            // guard against the `count - 1` underflow below.
            return Ok(None);
        }
        let rid = *windows::Win32::Security::GetSidSubAuthority(sid, count - 1);
        Ok(Some(rid_is_elevated(rid)))
    }
}

fn status_sync() -> Result<SteamStatus> {
    match find_by_name_sync_shim("steam.exe")? {
        None => Ok(SteamStatus {
            running: false,
            elevated: false,
        }),
        Some(handle) => {
            let elevated = process_is_elevated(handle.pid)?.unwrap_or(false);
            Ok(SteamStatus {
                running: true,
                elevated,
            })
        }
    }
}

// process::find_by_name is async (spawn_blocking-wrapped); status_sync
// itself runs *inside* a spawn_blocking already (see `status()` below), so
// it needs the synchronous snapshot directly rather than awaiting the
// async wrapper from inside a blocking context. Re-uses the same
// snapshot_processes() this module's sibling already has — call through
// process.rs's pub(super) sync helper rather than duplicating the
// Toolhelp-snapshot logic a second time here.
fn find_by_name_sync_shim(name: &str) -> Result<Option<crate::system::ProcHandle>> {
    crate::system::windows::process::find_by_name_sync_pub(name)
}

fn launch_sync(spec: &Cs2LaunchSpec) -> Result<()> {
    let uri = wide(&format!("steam://run/{}/", spec.app_id));
    let operation = wide("open");
    // SAFETY: `operation` and `uri` are local, NUL-terminated `Vec<u16>`s
    // (from `wide()` above) that outlive this call; the other three
    // arguments are `None`/null, no preconditions.
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(operation.as_ptr()),
            PCWSTR(uri.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW returns a value > 32 on success (a fake HINSTANCE, per
    // the documented, if archaic, convention) and <= 32 on failure.
    if (result.0 as isize) <= 32 {
        return Err(Error::msg(format!(
            "ShellExecuteW(steam://run/{}/) failed, code {}",
            spec.app_id, result.0 as isize
        )));
    }
    Ok(())
}

/// Sends `WM_CLOSE` to `pid`'s visible main window, if it has one -- NOT
/// `SW_MINIMIZE` and NOT process termination, just the same message Windows
/// sends when a user clicks a window's X button. A no-op, not an error,
/// when the process has no visible window right now (e.g. already
/// minimized/hidden) -- idempotent, matching this codebase's other
/// window/process helpers. Generic over `pid` so it's testable against a
/// safe, disposable window (e.g. a spawned `notepad.exe`) without needing a
/// real Steam process -- see this module's tests.
fn post_close(hwnd: windows::Win32::Foundation::HWND) {
    // SAFETY: `hwnd` was just returned by a live `EnumWindows` enumeration
    // (by `find_visible_window_for_pid` or `find_visible_window_by_title`),
    // so it names a real window that existed a moment ago. `PostMessageW`
    // queues the message into that window's message loop and returns
    // immediately -- it does not dereference `hwnd` itself beyond the
    // handle value, and a window that closes between the lookup and this
    // call is a message silently dropped by the OS (documented
    // `PostMessageW` behavior for an invalid handle), not undefined
    // behavior.
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
    }
}

// No longer used by production code -- `close_window_sync` below looks
// Steam's window up by title, not by pid, once it turned out Steam's own
// pid doesn't own its main window. Kept, `#[cfg(test)]`-gated, purely to
// exercise the underlying `find_visible_window_for_pid` + `post_close`
// mechanism generically (see this module's `close_window_for_pid_...` test).
#[cfg(test)]
fn close_window_for_pid_sync(pid: u32) -> Result<()> {
    let Some(hwnd) = super::input::find_visible_window_for_pid(pid) else {
        return Ok(());
    };
    post_close(hwnd);
    Ok(())
}

/// The exact title of Steam's main client window -- confirmed live
/// (2026-09-04, real Steam client on this dev rig).
const STEAM_MAIN_WINDOW_TITLE: &str = "Steam";

/// Sends `WM_CLOSE` to Steam's main window specifically -- simulates the
/// user clicking its window's X button. Whether this hides Steam to the
/// tray or fully exits the client depends entirely on Steam's own "Close
/// button minimizes Steam instead of exiting" setting; the caller
/// (`run::execute::steam::ensure_steam_running`) re-checks `find_process`
/// afterward rather than assuming it held.
///
/// Looks the window up **by title, not by `steam.exe`'s pid** -- confirmed
/// live that Steam's main window (titled "Steam") is actually owned by one
/// of its own `steamwebhelper.exe` child processes, not `steam.exe` itself,
/// so a pid-based lookup rooted at `steam.exe` silently finds nothing (the
/// bug this fixes: `close_window_sync` looked like it worked -- no error,
/// safety-net check still passed, since Steam never actually exited -- but
/// never actually closed anything). Steam spawns several `steamwebhelper.exe`
/// instances, so switching to a pid-based lookup keyed on that name instead
/// wouldn't reliably find the right one either; title-based lookup
/// sidesteps needing to know which process owns the window at all.
///
/// Polls for the window rather than checking once, same reasoning as
/// `wait_for_visible_window` already established for CS2's own window:
/// confirmed live that `steamwebhelper.exe` becoming discoverable as a
/// *process* (what `ensure_steam_running`'s readiness wait checks) does not
/// mean its "Steam"-titled *window* exists yet -- a one-shot lookup right
/// after that wait found nothing and silently no-op'd, so the window
/// visibly stayed open despite `ensure_steam_running` reporting success.
/// Best-effort past `WINDOW_APPEAR_TIMEOUT`: closing the window is a
/// cosmetic nicety, not something worth failing the whole launch sequence
/// over, so a timeout logs a warning and returns `Ok` rather than erroring.
const WINDOW_APPEAR_TIMEOUT: Duration = Duration::from_secs(10);

fn close_window_sync() -> Result<()> {
    let Some(hwnd) = super::input::wait_for_visible_window_by_title(
        STEAM_MAIN_WINDOW_TITLE,
        WINDOW_APPEAR_TIMEOUT,
    ) else {
        tracing::warn!(
            "Steam's \"{STEAM_MAIN_WINDOW_TITLE}\" window did not appear within {WINDOW_APPEAR_TIMEOUT:?} -- \
             nothing to close (this doesn't fail the launch, just leaves the window open)"
        );
        return Ok(());
    };
    post_close(hwnd);
    Ok(())
}

pub async fn close_window() -> Result<()> {
    tokio::task::spawn_blocking(close_window_sync)
        .await
        .map_err(|e| Error::msg(format!("close_steam_window task panicked: {e}")))?
}

pub async fn status() -> Result<SteamStatus> {
    tokio::task::spawn_blocking(status_sync)
        .await
        .map_err(|e| Error::msg(format!("steam_status task panicked: {e}")))?
}

pub async fn launch(spec: &Cs2LaunchSpec) -> Result<()> {
    let spec = spec.clone();
    tokio::task::spawn_blocking(move || launch_sync(&spec))
        .await
        .map_err(|e| Error::msg(format!("launch_cs2 task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Proves the real Win32 mechanism (find a window for a pid, send it
    /// `WM_CLOSE`) actually works, without needing a real Steam process --
    /// a spawned `notepad.exe` is meant to be a safe, disposable substitute
    /// with a real top-level window, matching `process.rs`'s own
    /// `kill_tree_terminates_a_real_spawned_process` precedent. `#[ignore]`d:
    /// confirmed live on this Windows 11 build that `notepad.exe`'s own pid
    /// never owns a visible top-level window at all -- it's a thin launcher
    /// that spawns the real packaged Notepad app as its own CHILD process
    /// (the same indirection `process.rs`'s own test comment already
    /// documents for its `taskkill /T`-based cleanup, which sidesteps this
    /// because tree-killing doesn't care which pid owns the window --
    /// `find_visible_window_for_pid` does). Not a portable, CI-safe target
    /// for a window-*lookup* test the way it is for a tree-*kill* test; the
    /// real end-to-end verification of this mechanism is the
    /// `steam-launch-check` CLI command against real Steam, which has no
    /// such launcher indirection.
    #[tokio::test]
    #[ignore = "notepad.exe's own pid doesn't own a visible window on this Windows 11 build (a \
                packaged child process does) -- confirmed live, not a portable test target for \
                find_visible_window_for_pid; run manually and adapt if you need to re-verify, or \
                use `voidframe-cli steam-launch-check` against real Steam instead"]
    async fn close_window_for_pid_closes_a_real_spawned_windows_window() {
        let mut child = std::process::Command::new("notepad.exe")
            .spawn()
            .expect("failed to spawn notepad.exe for this test");
        let pid = child.id();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        assert!(
            super::super::input::find_visible_window_for_pid(pid).is_some(),
            "notepad.exe should have a real visible window shortly after spawning"
        );

        close_window_for_pid_sync(pid).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        assert!(
            super::super::input::find_visible_window_for_pid(pid).is_none(),
            "WM_CLOSE should have closed notepad's window"
        );

        // Best-effort cleanup in case the assertion above ever fails (or
        // notepad's launcher spawned a child process that outlives the
        // window close), so this test never leaks a stray notepad window --
        // same reasoning as `process.rs`'s own real-process tests.
        let _ = std::process::Command::new("taskkill.exe")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output();
        let _ = child.wait();
    }

    #[test]
    fn rid_is_elevated_thresholds_correctly() {
        assert!(!rid_is_elevated(0x1000)); // Low
        assert!(!rid_is_elevated(0x2000)); // Medium
        assert!(rid_is_elevated(0x3000)); // High
        assert!(rid_is_elevated(0x4000)); // System
    }
}
