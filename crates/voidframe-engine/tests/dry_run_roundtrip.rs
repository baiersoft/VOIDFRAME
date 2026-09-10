//! End-to-end proof that the M1 engine core works: load a project, apply
//! every enabled scenario against a `MockController`, revert it, and assert
//! the (simulated) system is back exactly where it started.

use voidframe_engine::journal::Journal;
use voidframe_engine::model::project::Project;
use voidframe_engine::mutation::{apply_scenario, revert_scenario};
use voidframe_engine::system::MockController;

/// A throwaway `no_return` shield for `apply_module`/`apply_scenario`:
/// these tests have no live run whose abort race could need shielding, and
/// nothing here observes the flag. The receiver is dropped immediately --
/// the only sender is `power_plan::apply`'s guard, which ignores send
/// errors.
fn no_return() -> tokio::sync::watch::Sender<bool> {
    tokio::sync::watch::channel(false).0
}

const PROJECT: &str = r#"{
  "schema_version":"2.0.0","id":"p1","name":"Test","description":"d",
  "created_at":"2026-09-01T00:00:00Z","settings":{},
  "baseline":{"name":"Stock","description":"clean"},
  "scenarios":[
    {"id":"s1","name":"Core Parking + FSO","description":"d","modules":[
      {"type":"powercfg","sub":"sub_processor","setting":"CPMINCORES","value":100},
      {"type":"registry","hive":"HKCU","subkey":"System\\GameConfigStore","value_name":"GameDVR_FSEBehavior","value_type":"DWORD","value":2}
    ]},
    {"id":"s2","name":"Idle Disable","description":"d","modules":[
      {"type":"powercfg","sub":"sub_processor","setting":"IDLEDISABLE","value":1}
    ]}
  ]
}"#;

#[tokio::test]
async fn full_run_apply_then_revert_leaves_system_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let pp = dir.path().join("p1.json");
    std::fs::write(&pp, PROJECT).unwrap();
    let project = Project::load(&pp).unwrap();

    let mock = MockController::new()
        .with_powercfg(
            "sub_processor",
            "CPMINCORES",
            voidframe_engine::system::AcDc { ac: 50, dc: 50 },
        )
        .with_powercfg(
            "sub_processor",
            "IDLEDISABLE",
            voidframe_engine::system::AcDc { ac: 0, dc: 0 },
        );
    let before = mock.snapshot();

    for sc in project.enabled_scenarios() {
        let jp = dir.path().join(format!("journal-{}.jsonl", sc.id));
        let mut j = Journal::open(&jp).unwrap();
        apply_scenario(sc, "run1", dir.path(), &mock, &mut j, &no_return())
            .await
            .unwrap();
        drop(j);
        let report = revert_scenario(&sc.id, dir.path(), &jp, &mock)
            .await
            .unwrap();
        assert!(
            report.verify_failures.is_empty(),
            "verify failures: {:?}",
            report.verify_failures
        );
    }

    assert_eq!(before, mock.snapshot());
}

#[tokio::test]
async fn scenario_that_fails_mid_apply_reverts_clean() {
    let dir = tempfile::tempdir().unwrap();
    let pp = dir.path().join("p1.json");
    std::fs::write(&pp, PROJECT).unwrap();
    let project = Project::load(&pp).unwrap();
    let sc = project.scenarios.iter().find(|s| s.id == "s1").unwrap();

    let mock = MockController::new().with_powercfg(
        "sub_processor",
        "CPMINCORES",
        voidframe_engine::system::AcDc { ac: 50, dc: 50 },
    );
    let before = mock.snapshot();

    // Arming fail_next_write before the scenario starts fails its FIRST
    // module (powercfg's own `write_powercfg` call inside `apply()`), so
    // `apply_scenario` bails at module 0 and the second (registry) module is
    // never attempted. The journal ends up with one unconfirmed
    // (`applied: false`) record — the write it describes never actually
    // landed on the mock — which is exactly the "crash mid-mutation" shape
    // `revert_all`'s defensive handling exists for (journal §6.1/§6.2).
    let jp = dir.path().join("j.jsonl");
    let mut j = Journal::open(&jp).unwrap();
    mock.fail_next_write("disk full");
    let res = apply_scenario(sc, "run1", dir.path(), &mock, &mut j, &no_return()).await;
    drop(j);

    assert!(res.is_err(), "expected the scenario to fail mid-apply");
    let _ = revert_scenario(&sc.id, dir.path(), &jp, &mock)
        .await
        .unwrap();
    assert_eq!(before, mock.snapshot());
}
