//! `ThermalLog` — persists a run's thermal data (docs/superpowers/specs/2026-09-04-thermal-cooldown-hardware-info-design.md §A2) to
//! `runs/<run_id>/thermal.json`: the pre-run baseline reading plus every
//! cooldown-check reading taken during each inter-scenario break, keyed by
//! scenario id (`"baseline"` for the break right after the BASELINE phase).
//!
//! Written best-effort by `run::execute` -- a write failure here is a
//! warning, never a run failure (results.json is the run's real record;
//! this file is diagnostic).

use crate::error::Result;
use crate::paths::atomic_write;
use crate::run::execute::thermal::ThermalReading;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermalLog {
    pub baseline: Option<ThermalReading>,
    pub cooldown_checks: Vec<(String, Vec<ThermalReading>)>,
}

impl ThermalLog {
    /// Pretty-print + atomic-write this log to `run_dir/thermal.json`.
    pub fn write(&self, run_dir: &Path) -> Result<()> {
        atomic_write(
            &run_dir.join("thermal.json"),
            &serde_json::to_vec_pretty(self)?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_round_trips_a_thermal_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = ThermalLog {
            baseline: Some(ThermalReading {
                cpu_temp_celsius: 60.0,
                gpu_temp_celsius: Some(50.0),
                sample_count: 30,
            }),
            cooldown_checks: vec![(
                "scen_1".into(),
                vec![ThermalReading {
                    cpu_temp_celsius: 62.0,
                    gpu_temp_celsius: None,
                    sample_count: 30,
                }],
            )],
        };
        log.write(dir.path()).unwrap();
        let read_back: ThermalLog = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("thermal.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(read_back.baseline.unwrap().cpu_temp_celsius, 60.0);
        assert_eq!(read_back.cooldown_checks[0].0, "scen_1");
    }
}
