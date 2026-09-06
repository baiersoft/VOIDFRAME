pub mod catalog;
pub mod config;
pub mod hardware;
pub mod launch_args;
pub mod power_plan;
pub mod preflight;
pub mod projects;
pub mod results;
pub mod rollback;
pub mod run;
pub mod run_store;
pub mod shell;

/// Turns a `spawn_blocking` `JoinError` (a panic on the blocking task) into
/// this crate's own `Result<T, String>` error shape -- the wording mirrors
/// `voidframe_engine`'s own blocking wrappers (e.g.
/// `system::windows::registry::read`'s `"registry read task panicked: {e}"`),
/// adapted to this crate's `String` error type instead of the engine's
/// `Error`. Shared by every command in this module that moves blocking
/// filesystem I/O onto `spawn_blocking`, so the wording stays consistent
/// across call sites instead of drifting per file.
pub(crate) fn join_error_to_string(what: &str, e: tokio::task::JoinError) -> String {
    format!("{what} task panicked: {e}")
}

#[cfg(test)]
mod tests {
    use super::join_error_to_string;

    /// Regression/coverage for the Stage C boundary-review gap this module
    /// closes: every blocking-fs command now maps a `spawn_blocking`
    /// `JoinError` through this helper instead of letting a panicked
    /// blocking task propagate as an unhandled panic across the `#[tauri::
    /// command]` boundary. Proves the mapping itself returns a readable
    /// `String`, not a panic.
    #[tokio::test]
    async fn join_error_to_string_turns_a_panicked_blocking_task_into_a_readable_message() {
        let handle = tokio::task::spawn_blocking(|| {
            panic!("intentional panic to produce a JoinError for this test");
        });
        let join_err = handle.await.unwrap_err();
        let msg = join_error_to_string("test_op", join_err);
        assert!(msg.contains("test_op"), "{msg}");
        assert!(msg.contains("task panicked"), "{msg}");
    }
}
