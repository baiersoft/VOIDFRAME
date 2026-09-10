//! Observations `execute()` makes once, at SNAPSHOT, that every later phase
//! reads. Built inside `execute()`, never by a caller — the fields
//! `RunConfig` used to carry with a "leave `None`" warning.

use super::RunConfig;
use crate::system::PowerPlan;

pub(super) struct RunContext<'a> {
    pub config: &'a RunConfig,
    /// Installed CS2 build when the run entered SNAPSHOT; `None` = not readable.
    pub start_build_id: Option<String>,
    /// The user's own launch options at SNAPSHOT, reserved tokens stripped.
    pub start_launch_args: String,
    /// The exact, un-reconciled launch options string read at SNAPSHOT --
    /// before `strip_reserved` (docs/superpowers/sdd/2026-09-05-run-lifecycle-review-fixes/final-fix-brief.md
    /// Important 1). `rollback`'s `restore_launch_options` compares against
    /// this too, not just `reconcile(start_launch_args)`: a run that never
    /// gets as far as BASELINE's own write (an Abort, or a failure inside
    /// `prepare_cs2_session` before it) leaves the live value at exactly
    /// this string, which on an un-reconciled first run never equals
    /// `reconcile(start_launch_args)` -- without this field that mismatch
    /// alone would make ROLLBACK kill Steam and write reserved tokens into
    /// a config nothing here actually changed.
    pub start_launch_args_raw: String,
    /// Active power plan at SNAPSHOT; restored in ROLLBACK.
    pub start_power_plan: PowerPlan,
    /// `true` once the operator has clicked Abort -- set by `spawn_run`'s
    /// control-channel tee task, independent of the `ControlMsg` `mpsc`
    /// channel (which stays reserved for Pause/Resume/OperatorAcknowledged,
    /// consumed only at existing safe checkpoints). A `watch::Receiver` is
    /// cheap to `.clone()`, so every long wait that should react to Abort
    /// promptly (CS2-readiness waits, capture, thermal sampling) clones its
    /// own handle and races it via `tokio::select!` rather than needing
    /// exclusive `&mut` access to one shared receiver.
    pub abort: tokio::sync::watch::Receiver<bool>,
    /// The "point of no return" shield the top-level abort race
    /// (`control::wait_for_abort_allowed`) consults before it is allowed to
    /// drop the run body. Set permanently by `reboot_sequence`/`finish_run`/
    /// `handle_body_failure` (a reboot or the final cleanup is underway --
    /// dropping the body there would strand half-done work), and flipped on
    /// and off again briefly (via `no_return::NoReturnGuard`) by
    /// `mutation::power_plan`'s create branch around its one
    /// journal-after-apply `.await` and by `scenario::revert_stage` around
    /// its journal replay. A `Sender` (not a
    /// `Receiver`): everything holding a `RunContext` is a *producer* of
    /// shielded windows; the single consumer is the abort arm in
    /// `execute()`.
    pub no_return: tokio::sync::watch::Sender<bool>,
}
