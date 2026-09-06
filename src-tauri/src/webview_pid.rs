//! `RunConfig.webview_root_pid` — docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §2.2's "quiet window" suspends the
//! whole WebView2 process tree around each measure iteration's capture, so
//! Tauri's own UI doesn't steal CPU/GPU cycles mid-measurement. Tauri 2.x's
//! default WebView2 configuration spawns WebView2's renderer/GPU helper
//! processes as descendants of THIS app's own process -- so this app's own
//! pid is already the correct tree root; `suspend_process_tree`
//! (`docs/superpowers/plans/2026-09-01-m1-phase-3b-cs2-control-run-loop.md`,
//! `system::windows::process::collect_tree`) walks the whole descendant
//! tree from there, not just the exact pid given.

#[cfg(windows)]
pub fn current_process_id() -> u32 {
    // SAFETY: no arguments, no preconditions; always sound.
    unsafe { windows::Win32::System::Threading::GetCurrentProcessId() }
}
