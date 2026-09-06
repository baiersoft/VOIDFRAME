//! Small control-channel helpers shared by the run loop's various wait
//! points: `honor_pause` (the existing "block here until Resume, or bail on
//! Abort" checkpoint, factored out so `maybe_break` can share it with
//! `run_one_iteration` rather than duplicating it) and `abortable` (races any
//! future against the operator-abort signal, independent of the `ControlMsg`
//! channel `honor_pause` consumes -- see `RunContext::abort`'s own doc
//! comment for why these are two separate mechanisms).

use crate::error::{Error, Result};
use crate::run::ControlMsg;
use std::future::Future;
use tokio::sync::{mpsc, watch};

/// Blocks here if `ControlMsg::Pause` is already waiting on `control`, until
/// `ControlMsg::Resume` arrives -- returns immediately (a no-op) if nothing
/// is pending. `ControlMsg::Abort` at the head of `control` ends this
/// checkpoint with an aborted error rather than being silently discarded --
/// a direct `execute()` caller (`execute` is `pub`) that sends `Abort` while
/// the run is not paused must still have it observed here, not eaten.
/// `ControlMsg::Abort` seen while paused likewise ends the wait with an
/// aborted error rather than resuming; every other message
/// (`OperatorAcknowledged`, a stray `Pause`) is ignored and the wait
/// continues.
pub(super) async fn honor_pause(control: &mut mpsc::Receiver<ControlMsg>) -> Result<()> {
    match control.try_recv() {
        Ok(ControlMsg::Pause) => loop {
            match control.recv().await {
                Some(ControlMsg::Resume) => break,
                Some(ControlMsg::Abort) => {
                    return Err(Error::aborted("aborted while paused".into()));
                }
                _ => continue,
            }
        },
        Ok(ControlMsg::Abort) => return Err(Error::aborted("operator requested Abort".into())),
        _ => {}
    }
    Ok(())
}

/// Resolves once `abort`'s value genuinely becomes `true` -- and only then.
/// Deliberately NOT `watch::Receiver::wait_for(|v| *v)`: that resolves
/// (with an `Err`) the instant the sender side is ever dropped, regardless
/// of the value it last held, which would make a merely-out-of-scope sender
/// (every test that doesn't care about abort, and in principle a future
/// caller with no reason to keep it alive past the run) look identical to a
/// genuine operator Abort. A closed channel that was never actually flipped
/// to `true` means "abort is not going to happen" -- this waits forever in
/// that case instead, so `abortable`'s `select!` always takes its other
/// branch to completion.
async fn wait_for_abort_requested(abort: &mut watch::Receiver<bool>) {
    loop {
        if *abort.borrow() {
            return;
        }
        if abort.changed().await.is_err() {
            // Sender dropped. One last check in case it was set to `true`
            // right before that -- `changed()`'s `Err` alone says nothing
            // about the value it's closing on.
            if *abort.borrow() {
                return;
            }
            std::future::pending::<()>().await;
        }
    }
}

/// Races `fut` against the operator-abort signal, returning
/// `Err(Error::aborted(..))` the instant abort fires instead of waiting for
/// `fut` to run to completion. Every long wait in the run loop (CS2-readiness
/// detection, a PresentMon capture, thermal sampling, an inter-scenario
/// break) is wrapped in this so an Abort click takes effect within one
/// `select!` poll rather than however long that wait would otherwise take.
pub(super) async fn abortable<T>(
    mut abort: watch::Receiver<bool>,
    fut: impl Future<Output = Result<T>>,
) -> Result<T> {
    tokio::select! {
        result = fut => result,
        () = wait_for_abort_requested(&mut abort) => Err(Error::aborted("operator requested Abort".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Medium-severity finding this fixes: `Abort` at the head of
    /// `control` (the run not currently paused) used to be silently popped
    /// and discarded by `try_recv()`. It must now end the checkpoint with an
    /// aborted error instead.
    #[tokio::test]
    async fn honor_pause_returns_aborted_when_abort_is_at_the_head() {
        let (tx, mut rx) = mpsc::channel(4);
        tx.send(ControlMsg::Abort).await.unwrap();

        let err = honor_pause(&mut rx).await.unwrap_err();
        assert!(err.is_aborted());
    }

    /// Any other non-Pause message at the head (e.g. `Resume`, sent when
    /// nothing is paused) is a no-op -- `honor_pause` neither blocks nor
    /// errors on it.
    #[tokio::test]
    async fn honor_pause_ignores_a_non_pause_non_abort_head_message() {
        let (tx, mut rx) = mpsc::channel(4);
        tx.send(ControlMsg::Resume).await.unwrap();

        honor_pause(&mut rx).await.unwrap();
    }

    /// An empty channel is a no-op.
    #[tokio::test]
    async fn honor_pause_is_a_noop_on_an_empty_channel() {
        let (_tx, mut rx) = mpsc::channel::<ControlMsg>(4);

        honor_pause(&mut rx).await.unwrap();
    }
}
