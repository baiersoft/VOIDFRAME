//! Small control-channel helpers shared by the run loop's various wait
//! points: `honor_pause` (the existing "block here until Resume, or bail on
//! Abort" checkpoint, factored out so `maybe_break` can share it with
//! `run_one_iteration` rather than duplicating it), `wait_for_abort_allowed`
//! (the abort arm `execute` races the whole run body against, independent of
//! the `ControlMsg` channel `honor_pause` consumes -- see
//! `RunContext::abort`'s own doc comment for why these are two separate
//! mechanisms), and `drain_shutdown_toggle` (the live `shutdown_when_complete`
//! toggle -- a second, dedicated `watch::channel<bool>`, deliberately never folded into
//! `ControlMsg`: `honor_pause`'s `try_recv()` above discards any message it
//! doesn't recognize, so a shared channel would risk a toggle arriving right
//! at a `honor_pause` checkpoint being silently eaten -- see
//! `docs/superpowers/specs/2026-09-06-m3-autonomous-reboots-design.md` for
//! the full reasoning). `watch` rather than `mpsc`: this toggle only ever
//! needs "the latest value", a `watch::Sender::send` never blocks or errors
//! on capacity (unlike a bounded `mpsc`, which could make the Tauri command
//! that sends it hang if this run went a long stretch between checkpoints),
//! and there's no queue to drain -- `drain_shutdown_toggle` is a single
//! comparison against `*toggle.borrow()`.

use crate::error::{Error, Result};
use crate::model::progress::RunProgress;
use crate::run::{ControlMsg, EngineEvent};
use std::path::Path;
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

/// Applies `toggle`'s current value to `progress.shutdown_when_complete` if
/// it has changed since the last time this checkpoint ran -- a `watch`
/// channel always holds exactly the latest value the operator settled on
/// (see this module's own doc comment for why `watch` was chosen over an
/// `mpsc` queue here), so there is nothing to drain, just one comparison. A
/// no-op (no disk write) when `toggle`'s value already matches
/// `progress.shutdown_when_complete`. Otherwise updates `progress` and saves
/// it immediately: this run can span multiple process lifetimes across a
/// VOIDFRAME-initiated reboot, and `progress.json` is the only thing that
/// survives one, so a toggle that arrives moments before a reboot must not
/// be lost with it.
pub(super) fn drain_shutdown_toggle(
    toggle: &watch::Receiver<bool>,
    progress: &mut RunProgress,
    run_dir: &Path,
) -> Result<()> {
    let value = *toggle.borrow();
    if value != progress.shutdown_when_complete {
        progress.shutdown_when_complete = value;
        progress.save(run_dir)?;
    }
    Ok(())
}

/// The checkpoint every `scenario_loop` reboot site runs immediately before
/// handing off to `reboot_sequence`, in this order:
///
/// 1. Finding 1+2 fix: a toggle sent while the scenario was applying/
///    reverting must survive the reboot -- drained into `progress` (and
///    saved) before `reboot_sequence` writes `progress.json` and kills this
///    process.
/// 2. Root-cause fix (M3 safety review, round 3): an Abort observed here,
///    immediately before the reboot would fire, must stop the run instead --
///    neither `apply_stage`/`revert_stage` nor `maybe_break` (the loop's only
///    other abort checkpoint) reads `abort` on these paths, so without this
///    the reboot would proceed and the resumed process would run the rest of
///    the benchmark (and its own end-of-run shutdown decision) as if the
///    operator never clicked Abort. `progress.abort_requested` is persisted
///    so `begin_resume` independently refuses to continue even if some future
///    path reached a reboot without this same check (Layer 2,
///    defense-in-depth).
pub(super) fn pre_reboot_checkpoint(
    shutdown_toggle: &watch::Receiver<bool>,
    abort: &watch::Receiver<bool>,
    progress: &mut RunProgress,
    run_dir: &Path,
) -> Result<()> {
    drain_shutdown_toggle(shutdown_toggle, progress, run_dir)?;
    if *abort.borrow() {
        progress.abort_requested = true;
        progress.save(run_dir)?;
        return Err(Error::aborted("operator requested Abort".into()));
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
/// that case instead, so [`wait_for_abort_allowed`]'s own abort arm never
/// resolves and `execute()`'s outer `select!` always lets the body run to
/// completion.
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

/// The abort arm `execute()` races the entire run body against. Resolves
/// only when an operator Abort fires while the run is not currently
/// shielded. `no_return` is not a one-shot switch: a mutation that must not
/// be split by cancellation (e.g. PowerPlanCreate's OS call, see
/// `mutation/power_plan.rs`) can flip it true for just its own duration and
/// false again afterward -- an Abort observed during that window is
/// deferred (with a LogLine) and re-checked the instant the window lifts,
/// never lost. Once a shield never lifts again (the permanent case: reboot
/// initiation / finalize underway), this future never resolves, so the
/// outer `select!` always lets the body finish.
pub(super) async fn wait_for_abort_allowed(
    mut abort: watch::Receiver<bool>,
    mut no_return: watch::Receiver<bool>,
    events: &mpsc::Sender<EngineEvent>,
) {
    loop {
        wait_for_abort_requested(&mut abort).await;
        // `borrow_and_update`, not `borrow`: marking the value seen is what
        // keeps the `changed()` below from returning immediately on a version
        // this check already looked at, which would otherwise re-emit the
        // LogLine a second time in the permanent (reboot/finalize) case.
        if !*no_return.borrow_and_update() {
            return;
        }
        let _ = events
            .send(EngineEvent::LogLine {
                text: "Abort requested, but the run is past the point of no return (a reboot, \
                       final cleanup, or an in-flight uninterruptible mutation is underway) -- \
                       deferring until it's safe to cancel."
                    .into(),
            })
            .await;
        // Wait for the shield to lift. If every sender is dropped instead,
        // it can never lift -- pend forever, same as the permanent case.
        if no_return.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
        // Loop back: abort is still observed true (a watch retains its
        // latest value), so `wait_for_abort_requested` returns immediately
        // and we re-check `no_return` -- resolves if it's false now, waits
        // again if another shield started first.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_for_abort_allowed_resolves_when_abort_fires_before_no_return() {
        let (events_tx, _events_rx) = mpsc::channel(4);
        let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        let (_no_return_tx, no_return_rx) = tokio::sync::watch::channel(false);
        abort_tx.send(true).unwrap();
        tokio::time::timeout(
            std::time::Duration::from_millis(500),
            wait_for_abort_allowed(abort_rx, no_return_rx, &events_tx),
        )
        .await
        .expect("must resolve promptly when abort fires while cancellable");
    }

    #[tokio::test]
    async fn wait_for_abort_allowed_stays_pending_while_no_return_is_permanently_set() {
        let (events_tx, mut events_rx) = mpsc::channel(4);
        let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        let (no_return_tx, no_return_rx) = tokio::sync::watch::channel(false);
        no_return_tx.send(true).unwrap();
        abort_tx.send(true).unwrap();
        let refused = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            wait_for_abort_allowed(abort_rx, no_return_rx, &events_tx),
        )
        .await;
        assert!(
            refused.is_err(),
            "must stay pending while no_return never lifts"
        );
        let ev = events_rx
            .recv()
            .await
            .expect("a deferral LogLine must have been emitted");
        match ev {
            crate::run::EngineEvent::LogLine { text } => {
                assert!(text.contains("deferring"), "{text}")
            }
            other => panic!("expected a LogLine, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_for_abort_allowed_resolves_as_soon_as_a_temporary_shield_lifts() {
        let (events_tx, mut events_rx) = mpsc::channel(4);
        let (abort_tx, abort_rx) = tokio::sync::watch::channel(false);
        let (no_return_tx, no_return_rx) = tokio::sync::watch::channel(true); // shielded from the start
        abort_tx.send(true).unwrap();

        let handle = tokio::spawn(async move {
            wait_for_abort_allowed(abort_rx, no_return_rx, &events_tx).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        no_return_tx.send(false).unwrap();

        tokio::time::timeout(std::time::Duration::from_millis(500), handle)
            .await
            .expect("must resolve promptly once the temporary shield lifts")
            .unwrap();
        let ev = events_rx
            .recv()
            .await
            .expect("a deferral LogLine must have been emitted");
        match ev {
            crate::run::EngineEvent::LogLine { text } => {
                assert!(text.contains("deferring"), "{text}")
            }
            other => panic!("expected a LogLine, got {other:?}"),
        }
    }

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

    /// A minimal but valid `RunProgress`, matching
    /// `model::progress::tests::project`'s own fixture pattern -- field
    /// values don't matter here beyond `shutdown_when_complete`.
    fn progress_with(shutdown_when_complete: bool) -> RunProgress {
        use crate::model::progress::{Cursor, Stage};
        use crate::model::project::{Baseline, Project};
        use crate::system::PowerPlan;

        RunProgress {
            schema_version: crate::model::SCHEMA_VERSION.to_string(),
            run_id: "r1".into(),
            project: Project {
                schema_version: crate::model::SCHEMA_VERSION.to_string(),
                id: "p1".into(),
                name: "P".into(),
                description: "d".into(),
                created_at: "2026-09-06T00:00:00Z".into(),
                settings: serde_json::from_str("{}").unwrap(),
                baseline: Baseline {
                    name: "Stock".into(),
                    description: "d".into(),
                },
                scenarios: vec![],
            },
            start_build_id: None,
            start_launch_args: String::new(),
            start_launch_args_raw: String::new(),
            start_power_plan: PowerPlan {
                guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
                name: "Balanced".into(),
                active: true,
            },
            thermal_baseline: None,
            completed: vec![],
            unstable: vec![],
            cursor: Cursor {
                index: 0,
                stage: Stage::Apply,
            },
            reboot: None,
            shutdown_when_complete,
            skip_revert_once: false,
            abort_requested: false,
        }
    }

    /// A changed value is applied to `progress` and persisted to
    /// `progress.json`.
    #[test]
    fn drain_shutdown_toggle_applies_and_saves_a_changed_value() {
        let dir = tempfile::tempdir().unwrap();
        let (_tx, rx) = watch::channel(true);
        let mut progress = progress_with(false);

        drain_shutdown_toggle(&rx, &mut progress, dir.path()).unwrap();

        assert!(progress.shutdown_when_complete);
        let reloaded = RunProgress::load(dir.path()).unwrap();
        assert!(reloaded.shutdown_when_complete);
    }

    /// A `watch` channel structurally holds only the latest value -- sending
    /// several updates before this checkpoint is ever reached still leaves
    /// only the last one to be observed, with no drain loop needed to get
    /// that property.
    #[test]
    fn drain_shutdown_toggle_always_reflects_the_newest_value_sent() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = watch::channel(false);
        tx.send(true).unwrap();
        tx.send(false).unwrap();
        tx.send(true).unwrap();
        let mut progress = progress_with(false);

        drain_shutdown_toggle(&rx, &mut progress, dir.path()).unwrap();

        assert!(progress.shutdown_when_complete);
    }

    /// An unchanged value is a no-op -- no spurious `progress.json` write.
    #[test]
    fn drain_shutdown_toggle_is_a_noop_when_the_value_is_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let (_tx, rx) = watch::channel(false);
        let mut progress = progress_with(false);

        drain_shutdown_toggle(&rx, &mut progress, dir.path()).unwrap();

        assert!(!progress.shutdown_when_complete);
        assert!(
            !dir.path().join("progress.json").exists(),
            "an unchanged value must not write progress.json"
        );
    }
}
