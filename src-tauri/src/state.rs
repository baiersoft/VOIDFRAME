//! `AppState` — everything a Tauri command needs that isn't per-call
//! input: the resolved data root, the loaded `config.json`, one
//! long-lived `WindowsController`, the `store::StoreHandle` actor, and
//! (while a run is active) its `ControlMsg` sender.

use std::sync::Arc;
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};
use voidframe_engine::model::Config;
use voidframe_engine::paths::{DataRoot, InstanceLock, acquire_instance_lock};
use voidframe_engine::run::ControlMsg;
use voidframe_engine::store::StoreHandle;
use voidframe_engine::system::{SystemController, WindowsController};

pub struct ActiveRun {
    /// Currently unread. It was originally added expecting
    /// `get_run_snapshot` and `rollback_now` to need "which run_id is
    /// currently active" -- both ended up sourcing that from the store's
    /// own persisted `RunState` instead (which is also what makes them work
    /// after a crash, when no `ActiveRun` exists at all), so nothing reads
    /// this field today. Kept because it costs one `String` per active run
    /// and is the obvious thing any future in-process "is *this* run still
    /// the active one" check would need; `#[expect(dead_code)]` (not
    /// `allow`) so the compiler tells us the moment that stops being true.
    #[expect(dead_code)]
    pub run_id: String,
    pub control_tx: mpsc::Sender<ControlMsg>,
    /// The live `shutdown_when_complete` toggle -- a separate, dedicated
    /// channel from `control_tx`, deliberately never folded into
    /// `ControlMsg` (see `voidframe_engine::run::execute::control`'s own
    /// doc comment for why: `honor_pause`'s `try_recv()` silently discards
    /// any message it doesn't recognize, which would risk a toggle sent
    /// right at that checkpoint being lost). A `watch::Sender`, not an
    /// `mpsc::Sender`: only the latest value ever matters, and
    /// `watch::Sender::send` never blocks or errors on capacity, so a
    /// reboot-heavy run that goes a long stretch between drain checkpoints
    /// can never make `set_shutdown_when_complete` hang.
    pub shutdown_toggle_tx: watch::Sender<bool>,
}

pub struct AppState {
    pub data_root: DataRoot,
    pub config: std::sync::Mutex<Config>,
    pub sys: Arc<dyn SystemController>,
    pub store: StoreHandle,
    pub active_run: AsyncMutex<Option<ActiveRun>>,
    /// Whether this process was launched via `--resume` (spec §3.4) --
    /// known synchronously at process launch (`cli_modes::parse` in
    /// `lib.rs`, well before `.setup()` runs), not tied to whether
    /// `resume_run_impl`'s own background task has actually finished (or
    /// even started) resuming that run. The frontend reads this at mount
    /// via `is_resume_launch` to tell "the backend is actively and
    /// correctly resuming this exact run right now" apart from "a previous
    /// run crashed" -- tying it to `resume_run_impl`'s completion instead
    /// would reintroduce that exact race, since a resume can take a while
    /// (e.g. it calls `settle_wait`) and the frontend's mount effect can
    /// easily run first.
    pub resume_launch: bool,
    /// Held for `AppState`'s entire lifetime (i.e. the whole app run, since
    /// `AppState` is handed to `.manage()` in `lib.rs` and only dropped on
    /// process exit). Never read after construction — its sole purpose is
    /// the `Drop` impl on [`InstanceLock`], which releases
    /// `state/instance.lock` so a later launch can reacquire it. Dropping
    /// this early (e.g. by not storing it here at all) would let a second,
    /// concurrently-launched instance start its own store actor over the
    /// same `state/current.json`, which is exactly the single-instance bug
    /// this field exists to prevent (docs/02-architecture.md's singleton
    /// guard).
    #[expect(dead_code, reason = "held for its Drop side effect only")]
    instance_lock: InstanceLock,
}

impl AppState {
    /// **Must be called from inside a tokio runtime** — see
    /// [`AppState::with_data_root`]. `lib.rs`'s `run()` does this via
    /// `tauri::async_runtime::block_on`.
    ///
    /// `VOIDFRAME_DATA_ROOT`, when set, overrides the real
    /// `%LOCALAPPDATA%\baiersoft\VOIDFRAME` resolution with an arbitrary
    /// directory -- for `e2e/`'s real-binary WebdriverIO suite (wdio.conf.ts
    /// points it at a fresh temp dir per run), so those tests start from a
    /// clean slate and never read or mutate the operator's actual projects/
    /// config/run history. Not gated behind the `webdriver-testing` feature
    /// like the automation plugin: this only changes *where* files are read
    /// and written, exposes no new surface, and is equally useful for a
    /// developer manually testing against disposable state.
    ///
    /// `resume_launch`: `lib.rs`'s own `resume_on_start` (`cli_mode ==
    /// CliMode::Resume`), passed straight through into
    /// [`AppState::resume_launch`] -- see that field's own doc comment for
    /// why the frontend needs it exposed synchronously rather than derived
    /// from `resume_run_impl`'s completion.
    pub fn new(resume_launch: bool) -> anyhow::Result<Self> {
        let data_root = match std::env::var_os("VOIDFRAME_DATA_ROOT") {
            Some(path) => DataRoot::with_base(std::path::PathBuf::from(path))?,
            None => DataRoot::resolve(false)?,
        };
        let mut state = Self::with_data_root(data_root)?;
        state.resume_launch = resume_launch;
        Ok(state)
    }

    /// The real constructor, split out from [`AppState::new`] so tests can
    /// exercise the identical store-spawning path against a tempdir instead
    /// of the machine's real `%LOCALAPPDATA%`.
    ///
    /// **Must be called from inside a tokio runtime.**
    /// `voidframe_engine::store::spawn_store` is a *synchronous* function
    /// whose body is a bare `tokio::spawn`, so it panics ("there is no
    /// reactor running") when called with no runtime entered on the current
    /// thread — nothing in its signature hints at that requirement, which is
    /// exactly how it went unnoticed until it broke app startup outright.
    ///
    /// Acquires the process-level single-instance lock
    /// (`state/instance.lock`, see `voidframe_engine::paths`) before doing
    /// anything else. Without this, two elevated launches against the same
    /// `data_root` would each spawn their own store actor over the same
    /// `state/current.json` and could race each other's runs/rollbacks —
    /// see docs/02-architecture.md's singleton guard.
    pub fn with_data_root(data_root: DataRoot) -> anyhow::Result<Self> {
        let instance_lock = acquire_instance_lock(&data_root)?;
        let config = Config::load(&data_root.config_path())?;
        // `spawn_store` takes ownership of a `DataRoot`; `data_root` itself
        // is still needed below, so hand the actor a clone.
        let store = voidframe_engine::store::spawn_store(data_root.clone());
        Ok(AppState {
            data_root,
            config: std::sync::Mutex::new(config),
            sys: Arc::new(WindowsController::new()),
            store,
            active_run: AsyncMutex::new(None),
            // Not a parameter here: every other caller of `with_data_root`
            // (this file's and `commands/run.rs`'s own tests) is not
            // simulating a `--resume` launch, so `false` is the correct
            // default for all of them. `AppState::new` -- the one real
            // caller that needs a non-default value -- overwrites it right
            // after this call returns.
            resume_launch: false,
            instance_lock,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for the bug that made this app unable to start in any
    /// build profile: `lib.rs`'s `run()` is a plain, non-async fn that calls
    /// `AppState::new()` before `tauri::Builder` (and therefore any tokio
    /// runtime) exists, while `AppState`'s construction reaches
    /// `store::spawn_store`'s bare `tokio::spawn`. Without the
    /// `tauri::async_runtime::block_on` wrapper, that panics with "there is
    /// no reactor running, must be called from the context of a Tokio 1.x
    /// runtime" — every command in this app was unreachable because of it.
    ///
    /// This test is deliberately a plain `#[test]`, NOT a `#[tokio::test]`:
    /// the whole point is to start from a genuinely runtime-free thread, the
    /// way `main` does. 193 green engine tests never caught this precisely
    /// because `spawn_store` had no production caller before this plan —
    /// only `#[tokio::test]`s, which always already run inside a runtime.
    ///
    /// Uses `with_data_root` + a tempdir rather than `new()` so it stays
    /// hermetic (no writes into the developer's real
    /// `%LOCALAPPDATA%\baiersoft\VOIDFRAME`); `new()` is a one-line
    /// `DataRoot::resolve(false)?` delegation to this exact function, and it
    /// is `with_data_root` that contains the runtime-requiring call.
    #[test]
    fn app_state_can_be_constructed_from_a_non_async_context() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let result = tauri::async_runtime::block_on(async { AppState::with_data_root(data_root) });
        assert!(result.is_ok(), "{:?}", result.err());

        // Prove the spawned actor is genuinely live on the runtime this
        // entered (not merely that construction returned `Ok`): a store
        // whose task never attached would have a receiver that was dropped
        // immediately, and `snapshot()` — which needs a round trip through
        // the actor — would error with "store closed"/"store dropped".
        let state = result.unwrap();
        let snapshot = tauri::async_runtime::block_on(async { state.store.snapshot().await });
        assert!(snapshot.is_ok(), "{:?}", snapshot.err());
    }

    /// Proves the M5 single-instance wiring: with the instance lock already
    /// held on a data root (simulating a first, still-running launch),
    /// `with_data_root` must refuse to construct a second `AppState` against
    /// that same root, instead of silently spawning a second store actor
    /// over the same `state/current.json`.
    #[test]
    fn with_data_root_refuses_a_second_instance_on_the_same_data_root() {
        let dir = tempfile::tempdir().unwrap();
        let data_root = DataRoot::with_base(dir.path().to_path_buf()).unwrap();

        // Simulate the first, already-running instance holding the lock.
        let _first_instance_lock = acquire_instance_lock(&data_root).unwrap();

        let result =
            tauri::async_runtime::block_on(async { AppState::with_data_root(data_root.clone()) });
        assert!(
            result.is_err(),
            "a second AppState must not construct while the instance lock is held"
        );
    }
}
