//! Real per-process CPU affinity via `OpenProcess` + `SetProcessAffinityMask`.
//! Verified against the current process before this file was written (see
//! `docs/superpowers/plans/2026-09-01-m1-phase-3a-windows-controller.md`'s
//! process-affinity task) — safe, self-contained round trip (set, verify,
//! restore), no other process touched.

use crate::error::{Error, Result};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_SET_INFORMATION, SetProcessAffinityMask,
};

fn set_sync(pid: u32, mask: u64) -> Result<()> {
    // SetProcessAffinityMask is inherently single-group (64-bit-wide) on
    // x64 Windows — M1's affinity presets (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §4.3: pcore_only / ccd0 /
    // exclude_core0 / explicit_mask) never need more than one processor
    // group on realistic consumer hardware. `mask as usize` below is
    // lossless on x64 (both are 64-bit); a future >64-logical-processor
    // machine would need SetProcessGroupAffinity instead, out of scope here.

    // SAFETY: `pid` is a plain integer; the returned handle is owned by this
    // function and closed exactly once, below, on every path (the
    // `CloseHandle` call a few lines down).
    let handle = unsafe {
        OpenProcess(
            PROCESS_SET_INFORMATION | PROCESS_QUERY_INFORMATION,
            false,
            pid,
        )
    }
    .map_err(|e| Error::msg(format!("OpenProcess({pid}) failed: {e}")))?;

    // SAFETY: `handle` is a live, owned handle from `OpenProcess` above;
    // `mask as usize` is a plain integer, not a pointer.
    let result = unsafe { SetProcessAffinityMask(handle, mask as usize) };

    // Always close the handle, even if the mask set failed — don't leak it
    // on the error path.
    // SAFETY: `handle` is the same owned handle from `OpenProcess` above,
    // closed here exactly once.
    let close_result = unsafe { CloseHandle(handle) };

    result.map_err(|e| {
        Error::msg(format!(
            "SetProcessAffinityMask(pid={pid}, mask={mask:#x}) failed: {e}"
        ))
    })?;
    close_result.map_err(|e| Error::msg(format!("CloseHandle after affinity set failed: {e}")))?;
    Ok(())
}

pub async fn set(pid: u32, mask: u64) -> Result<()> {
    tokio::task::spawn_blocking(move || set_sync(pid, mask))
        .await
        .map_err(|e| Error::msg(format!("affinity set task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Threading::{GetCurrentProcessId, GetProcessAffinityMask};

    fn current_mask() -> (usize, usize) {
        // SAFETY: no arguments, no preconditions; always sound.
        let pid = unsafe { GetCurrentProcessId() };
        // SAFETY: `pid` is a plain integer (the current process's own pid,
        // just obtained above); the returned handle is owned by this
        // function and closed exactly once, below.
        let handle = unsafe {
            OpenProcess(
                PROCESS_SET_INFORMATION | PROCESS_QUERY_INFORMATION,
                false,
                pid,
            )
        }
        .unwrap();
        let mut process_mask: usize = 0;
        let mut system_mask: usize = 0;
        // SAFETY: `handle` is a live, owned handle from `OpenProcess` above;
        // `process_mask`/`system_mask` are `&mut` locals that outlive this
        // call.
        unsafe { GetProcessAffinityMask(handle, &mut process_mask, &mut system_mask) }.unwrap();
        // SAFETY: `handle` is the same owned handle from `OpenProcess`
        // above, closed here exactly once.
        unsafe { CloseHandle(handle) }.unwrap();
        (process_mask, system_mask)
    }

    // Self-contained: only ever touches the test process's own affinity,
    // and restores it before returning even if a later assertion would
    // otherwise leave it changed — matches the pattern already verified by
    // hand during this plan's pre-writing check (set -> verify -> restore
    // -> verify, against the real current process).
    #[tokio::test]
    async fn set_then_restore_round_trips_against_the_current_process() {
        // SAFETY: no arguments, no preconditions; always sound.
        let pid = unsafe { GetCurrentProcessId() };
        let (original_mask, _system_mask) = current_mask();
        assert!(original_mask > 0, "process must start with a nonzero mask");

        set(pid, 0x1).await.unwrap();
        let (after_set, _) = current_mask();
        assert_eq!(after_set, 0x1);

        set(pid, original_mask as u64).await.unwrap();
        let (restored, _) = current_mask();
        assert_eq!(restored, original_mask);
    }

    #[tokio::test]
    async fn invalid_pid_returns_an_error_not_a_panic() {
        // PID 0 is the System Idle Process — never a valid OpenProcess
        // target for PROCESS_SET_INFORMATION.
        let e = set(0, 0x1).await.unwrap_err();
        assert!(e.to_string().contains("OpenProcess"));
    }
}
