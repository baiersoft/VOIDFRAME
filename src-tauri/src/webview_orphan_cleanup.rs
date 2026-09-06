//! Cleans up orphaned WebView2 helper processes left behind by a previous
//! VOIDFRAME instance that was killed (e.g. via Task Manager) while its own
//! webview process tree was suspended (`suspend_process_tree`,
//! `crates/voidframe-engine/src/run/execute/scenario.rs`). Confirmed live
//! 2026-09-05: `TerminateProcess` on the parent does not cascade to
//! children, so a suspended WebView2 browser process (plus its
//! renderer/GPU/utility helpers) survives indefinitely as a frozen orphan —
//! and the *next* launch's own WebView2 environment can end up reusing that
//! same frozen browser process (WebView2's platform-level behavior of
//! sharing one browser process per user-data-folder) instead of spawning a
//! fresh one, producing a permanently blank, unresponsive window.
//!
//! Meant to run once, early in `run()`, right after `AppState::new()`
//! succeeds and strictly before `tauri::Builder`'s own webview creation —
//! the single-instance lock `AppState::new()` just acquired confirms no
//! other legitimate VOIDFRAME instance is currently running, so anything
//! matching the marker below at that point can only be a stale leftover.
//!
//! Two real bugs found and fixed live against this machine's actual
//! orphaned processes while building this (2026-09-05), both worth naming
//! since either alone would have made this module either hang the app or
//! silently do nothing:
//! - COM initialized apartment-threaded (`COINIT_APARTMENTTHREADED`,
//!   matching `deelevate.rs`'s convention) instead of `COINIT_MULTITHREADED`
//!   — an STA thread with no message pump (this runs long before any exists)
//!   can hang on a semisynchronous WMI call waiting for a marshaled callback
//!   that never arrives. Fixed by using MTA instead, which these WMI
//!   interfaces (no UI-thread affinity) don't need STA for anyway.
//! - No `CoSetProxyBlanket` call on the `IWbemServices` proxy after
//!   `ConnectServer` — without it, `ExecQuery` fails with
//!   `WBEM_E_ACCESS_DENIED` (`0x80041003`); this is the standard,
//!   universally-documented step every WMI C++ consumer performs before
//!   querying, easy to miss reaching for the interfaces directly instead of
//!   a higher-level wrapper (e.g. PowerShell's `Get-CimInstance`, which
//!   handles it internally — that's why manually confirming the underlying
//!   query with it worked while this module's first version silently found
//!   nothing).

use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    CoSetProxyBlanket, CoUninitialize, EOAC_NONE, RPC_C_AUTHN_LEVEL_CALL,
    RPC_C_IMP_LEVEL_IMPERSONATE,
};
use windows::Win32::System::Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE};
use windows::Win32::System::Variant::{VARIANT, VT_BSTR, VT_I4};
use windows::Win32::System::Wmi::{
    IWbemClassObject, IWbemLocator, WBEM_FLAG_FORWARD_ONLY, WBEM_FLAG_RETURN_IMMEDIATELY,
    WBEM_GENERIC_FLAG_TYPE, WbemLocator,
};
use windows::core::{BSTR, w};

/// WebView2 command-line markers VOIDFRAME's own hosted webview processes
/// carry — confirmed live via `Get-CimInstance Win32_Process` against real
/// orphaned instances (2026-09-05). The main browser/renderer/GPU/utility
/// processes all carry `--webview-exe-name=<name>` (the literal hosting exe
/// name — stable across installs, tied to the compiled binary name, not
/// `productName`/branding), but the `crashpad-handler` helper process does
/// not; it only carries `--user-data-dir=...\<identifier>\EBWebView`
/// (`<identifier>` from `tauri.conf.json`, `com.baiersoft.voidframe`).
/// Matching on either is still specific enough that no other application on
/// the machine could ever emit it.
const WEBVIEW_MARKERS: [&str; 2] = [
    "--webview-exe-name=voidframe.exe",
    r"com.baiersoft.voidframe\EBWebView",
];

/// Reads a `VT_BSTR` variant as a `String`, or `None` for any other variant
/// type (including `VT_NULL`/`VT_EMPTY`, which WMI returns for an absent
/// property).
fn variant_as_string(v: &VARIANT) -> Option<String> {
    // SAFETY: `Anonymous.Anonymous` is only read after confirming `vt ==
    // VT_BSTR` immediately above, which is this union's own documented
    // discriminant for which field is actually initialized.
    unsafe {
        if v.Anonymous.Anonymous.vt == VT_BSTR {
            let bstr = &v.Anonymous.Anonymous.Anonymous.bstrVal;
            Some(bstr.to_string())
        } else {
            None
        }
    }
}

/// Reads a `VT_I4` variant as a `u32` (WMI reports `Win32_Process.ProcessId`
/// as a signed 32-bit integer even though a pid is logically unsigned; a
/// real pid never exceeds `i32::MAX`).
fn variant_as_pid(v: &VARIANT) -> Option<u32> {
    // SAFETY: same discriminant-checked-first pattern as `variant_as_string`.
    unsafe {
        if v.Anonymous.Anonymous.vt == VT_I4 {
            let raw = v.Anonymous.Anonymous.Anonymous.lVal;
            u32::try_from(raw).ok()
        } else {
            None
        }
    }
}

/// Best-effort: a process that's already gone by the time this runs isn't
/// an error for the whole sweep, matching this codebase's established
/// tree-kill convention (`voidframe-engine`'s `kill_tree_sync`).
fn kill_process(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
    // SAFETY: `handle`, if obtained, is a process handle from `OpenProcess`
    // for this exact `pid`; `TerminateProcess` is called with that handle
    // and a plain exit-code constant; `handle` is closed exactly once,
    // regardless of `TerminateProcess`'s result.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_TERMINATE, false, pid) else {
            return false;
        };
        let ok = TerminateProcess(handle, 1).is_ok();
        let _ = CloseHandle(handle);
        ok
    }
}

/// Runs the sweep. Returns the number of orphaned processes killed.
/// Failure to even reach WMI (COM/query setup) is reported so a caller can
/// log it, but is never treated as fatal to app startup — this is a
/// best-effort cleanup of a known failure mode, not a correctness
/// requirement for launching at all.
pub fn cleanup_orphaned_webview_processes() -> anyhow::Result<usize> {
    // SAFETY: `None` (no `pvReserved`) and `COINIT_MULTITHREADED` are both
    // plain, always-valid arguments; the single resulting successful
    // initialization is unconditionally balanced by the single
    // `CoUninitialize` call at the end of this function, on every path out
    // (including an early `?` inside the closure below) — the same "run in
    // a closure, uninitialize after, then propagate" pattern already
    // established in `crates/voidframe-engine/src/system/windows/deelevate.rs`.
    //
    // Deliberately MTA, NOT STA (unlike `deelevate.rs`'s `ShellWindows` use,
    // which genuinely needs STA for that specific shell-UI object):
    // `IWbemLocator`/`IWbemServices` have no UI-thread affinity, and WMI's
    // semisynchronous `ExecQuery`/`IEnumWbemClassObject::Next` marshal
    // results back to the calling apartment from its own worker threads —
    // on an STA thread that requirement can only be satisfied by pumping
    // Windows messages, which this function runs long before any message
    // loop exists (`tauri::Builder`'s own, or otherwise). This is the
    // leading suspect for a real 2026-09-05 report of the app not starting
    // at all after this module was added (an STA-without-pump deadlock is
    // characteristically intermittent, not 100%-reproducible every launch,
    // which fits) -- not confirmed against a live-attached debugger, but
    // MTA is unambiguously the correct choice regardless: these WMI
    // interfaces have no UI-thread affinity, and MTA-to-MTA calls dispatch
    // directly with no message-pump requirement at all.
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|e| anyhow::anyhow!("CoInitializeEx failed: {e}"))?;
    let result = (|| -> anyhow::Result<usize> {
        // SAFETY: `CLSCTX_INPROC_SERVER` requests an in-process COM server
        // — WMI's locator is a standard in-proc DLL activation;
        // `WbemLocator` is this crate's generated CoClass wrapper for the
        // well-known `CLSID_WbemLocator` constant; `None` for `pUnkOuter`
        // (no aggregation) is documented-valid.
        let locator: IWbemLocator = unsafe {
            CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| anyhow::anyhow!("CoCreateInstance(WbemLocator) failed: {e}"))?
        };

        // SAFETY: every `BSTR`/query-string argument is a plain, valid,
        // NUL-free Rust string wrapped by this crate's own `BSTR`
        // conversion; `None` for the optional `IWbemContext` is
        // documented-valid ("no context").
        let services = unsafe {
            locator
                .ConnectServer(
                    &BSTR::from("ROOT\\CIMV2"),
                    &BSTR::new(),
                    &BSTR::new(),
                    &BSTR::new(),
                    0,
                    &BSTR::new(),
                    None,
                )
                .map_err(|e| anyhow::anyhow!(r"ConnectServer(ROOT\CIMV2) failed: {e}"))?
        };

        // SAFETY: `services` is the just-obtained, valid `IWbemServices`
        // proxy; `None` for `pServerPrincName`/`pAuthInfo` (use the
        // process's own default identity) and `EOAC_NONE` (no extra
        // capabilities) are documented-valid. Without this call,
        // `ExecQuery` below fails with `WBEM_E_ACCESS_DENIED`
        // (`0x80041003`) — confirmed live 2026-09-05 — since a freshly
        // `ConnectServer`'d proxy's default authentication/impersonation
        // level isn't sufficient to query `Win32_Process`; this is the
        // standard, universally-documented step every WMI C++ consumer
        // performs on the proxy before issuing any query.
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
            .map_err(|e| anyhow::anyhow!("CoSetProxyBlanket failed: {e}"))?
        };

        let query =
            "SELECT ProcessId, CommandLine FROM Win32_Process WHERE Name = 'msedgewebview2.exe'";
        // SAFETY: same BSTR/context validity reasoning as `ConnectServer`
        // above; the flags are the documented-recommended combination for a
        // synchronous, forward-only, semi-synchronous query.
        let enumerator = unsafe {
            services
                .ExecQuery(
                    &BSTR::from("WQL"),
                    &BSTR::from(query),
                    WBEM_GENERIC_FLAG_TYPE(
                        WBEM_FLAG_FORWARD_ONLY.0 | WBEM_FLAG_RETURN_IMMEDIATELY.0,
                    ),
                    None,
                )
                .map_err(|e| anyhow::anyhow!("ExecQuery failed: {e}"))?
        };

        let mut killed = 0usize;
        loop {
            let mut row: [Option<IWbemClassObject>; 1] = [None];
            let mut returned = 0u32;
            // SAFETY: `row` is a 1-element buffer matching `ltimeout`'s
            // "fetch at most 1" contract; `returned` is a valid `u32`
            // out-pointer. `-1` (`WBEM_INFINITE`) blocks until an object is
            // available or the enumerator is exhausted — acceptable here
            // since this whole sweep is a one-shot, bounded-size startup
            // check, not a long-lived loop.
            let hr = unsafe { enumerator.Next(-1, &mut row, &mut returned) };
            if hr.is_err() || returned == 0 {
                break;
            }
            let Some(obj) = row[0].take() else { break };

            let mut cmdline_var = VARIANT::default();
            // SAFETY: `cmdline_var` is a valid, default-initialized
            // out-pointer; `w!("CommandLine")` is a real WMI property name
            // for `Win32_Process`.
            let cmdline = unsafe {
                obj.Get(w!("CommandLine"), 0, &mut cmdline_var, None, None)
                    .ok()
                    .and_then(|()| variant_as_string(&cmdline_var))
            };

            if cmdline
                .as_deref()
                .is_some_and(|c| WEBVIEW_MARKERS.iter().any(|m| c.contains(m)))
            {
                let mut pid_var = VARIANT::default();
                // SAFETY: same as the `CommandLine` read above, for the
                // `ProcessId` property.
                let pid = unsafe {
                    obj.Get(w!("ProcessId"), 0, &mut pid_var, None, None)
                        .ok()
                        .and_then(|()| variant_as_pid(&pid_var))
                };
                if let Some(pid) = pid
                    && kill_process(pid)
                {
                    killed += 1;
                }
            }
        }
        Ok(killed)
    })();
    // SAFETY: matches the single successful `CoInitializeEx` call above —
    // runs unconditionally, on every path out of the closure.
    unsafe { CoUninitialize() };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Genuinely runs the real WMI/COM sweep against this machine's actual
    /// running processes -- exercises the exact `CoInitializeEx(MTA)` path
    /// `run()` calls at startup, on the same kind of thread `cargo test`
    /// gives it (no message pump), which is what a live-attached debugger
    /// isn't needed to confirm here: if this hangs, so does app startup.
    /// `#[ignore]`d for the same reason `powercfg.rs`'s real-rig tests are
    /// -- mutates real system state (kills matching processes) and needs a
    /// real Windows desktop, not CI.
    #[test]
    #[ignore = "runs a real WMI query and can kill real msedgewebview2.exe processes on this machine -- run manually"]
    fn cleanup_completes_promptly_and_does_not_hang() {
        let start = std::time::Instant::now();
        let result = cleanup_orphaned_webview_processes();
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "took {elapsed:?} -- should complete in well under a second for a query this narrow; \
             anything multi-second suggests the STA-message-pump deadlock this test exists to catch"
        );
        assert!(result.is_ok(), "sweep itself errored: {result:?}");
    }

    #[test]
    fn variant_as_string_reads_a_real_bstr() {
        let variant = VARIANT::from("--webview-exe-name=voidframe.exe --foo");
        assert_eq!(
            variant_as_string(&variant).as_deref(),
            Some("--webview-exe-name=voidframe.exe --foo")
        );
    }

    #[test]
    fn variant_as_string_is_none_for_a_non_bstr_variant() {
        let variant = VARIANT::from(42i32);
        assert_eq!(variant_as_string(&variant), None);
    }

    #[test]
    fn variant_as_pid_reads_a_real_i4() {
        let variant = VARIANT::from(8640i32);
        assert_eq!(variant_as_pid(&variant), Some(8640));
    }

    #[test]
    fn variant_as_pid_is_none_for_a_non_i4_variant() {
        let variant = VARIANT::from("not a pid");
        assert_eq!(variant_as_pid(&variant), None);
    }

    #[test]
    fn variant_as_pid_rejects_a_negative_i4() {
        let variant = VARIANT::from(-1i32);
        assert_eq!(variant_as_pid(&variant), None);
    }
}
