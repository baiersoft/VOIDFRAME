//! Per-project run settings.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

fn default_schema() -> String {
    super::SCHEMA_VERSION.to_string()
}
fn default_measure() -> u32 {
    3
}
fn default_warmup() -> u32 {
    2
}
fn default_capture() -> u32 {
    105
}
fn default_map() -> String {
    "3240880604".to_string()
}
fn default_watchdog() -> u32 {
    120
}

/// Run parameters. Every field has a spec default so an empty `{}` deserializes.
///
/// `warmup_loops` is a placeholder calibrated in
/// `docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md` §11a;
/// `measure_loops` must be one of `stats::v3::CALIBRATED_N_LADDER`'s
/// calibrated values (enforced by [`Settings::validate`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Settings {
    #[serde(default = "default_schema")]
    pub schema_version: String,
    #[serde(default = "default_measure")]
    pub measure_loops: u32,
    #[serde(default = "default_warmup")]
    pub warmup_loops: u32,
    #[serde(default = "default_capture")]
    pub capture_seconds: u32,
    #[serde(default = "default_map")]
    pub map_id: String,
    #[serde(default = "default_watchdog")]
    pub watchdog_seconds: u32,
    #[serde(default)]
    pub netcon_port: Option<u16>,
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        if !crate::stats::v3::CALIBRATED_N_LADDER.contains(&self.measure_loops) {
            return Err(Error::msg(format!(
                "measure_loops must be one of {:?} (this crate's calibrated WCPS v3 ladder) -- got {}",
                crate::stats::v3::CALIBRATED_N_LADDER,
                self.measure_loops
            )));
        }
        // `map_id` is a Steam workshop addon id and is always purely
        // numeric — it gets interpolated unvalidated into a
        // `map_workshop {map_id} de_dust2` console command sent via
        // `reissue_map`/`SendInput`, and Source's console treats `;` as a
        // command separator, so anything else here would be a console
        // command injection vector (e.g. `"3240880604; sv_cheats 1"`).
        if self.map_id.is_empty() || !self.map_id.chars().all(|c| c.is_ascii_digit()) {
            return Err(Error::msg(
                "map_id must be a non-empty numeric Steam workshop addon id".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.measure_loops, 3);
        assert_eq!(s.warmup_loops, 2);
        assert_eq!(s.capture_seconds, 105);
        assert_eq!(s.map_id, "3240880604");
        assert_eq!(s.watchdog_seconds, 120);
        assert_eq!(s.netcon_port, None);
    }

    #[test]
    fn rejects_a_measure_loops_value_outside_the_calibrated_ladder() {
        let below_ladder = Settings {
            measure_loops: 2,
            ..serde_json::from_str("{}").unwrap()
        };
        assert!(below_ladder.validate().is_err());

        // Plausible-looking but NOT calibrated -- must still be rejected,
        // not silently accepted just because it's "close enough" to 5/8.
        let uncalibrated = Settings {
            measure_loops: 6,
            ..serde_json::from_str("{}").unwrap()
        };
        assert!(uncalibrated.validate().is_err());

        for n in [5, 8] {
            let s = Settings {
                measure_loops: n,
                ..serde_json::from_str("{}").unwrap()
            };
            assert!(s.validate().is_ok(), "measure_loops={n} should be accepted");
        }
    }

    #[test]
    fn default_map_id_passes_validation() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert!(s.validate().is_ok());
    }

    #[test]
    fn rejects_a_map_id_containing_a_console_command_separator() {
        // `map_id` is interpolated unvalidated into
        // `map_workshop {map_id} de_dust2`, sent via `reissue_map` ->
        // `SendInput` straight into CS2's console — `;` separates console
        // commands, so a crafted map_id could run arbitrary commands.
        let s = Settings {
            map_id: "3240880604; sv_cheats 1".into(),
            ..serde_json::from_str("{}").unwrap()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_a_non_numeric_map_id() {
        let s = Settings {
            map_id: "not_a_number".into(),
            ..serde_json::from_str("{}").unwrap()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn rejects_an_empty_map_id() {
        let s = Settings {
            map_id: "".into(),
            ..serde_json::from_str("{}").unwrap()
        };
        assert!(s.validate().is_err());
    }
}
