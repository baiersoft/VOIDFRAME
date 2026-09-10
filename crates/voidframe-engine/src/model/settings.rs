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

/// AveYo's `benchmark2.cfg` disconnects ~58s after `BenchmarkStarted`, so
/// the settle before capture plus the timed capture must end before that
/// point (spec D6, confirmed live: settle 5s + capture 57s = 62s from the
/// map-loaded marker, with `BenchmarkStarted` ~3-4s after it). Both halves
/// live here so the run loop's settle (`run::execute::scenario::
/// settle_before_capture`) and `Settings::validate`'s bound on
/// `capture_seconds` can never drift apart.
pub const AVEYO_SETTLE_SECONDS: u64 = 5;
pub const AVEYO_MAX_CAPTURE_SECONDS: u32 = 57;
fn default_map() -> String {
    "3240880604".to_string()
}
fn default_watchdog() -> u32 {
    120
}

/// Which benchmark a project runs. `WorkshopDust2` is the original M1
/// benchmark (spec D6 in `docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md`);
/// `AveYoCfgV2` is AveYo's MIT-licensed `benchmark.cfg`/`benchmark2.cfg`
/// (docs/superpowers/specs/2026-09-09-aveyo-benchmark-design.md). Not a
/// `Module` — this is project identity (like `map_id`), with no apply/revert
/// or journal entry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum BenchmarkKind {
    #[default]
    WorkshopDust2,
    #[serde(rename = "aveyo_cfg_v2")]
    AveYoCfgV2,
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
    #[serde(default)]
    pub benchmark_kind: BenchmarkKind,
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
        // Exhaustive on purpose: a third kind must decide both questions
        // here rather than silently inheriting one branch's answer.
        match self.benchmark_kind {
            BenchmarkKind::WorkshopDust2 => {
                // `map_id` is a Steam workshop addon id and is always purely
                // numeric — it gets interpolated unvalidated into a
                // `map_workshop {map_id} de_dust2` console command sent via
                // `send_console_command`/`SendInput`, and Source's console
                // treats `;` as a command separator, so anything else here
                // would be a console command injection vector (e.g.
                // `"3240880604; sv_cheats 1"`).
                if self.map_id.is_empty() || !self.map_id.chars().all(|c| c.is_ascii_digit()) {
                    return Err(Error::msg(
                        "map_id must be a non-empty numeric Steam workshop addon id".into(),
                    ));
                }
            }
            // `map_id` only means anything for the Workshop benchmark --
            // AveYo's cfg-triggered benchmark never references it (spec
            // D4). AveYo's own constraint is the capture window, checked by
            // [`Settings::validate_capture_window`] at run readiness rather
            // than here: this method gates `Project::load`, and an older
            // AveYo project saved when the default was 60s or 75s must keep
            // loading (and stay editable) instead of silently vanishing
            // from the dashboard.
            BenchmarkKind::AveYoCfgV2 => {}
        }
        Ok(())
    }

    /// The run-readiness half of validation: serde's `capture_seconds`
    /// default (105, Dust2's) cannot see `benchmark_kind`, so a hand-written
    /// AveYo project that omits it -- or a Dust2 project flipped to AveYo --
    /// would otherwise run a 105s capture over a ~58s benchmark and record
    /// ~45s of main-menu frames as if they were gameplay. Called by
    /// `Project::validate` (the UI's pre-run check) and by the run loop at
    /// PREFLIGHT, so every front end refuses it with the same message.
    pub fn validate_capture_window(&self) -> Result<()> {
        match self.benchmark_kind {
            BenchmarkKind::WorkshopDust2 => Ok(()),
            BenchmarkKind::AveYoCfgV2 if self.capture_seconds > AVEYO_MAX_CAPTURE_SECONDS => {
                Err(Error::msg(format!(
                    "capture_seconds must be at most {AVEYO_MAX_CAPTURE_SECONDS} for the AveYo \
                     benchmark.cfg v2 benchmark (its scripted run ends ~58s in, and the \
                     {AVEYO_SETTLE_SECONDS}s settle before capture counts against that window) \
                     -- got {}",
                    self.capture_seconds
                )))
            }
            BenchmarkKind::AveYoCfgV2 => Ok(()),
        }
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
        assert_eq!(s.benchmark_kind, BenchmarkKind::WorkshopDust2);
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
        // `map_workshop {map_id} de_dust2`, sent via `send_console_command` ->
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

    #[test]
    fn default_benchmark_kind_is_workshop_dust2() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.benchmark_kind, BenchmarkKind::WorkshopDust2);
    }

    #[test]
    fn aveyo_kind_skips_map_id_validation() {
        let s = Settings {
            benchmark_kind: BenchmarkKind::AveYoCfgV2,
            map_id: "".into(),
            ..serde_json::from_str("{}").unwrap()
        };
        assert!(s.validate().is_ok());
    }

    /// The serde default (105, Dust2's) cannot see `benchmark_kind`, so an
    /// AveYo project that never set `capture_seconds` -- a hand-written JSON
    /// for `voidframe-cli`, or a Dust2 project flipped to AveYo -- must be
    /// refused at run readiness rather than run a 105s capture over a ~58s
    /// benchmark.
    #[test]
    fn aveyo_kind_rejects_a_capture_longer_than_its_benchmark_at_run_readiness() {
        let defaulted: Settings =
            serde_json::from_str(r#"{"benchmark_kind":"aveyo_cfg_v2"}"#).unwrap();
        let err = defaulted.validate_capture_window().unwrap_err();
        assert!(err.to_string().contains("capture_seconds"), "{err}");

        let just_over = Settings {
            capture_seconds: AVEYO_MAX_CAPTURE_SECONDS + 1,
            ..defaulted.clone()
        };
        assert!(just_over.validate_capture_window().is_err());

        let at_the_bound = Settings {
            capture_seconds: AVEYO_MAX_CAPTURE_SECONDS,
            ..defaulted
        };
        assert!(at_the_bound.validate_capture_window().is_ok());
    }

    /// Load-time validation deliberately does NOT apply the bound: an
    /// older AveYo project saved with the branch's earlier 60s/75s default
    /// must still load and be editable, not vanish from the dashboard.
    #[test]
    fn an_oversized_aveyo_capture_still_passes_load_time_validation() {
        let s: Settings =
            serde_json::from_str(r#"{"benchmark_kind":"aveyo_cfg_v2","capture_seconds":75}"#)
                .unwrap();
        assert!(s.validate().is_ok());
        assert!(s.validate_capture_window().is_err());
    }

    /// Dust2's 105s default is untouched by the AveYo bound.
    #[test]
    fn workshop_dust2_kind_keeps_the_105s_default_capture() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.capture_seconds, 105);
        assert!(s.validate().is_ok());
        assert!(s.validate_capture_window().is_ok());
    }

    #[test]
    fn workshop_dust2_kind_still_rejects_a_bad_map_id() {
        let s = Settings {
            benchmark_kind: BenchmarkKind::WorkshopDust2,
            map_id: "not_a_number".into(),
            ..serde_json::from_str("{}").unwrap()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn benchmark_kind_round_trips_through_json_as_snake_case() {
        let s = Settings {
            benchmark_kind: BenchmarkKind::AveYoCfgV2,
            ..serde_json::from_str("{}").unwrap()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert!(
            json.contains("\"benchmark_kind\":\"aveyo_cfg_v2\""),
            "got {json}"
        );
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.benchmark_kind, BenchmarkKind::AveYoCfgV2);
    }
}
