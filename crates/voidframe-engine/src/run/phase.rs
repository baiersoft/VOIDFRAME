//! The run's phase vocabulary, shared by the engine (emits it), the Tauri
//! shell (persists it in `RunState` and picks the journal to roll back
//! from it) and the UI (renders it). One enum instead of the free strings
//! all three used to parse independently (ARCHITECTURE_PROPOSED.md §1.2 F2).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Phase {
    Preflight,
    Snapshot,
    ThermalBaseline,
    Baseline,
    Scenario { id: String },
    Rollback,
    Report,
}

impl Phase {
    /// The scenario whose journal is "current" during this phase --
    /// `"baseline"` for the baseline (its journal file is
    /// `journal-baseline.jsonl`), the scenario's own id for a scenario,
    /// `None` for run-scoped phases. Crash recovery (`rollback_now`) picks
    /// the journal to reverse-replay from this.
    pub fn scenario_id(&self) -> Option<&str> {
        match self {
            Phase::Baseline => Some("baseline"),
            Phase::Scenario { id } => Some(id),
            _ => None,
        }
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Phase::Preflight => f.write_str("preflight"),
            Phase::Snapshot => f.write_str("snapshot"),
            Phase::ThermalBaseline => f.write_str("thermal_baseline"),
            Phase::Baseline => f.write_str("baseline"),
            Phase::Scenario { id } => write!(f, "scenario:{id}"),
            Phase::Rollback => f.write_str("rollback"),
            Phase::Report => f.write_str("report"),
        }
    }
}

/// Whether an iteration is a (never captured) warmup or a measured one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum IterationKind {
    Warmup,
    Measure,
}

/// Which game-synchronisation tier produced a run's captures
/// (`docs/03-functional-spec.md` §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum DetectionTier {
    LogTail,
    FixedWindow,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scenario_id_is_baseline_for_baseline_and_the_id_for_a_scenario() {
        assert_eq!(Phase::Baseline.scenario_id(), Some("baseline"));
        assert_eq!(
            Phase::Scenario { id: "s2".into() }.scenario_id(),
            Some("s2")
        );
        for run_scoped in [
            Phase::Preflight,
            Phase::Snapshot,
            Phase::ThermalBaseline,
            Phase::Rollback,
            Phase::Report,
        ] {
            assert_eq!(run_scoped.scenario_id(), None, "{run_scoped}");
        }
    }

    #[test]
    fn serializes_as_a_kind_tagged_object() {
        assert_eq!(
            serde_json::to_string(&Phase::Preflight).unwrap(),
            r#"{"kind":"preflight"}"#
        );
        assert_eq!(
            serde_json::to_string(&Phase::Scenario { id: "s1".into() }).unwrap(),
            r#"{"kind":"scenario","id":"s1"}"#
        );
        let back: Phase = serde_json::from_str(r#"{"kind":"scenario","id":"s1"}"#).unwrap();
        assert_eq!(back, Phase::Scenario { id: "s1".into() });
    }

    #[test]
    fn display_matches_the_pre_enum_log_strings() {
        assert_eq!(Phase::ThermalBaseline.to_string(), "thermal_baseline");
        assert_eq!(
            Phase::Scenario {
                id: "core-parking".into()
            }
            .to_string(),
            "scenario:core-parking"
        );
    }

    #[test]
    fn iteration_kind_and_detection_tier_are_snake_case_strings() {
        assert_eq!(
            serde_json::to_string(&IterationKind::Warmup).unwrap(),
            r#""warmup""#
        );
        assert_eq!(
            serde_json::to_string(&DetectionTier::LogTail).unwrap(),
            r#""log_tail""#
        );
    }
}
