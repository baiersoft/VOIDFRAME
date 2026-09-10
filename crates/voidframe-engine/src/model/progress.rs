//! The on-disk cursor of a run (spec §3.1): everything `execute()` needs
//! to re-enter the scenario loop after the process died at a reboot.
//! Written by the run loop at every phase boundary via
//! [`RunProgress::save`] (atomic), read back by `--resume`.

use crate::error::Result;
use crate::model::project::Project;
use crate::model::results::ScenarioResult;
use crate::paths::atomic_write;
use crate::run::execute::thermal::ThermalReading;
use crate::run::phase::RebootReason;
use crate::system::PowerPlan;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const PROGRESS_FILE: &str = "progress.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct UnstableScenario {
    pub scenario_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Apply,
    Measure,
    Revert,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Cursor {
    /// 0 = baseline, then the enabled scenarios in project order.
    pub index: u32,
    pub stage: Stage,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PendingReboot {
    pub reason: RebootReason,
    pub scenario_id: String,
    pub initiated_at: String,
    pub boot_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct RunProgress {
    pub schema_version: String,
    pub run_id: String,
    pub project: Project,
    pub start_build_id: Option<String>,
    pub start_launch_args: String,
    pub start_launch_args_raw: String,
    pub start_power_plan: PowerPlan,
    pub thermal_baseline: Option<ThermalReading>,
    pub completed: Vec<ScenarioResult>,
    pub unstable: Vec<UnstableScenario>,
    pub cursor: Cursor,
    pub reboot: Option<PendingReboot>,
    pub shutdown_when_complete: bool,
    /// Set by the resume path after a bad boot reverted the scenario at
    /// the cursor; the loop's Revert arm skips `revert_stage` once and
    /// clears it.
    #[serde(default)]
    pub skip_revert_once: bool,
    /// Set immediately before `scenario_loop` would otherwise call
    /// `reboot_sequence` while an operator Abort is pending (`*ctx.abort.
    /// borrow()` true), so the reboot is skipped and this run's `Err` exit
    /// is durable across a process restart -- see `begin_resume`'s own
    /// check of this field. `#[serde(default)]` for the same reason as
    /// `skip_revert_once`: every `progress.json` written before this field
    /// existed must still parse.
    #[serde(default)]
    pub abort_requested: bool,
}

impl RunProgress {
    pub fn path(run_dir: &Path) -> PathBuf {
        run_dir.join(PROGRESS_FILE)
    }

    pub fn save(&self, run_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(run_dir)?;
        atomic_write(&Self::path(run_dir), &serde_json::to_vec_pretty(self)?)
    }

    pub fn load(run_dir: &Path) -> Result<RunProgress> {
        let bytes = std::fs::read(Self::path(run_dir))?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::project::{Baseline, Scenario};
    use crate::model::results::Verdict;

    fn project() -> Project {
        Project {
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
            scenarios: vec![Scenario {
                id: "hags".into(),
                name: "HAGS".into(),
                description: "d".into(),
                enabled: true,
                modules: vec![],
            }],
        }
    }

    fn result(id: &str) -> ScenarioResult {
        ScenarioResult {
            scenario_id: id.into(),
            name: id.into(),
            is_baseline: id == "baseline",
            aggregated: crate::mock_harness::mock_metrics(),
            per_iteration: vec![crate::mock_harness::mock_metrics()],
            metric_deltas: vec![],
            wcps: 0.0,
            verdict: Verdict::ConfirmedSame,
            script_reverted_unverified: false,
        }
    }

    #[test]
    fn progress_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let p = RunProgress {
            schema_version: crate::model::SCHEMA_VERSION.to_string(),
            run_id: "r1".into(),
            project: project(),
            start_build_id: Some("199".into()),
            start_launch_args: "-novid".into(),
            start_launch_args_raw: "-condebug -novid".into(),
            start_power_plan: PowerPlan {
                guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
                name: "Balanced".into(),
                active: true,
            },
            thermal_baseline: Some(ThermalReading {
                cpu_temp_celsius: 41.0,
                gpu_temp_celsius: None,
                sample_count: 30,
            }),
            completed: vec![result("baseline")],
            unstable: vec![],
            cursor: Cursor {
                index: 1,
                stage: Stage::Measure,
            },
            reboot: Some(PendingReboot {
                reason: RebootReason::ApplyNext,
                scenario_id: "hags".into(),
                initiated_at: "2026-09-06T22:00:00Z".into(),
                boot_count: 0,
            }),
            shutdown_when_complete: true,
            skip_revert_once: false,
            abort_requested: false,
        };
        p.save(dir.path()).unwrap();
        assert!(dir.path().join("progress.json").exists());
        let back = RunProgress::load(dir.path()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn load_of_a_missing_file_is_an_io_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = RunProgress::load(dir.path()).unwrap_err();
        assert!(err.is_io(), "{err}");
    }
}
