//! Simulated console input for CS2 map re-issue — `SendInput` targeting
//! VOIDFRAME's own F9 keybind (see `cs2::keybind_cfg`). Confirmed working
//! via the equivalent PowerShell P/Invoke in `spike/15-sendinput-map-
//! reissue-test.ps1` (`~` key, not F9 — the keybind moved to F12 after
//! that spike per `spike/findings.md` §6b's design correction, then off
//! F12 again to F9 once F12 turned out to collide with Steam's own
//! default screenshot hotkey — the SendInput mechanism itself is
//! identical regardless of which key). The official
//! `windows` crate's own `INPUT`/`KEYBDINPUT`/`MOUSEINPUT` struct
//! definitions are used directly here — the two struct-layout bugs the
//! spike's hand-rolled PowerShell P/Invoke hit cannot recur in this code,
//! since nothing here redefines those structs by hand.
//!
//! The `SendInput` calls below only reach CS2 if CS2's own window is the
//! foreground window at the OS level. `send_console_command_sync` therefore looks
//! CS2's window up explicitly by process name (`cs2.exe`, via the sibling
//! `process` module's process lookup + `EnumWindows`) rather than trusting
//! whatever window happens to have focus already — see `spike/findings.md`
//! §6a: "Bring the target window to the foreground (verify via
//! `GetForegroundWindow`, don't just assume `SetForegroundWindow`
//! succeeded)".

use crate::error::{Error, Result};
use crate::system::windows::process::find_by_name_sync_pub;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, SendInput, VIRTUAL_KEY, VK_ESCAPE, VK_F9, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsWindowVisible, SetForegroundWindow,
};
use windows::core::BOOL;

fn key_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn unicode_char_input(ch: u16, key_up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if key_up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(0),
                wScan: ch,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

fn send(inputs: &[INPUT]) -> Result<()> {
    // SAFETY: `inputs` is a `&[INPUT]` slice, so its pointer and element
    // count (`inputs.len()`) are both known and valid; `size_of::<INPUT>()`
    // is the documented per-element `cbSize`.
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        return Err(Error::msg(format!(
            "SendInput only accepted {sent} of {} events",
            inputs.len()
        )));
    }
    Ok(())
}

/// Gap between successive typed characters — matches
/// `spike/15-sendinput-map-reissue-test.ps1`'s proven-live value
/// (`-Milliseconds 15` between characters) exactly. That spike sent each
/// key's down+up as a single batched `SendInput` call, same as
/// `press_and_release`/`type_unicode` below do — splitting a key's own
/// down/up into two separate calls with a gap between them was tried in
/// this codebase and was never actually proven necessary (the live test it
/// was meant to fix failed for an unrelated reason, see
/// `SETTLE_AFTER_WINDOW_FOUND`'s doc comment); don't reintroduce it without
/// live evidence it's needed.
const BETWEEN_CHARS_GAP: Duration = Duration::from_millis(15);

fn press_and_release(vk: VIRTUAL_KEY) -> Result<()> {
    send(&[key_input(vk, false), key_input(vk, true)])
}

fn type_unicode(text: &str) -> Result<()> {
    for ch in text.encode_utf16() {
        send(&[unicode_char_input(ch, false), unicode_char_input(ch, true)])?;
        std::thread::sleep(BETWEEN_CHARS_GAP);
    }
    Ok(())
}

/// Carries the PID being searched for, and the matching visible top-level
/// window handle once `enum_window_proc` finds one (or `None` if
/// enumeration finishes without a match).
struct FindWindowCtx {
    target_pid: u32,
    found: Option<HWND>,
}

/// `EnumWindows` callback: checks each top-level window's owning process id
/// against `ctx.target_pid`, and records the first visible match. Returning
/// `BOOL(0)` stops enumeration early (found); `BOOL(1)` continues it.
unsafe extern "system" fn enum_window_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the pointer `find_visible_window_for_pid` set to
    // a stack-local `FindWindowCtx` that outlives the synchronous
    // `EnumWindows` call this callback runs inside of, cast back here to
    // the same type it was created as. `hwnd` came from `EnumWindows`
    // itself; `pid` is a `&mut` local sized as passed.
    unsafe {
        let ctx = &mut *(lparam.0 as *mut FindWindowCtx);
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == ctx.target_pid && IsWindowVisible(hwnd).as_bool() {
            ctx.found = Some(hwnd);
            return BOOL(0); // stop enumeration — found it
        }
        BOOL(1) // continue
    }
}

/// The first visible top-level window owned by process `pid`, or `None` if
/// that process has no visible window (not running, or running with no UI
/// yet).
pub(super) fn find_visible_window_for_pid(pid: u32) -> Option<HWND> {
    let mut ctx = FindWindowCtx {
        target_pid: pid,
        found: None,
    };
    // SAFETY: `EnumWindows` is called synchronously, so `ctx` (a stack
    // local) outlives the call; `enum_window_proc` casts the `LPARAM` back
    // to the same `*mut FindWindowCtx` type it is set to here.
    unsafe {
        let _ = EnumWindows(
            Some(enum_window_proc),
            LPARAM(&mut ctx as *mut FindWindowCtx as isize),
        );
    }
    ctx.found
}

/// Carries the title being searched for, and the matching visible top-level
/// window handle once `enum_window_by_title_proc` finds one (or `None` if
/// enumeration finishes without a match).
struct FindWindowByTitleCtx<'a> {
    target_title: &'a str,
    found: Option<HWND>,
}

/// `EnumWindows` callback: checks each visible top-level window's title
/// against `ctx.target_title` (exact match), and records the first one that
/// matches. Same early-stop convention as `enum_window_proc`.
unsafe extern "system" fn enum_window_by_title_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    // SAFETY: `lparam` is the pointer `find_visible_window_by_title` set to
    // a stack-local `FindWindowByTitleCtx` that outlives the synchronous
    // `EnumWindows` call this callback runs inside of, cast back here to
    // the same type it was created as. `hwnd` came from `EnumWindows`
    // itself; `buf` is a `Vec<u16>` sized `len + 1` (from the immediately
    // preceding `GetWindowTextLengthW` call on the same `hwnd`), which
    // `GetWindowTextW` writes into and returns the copied length for.
    unsafe {
        let ctx = &mut *(lparam.0 as *mut FindWindowByTitleCtx);
        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }
        let len = GetWindowTextLengthW(hwnd);
        if len == 0 {
            return BOOL(1);
        }
        let mut buf = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, &mut buf);
        if copied == 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&buf[..copied as usize]);
        if title == ctx.target_title {
            ctx.found = Some(hwnd);
            return BOOL(0); // stop enumeration — found it
        }
        BOOL(1)
    }
}

/// The first visible top-level window whose title (`GetWindowTextW`) is
/// exactly `title`, or `None` if no such window exists right now. Used
/// instead of a pid-based lookup for Steam's main window specifically: its
/// title ("Steam") is stable, but which *process* owns it is not obviously
/// `steam.exe` -- confirmed live (2026-09-04, real Steam client) that the
/// window titled "Steam" is actually owned by one of Steam's own
/// `steamwebhelper.exe` child processes, not `steam.exe` itself. A
/// pid-based lookup rooted at `steam.exe` therefore finds nothing; matching
/// by title sidesteps needing to know which process owns it at all.
pub(super) fn find_visible_window_by_title(title: &str) -> Option<HWND> {
    let mut ctx = FindWindowByTitleCtx {
        target_title: title,
        found: None,
    };
    // SAFETY: `EnumWindows` is called synchronously, so `ctx` (a stack
    // local) outlives the call; `enum_window_by_title_proc` casts the
    // `LPARAM` back to the same `*mut FindWindowByTitleCtx` type it is set
    // to here.
    unsafe {
        let _ = EnumWindows(
            Some(enum_window_by_title_proc),
            LPARAM(&mut ctx as *mut FindWindowByTitleCtx as isize),
        );
    }
    ctx.found
}

/// Same reasoning as `wait_for_visible_window` below, for the title-based
/// lookup: confirmed live (2026-09-04) that `steamwebhelper.exe` becoming
/// discoverable as a process does not mean its "Steam"-titled window exists
/// yet — `close_steam_window` calling `find_visible_window_by_title` once,
/// right after `ensure_steam_running`'s process-existence wait succeeded,
/// found nothing and silently no-op'd (treated as "already closed"), so the
/// window visibly stayed open. TODO(RED): implement.
pub(super) fn wait_for_visible_window_by_title(title: &str, timeout: Duration) -> Option<HWND> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(hwnd) = find_visible_window_by_title(title) {
            return Some(hwnd);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// `find_process`/`wait_for_process`
/// (`docs/superpowers/plans/2026-09-01-m1-phase-3b-cs2-control-run-loop.md`)
/// only confirm the *process*
/// exists — Source 2 games routinely take several seconds after process
/// creation before their main window is actually created (shader/asset
/// init), especially on a cold/first launch. Confirmed live: a real run
/// found `cs2.exe`'s pid immediately but `find_visible_window_for_pid`
/// still returned `None` for it a moment later — the process existing is
/// not sufficient, so this polls rather than checking once.
fn wait_for_visible_window(pid: u32, timeout: Duration) -> Option<HWND> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(hwnd) = find_visible_window_for_pid(pid) {
            return Some(hwnd);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Buffer after CS2's window is found and foregrounded, before sending any
/// input. `run::execute::wait_for_menu_ready` (a real `console.log` signal,
/// checked once per CS2 launch before the first `send_console_command` call) answers
/// the *game-logic* readiness question, but does not replace this: live
/// testing directly disproved the assumption that it made a short local
/// settle sufficient. Shrinking this to 500ms (on that assumption) broke a
/// previously-working run outright (console never opened at all); restoring
/// it back to the value that was actually proven live — a full 10s after
/// the window is found/foregrounded — fixed it again. Treat this as the
/// empirically-required number, not a guess to keep trimming: whatever
/// combination of window-manager/render/focus settling CS2 needs after
/// `SetForegroundWindow` before it will actually process `SendInput`, it
/// needs roughly this long, independent of what `console.log` says.
const SETTLE_AFTER_WINDOW_FOUND: Duration = Duration::from_secs(10);

/// Time for the console panel's own open animation to finish and its text
/// field to actually gain input focus, after the `F9` keypress that opens
/// it. Matches `spike/15-sendinput-map-reissue-test.ps1`'s proven-live
/// `-ConsoleOpenDelayMs` default exactly. Distinct from
/// `SETTLE_AFTER_WINDOW_FOUND` (OS-level foreground-window settle, before
/// anything is pressed) — this is game-UI-level settle, after the first
/// real keypress.
const AFTER_CONSOLE_OPEN: Duration = Duration::from_millis(400);

/// Brief pause after typing, before `Enter`. Matches the spike's
/// `-AfterTypeDelayMs` default exactly.
const BEFORE_SUBMIT: Duration = Duration::from_millis(200);

/// Finds cs2.exe's window, brings it to the foreground (verifying the OS
/// actually agreed, not just trusting the return value), and settles for
/// `settle` before returning. Shared by `send_console_command_sync` (a long settle —
/// this may be the first input sent after a fresh launch/relaunch) and
/// `hide_console_sync` (a short settle — by the time that runs, the caller
/// has already confirmed via `console.log` that CS2 is fully interactive).
fn foreground_cs2(settle: Duration) -> Result<()> {
    let cs2 = find_by_name_sync_pub("cs2.exe")?
        .ok_or_else(|| Error::msg("cs2.exe is not running — cannot send it input".into()))?;

    // 45s, not 20s: a relaunch (watchdog retry) has more overhead than a
    // cold launch (Steam noticing the old process died, tearing it down,
    // starting fresh) — confirmed live, a relaunch exceeded a 20s budget
    // that a first launch had cleared comfortably.
    let target_hwnd = wait_for_visible_window(cs2.pid, Duration::from_secs(45)).ok_or_else(|| {
        Error::msg("cs2.exe is running but never created a visible window within 45s — cannot send it input".into())
    })?;

    // SAFETY: `target_hwnd` came from `wait_for_visible_window` /
    // `EnumWindows` moments ago; `SetForegroundWindow` takes no out-pointers
    // or buffers.
    let _ = unsafe { SetForegroundWindow(target_hwnd) };
    // SAFETY: no arguments, no preconditions; always sound.
    let actual_foreground: HWND = unsafe { GetForegroundWindow() };
    if actual_foreground != target_hwnd {
        return Err(Error::msg(
            "SetForegroundWindow did not actually bring CS2's window to the foreground — cannot guarantee input reaches CS2".into(),
        ));
    }

    std::thread::sleep(settle);
    Ok(())
}

fn send_console_command_sync(command: &str) -> Result<()> {
    foreground_cs2(SETTLE_AFTER_WINDOW_FOUND)?;

    press_and_release(VK_F9)?; // toggleconsole (open — see keybind_cfg for why toggleconsole, not showconsole)
    // The console panel itself animates open and needs to actually gain
    // input focus before typed characters land in its text field — confirmed
    // live that firing the next step immediately (zero gap) after a
    // *working* console-open keypress still produced no typed text, no
    // Enter, and no console.log output at all.
    std::thread::sleep(AFTER_CONSOLE_OPEN);

    type_unicode(command)?;
    std::thread::sleep(BEFORE_SUBMIT);

    press_and_release(VK_RETURN)?;

    // Deliberately does NOT close the console here. A real cold map load
    // can take far longer than any fixed delay chosen up front, and a
    // fixed delay picked to close "safely" was confirmed live to close too
    // early — leaving the console open through the rest of the load. The
    // caller (`run::execute`) closes it via `hide_console`, once it has
    // confirmed from a real `console.log` marker that the load actually
    // finished, rather than guessing when that is from here.
    Ok(())
}

pub async fn send_console_command(command: &str) -> Result<()> {
    let command = command.to_string();
    tokio::task::spawn_blocking(move || send_console_command_sync(&command))
        .await
        .map_err(|e| Error::msg(format!("send_console_command task panicked: {e}")))?
}

/// Settle after re-foregrounding CS2's window, before sending `Escape`.
/// Much shorter than `SETTLE_AFTER_WINDOW_FOUND`: by the time this runs the
/// caller has already confirmed, from a real `console.log` marker, that
/// CS2 is fully interactive — this only covers `SetForegroundWindow`'s own
/// brief OS-level settle. Confirmed live end-to-end at 300ms (matching
/// `spike/15-sendinput-map-reissue-test.ps1`'s foreground-settle value),
/// then 120ms; trimmed further to 20ms per live feedback. Still a fixed
/// guess, not a signal-based wait — there is no `console.log` line for
/// "the OS finished re-foregrounding a window" to wait on instead — so
/// treat this as a value to keep tuning from live results, not a proven
/// floor the way `SETTLE_AFTER_WINDOW_FOUND` is. If this turns out too
/// short (Escape sent before the OS is actually ready, so the console
/// doesn't close), that will look like the same symptom the toggleconsole
/// desync bug did — a console left open into the next iteration.
const SETTLE_BEFORE_HIDE: Duration = Duration::from_millis(20);

/// Closes CS2's console via `Escape` — Source engine's standard "dismiss
/// whatever's open" key, confirmed live to close the console even mid map
/// load (laggier there than at rest, but it lands). Not the same key as
/// the `F12`/`toggleconsole` bind that opens it: `Escape` needs no custom
/// `.cfg` entry, and — the actual point — is a dedicated close action
/// rather than a toggle, so it never depends on assuming the console is
/// currently open the way a second `F12` press would (see
/// `cs2::keybind_cfg`'s `CFG_CONTENTS` doc comment for how that desynced
/// in practice).
fn hide_console_sync() -> Result<()> {
    foreground_cs2(SETTLE_BEFORE_HIDE)?;
    press_and_release(VK_ESCAPE)?;
    Ok(())
}

pub async fn hide_console() -> Result<()> {
    tokio::task::spawn_blocking(hide_console_sync)
        .await
        .map_err(|e| Error::msg(format!("hide_console task panicked: {e}")))?
}

/// Asks CS2 to quit via its own console `quit` command — CS2's own,
/// sanctioned shutdown path — rather than external termination. Confirmed
/// live: `TerminateProcess` against a genuinely-running cs2.exe can be
/// denied outright by Windows (`ACCESS_DENIED`, `OpenProcess` itself still
/// succeeds) — the pattern matches an anti-cheat protection (CS2 uses VAC)
/// blocking external termination of a live session, not a VOIDFRAME bug.
/// Best-effort: if cs2.exe isn't running, or its window can't be found/
/// foregrounded, this returns `Err` the same way `send_console_command` would — the
/// caller (`run::execute::graceful_kill_cs2`) treats that as "graceful
/// quit didn't work this time" and falls back to force-killing, not as
/// fatal.
fn quit_cs2_sync() -> Result<()> {
    foreground_cs2(SETTLE_BEFORE_HIDE)?;
    press_and_release(VK_F9)?; // toggleconsole — open
    std::thread::sleep(AFTER_CONSOLE_OPEN);
    type_unicode("quit")?;
    std::thread::sleep(BEFORE_SUBMIT);
    press_and_release(VK_RETURN)?;
    Ok(())
}

pub async fn quit_cs2() -> Result<()> {
    tokio::task::spawn_blocking(quit_cs2_sync)
        .await
        .map_err(|e| Error::msg(format!("quit_cs2 task panicked: {e}")))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_struct_size_matches_native_x64_layout() {
        // The exact class of bug the spike's hand-rolled PowerShell version
        // hit twice (a wrong union size silently rejects every SendInput
        // call with ERROR_INVALID_PARAMETER) — cheap to assert here since
        // the `windows` crate's own INPUT definition is trusted, but
        // asserting the expected size directly makes a future crate-version
        // regression fail loudly in CI rather than silently at runtime.
        assert_eq!(std::mem::size_of::<INPUT>(), 40);
    }

    #[test]
    fn key_input_sets_keyup_flag_correctly() {
        let down = key_input(VK_F9, false);
        let up = key_input(VK_F9, true);
        // SAFETY: `down`/`up` were just built by `key_input`, which always
        // initializes `INPUT_0`'s `ki` variant, so reading `.ki` matches how
        // the union was written.
        assert_eq!(unsafe { down.Anonymous.ki }.dwFlags, KEYBD_EVENT_FLAGS(0));
        // SAFETY: same as above.
        assert_eq!(unsafe { up.Anonymous.ki }.dwFlags, KEYEVENTF_KEYUP);
    }

    #[test]
    fn unicode_char_input_sets_unicode_flag_and_scan_code() {
        let inp = unicode_char_input('A' as u16, false);
        // SAFETY: `inp` was just built by `unicode_char_input`, which always
        // initializes `INPUT_0`'s `ki` variant, so reading `.ki` matches how
        // the union was written.
        let ki = unsafe { inp.Anonymous.ki };
        assert_eq!(ki.wVk, VIRTUAL_KEY(0));
        assert_eq!(ki.wScan, 'A' as u16);
        assert_eq!(ki.dwFlags, KEYEVENTF_UNICODE);
    }

    #[test]
    fn wait_for_visible_window_times_out_rather_than_blocking_forever() {
        // pid 0 (System Idle Process) never owns a window, so this exercises
        // the real retry loop's timeout path end to end — confirms it
        // actually polls (returns None only after roughly the requested
        // duration, not instantly) and does terminate (not an infinite
        // loop), without depending on CS2 or any other specific process.
        let start = Instant::now();
        let result = wait_for_visible_window(0, Duration::from_millis(600));
        let elapsed = start.elapsed();
        assert!(result.is_none());
        assert!(
            elapsed >= Duration::from_millis(600),
            "returned before the requested timeout elapsed: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "took far longer than the requested timeout: {elapsed:?}"
        );
    }

    #[test]
    fn wait_for_visible_window_by_title_times_out_rather_than_blocking_forever() {
        // Same shape as wait_for_visible_window_times_out_rather_than_blocking_forever
        // just below: a title guaranteed never to exist exercises the real
        // retry loop's timeout path end to end -- confirms it actually
        // polls (returns None only after roughly the requested duration,
        // not instantly) and does terminate.
        let start = Instant::now();
        let result = wait_for_visible_window_by_title(
            "voidframe-test-title-that-will-never-exist-8f3a2c91",
            Duration::from_millis(600),
        );
        let elapsed = start.elapsed();
        assert!(result.is_none());
        assert!(
            elapsed >= Duration::from_millis(600),
            "returned before the requested timeout elapsed: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "took far longer than the requested timeout: {elapsed:?}"
        );
    }

    #[test]
    fn find_visible_window_by_title_returns_none_for_a_title_that_does_not_exist() {
        // Same reasoning as the pid-based negative case just below: no
        // offline fixture is practical, so this runs against the machine's
        // real live windows with a title guaranteed not to match anything.
        assert!(
            find_visible_window_by_title("voidframe-test-title-that-will-never-exist-8f3a2c91")
                .is_none()
        );
    }

    #[test]
    fn find_visible_window_for_pid_returns_none_for_a_pid_with_no_window() {
        // A meaningful fully-offline test isn't practical here — the
        // matching logic (`enum_window_proc`) only does anything against
        // real HWNDs handed to it by a real `EnumWindows` call, so there is
        // no fixture to construct in isolation. This test instead runs the
        // real function against the machine's real live windows, targeting
        // pid 0 (the System Idle Process on Windows — guaranteed to never
        // own a visible top-level window) as a negative case: confirms the
        // "no match found" path returns `None` rather than panicking or
        // returning a false positive, without depending on CS2 or any
        // other specific process being present.
        assert!(find_visible_window_for_pid(0).is_none());
    }
}
