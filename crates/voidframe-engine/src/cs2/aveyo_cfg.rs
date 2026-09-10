//! AveYo's `benchmark.cfg`/`benchmark2.cfg` (MIT-licensed, bundled
//! byte-for-byte in `data/cs2-benchmarks/aveyo/` — see that directory's
//! `ATTRIBUTION.md`) — the second benchmark kind
//! (`crate::model::BenchmarkKind::AveYoCfgV2`). `include_str!`-embedded at
//! compile time, the same pattern `model/catalog.rs`'s
//! `EMBEDDED_CATALOG_JSON` and `run/execute/signatures.rs`'s
//! `EMBEDDED_SIGNATURES_JSON` already use for `data/`'s other bundled
//! files, scaled up from `keybind_cfg`'s 2-line hardcoded string constant.

use crate::error::Result;
use std::path::Path;

pub const BENCHMARK_CFG_FILENAME: &str = "benchmark.cfg";
pub const BENCHMARK2_CFG_FILENAME: &str = "benchmark2.cfg";

const BENCHMARK_CFG: &str = include_str!("../../../../data/cs2-benchmarks/aveyo/benchmark.cfg");
const BENCHMARK2_CFG: &str = include_str!("../../../../data/cs2-benchmarks/aveyo/benchmark2.cfg");

/// Writes (or overwrites — idempotent) both AveYo cfg files, verbatim, into
/// CS2's own `cfg\` directory. Only called for `BenchmarkKind::AveYoCfgV2`
/// projects (`run/execute/launch.rs`'s `prepare_cs2_session`) —
/// `WorkshopDust2` projects never touch CS2's `cfg\` directory beyond
/// `keybind_cfg`'s own file.
pub fn ensure_written(cs2_cfg_dir: &Path) -> Result<()> {
    super::write_cfg_files(
        cs2_cfg_dir,
        &[
            (BENCHMARK_CFG_FILENAME, BENCHMARK_CFG),
            (BENCHMARK2_CFG_FILENAME, BENCHMARK2_CFG),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_benchmark_cfg_looks_like_the_real_aveyo_file() {
        assert!(BENCHMARK_CFG.contains("Benchmark.cfg by AveYo"));
        assert!(BENCHMARK_CFG.contains("v2026.05.19"));
        assert!(BENCHMARK_CFG.trim_end().ends_with("/// benchmark.cfg"));
    }

    #[test]
    fn embedded_benchmark2_cfg_looks_like_the_real_aveyo_file() {
        assert!(BENCHMARK2_CFG.contains("Benchmark.cfg by AveYo"));
        assert!(BENCHMARK2_CFG.contains("v2026.05.19"));
        assert!(BENCHMARK2_CFG.trim_end().ends_with("/// benchmark2.cfg"));
    }

    #[test]
    fn ensure_written_copies_both_embedded_files_into_the_cfg_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("cfg");

        ensure_written(&cfg_dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(cfg_dir.join(BENCHMARK_CFG_FILENAME)).unwrap(),
            BENCHMARK_CFG
        );
        assert_eq!(
            std::fs::read_to_string(cfg_dir.join(BENCHMARK2_CFG_FILENAME)).unwrap(),
            BENCHMARK2_CFG
        );
    }

    #[test]
    fn ensure_written_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("cfg");

        ensure_written(&cfg_dir).unwrap();
        ensure_written(&cfg_dir).unwrap();

        assert!(cfg_dir.join(BENCHMARK_CFG_FILENAME).exists());
        assert!(cfg_dir.join(BENCHMARK2_CFG_FILENAME).exists());
    }

    #[test]
    fn ensure_written_creates_a_missing_cfg_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_dir = tmp.path().join("nested").join("cfg");
        assert!(!cfg_dir.exists());

        ensure_written(&cfg_dir).unwrap();

        assert!(cfg_dir.join(BENCHMARK_CFG_FILENAME).exists());
    }
}
