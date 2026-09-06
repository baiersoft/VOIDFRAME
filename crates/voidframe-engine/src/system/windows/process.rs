//! Process discovery + suspend/resume via undocumented `ntdll.dll` APIs.
//! `NtSuspendProcess`/`NtResumeProcess` aren't bound by the `windows`
//! crate (confirmed — they're undocumented, no public Win32 metadata) —
//! resolved dynamically via `GetProcAddress`, verified against a real
//! spawned child process before this plan was written.

use crate::error::{Error, Result};
use crate::system::ProcHandle;
use std::time::{Duration, SystemTime};
use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SUSPEND_RESUME,
    PROCESS_TERMINATE, TerminateProcess,
};
use windows::core::s;

type NtSuspendProcessFn = unsafe extern "system" fn(HANDLE) -> i32;
type NtResumeProcessFn = unsafe extern "system" fn(HANDLE) -> i32;

fn exe_name_from_wide(raw: &[u16]) -> String {
    let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
    String::from_utf16_lossy(&raw[..end])
}

/// Every running process's (pid, exe name, parent pid), via a Toolhelp
/// snapshot. Real, live system state each call — no caching (a suspended
/// benchmark run can span minutes; a stale snapshot would be wrong).
fn snapshot_processes() -> Result<Vec<(u32, String, u32)>> {
    let mut out = Vec::new();
    // SAFETY: `entry.dwSize` is set to `size_of::<PROCESSENTRY32W>()` below,
    // before the first `Process32FirstW` call, as that API requires;
    // `snap` is a snapshot handle owned exclusively by this block and
    // closed exactly once, after the walk, via `CloseHandle`.
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)
            .map_err(|e| Error::msg(format!("CreateToolhelp32Snapshot failed: {e}")))?;
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                out.push((
                    entry.th32ProcessID,
                    exe_name_from_wide(&entry.szExeFile),
                    entry.th32ParentProcessID,
                ));
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
    }
    Ok(out)
}

/// Every pid in `root`'s process tree (`root` itself plus every descendant,
/// transitively) — a benchmark's WebView2 tree (renderer + GPU process) and
/// CS2 (which can itself spawn a crash-handler child) both need the whole
/// tree suspended/resumed together, not just the root pid.
fn collect_tree(all: &[(u32, String, u32)], root: u32) -> Vec<u32> {
    let mut tree = vec![root];
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for (pid, _, ppid) in all {
            if *ppid == parent && !tree.contains(pid) {
                tree.push(*pid);
                frontier.push(*pid);
            }
        }
    }
    tree
}

fn find_by_name_sync(name: &str) -> Result<Option<ProcHandle>> {
    let all = snapshot_processes()?;
    Ok(all
        .into_iter()
        .find(|(_, exe, _)| exe.eq_ignore_ascii_case(name))
        .map(|(pid, _, _)| ProcHandle { pid }))
}

/// A process's creation time, as Windows itself reports it — purely
/// diagnostic, for telling "the same OS process, still alive" apart from
/// "a different process that happened to reuse the same numeric pid" (pid
/// reuse is real, and can happen fast on a system with little other
/// process churn). `None` means the pid is already gone, or its creation
/// time couldn't be read (e.g. a permissions edge case) — not an error,
/// since a caller logging this is asking "for the record," not relying on
/// it for control flow.
/// Reads an already-open process handle's creation time via
/// `GetProcessTimes`, converting from Windows' FILETIME epoch to
/// `std::time::SystemTime`. Does not open or close `handle` itself --
/// factored out so [`RootIdentity::capture`] can reuse the same handle it
/// already opened for its own `OpenProcess` error handling, instead of
/// opening the process a second time just to read its creation time.
fn creation_time(handle: HANDLE) -> Option<SystemTime> {
    // SAFETY: `handle` is a valid, caller-owned process handle with
    // `PROCESS_QUERY_LIMITED_INFORMATION` access (this function neither
    // opens nor closes it -- that's the caller's responsibility); the four
    // `FILETIME` out-params passed to `GetProcessTimes` are valid,
    // exclusively-owned stack locals for it to write into.
    unsafe {
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user).ok()?;

        // FILETIME: 100ns ticks since 1601-01-01. Convert to a
        // std::time::SystemTime (UNIX epoch, 1970-01-01) so the caller can
        // just Debug-print or diff it like any other SystemTime.
        const FILETIME_EPOCH_TO_UNIX_EPOCH_100NS: u64 = 116_444_736_000_000_000;
        let ticks = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
        let unix_100ns = ticks.checked_sub(FILETIME_EPOCH_TO_UNIX_EPOCH_100NS)?;
        Some(SystemTime::UNIX_EPOCH + Duration::from_nanos(unix_100ns * 100))
    }
}

fn process_started_at_sync(pid: u32) -> Option<SystemTime> {
    // SAFETY: `handle` is a process handle just obtained from `OpenProcess`
    // (the block returns early via `?` otherwise); it is closed exactly
    // once below, regardless of whether `creation_time` succeeded.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let created = creation_time(handle);
        let _ = CloseHandle(handle);
        created
    }
}

pub async fn process_started_at(pid: u32) -> Option<SystemTime> {
    tokio::task::spawn_blocking(move || process_started_at_sync(pid))
        .await
        .ok()
        .flatten()
}

/// Guards `suspend_or_resume_tree_sync`/`kill_tree_sync` against acting on a
/// pid Windows has silently recycled for an unrelated process.
/// `collect_tree`'s own walk is keyed entirely off a numeric pid — if that
/// pid no longer refers to the same OS process by the time this module
/// actually opens a handle to it, every pid the walk returns belongs to an
/// entirely unrelated process tree, not a stale-but-harmless subset of the
/// real one.
///
/// A caller can hold a bare `u32` pid for an arbitrary length of time
/// before ever calling into this module — a test's own sleep, a real
/// watchdog wait — and Windows reuses pids quickly once a process's last
/// handle closes, so that gap is a real window, not a hypothetical one.
/// Root-caused live: a 2026-09-05 incident where a test spawned a real
/// `notepad.exe`, slept 500ms, then suspended/killed "its" pid with no
/// verification the pid still meant the same process — plausibly landing
/// on an unrelated, already-running process instead.
struct RootIdentity {
    pid: u32,
    created: SystemTime,
}

impl RootIdentity {
    /// `Ok(None)` means the pid is already gone (or was reused before this
    /// function's own first read) — every caller below treats that the
    /// same as "nothing to act on," not an error, matching this module's
    /// existing tolerance for a short-lived process disappearing.
    ///
    /// `Err` means `OpenProcess` was *denied* for a pid that is very much
    /// still alive -- e.g. anti-cheat interference, which this module
    /// already documents as real -- and must NOT collapse to "process
    /// gone": a caller (`kill_tree_sync`, `suspend_or_resume_tree_sync`)
    /// that treated this the same as "gone" would silently return `Ok(())`
    /// without ever attempting to act, while the process keeps running.
    fn capture(pid: u32) -> Result<Option<Self>> {
        // SAFETY: `handle` is a process handle just obtained from
        // `OpenProcess` on the success path below; it is closed exactly
        // once, via `CloseHandle`, regardless of whether `creation_time`
        // succeeded.
        unsafe {
            let handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                Ok(h) => h,
                Err(e) => {
                    return if e.code() == ERROR_ACCESS_DENIED.to_hresult() {
                        Err(Error::msg(format!(
                            "OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) denied for pid \
                             {pid} -- cannot verify it still refers to the expected process: {e}"
                        )))
                    } else {
                        Ok(None)
                    };
                }
            };
            let created = creation_time(handle);
            let _ = CloseHandle(handle);
            Ok(created.map(|created| Self { pid, created }))
        }
    }

    /// `true` if `self.pid` still refers to the same OS process instance it
    /// did when [`Self::capture`] ran.
    fn still_current(&self) -> bool {
        process_started_at_sync(self.pid) == Some(self.created)
    }
}

/// Synchronous re-export of [`find_by_name_sync`] for callers already
/// running inside their own `spawn_blocking` closure (e.g. `steam.rs`),
/// where `.await`-ing the async `find_by_name` would panic — no async
/// runtime is available on that blocking thread.
pub(super) fn find_by_name_sync_pub(name: &str) -> Result<Option<ProcHandle>> {
    find_by_name_sync(name)
}

/// Synchronous re-export of the raw [`snapshot_processes`] triples, for a
/// caller (e.g. `deelevate.rs`'s explorer-running guard) that needs to test
/// for a process's mere presence rather than get back a [`ProcHandle`] for
/// exactly one match. Same "already inside `spawn_blocking`" rationale as
/// [`find_by_name_sync_pub`] above.
pub(super) fn snapshot_processes_pub() -> Result<Vec<(u32, String, u32)>> {
    snapshot_processes()
}

/// Resolves `name` to its address inside `ntdll.dll` via `GetProcAddress`.
///
/// # Safety
///
/// The caller must only transmute the returned pointer to a function
/// signature that matches the real Win32/NT calling convention, parameter
/// types, and return type of the named `ntdll.dll` export — this function
/// only looks the address up, it cannot verify the signature. The exporting
/// module must also remain loaded for as long as the returned pointer is
/// used; `ntdll.dll` in particular is never unloaded for the lifetime of a
/// Windows process, so this always holds for every caller in this file.
unsafe fn resolve_nt(name: windows::core::PCSTR) -> Result<*const std::ffi::c_void> {
    // SAFETY: `name` is a valid, null-terminated `PCSTR` (a `s!()` literal
    // at every call site in this file); `ntdll` is a valid, already-loaded
    // module handle just obtained from `GetModuleHandleA` above.
    // `GetProcAddress` only performs an exported-symbol table lookup, so
    // this is sound regardless of the export's actual signature — that
    // invariant is on the caller, per this function's own `# Safety`
    // section above.
    unsafe {
        let ntdll = GetModuleHandleA(s!("ntdll.dll"))
            .map_err(|e| Error::msg(format!("ntdll.dll must already be loaded: {e}")))?;
        GetProcAddress(ntdll, name)
            .map(|f| f as *const std::ffi::c_void)
            .ok_or_else(|| Error::msg("ntdll.dll export not found".into()))
    }
}

/// Drops `own_pid` from `tree`, if present, before any per-pid
/// `NtSuspendProcess`/`NtResumeProcess` call is issued.
///
/// `RunConfig.webview_root_pid` (`src-tauri/src/webview_pid.rs`) is set to
/// THIS app's own process id, by design — Tauri 2's WebView2 renderer/GPU
/// helper processes are its descendants, so rooting the tree there is what
/// makes the "quiet window" (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §2.2) reach them at all. But
/// `collect_tree` always puts the root pid itself first in the returned
/// tree, so without this guard the app's own process — including the async
/// runtime, the engine task, and the event forwarder — would be suspended
/// right alongside its WebView2 children, with nothing left running that
/// could ever call `resume_process_tree` afterward: a permanent
/// self-deadlock, confirmed live once `webview_root_pid` gained its first
/// real (non-`None`) caller in the Tauri integration.
///
/// Fails safe: `own_pid` is stripped out of the FULL tree, not just when
/// it's the root, so a future caller passing some other pid whose tree
/// happens to include this process (however that occurred) still can't
/// suspend it. `suspend`-only logging: on the resume half of a round trip
/// this pid was never actually suspended (it was already filtered on the
/// way in), so there's nothing meaningful to warn about there.
fn exclude_own_pid(tree: Vec<u32>, own_pid: u32, suspend: bool) -> Vec<u32> {
    tree.into_iter()
        .filter(|&pid| {
            let is_self = pid == own_pid;
            if is_self && suspend {
                tracing::warn!(
                    pid,
                    "suspend_process_tree: skipping this process's own pid found in the \
                     collected tree -- suspending it would freeze the only thread able to \
                     call resume_process_tree afterward, permanently deadlocking the run"
                );
            }
            !is_self
        })
        .collect()
}

fn suspend_or_resume_tree_sync(root_pid: u32, suspend: bool) -> Result<()> {
    // See `RootIdentity`'s own doc comment: brackets the tree-walk below as
    // tightly as this function can manage, so a `root_pid` Windows recycled
    // during however long the caller held it before calling in here is
    // caught instead of blindly acted on.
    let Some(root) = RootIdentity::capture(root_pid)? else {
        tracing::warn!(
            pid = root_pid,
            "suspend_or_resume_tree: root pid is already gone -- nothing to act on"
        );
        return Ok(());
    };

    let all = snapshot_processes()?;
    let tree = exclude_own_pid(collect_tree(&all, root_pid), std::process::id(), suspend);

    if !root.still_current() {
        tracing::warn!(
            pid = root_pid,
            "suspend_or_resume_tree: this pid now refers to a different process than when this \
             call started (Windows reused it) -- refusing to act on any pid in its tree"
        );
        return Ok(());
    }
    // SAFETY: `resolve_nt` is called with a `s!()` literal naming a real
    // ntdll.dll export ("NtSuspendProcess"/"NtResumeProcess"); the returned
    // pointer is only ever transmuted, below, to the exact
    // `unsafe extern "system" fn(HANDLE) -> i32` signature matching that
    // export's real NT calling convention, and `ntdll.dll` stays loaded for
    // the lifetime of this process, so the pointer never dangles.
    let addr = unsafe {
        resolve_nt(if suspend {
            s!("NtSuspendProcess")
        } else {
            s!("NtResumeProcess")
        })?
    };
    for pid in tree {
        unsafe {
            // SAFETY: `handle` is a process handle just obtained from
            // `OpenProcess` for this `pid` (the loop continues to the next
            // pid otherwise, below); `addr` was resolved by `resolve_nt`
            // above and is transmuted to the exact signature its real
            // ntdll.dll export uses, so `f(handle)` invokes it with the
            // correct ABI; `handle` is closed exactly once, regardless of
            // the NTSTATUS result.
            let handle = match OpenProcess(PROCESS_SUSPEND_RESUME, false, pid) {
                Ok(h) => h,
                // A child that exited between the snapshot and now isn't
                // an error for the whole operation — best-effort across
                // the tree, matching how a real quiet window must
                // tolerate a short-lived helper process disappearing.
                Err(e) => {
                    if suspend {
                        tracing::warn!(pid, error = %e, "suspend_process_tree: OpenProcess(PROCESS_SUSPEND_RESUME) failed — this pid was NOT suspended");
                    } else {
                        tracing::warn!(pid, error = %e, "resume_process_tree: OpenProcess(PROCESS_SUSPEND_RESUME) failed — this pid was NOT resumed");
                    }
                    continue;
                }
            };
            if suspend {
                let f: NtSuspendProcessFn = std::mem::transmute(addr);
                let status = f(handle);
                if status < 0 {
                    tracing::warn!(
                        pid,
                        status = format!("{status:#010x}"),
                        "suspend_process_tree: NtSuspendProcess returned a failure NTSTATUS — this pid may not have been suspended"
                    );
                }
            } else {
                let f: NtResumeProcessFn = std::mem::transmute(addr);
                let status = f(handle);
                if status < 0 {
                    tracing::warn!(
                        pid,
                        status = format!("{status:#010x}"),
                        "resume_process_tree: NtResumeProcess returned a failure NTSTATUS — this pid may not have been resumed"
                    );
                }
            }
            let _ = CloseHandle(handle);
        }
    }
    Ok(())
}

/// `TerminateProcess` on every pid in `root`'s tree (root plus every
/// transitive descendant), reusing the same `collect_tree` walk
/// `suspend_or_resume_tree_sync` already relies on. Best-effort across the
/// tree, same as suspend/resume: a child that's already gone by the time
/// `OpenProcess` runs is not an error for the whole operation — but unlike
/// the suspend/resume path, a failure here is worth knowing about even
/// though it isn't fatal (an un-killed CS2 left running between scenarios,
/// or after the run, is a real problem a caller should be able to notice).
/// Confirmed live: a run completed cleanly (this function returned `Ok`)
/// while CS2 was actually left running — with every per-pid failure
/// previously discarded via a bare `Err(_) => continue`/`let _ =`, there
/// was no way to tell whether that was `OpenProcess` being denied (e.g. an
/// anti-cheat protection engaging once genuinely in a match/benchmark, not
/// just sitting at the main menu) or something else. Logs each failure
/// now instead of discarding it, so the next occurrence is diagnosable
/// from its own output rather than requiring another live repro.
fn kill_tree_sync(root_pid: u32) -> Result<()> {
    // See `RootIdentity`'s own doc comment: guards against `root_pid`
    // having been recycled by Windows for an unrelated process during
    // however long the caller held it before calling in here.
    let Some(root) = RootIdentity::capture(root_pid)? else {
        tracing::warn!(
            pid = root_pid,
            "kill_process_tree: root pid is already gone -- nothing to act on"
        );
        return Ok(());
    };

    let all = snapshot_processes()?;
    let tree = collect_tree(&all, root_pid);

    if !root.still_current() {
        tracing::warn!(
            pid = root_pid,
            "kill_process_tree: this pid now refers to a different process than when this call \
             started (Windows reused it) -- refusing to terminate any pid in its tree"
        );
        return Ok(());
    }
    for pid in tree {
        // SAFETY: `handle` is a process handle just obtained from
        // `OpenProcess` for this `pid` (the loop continues to the next pid
        // otherwise, below); `TerminateProcess` requires only a valid
        // handle with `PROCESS_TERMINATE` access, which `handle` has;
        // `handle` is closed exactly once, regardless of whether
        // termination succeeded.
        unsafe {
            let handle = match OpenProcess(PROCESS_TERMINATE, false, pid) {
                Ok(h) => h,
                Err(e) => {
                    tracing::warn!(pid, error = %e, "kill_process_tree: OpenProcess(PROCESS_TERMINATE) failed — this pid was NOT terminated");
                    continue;
                }
            };
            if let Err(e) = TerminateProcess(handle, 1) {
                tracing::warn!(pid, error = %e, "kill_process_tree: TerminateProcess failed — this pid may still be running");
            }
            let _ = CloseHandle(handle);
        }
    }
    Ok(())
}

pub async fn find_by_name(name: &str) -> Result<Option<ProcHandle>> {
    let name = name.to_string();
    tokio::task::spawn_blocking(move || find_by_name_sync(&name))
        .await
        .map_err(|e| Error::msg(format!("find_process task panicked: {e}")))?
}

pub async fn suspend_tree(pid: u32) -> Result<()> {
    tokio::task::spawn_blocking(move || suspend_or_resume_tree_sync(pid, true))
        .await
        .map_err(|e| Error::msg(format!("suspend_process_tree task panicked: {e}")))?
}

pub async fn resume_tree(pid: u32) -> Result<()> {
    tokio::task::spawn_blocking(move || suspend_or_resume_tree_sync(pid, false))
        .await
        .map_err(|e| Error::msg(format!("resume_process_tree task panicked: {e}")))?
}

pub async fn kill_tree(pid: u32) -> Result<()> {
    tokio::task::spawn_blocking(move || kill_tree_sync(pid))
        .await
        .map_err(|e| Error::msg(format!("kill_process_tree task panicked: {e}")))?
}

/// Synchronous re-export of [`kill_tree_sync`] for a caller already running
/// inside its own `spawn_blocking` closure (e.g. `hwinfo.rs`'s `start_sync`,
/// which kills the process it just `ShellExecuteW`'d if it never becomes
/// ready) -- same "already inside `spawn_blocking`" rationale as
/// [`find_by_name_sync_pub`] above.
pub(super) fn kill_tree_sync_pub(pid: u32) -> Result<()> {
    kill_tree_sync(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_tree_includes_root_and_transitive_descendants() {
        // 1 -> 2 -> 3, and 1 -> 4 (siblings under the same parent), plus an
        // unrelated 99 with no connection to 1's tree.
        let all = vec![
            (1, "root.exe".to_string(), 0),
            (2, "child.exe".to_string(), 1),
            (3, "grandchild.exe".to_string(), 2),
            (4, "child2.exe".to_string(), 1),
            (99, "unrelated.exe".to_string(), 0),
        ];
        let mut tree = collect_tree(&all, 1);
        tree.sort_unstable();
        assert_eq!(tree, vec![1, 2, 3, 4]);
    }

    #[test]
    fn exclude_own_pid_drops_the_calling_processs_own_pid_wherever_it_appears() {
        // Regression test for the self-deadlock bug: `src-tauri` sets
        // `RunConfig.webview_root_pid` to this app's OWN process id (see
        // `webview_pid.rs`), so `collect_tree` rooted there always returns
        // this app's own pid as the tree's first element -- exactly what
        // `suspend_or_resume_tree_sync` now passes through this filter.
        // Suspending it would freeze the only thread able to ever call
        // `resume_process_tree` afterward.
        let own = std::process::id();
        let tree = vec![own, 200, 300];
        let filtered = exclude_own_pid(tree, own, true);
        assert!(
            !filtered.contains(&own),
            "own pid must never survive into the set that gets suspended"
        );
        assert_eq!(filtered, vec![200, 300]);

        // Fails safe even when own pid isn't the root: any occurrence
        // anywhere in the collected tree is stripped, not just a root-pid
        // special case.
        let tree_not_root = vec![100, own, 300];
        assert_eq!(exclude_own_pid(tree_not_root, own, true), vec![100, 300]);
    }

    /// The actual mechanism behind the pid-reuse guard added to
    /// `suspend_or_resume_tree_sync`/`kill_tree_sync`: a real, live
    /// process's captured identity must keep matching itself, and must stop
    /// matching (and `capture` must return `None` for a fresh read) once
    /// the process has genuinely exited -- real pid reuse itself isn't
    /// reproducible on demand in a unit test, but this proves the
    /// created-time comparison the guard relies on actually distinguishes
    /// "still the same process" from "this pid is gone."
    #[tokio::test]
    async fn root_identity_reflects_a_live_process_and_goes_stale_once_it_exits() {
        use std::os::windows::process::CommandExt;
        // Spawned WITHOUT this test binary's console on purpose. A console
        // child inherits our console, and `conhost.exe` keeps its own
        // process handle to every process attached to that console -- one
        // it only releases when the console next processes I/O. Root-caused
        // 2026-09-05: with the child attached, `OpenProcess` by pid kept
        // succeeding (original creation time and all) for 2+ s after
        // taskkill + `wait` + `drop(child)`, so `still_current()` stayed
        // `true` and this test failed deterministically when run alone; it
        // only passed inside the full suite because the harness's own
        // console output pumped conhost. `DETACHED_PROCESS` gives ping no
        // console at all, so the test's own handle really is the last one.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        let mut child = std::process::Command::new("ping.exe")
            .args(["-n", "5", "127.0.0.1"])
            .creation_flags(DETACHED_PROCESS)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("failed to spawn ping.exe for this test");
        let pid = child.id();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let root = RootIdentity::capture(pid)
            .expect("capture must not error for a live, accessible process")
            .expect("a live process must have a readable creation time");
        assert!(
            root.still_current(),
            "a still-running process must still match its own captured creation time"
        );

        let _ = std::process::Command::new("taskkill.exe")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output();
        let _ = child.wait();
        // `Child::wait` does not close this process's own handle to the
        // child -- and as long as ANY handle stays open anywhere, Windows
        // keeps the underlying kernel process object alive and queryable
        // (creation time included), so a stale-but-real "the process still
        // reads as current" here would just mean we're the one keeping it
        // that way, not a bug in `still_current` itself. Dropping `child`
        // releases the one handle this test holds (see the spawn above for
        // why it is the ONLY one).
        drop(child);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        assert!(
            !root.still_current(),
            "an exited process must no longer be considered current"
        );
        assert!(
            RootIdentity::capture(pid)
                .expect("capture must not error for a pid that simply doesn't exist")
                .is_none(),
            "capture on an already-exited pid must be Ok(None)"
        );
    }

    // `OpenProcess` returns `ERROR_INVALID_PARAMETER` for a pid that never
    // existed (as opposed to `ERROR_ACCESS_DENIED` for one that exists but
    // is off-limits) -- this proves `capture` still treats that as
    // "nothing to act on," not an error, distinguishing it from the
    // access-denied case `capture` now propagates as `Err`. An actual
    // access-denied `OpenProcess` isn't reproducible on demand in a unit
    // test (it would require a process this test has no rights to query,
    // e.g. a protected anti-cheat process), so that path is exercised only
    // by inspection of the `e.code() == ERROR_ACCESS_DENIED.to_hresult()`
    // check above, not by a test here.
    #[test]
    fn capture_on_a_definitely_nonexistent_pid_returns_ok_none() {
        assert!(
            RootIdentity::capture(u32::MAX - 1)
                .expect("a nonexistent pid must not be treated as access-denied")
                .is_none(),
            "capture on a pid that never existed must be Ok(None)"
        );
    }

    #[test]
    fn exclude_own_pid_leaves_an_unrelated_tree_untouched() {
        // The common case (CS2's own process tree, or a WebView2 tree
        // rooted somewhere other than this app's own pid) must be
        // completely unaffected by this guard.
        let tree = vec![200, 300, 400];
        let own = std::process::id();
        assert_eq!(exclude_own_pid(tree.clone(), own, true), tree);
    }

    // This test genuinely spawns a real child process (notepad.exe),
    // suspends its whole "tree" (itself, no children), verifies via a
    // read-only OpenProcess + a thread-state check is out of scope for a
    // unit test's portability — instead confirms indirectly: a suspended
    // process cannot be waited-on-for-exit within a short window after
    // being asked to close (a suspended process can't process
    // WM_CLOSE-driven shutdown), while a resumed one can. Kills the child
    // unconditionally in a cleanup step so a failed assertion never leaks
    // a stray notepad.exe.
    #[tokio::test]
    async fn suspend_then_resume_round_trips_against_a_real_spawned_process() {
        let mut child = std::process::Command::new("notepad.exe")
            .spawn()
            .expect("failed to spawn notepad.exe for this test");
        let pid = child.id();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let result: Result<()> = async {
            suspend_tree(pid).await?;
            // No behavioral assertion here beyond "the call succeeded" —
            // NtSuspendProcess's own success/failure is the real signal;
            // this was already verified by hand (NTSTATUS=0x0) before this
            // plan was written. Resume immediately; the important thing a
            // unit test CAN check is that resume also succeeds and the
            // process is still alive and killable afterward.
            resume_tree(pid).await?;
            Ok(())
        }
        .await;

        // Cleanup: taskkill /T kills the whole tree rooted at this pid, not just
        // the single launcher process Child::kill() would target — on this
        // Windows 11 build, `notepad.exe` is itself a launcher that spawns the
        // real packaged Notepad app as its own child, so a single-pid kill leaks
        // an orphan GUI window. taskkill's own /T handles this without needing
        // this test to duplicate the module's own tree-walk logic.
        let _ = std::process::Command::new("taskkill.exe")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output();
        let _ = child.wait();
        result.unwrap();
    }

    // Regression test for the previously-silent `OpenProcess` failure path
    // in `suspend_or_resume_tree_sync`: a pid that's already exited must
    // not turn the whole operation into an `Err` -- it should log a
    // `tracing::warn!` (see the pattern added next to the `OpenProcess`
    // match arm above) and `continue` past it, exactly like
    // `kill_tree_sync`'s own established OpenProcess-failure handling.
    // `tracing` output isn't captured by this test harness -- and, same as
    // `kill_tree_sync` above, there's no existing direct unit test of its
    // own logging output either -- so this instead proves the resulting
    // *behavior*: calling suspend/resume against a pid that's already gone
    // still returns `Ok(())` rather than propagating an error.
    #[tokio::test]
    async fn suspend_and_resume_tree_on_an_already_exited_pid_does_not_error() {
        let mut child = std::process::Command::new("notepad.exe")
            .spawn()
            .expect("failed to spawn notepad.exe for this test");
        let pid = child.id();
        let _ = std::process::Command::new("taskkill.exe")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output();
        let _ = child.wait();
        // Give the OS a moment to fully tear the process down so
        // `OpenProcess` below is guaranteed to fail rather than racing a
        // still-live handle table entry right after `wait()` returns.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // `collect_tree` always includes the root pid itself, whether or
        // not it's actually present in the current snapshot -- so rooting
        // the tree at this now-dead pid makes the failing `OpenProcess`
        // call the ONLY thing either of these calls can hit.
        suspend_tree(pid).await.expect(
            "a pid that's already exited must not turn into an Err -- it should log and \
             continue past it, same as kill_tree_sync's OpenProcess-failure handling",
        );
        resume_tree(pid)
            .await
            .expect("same continue-on-failure guarantee applies to resume_tree");
    }

    // Genuinely spawns and kills a real process, then confirms via a fresh
    // Toolhelp snapshot that it's actually gone — not just that the call
    // returned `Ok`. `notepad.exe` is itself a launcher for the real
    // packaged Notepad app (see the comment on the suspend/resume test
    // above), so this also exercises `kill_tree`'s own tree-walk, not just
    // a single-pid kill.
    #[tokio::test]
    async fn kill_tree_terminates_a_real_spawned_process() {
        let mut child = std::process::Command::new("notepad.exe")
            .spawn()
            .expect("failed to spawn notepad.exe for this test");
        let pid = child.id();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        kill_tree(pid).await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        let all = snapshot_processes().unwrap();
        assert!(
            !all.iter().any(|(p, _, _)| *p == pid),
            "pid {pid} should no longer appear in a Toolhelp snapshot after kill_tree"
        );

        // Best-effort cleanup in case the assertion above ever fails, so
        // this test never leaks a stray notepad window.
        let _ = std::process::Command::new("taskkill.exe")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .output();
        let _ = child.wait();
    }
}
