//! Loads the calibrated Hotelling T-squared thresholds and pacing TOST
//! margins produced by this project's research trail
//! (`study/research/scripts/calibrate_thresholds_v3.py`,
//! `study/research/wcps-v3-research-gaps-findings.md`). Compile-time
//! embedded, same convention as `model::catalog::EMBEDDED_CATALOG_JSON`.

use crate::error::{Error, Result};
use serde::Deserialize;
use std::collections::BTreeMap;

const EMBEDDED_THRESHOLDS_JSON: &str = include_str!("../../../data/calibrated_thresholds_v3.json");

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
pub struct PercentilePair {
    pub p95: f64,
    pub p97_5: f64,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct ThresholdPair {
    pub throughput: PercentilePair,
    pub pacing: PercentilePair,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct CalibratedThresholds {
    pub throughput_metrics: Vec<String>,
    pub pacing_metrics: Vec<String>,
    pub pacing_tost_margins_pct: BTreeMap<String, f64>,
    // Deserialized from the JSON's string-keyed "thresholds" object
    // (`{"2": {...}, "3": {...}, ...}`) directly into a `u32`-keyed map --
    // serde_json supports non-string map keys that implement `FromStr`
    // (`u32` does) transparently here, no manual parsing needed.
    pub thresholds: BTreeMap<u32, ThresholdPair>,
}

/// `n=2`'s own calibration is present in the embedded JSON (the calibration
/// script ran it like every other ladder point) but independently found
/// unreliable: its throughput `p97_5` jumps more than 200x versus `n=3`'s
/// (a ridge-regularization artifact on a near-degenerate 2-point pooled
/// covariance -- see `study/research/wcps-v3-research-gaps-findings.md`).
/// `for_n` refuses to serve it rather than silently handing a caller a
/// known-untrustworthy threshold; the data stays in the JSON as a record of
/// what was measured, it just isn't a valid answer to "give me n=2".
const KNOWN_UNRELIABLE_N: u32 = 2;

/// Every `n` this crate has a calibrated Hotelling threshold for, in
/// ascending order -- `n=1` is mathematically undefined (see
/// `study/research/wcps-v3-audit.md`), `n=2` is present in the data but
/// refused by `for_n` as unreliable (see `KNOWN_UNRELIABLE_N`'s own doc
/// comment). `Settings::validate` checks a configured `measure_loops`
/// against this exact list, so the two can never silently drift apart.
pub const CALIBRATED_N_LADDER: &[u32] = &[3, 4, 5, 8, 10, 15];

impl CalibratedThresholds {
    /// The calibrated threshold pair for exactly `n` measure iterations, or
    /// `None` if `n` isn't in the calibrated ladder (currently
    /// {2,3,4,5,8,10,15} -- n=1 is never present, see
    /// `study/research/wcps-v3-audit.md`'s confirmed n=1 finding: Hotelling's
    /// T^2 is mathematically undefined there, not merely uncalibrated) OR if
    /// `n` is `KNOWN_UNRELIABLE_N` (see that constant's doc comment --
    /// present in the data, deliberately refused here).
    pub fn for_n(&self, n: u32) -> Option<&ThresholdPair> {
        if n == KNOWN_UNRELIABLE_N {
            return None;
        }
        self.thresholds.get(&n)
    }
}

/// Parses the embedded calibrated-thresholds JSON. Fails only if the
/// embedded file itself is malformed -- since it's `include_str!`-baked at
/// compile time from a file this crate's own test suite (this module's own
/// tests, below) verifies against a known-good fixture, this should never
/// fail in a built binary; returning `Result` rather than panicking still
/// matches this crate's "no unjustified panics outside genuinely
/// unreachable states" convention.
pub fn load_calibrated_thresholds() -> Result<CalibratedThresholds> {
    serde_json::from_str(EMBEDDED_THRESHOLDS_JSON).map_err(Error::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_without_error_and_has_every_calibrated_n() {
        let thr = load_calibrated_thresholds().unwrap();
        for n in [3, 4, 5, 8, 10, 15] {
            assert!(
                thr.for_n(n).is_some(),
                "missing calibrated threshold for n={n}"
            );
        }
        assert!(
            thr.for_n(1).is_none(),
            "n=1 must not have a calibrated threshold (mathematically undefined)"
        );
    }

    #[test]
    fn n_equals_2_is_present_in_the_data_but_refused_as_unreliable() {
        let thr = load_calibrated_thresholds().unwrap();
        assert!(
            thr.thresholds.contains_key(&2),
            "n=2's calibration should still be present in the embedded JSON as a record"
        );
        assert!(
            thr.for_n(2).is_none(),
            "for_n(2) must refuse to serve the known-unreliable n=2 threshold"
        );
    }

    #[test]
    fn calibrated_n_ladder_matches_the_embedded_thresholds_exactly() {
        let thr = load_calibrated_thresholds().unwrap();
        let mut json_ns: Vec<u32> = thr.thresholds.keys().copied().collect();
        json_ns.sort_unstable();
        // n=2 is present in the JSON but refused by `for_n` -- excluded
        // from the public ladder on purpose, not a mismatch.
        json_ns.retain(|&n| n != 2);
        assert_eq!(json_ns, CALIBRATED_N_LADDER);
    }

    #[test]
    fn pacing_margins_are_the_current_research_validated_value() {
        // 10.0/10.0, not the earlier unvalidated 15.0/15.0 placeholder --
        // study/research/wcps-v3-research-gaps-findings.md's margin sweep
        // result. A future re-calibration is expected to change this
        // number and this test right along with it (update BOTH the
        // embedded JSON and this assertion together, never one without the
        // other) -- this test's job is to catch an accidental stale copy,
        // not to freeze the number forever.
        let thr = load_calibrated_thresholds().unwrap();
        assert_eq!(
            thr.pacing_tost_margins_pct.get("stutter_count_pct"),
            Some(&10.0)
        );
        assert_eq!(
            thr.pacing_tost_margins_pct
                .get("mean_abs_animation_error_ms"),
            Some(&10.0)
        );
    }
}
