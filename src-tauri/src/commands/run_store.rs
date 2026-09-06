//! Translates `EngineEvent`s into `RunState` patches on the store actor.
//! Only `PhaseChanged`, `ScenarioComplete`, `RunComplete` and `RunFailed`
//! touch crash-recovery state; every other event is UI-only.

use voidframe_engine::run::EngineEvent;
use voidframe_engine::store::StoreHandle;

pub(crate) async fn update_store_from_event(
    store: &StoreHandle,
    ev: &EngineEvent,
) -> Result<(), voidframe_engine::error::Error> {
    match ev {
        EngineEvent::PhaseChanged { phase } => {
            // `current_scenario` is what `rollback_now` uses to pick which
            // `journal-<scenario>.jsonl` to reverse-replay after a crash.
            // Run-scoped phases (preflight, snapshot, rollback, report)
            // leave it untouched: a crash during ROLLBACK must still know
            // which scenario's mutations were the last ones applied.
            let phase = phase.clone();
            store
                .patch(move |s| {
                    if let Some(id) = phase.scenario_id() {
                        s.current_scenario = Some(id.to_string());
                    }
                    s.phase = phase;
                })
                .await
        }
        EngineEvent::ScenarioComplete { result } => {
            let id = result.scenario_id.clone();
            store.patch(move |s| s.completed_scenarios.push(id)).await
        }
        EngineEvent::RunComplete { .. } | EngineEvent::RunFailed { .. } => store.clear().await,
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::run::Phase;
    use voidframe_engine::store::RunState;

    fn initial_state() -> RunState {
        RunState {
            schema_version: "1.1.0".into(),
            run_id: "r1".into(),
            project_id: "p1".into(),
            phase: Phase::Preflight,
            current_scenario: None,
            completed_scenarios: vec![],
            revision: 1,
        }
    }

    #[tokio::test]
    async fn phase_changed_patches_current_scenario_not_just_phase() {
        let dir = tempfile::tempdir().unwrap();
        let root = voidframe_engine::paths::DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let store = voidframe_engine::store::spawn_store(root);
        store.set_state(initial_state()).await.unwrap();

        update_store_from_event(
            &store,
            &EngineEvent::PhaseChanged {
                phase: Phase::Baseline,
            },
        )
        .await
        .unwrap();
        let snap = store.snapshot().await.unwrap().unwrap();
        assert_eq!(snap.phase, Phase::Baseline);
        assert_eq!(snap.current_scenario.as_deref(), Some("baseline"));

        update_store_from_event(
            &store,
            &EngineEvent::PhaseChanged {
                phase: Phase::Scenario { id: "s2".into() },
            },
        )
        .await
        .unwrap();
        let snap = store.snapshot().await.unwrap().unwrap();
        assert_eq!(snap.current_scenario.as_deref(), Some("s2"));

        // A run-scoped phase advances `phase` but must not wipe the
        // scenario that ROLLBACK is cleaning up after.
        update_store_from_event(
            &store,
            &EngineEvent::PhaseChanged {
                phase: Phase::Rollback,
            },
        )
        .await
        .unwrap();
        let snap = store.snapshot().await.unwrap().unwrap();
        assert_eq!(snap.phase, Phase::Rollback);
        assert_eq!(snap.current_scenario.as_deref(), Some("s2"));
    }
}
