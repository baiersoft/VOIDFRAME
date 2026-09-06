//! The single-writer state actor: owns `RunState` in memory, persists it to
//! `state/current.json` atomically on every change, and is the only thing
//! that touches that file (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §5.3 — "single writer").

use crate::error::{Error, Result};
use crate::paths::{DataRoot, atomic_write};
use crate::run::Phase;
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RunState {
    pub schema_version: String,
    pub run_id: String,
    pub project_id: String,
    pub phase: Phase,
    pub current_scenario: Option<String>,
    pub completed_scenarios: Vec<String>,
    pub revision: u32,
}

pub enum StoreCmd {
    SetState(RunState),
    Patch(Box<dyn FnOnce(&mut RunState) + Send>),
    Snapshot(oneshot::Sender<Option<RunState>>),
    Clear,
    Shutdown,
}

#[derive(Clone)]
pub struct StoreHandle {
    tx: mpsc::Sender<StoreCmd>,
}

impl StoreHandle {
    pub async fn set_state(&self, s: RunState) -> Result<()> {
        self.tx
            .send(StoreCmd::SetState(s))
            .await
            .map_err(|_| Error::msg("store closed".into()))
    }
    pub async fn patch(&self, f: impl FnOnce(&mut RunState) + Send + 'static) -> Result<()> {
        self.tx
            .send(StoreCmd::Patch(Box::new(f)))
            .await
            .map_err(|_| Error::msg("store closed".into()))
    }
    pub async fn snapshot(&self) -> Result<Option<RunState>> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(StoreCmd::Snapshot(tx))
            .await
            .map_err(|_| Error::msg("store closed".into()))?;
        rx.await.map_err(|_| Error::msg("store dropped".into()))
    }
    pub async fn clear(&self) -> Result<()> {
        self.tx
            .send(StoreCmd::Clear)
            .await
            .map_err(|_| Error::msg("store closed".into()))
    }
    #[allow(dead_code)]
    pub async fn shutdown(&self) -> Result<()> {
        self.tx
            .send(StoreCmd::Shutdown)
            .await
            .map_err(|_| Error::msg("store closed".into()))
    }
}

/// Spawn the store actor. All access goes through the returned [`StoreHandle`];
/// nothing else may write `state/current.json`.
pub fn spawn_store(root: DataRoot) -> StoreHandle {
    let (tx, mut rx) = mpsc::channel::<StoreCmd>(64);
    let path = root.state_dir().join("current.json");
    // Load any state already on disk at actor startup. `current.json` still
    // existing here (with no live `AppState.active_run` to match it) is a
    // crash-recovery signal per docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §6.4: a run that reached a terminal
    // event through the normal event loop already had this file deleted by
    // `clear()` -- if it's still here, the previous session ended without
    // reaching that point (app killed, machine lost power).
    // A pre-1.1.0 current.json (string phase) does not parse and is
    // ignored; that build never shipped.
    let initial: Option<RunState> = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    tokio::spawn(async move {
        let mut state: Option<RunState> = initial;
        while let Some(cmd) = rx.recv().await {
            match cmd {
                StoreCmd::SetState(mut s) => {
                    s.revision += 1;
                    let _ = atomic_write(&path, &serde_json::to_vec_pretty(&s).unwrap_or_default());
                    state = Some(s);
                }
                StoreCmd::Patch(f) => {
                    if let Some(s) = state.as_mut() {
                        f(s);
                        s.revision += 1;
                        let _ =
                            atomic_write(&path, &serde_json::to_vec_pretty(s).unwrap_or_default());
                    }
                }
                StoreCmd::Snapshot(reply) => {
                    let _ = reply.send(state.clone());
                }
                StoreCmd::Clear => {
                    state = None;
                    let _ = std::fs::remove_file(&path);
                }
                StoreCmd::Shutdown => break,
            }
        }
    });
    StoreHandle { tx }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::DataRoot;

    fn rs() -> RunState {
        RunState {
            schema_version: "1.1.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            phase: Phase::Preflight,
            current_scenario: None,
            completed_scenarios: vec![],
            revision: 0,
        }
    }

    #[tokio::test]
    async fn set_patch_snapshot_and_persist() {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let store = spawn_store(root.clone());

        store.set_state(rs()).await.unwrap();
        store.patch(|s| s.phase = Phase::Baseline).await.unwrap();
        let snap = store.snapshot().await.unwrap().unwrap();
        assert_eq!(snap.phase, Phase::Baseline);
        assert!(snap.revision >= 2);

        let on_disk: RunState =
            serde_json::from_slice(&std::fs::read(root.state_dir().join("current.json")).unwrap())
                .unwrap();
        assert_eq!(on_disk.phase, Phase::Baseline);

        store.clear().await.unwrap();
        // `clear()` only guarantees the command was enqueued, not processed.
        // `snapshot()` synchronizes with the actor (its reply only arrives
        // after everything queued before it — including this Clear — has
        // been handled), so check filesystem state only after this rendezvous.
        assert!(store.snapshot().await.unwrap().is_none());
        assert!(!root.state_dir().join("current.json").exists());
    }

    /// A crash (app killed, machine lost power) skips the normal `clear()`
    /// cleanup, so `current.json` is still on disk the next time the app
    /// starts a fresh `spawn_store` actor with no in-memory state at all.
    /// That leftover file is docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §6.4's crash-recovery signal -- the actor
    /// must read it back into its initial `state`, not start blind.
    #[tokio::test]
    async fn spawn_store_loads_existing_current_json_at_startup() {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::with_base(dir.path().to_path_buf()).unwrap();

        // Simulate a crash: write current.json directly, as a prior
        // session's store actor would have, without ever calling clear().
        let leftover = rs();
        std::fs::write(
            root.state_dir().join("current.json"),
            serde_json::to_vec_pretty(&leftover).unwrap(),
        )
        .unwrap();

        let store = spawn_store(root.clone());
        let snap = store.snapshot().await.unwrap().unwrap();
        assert_eq!(snap.run_id, leftover.run_id);
    }
}
