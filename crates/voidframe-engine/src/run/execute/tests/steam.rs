use super::*;

/// Unit-tests `graceful_kill_cs2` directly rather than through a full
/// `run_scenario` — its poll loop reads `console.log`-independent state
/// (`find_process`), so there's no need to entangle this with
/// `LogTail`'s own "only sees lines written after open" timing, and a
/// full integration test made that entanglement a real, misleading
/// failure (a real bug in the TEST, not in `graceful_kill_cs2`: the
/// stale-cleanup's own poll loop pushed `LogTail::open` later than
/// `write_console_log_lines`' fixed schedule assumed, so lines it wrote
/// landed before the tail opened and were invisible to it — nothing to
/// do with `graceful_kill_cs2`'s own correctness).
///
/// This branch: when `quit_cs2_gracefully` actually works (the process
/// disappears), no `kill_process_tree` fallback should ever fire —
/// confirms the poll loop returns as soon as the process is gone, not
/// after burning `GRACEFUL_QUIT_TIMEOUT` regardless. `tokio::time::
/// pause()` makes this a correctness assertion, not just a speed one:
/// without it, a bug that always waited out the full timeout before
/// checking would still pass, just slowly.
#[tokio::test]
async fn graceful_kill_cs2_skips_the_force_kill_when_quit_actually_works() {
    tokio::time::pause();
    let sys = MockController::new()
        .with_process("cs2.exe", 7777)
        .with_graceful_quit_working();

    graceful_kill_cs2(&sys).await.unwrap();

    assert_eq!(sys.quit_cs2_calls(), 1);
    assert!(
        sys.killed_pids().is_empty(),
        "quit_cs2_gracefully already removed the process — no force-kill fallback needed"
    );
}

/// The other branch: `quit_cs2_gracefully` doesn't remove the process
/// (the mock's default — matches every real run before this session's
/// live-testing found the `TerminateProcess` `ACCESS_DENIED` bug, where
/// `quit` hadn't been tried at all yet) — the poll loop must still
/// eventually fall back to a real force kill, not wait forever or give
/// up silently.
#[tokio::test]
async fn graceful_kill_cs2_falls_back_to_force_kill_when_quit_does_not_work() {
    tokio::time::pause();
    let sys = MockController::new().with_process("cs2.exe", 8888);

    graceful_kill_cs2(&sys).await.unwrap();

    assert_eq!(sys.quit_cs2_calls(), 1);
    assert_eq!(
        sys.killed_pids(),
        vec![8888],
        "quit didn't remove the process, so the poll loop times out and falls back — \
         re-querying by name for whatever cs2.exe is actually there, not a pid it \
         started with (it isn't even passed one anymore)"
    );
}

/// A process that only becomes discoverable on the exact
/// call `wait_for_process` makes right at its timeout boundary must
/// still be killed, not left completely untracked. Deterministic (no
/// real timing or races): `find_process` returns `None` for the loop's
/// one regular check, then `Some` starting from the very next call —
/// exactly the shape of that race, without needing
/// to actually land it via real elapsed time.
#[tokio::test]
async fn wait_for_process_kills_a_process_that_only_appears_on_the_final_check() {
    let sys = MockController::new().with_process_visible_after("cs2.exe", 7777, 1);

    let result = wait_for_process(&sys, "cs2.exe", Duration::ZERO, true).await;

    assert!(result.is_err(), "still reports the timeout to the caller");
    assert_eq!(
        sys.killed_pids(),
        vec![7777],
        "the process found on the final post-timeout check must be killed, not left \
         untracked for cleanup"
    );
}

#[tokio::test]
async fn ensure_steam_running_is_a_noop_when_steam_already_running() {
    let sys = MockController::new().with_steam_status(SteamStatus {
        running: true,
        elevated: false,
    });
    ensure_steam_running(&sys).await.unwrap();
    assert_eq!(
        sys.deelevate_calls(),
        0,
        "already running -- must never attempt a launch"
    );
    assert_eq!(
        sys.close_steam_window_calls(),
        0,
        "Steam was already running -- this call didn't cause its window to pop up, must not touch it"
    );
}

#[tokio::test]
async fn ensure_steam_running_waits_for_steamwebhelper_not_just_steam_exe() {
    tokio::time::pause();
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: false,
            elevated: false,
        })
        .with_process_on_deelevate("steam.exe", 5150)
        .with_process_visible_after("steamwebhelper.exe", 5151, 3);
    ensure_steam_running(&sys).await.unwrap();
    assert!(
        sys.find_process_calls("steamwebhelper.exe") >= 3,
        "must actually poll steamwebhelper.exe multiple times before returning, not just check \
         steam.exe alone -- got {} calls",
        sys.find_process_calls("steamwebhelper.exe")
    );
}

#[tokio::test]
async fn ensure_steam_running_closes_steam_window_after_a_fresh_launch() {
    tokio::time::pause();
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: false,
            elevated: false,
        })
        .with_process_on_deelevate("steam.exe", 5150)
        .with_process("steamwebhelper.exe", 5151);
    ensure_steam_running(&sys).await.unwrap();
    assert_eq!(
        sys.close_steam_window_calls(),
        1,
        "must close Steam's window once it just launched it, so the user isn't left staring at it"
    );
}

#[tokio::test]
async fn ensure_steam_running_errors_when_closing_the_window_exits_steam() {
    tokio::time::pause();
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: false,
            elevated: false,
        })
        .with_process_on_deelevate("steam.exe", 5150)
        .with_process("steamwebhelper.exe", 5151)
        .with_close_exits_steam_client();
    let err = ensure_steam_running(&sys).await.unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("exit"),
        "must clearly name that Steam appears to have exited (e.g. the close-to-tray setting \
         isn't enabled), not fail silently or with an opaque message: {err}"
    );
}

#[tokio::test]
async fn ensure_steam_running_launches_deelevated_when_not_running() {
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: false,
            elevated: false,
        })
        .with_process_on_deelevate("steam.exe", 5150)
        .with_process("steamwebhelper.exe", 5151);
    ensure_steam_running(&sys).await.unwrap();
    assert_eq!(sys.deelevate_calls(), 1);
}

#[tokio::test]
async fn ensure_steam_running_fails_when_deelevated_launch_fails() {
    let sys = MockController::new()
        .with_steam_status(SteamStatus {
            running: false,
            elevated: false,
        })
        .with_deelevate_failing("no interactive desktop");
    let err = ensure_steam_running(&sys).await.unwrap_err();
    assert!(err.to_string().contains("no interactive desktop"));
}
