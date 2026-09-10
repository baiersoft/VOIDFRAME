//! Steam/CS2 process-lifecycle helpers: waiting for Steam to close/become
//! ready, launching Steam de-elevated if it isn't running, polling for a
//! named process to appear, and killing CS2 (gracefully via its own `quit`
//! console command, or by force). Shared by `super::scenario` and
//! `super::execute`.

use crate::error::{Error, Result};
use crate::system::SystemController;
use std::time::Duration;

pub(super) async fn wait_until_steam_closed(sys: &dyn SystemController) -> Result<()> {
    for _ in 0..60 {
        if !sys.steam_status().await?.running {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(Error::msg("timed out waiting for Steam to close".into()))
}

pub(super) async fn wait_until_steam_ready(sys: &dyn SystemController) -> Result<()> {
    for _ in 0..60 {
        let s = sys.steam_status().await?;
        if s.running && !s.elevated {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(Error::msg(
        "timed out waiting for Steam to become ready".into(),
    ))
}

/// How long `ensure_steam_running` waits for `steam.exe` to become
/// discoverable after firing `launch_deelevated` — same poll budget as
/// `wait_until_steam_ready`/`wait_until_steam_closed` just above.
const STEAM_LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);

/// Ensures Steam is running (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §2.1a rev 6), launching it de-elevated
/// via `launch_deelevated` if it isn't already up. Does nothing if Steam
/// is already running -- whether it's *elevated* is a separate
/// warn-not-block decision each caller makes (§4.4, §7.2 step 1), not this
/// helper's concern. Returns `Err` if Steam's install path can't be
/// resolved, the resolved `steam.exe` doesn't actually exist on disk (a
/// stale registry value), `launch_deelevated` itself fails (e.g. no
/// interactive desktop), or Steam doesn't become discoverable within
/// `STEAM_LAUNCH_TIMEOUT` -- every caller turns that into today's manual
/// "please start Steam" fallback, never a hard, silent failure.
pub(super) async fn ensure_steam_running(sys: &dyn SystemController) -> Result<()> {
    if sys.steam_status().await?.running {
        return Ok(());
    }
    let steam_exe = sys.steam_install_path().await?.join("steam.exe");
    // `-silent`: Steam's own flag to start minimized to the tray without
    // ever popping its main window up in the first place -- confirmed live
    // to be more reliable than the close_steam_window() fallback below on
    // its own, which depends on the window existing at the moment it's
    // polled for and on Steam's "close button minimizes" setting being on.
    sys.launch_deelevated(&steam_exe.to_string_lossy(), "-silent")
        .await?;
    wait_for_steam_and_webhelper(sys).await?;

    // Fallback for whatever `-silent` doesn't cover (e.g. a first-run EULA
    // or update prompt) -- close any window Steam did pop up (WM_CLOSE, not
    // a real quit; relies on Steam's own "Close button minimizes Steam
    // instead of exiting" setting) so it doesn't sit in the way of a
    // benchmark run. A no-op when `-silent` already left nothing to close.
    sys.close_steam_window().await?;
    // Safety net: closing a window only hides it to the tray if that Steam
    // setting is actually enabled -- if it isn't, WM_CLOSE fully exits the
    // client, and the caller's next `launch_cs2` would otherwise fail with
    // the same confusing 60-second timeout this whole fix exists to avoid.
    // Re-check both processes are still there, and fail clearly right here
    // instead of silently continuing toward that dead end.
    if sys.find_process("steam.exe").await?.is_none()
        || sys.find_process("steamwebhelper.exe").await?.is_none()
    {
        return Err(Error::msg(
            "Steam appears to have exited after closing its window -- check Steam's \"Close \
             button minimizes Steam instead of exiting\" setting"
                .into(),
        ));
    }
    Ok(())
}

/// Waits for BOTH `steam.exe` and `steamwebhelper.exe` to become
/// discoverable, on one shared deadline (not two sequential
/// `STEAM_LAUNCH_TIMEOUT` waits, which would double the worst-case wait to
/// 120s). `steamwebhelper.exe` is Steam's own CEF-based UI process, spawned
/// as part of the client's own startup -- its presence is a real proxy that
/// Steam has actually gotten somewhere, unlike `steam.exe`'s bare existence
/// (satisfied the instant the process is created, well before Steam is able
/// to service a `steam://run/` request) or `steamservice.exe` (a persistent
/// background Windows service independent of the client's own lifecycle --
/// deliberately not used here). Confirmed live to matter the same way
/// `input.rs`'s `wait_for_visible_window` doc comment already documents for
/// CS2's own window: "the process existing is not sufficient."
async fn wait_for_steam_and_webhelper(sys: &dyn SystemController) -> Result<()> {
    let deadline = tokio::time::Instant::now() + STEAM_LAUNCH_TIMEOUT;
    loop {
        let steam = sys.find_process("steam.exe").await?;
        let webhelper = sys.find_process("steamwebhelper.exe").await?;
        if steam.is_some() && webhelper.is_some() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            let missing = match (steam.is_some(), webhelper.is_some()) {
                (false, false) => "steam.exe and steamwebhelper.exe",
                (false, true) => "steam.exe",
                (true, false) => "steamwebhelper.exe",
                (true, true) => unreachable!(),
            };
            return Err(Error::msg(format!(
                "{missing} did not become discoverable within {STEAM_LAUNCH_TIMEOUT:?}"
            )));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Polls for `name` to become discoverable, up to `timeout`.
///
/// `kill_late` gates the timeout-boundary cleanup below and must be chosen
/// per call site: CS2 call sites pass `true` because an untracked cs2.exe
/// that appears right at the boundary would otherwise leak with no cleanup
/// path (see the comment inside the timeout branch). `ensure_steam_running`
/// passes `false` -- for Steam, a process that appears right at the
/// boundary IS the success case, and killing it directly would violate
/// this feature's core invariant that VOIDFRAME never force-closes an
/// already-running Steam just because it took a little longer than
/// expected to become discoverable (§2.1a rev 6).
pub(super) async fn wait_for_process(
    sys: &dyn SystemController,
    name: &str,
    timeout: Duration,
    kill_late: bool,
) -> Result<crate::system::ProcHandle> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(p) = sys.find_process(name).await? {
            return Ok(p);
        }
        if tokio::time::Instant::now() >= deadline {
            // `launch_cs2` may have succeeded but CS2 took just over
            // `timeout` to become discoverable. Neither CS2 call site that
            // uses this helper ever records a pid it never got back from
            // here, so a process that appears right around this boundary
            // would otherwise go completely untracked — no cleanup path
            // would ever kill it. One more check right here, and kill it
            // if it's now found (only when `kill_late` says this call site
            // wants that), narrows (without eliminating — a process
            // appearing after this final check is still missed) that
            // window.
            if kill_late && let Ok(Some(late)) = sys.find_process(name).await {
                tracing::warn!(
                    pid = late.pid,
                    name,
                    "process appeared right at the wait_for_process timeout boundary — \
                     killing it since it was never tracked for cleanup"
                );
                let _ = kill_process_tree(sys, late.pid).await;
            }
            return Err(Error::msg(format!(
                "{name} did not appear within {timeout:?}"
            )));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Thin wrapper around `SystemController::kill_process_tree`, kept as its
/// own free function to match this module's other small, single-purpose
/// helpers and their call-site readability (`kill_process_tree(sys, pid)`
/// reads the same as the other `wait_*`/`run_*` helpers here).
pub(super) async fn kill_process_tree(sys: &dyn SystemController, pid: u32) -> Result<()> {
    sys.kill_process_tree(pid).await
}

/// Grace period `graceful_kill_cs2` gives CS2 to act on its own `quit`
/// command before falling back to `kill_process_tree`. `quit` typically
/// takes CS2 well under this on a normal exit; generous mainly so a
/// slightly slower shutdown (e.g. saving settings) isn't mistaken for
/// "graceful quit didn't work."
const GRACEFUL_QUIT_TIMEOUT: Duration = Duration::from_secs(10);

/// Closes CS2 for a scenario that's ending normally (not a watchdog-hung
/// one — see below): try CS2's own `quit` console command first, and only
/// force-terminate if it's still running after `GRACEFUL_QUIT_TIMEOUT`.
///
/// Confirmed live: `kill_process_tree` alone can silently fail to actually
/// kill CS2 — its `TerminateProcess` call was denied outright by Windows
/// (`ACCESS_DENIED`) against a genuinely-running cs2.exe, even though
/// `OpenProcess` itself succeeded. That pattern matches an anti-cheat
/// protection (CS2 uses VAC) blocking external termination of a live
/// session — `quit` is CS2's own, sanctioned shutdown path, so it isn't
/// subject to that same protection.
///
/// Deliberately NOT used for the watchdog's own kill-and-relaunch retry
/// (`run_one_iteration`) or `run_scenario`'s error-path cleanup: both of
/// those fire specifically because CS2 is presumed unresponsive (a
/// watchdog timeout) or in an unknown state (mid-failure) — a graceful
/// `quit` attempt there would likely just burn `GRACEFUL_QUIT_TIMEOUT`
/// waiting on a game that was never going to respond, delaying an
/// already-bad situation instead of helping it.
pub(super) async fn graceful_kill_cs2(sys: &dyn SystemController) -> Result<()> {
    // Best-effort: if the console mechanism itself fails (cs2.exe already
    // gone, window not found, whatever), that's not fatal here — just fall
    // straight through to the poll-then-force-kill logic below, the same
    // as if this attempt had simply not worked.
    let _ = sys.quit_cs2_gracefully().await;

    // Poll by NAME ("is there any cs2.exe at all"), not by tracking one
    // specific pid — confirmed from real prior experience (an earlier
    // VOIDFRAME build, different OS): `quit` can sometimes fail to fully
    // close CS2 in a way that leaves Steam believing the game is still
    // running (blocking every subsequent launch attempt), with no
    // guarantee whatever's left behind still carries the pid this call
    // started with. Checking "is *that exact* pid gone" would miss that
    // case outright.
    let deadline = tokio::time::Instant::now() + GRACEFUL_QUIT_TIMEOUT;
    loop {
        if sys.find_process("cs2.exe").await?.is_none() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // quit didn't fully clear it out — force-kill whatever cs2.exe is
    // ACTUALLY there now (re-queried, not assumed to still be `pid`).
    if let Some(remaining) = sys.find_process("cs2.exe").await? {
        kill_process_tree(sys, remaining.pid).await?;
    }
    Ok(())
}
