//! CS2-specific orchestration logic. See docs/superpowers/plans/2026-09-01-m1-phase-3b-cs2-control-run-loop.md.

pub mod aveyo_cfg;
pub mod detection;
pub mod keybind_cfg;

use crate::error::{Error, Result};
use std::path::Path;

/// Writes (or overwrites -- idempotent) each `(filename, contents)` pair
/// into CS2's own `cfg\` directory, creating the directory first. A real
/// CS2 install always ships `cfg\` already, so the create is normally a
/// no-op -- done defensively rather than assumed, matching every caller's
/// "safe to call every time" contract. Shared by `keybind_cfg` and
/// `aveyo_cfg` so the two never drift on error wording or directory
/// handling.
pub(crate) fn write_cfg_files(cs2_cfg_dir: &Path, files: &[(&str, &str)]) -> Result<()> {
    std::fs::create_dir_all(cs2_cfg_dir)
        .map_err(|e| Error::msg(format!("creating {}: {e}", cs2_cfg_dir.display())))?;
    for (filename, contents) in files {
        let dest = cs2_cfg_dir.join(filename);
        std::fs::write(&dest, contents)
            .map_err(|e| Error::msg(format!("writing {}: {e}", dest.display())))?;
    }
    Ok(())
}
