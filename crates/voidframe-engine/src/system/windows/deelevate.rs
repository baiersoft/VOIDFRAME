//! De-elevates a launch by driving the already-running `explorer.exe` over
//! COM (`ExecInExplorer` technique — docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §2.1a rev 6). explorer performs
//! the actual `ShellExecute`, so the child process inherits explorer's own
//! (non-elevated, interactive-user) integrity, regardless of this
//! process's own elevated token. Verified end-to-end against `steam.exe`'s
//! full process tree.
//!
//! Two safety properties this module depends on, both enforced here rather
//! than assumed:
//! - explorer.exe must already be running as the interactive shell before
//!   `CoCreateInstance(ShellWindows, ...)` runs at all — otherwise COM can
//!   activate a *fresh, elevated* explorer instance instead, silently
//!   defeating the whole technique (the child would then inherit an
//!   elevated token, not a Medium-IL one).
//! - the blocking COM call itself is bounded by [`SHELL_EXECUTE_TIMEOUT`] —
//!   a hung explorer, or a UAC prompt it triggers, must not stall the
//!   caller (and therefore the whole run loop) forever.

use crate::error::{Error, Result};
use std::time::Duration;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize, IDispatch, IServiceProvider,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IShellBrowser, IShellDispatch2, IShellFolderViewDual, IShellView, IShellWindows,
    SVGIO_BACKGROUND, SWC_DESKTOP, SWFO_NEEDDISPATCH, ShellWindows,
};
use windows::core::{BSTR, GUID, Interface};

/// `SID_STopLevelBrowser` — the service GUID `IServiceProvider::QueryService`
/// needs to reach the desktop's `IShellBrowser` (`de-elevation-findings.md` §5).
const SID_S_TOP_LEVEL_BROWSER: GUID = GUID::from_u128(0x4c96be40_915c_11cf_99d3_00aa004ae837);

/// Bound on the whole `ShellExecute`-via-explorer round trip (`launch`'s
/// `spawn_blocking`). `ShellExecute` itself is normally near-instant — this
/// exists for the pathological case (a hung explorer.exe, or a UAC prompt it
/// raises that nobody dismisses) that would otherwise stall the caller, and
/// therefore the whole run loop (`run/execute.rs` just awaits `launch`),
/// forever. No existing constant in this crate covers "one blocking COM
/// call"; the closest analog, `run/execute.rs`'s `STEAM_LAUNCH_TIMEOUT`
/// (60s), bounds a *poll loop* waiting for a whole application to start, not
/// a single call — 30s is a judgment call, generous enough for a slow but
/// healthy desktop, short enough not to wedge the run loop for long.
const SHELL_EXECUTE_TIMEOUT: Duration = Duration::from_secs(30);

/// Pure predicate over a live process snapshot (see
/// `process::snapshot_processes_pub`'s `(pid, exe, ppid)` triples) — is
/// `explorer.exe` present at all. Extracted from the CoCreateInstance guard
/// below so it can be unit-tested against a fake process list without
/// touching the real OS.
fn explorer_is_running(processes: &[(u32, String, u32)]) -> bool {
    processes
        .iter()
        .any(|(_, exe, _)| exe.eq_ignore_ascii_case("explorer.exe"))
}

fn launch_via_explorer_sync(program: &str, args: &str) -> Result<()> {
    // Must run before CoCreateInstance(ShellWindows, ...) below: if
    // explorer.exe isn't already the running interactive shell, that call
    // can activate a brand-new, ELEVATED explorer instance instead — and
    // this whole technique's safety property (the child inherits explorer's
    // own non-elevated token) depends entirely on explorer already being
    // the Medium-IL desktop shell, not one COM just spun up on our behalf.
    let processes = crate::system::windows::process::snapshot_processes_pub()?;
    if !explorer_is_running(&processes) {
        return Err(Error::msg(
            "explorer.exe is not running as the interactive shell -- refusing to de-elevate \
             via ExecInExplorer (CoCreateInstance would otherwise be free to activate a fresh, \
             elevated explorer instance, defeating de-elevation entirely)"
                .into(),
        ));
    }

    // SAFETY: `CoInitializeEx` is balanced by the matching `CoUninitialize`
    // below on every path out of this closure (including error returns,
    // since it's outside the `(|| ...)()` closure below); this whole
    // function runs on a `spawn_blocking` worker thread. `windows-native`'s
    // COM caveat about apartment state leaking across pooled
    // `spawn_blocking` threads applies here in principle, but is an
    // existing, unchanged risk this task doesn't newly introduce or widen.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|e| Error::msg(format!("CoInitializeEx failed: {e}")))?;
        let r = (|| -> Result<()> {
            // `ShellWindows`'s CLSID is a well-known Win32 constant;
            // `CLSCTX_LOCAL_SERVER` requests an out-of-process server
            // (explorer.exe's own), never in-process code loaded into this
            // process. The returned `IShellWindows` is a COM-managed
            // reference, released automatically when it drops. (Covered by
            // this function's single outer `// SAFETY:` block above, not a
            // second unsafe block of its own.)
            let shell_windows: IShellWindows =
                CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER).map_err(|e| {
                    Error::msg(format!("CoCreateInstance(ShellWindows) failed: {e}"))
                })?;
            let empty = VARIANT::default();
            let mut hwnd = 0i32;
            let desktop = shell_windows
                .FindWindowSW(&empty, &empty, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH)
                .map_err(|e| Error::msg(format!("FindWindowSW(desktop) failed: {e}")))?;
            let provider: IServiceProvider = desktop
                .cast()
                .map_err(|e| Error::msg(format!("IServiceProvider cast failed: {e}")))?;
            let browser: IShellBrowser = provider
                .QueryService(&SID_S_TOP_LEVEL_BROWSER)
                .map_err(|e| Error::msg(format!("QueryService(IShellBrowser) failed: {e}")))?;
            let view: IShellView = browser
                .QueryActiveShellView()
                .map_err(|e| Error::msg(format!("QueryActiveShellView failed: {e}")))?;
            // Request IDispatch, then cast — requesting IShellFolderViewDual
            // directly returns E_NOINTERFACE (de-elevation-findings.md §5).
            let bg: IDispatch = view
                .GetItemObject::<IDispatch>(SVGIO_BACKGROUND)
                .map_err(|e| Error::msg(format!("GetItemObject(background) failed: {e}")))?;
            let folder: IShellFolderViewDual = bg
                .cast()
                .map_err(|e| Error::msg(format!("IShellFolderViewDual cast failed: {e}")))?;
            let dispatch: IShellDispatch2 = folder
                .Application()
                .map_err(|e| Error::msg(format!("Application() failed: {e}")))?
                .cast()
                .map_err(|e| Error::msg(format!("IShellDispatch2 cast failed: {e}")))?;

            let file = BSTR::from(program);
            let v_args = if args.is_empty() {
                VARIANT::default()
            } else {
                VARIANT::from(BSTR::from(args))
            };
            let none = VARIANT::default();
            dispatch
                .ShellExecute(&file, &v_args, &none, &none, &none)
                .map_err(|e| Error::msg(format!("ShellExecute({program}) failed: {e}")))
        })();
        CoUninitialize();
        r
    }
}

/// Runs the blocking `f` on `spawn_blocking`, bounded by `timeout`. `launch`
/// below is a thin wrapper around this with `SHELL_EXECUTE_TIMEOUT`; the
/// `Duration` is a parameter here (rather than baked in) so a test can drive
/// this same timeout/error path with a millisecond-scale bound against a
/// deliberately slow closure, instead of waiting out the real production
/// timeout.
///
/// On timeout, the underlying `spawn_blocking` task is NOT cancelled — it
/// can't be: tokio has no way to interrupt a blocking OS/COM call already in
/// progress on its worker thread. That task keeps running in the background
/// (harmlessly finishing the `ShellExecute` call, or continuing to wait on
/// whatever hung it) even though this function has already returned `Err`
/// to its caller.
async fn spawn_blocking_with_timeout<F>(timeout: Duration, f: F) -> Result<()>
where
    F: FnOnce() -> Result<()> + Send + 'static,
{
    let handle = tokio::task::spawn_blocking(f);
    match tokio::time::timeout(timeout, handle).await {
        Ok(join_result) => {
            join_result.map_err(|e| Error::msg(format!("launch_deelevated task panicked: {e}")))?
        }
        Err(_) => Err(Error::msg(format!(
            "launch_deelevated timed out after {timeout:?} waiting for explorer.exe's \
             ShellExecute -- possible hung explorer.exe or an unattended UAC prompt it raised"
        ))),
    }
}

pub async fn launch(program: &str, args: &str) -> Result<()> {
    let program = program.to_string();
    let args = args.to_string();
    spawn_blocking_with_timeout(SHELL_EXECUTE_TIMEOUT, move || {
        launch_via_explorer_sync(&program, &args)
    })
    .await
}

// The rest of this module (`launch_via_explorer_sync`'s actual COM chain)
// has no unit tests, same precedent as this module's sibling `steam.rs`'s
// `launch_sync`: raw FFI/COM with nothing further pure to extract, verified
// manually against the rig instead (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §11). The two guards added here
// (the explorer-running check and the timeout) DO have pure/parameterized
// seams, exercised below.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_is_running_rejects_when_explorer_is_not_in_the_process_list() {
        let processes = vec![
            (100, "steam.exe".to_string(), 1),
            (200, "cs2.exe".to_string(), 100),
        ];
        assert!(!explorer_is_running(&processes));
    }

    #[test]
    fn explorer_is_running_accepts_case_insensitively() {
        let processes = vec![(4, "EXPLORER.EXE".to_string(), 0)];
        assert!(explorer_is_running(&processes));
    }

    #[test]
    fn explorer_is_running_accepts_an_empty_list_as_absent() {
        assert!(!explorer_is_running(&[]));
    }

    // Proves the timeout path actually fires, without waiting anywhere near
    // the real `SHELL_EXECUTE_TIMEOUT` (30s): drives
    // `spawn_blocking_with_timeout` with a millisecond-scale `Duration`
    // against a closure that deliberately blocks longer than that bound.
    #[tokio::test]
    async fn spawn_blocking_with_timeout_fires_for_a_closure_that_outlives_it() {
        let result = spawn_blocking_with_timeout(Duration::from_millis(10), || {
            std::thread::sleep(Duration::from_millis(300));
            Ok(())
        })
        .await;

        let err = result.expect_err("a closure that outlives the timeout must return Err");
        assert!(
            err.to_string().contains("timed out"),
            "unexpected error message: {err}"
        );
    }

    #[tokio::test]
    async fn spawn_blocking_with_timeout_passes_through_a_fast_closures_result() {
        let result = spawn_blocking_with_timeout(Duration::from_secs(5), || Ok(())).await;
        assert!(result.is_ok());
    }
}
