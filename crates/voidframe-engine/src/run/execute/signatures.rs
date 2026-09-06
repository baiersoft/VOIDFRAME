//! Resolves and loads `signatures.json` for a scenario's detection channel.

use crate::cs2::detection::Signatures;
use crate::error::{Error, Result};
use std::path::Path;

/// Compile-time-embedded fallback for `signatures.json`: the repo-root
/// seed file, baked into the binary at build time. Neither
/// `<data_root>/signatures.json` nor a `data/` directory next to the
/// running executable exists in a real `cargo run`/deployed-binary
/// scenario (no build step copies the repo-root seed anywhere), so without
/// this fallback `load_signatures` below is unreachable outside a dev
/// checkout that happens to have one of those paths populated by hand.
const EMBEDDED_SIGNATURES_JSON: &str = include_str!("../../../../../data/signatures.json");

/// Loads `signatures.json`, preferring in order: a user override at
/// `<data_root>/signatures.json` (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.4's "overridable in the data
/// dir"); the repo-seeded copy shipped next to the running executable
/// (`docs/02-architecture.md`'s "repo `data/` holds only read-only seed
/// templates" convention); and finally the copy embedded into this binary
/// at compile time, so a genuinely unmodified deployment still works. No
/// existing seed-resolution helper was found in `paths.rs` to reuse
/// (checked before adding this).
pub(super) fn load_signatures(data_root: &Path) -> Result<Signatures> {
    let override_path = data_root.join("signatures.json");
    if override_path.exists() {
        return Signatures::from_file(&override_path);
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        let seeded = exe_dir.join("data").join("signatures.json");
        if seeded.exists() {
            return Signatures::from_file(&seeded);
        }
    }
    serde_json::from_str(EMBEDDED_SIGNATURES_JSON).map_err(Error::from)
}
