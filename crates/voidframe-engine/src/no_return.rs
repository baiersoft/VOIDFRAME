//! The "point of no return" shield's RAII half.
//!
//! `run::execute`'s top-level `select!` races the whole run body against
//! `control::wait_for_abort_allowed`, which refuses to drop the body while
//! the `no_return` watch reads `true` (see that function's own doc comment
//! for the resettable-shield semantics). This guard is how a *bounded*
//! stretch of uninterruptible work declares itself: engage it immediately
//! before the `.await` that must not be split, drop it immediately after.
//!
//! Deliberately not a one-shot switch and deliberately not held any longer
//! than the call it wraps -- a shield that never lifts makes Abort dead for
//! the rest of the run, which is only ever correct for the permanent cases
//! (`reboot_sequence`, `finish_run`, `handle_body_failure`) that set the
//! watch directly instead of using this type.

/// Flips `no_return` true for the duration of an uninterruptible
/// sub-operation, false again on drop (success, error, or panic alike).
pub(crate) struct NoReturnGuard<'a>(&'a tokio::sync::watch::Sender<bool>);

impl<'a> NoReturnGuard<'a> {
    pub(crate) fn engage(tx: &'a tokio::sync::watch::Sender<bool>) -> Self {
        let _ = tx.send(true);
        Self(tx)
    }
}

impl Drop for NoReturnGuard<'_> {
    fn drop(&mut self) {
        let _ = self.0.send(false);
    }
}
