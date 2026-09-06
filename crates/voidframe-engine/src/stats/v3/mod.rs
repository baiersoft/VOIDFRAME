//! WCPS v3's statistical core: Hotelling's T-squared two-vector test, TOST
//! equivalence testing, the 3 new pacing/throughput metrics, calibrated
//! threshold lookup, and the combined verdict. Wired into the real scoring
//! pipeline (`run::execute::score_scenarios`), replacing WCPS v2 entirely --
//! see `docs/superpowers/plans/2026-09-04-wcps-v3-pipeline-replace-v2.md`.

mod hotelling;
mod linalg;
mod metrics;
mod score;
mod thresholds;
mod tost;
mod verdict;
pub use hotelling::hotelling_t2_statistic;
pub use linalg::spd_inverse;
pub use metrics::{adaptive_frame_time_cv, mean_abs_animation_error_ms, stutter_count_pct};
pub use score::{WcpsV3Weights, compute_wcps_v3};
pub use thresholds::{CALIBRATED_N_LADDER, CalibratedThresholds, load_calibrated_thresholds};
pub use tost::{TostResult, tost_evaluate};
pub use verdict::evaluate_verdict;
