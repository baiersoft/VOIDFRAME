//! App-wide settings persisted at `<data_root>\config.json` (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §5.1) —
//! PresentMon path, HWiNFO path, and whether Dry Run is on by default.
//! Distinct from `Settings` (`settings.rs`), which is *per-project* run
//! parameters — this is the one thing shared across every project.

use crate::error::Result;
use crate::paths::atomic_write;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Joins an exe directory + tool exe name into one path string. Split out
/// from [`installed_tool_path`] purely so the join logic is testable
/// without depending on the live `current_exe()` of whatever process
/// happens to run the test.
fn tool_path_under(exe_dir: &Path, exe_name: &str) -> String {
    exe_dir.join(exe_name).to_string_lossy().into_owned()
}

/// Resolves `<the running exe's directory>\<exe_name>` — both PresentMon
/// and HWiNFO64 are bundled by the installer as flat sidecars directly next
/// to `voidframe.exe` (Tauri's `externalBin` has no destination-subfolder
/// support; sidecars always land at the install root, stripped of their
/// target-triple suffix — see
/// docs/superpowers/specs/2026-09-05-bundled-tools-alpha-install-design.md
/// §1-2), so this is a real, installer-guaranteed location, not a guess,
/// for any deployed build. `current_exe()` failing at all is not a
/// realistic deployed-binary scenario, but a settings default must never
/// panic, so this falls back to `fallback` rather than unwrapping.
fn installed_tool_path(exe_name: &str, fallback: &str) -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|dir| tool_path_under(dir, exe_name)))
        .unwrap_or_else(|| fallback.to_string())
}

fn default_presentmon_path() -> String {
    installed_tool_path("presentmon.exe", r"C:\presentmon.exe")
}

/// HWiNFO64's bundled install location. Unlike the pre-alpha era (see the
/// now-removed doc comment this replaced), VOIDFRAME ships and controls
/// HWiNFO64 itself now — a guessed default no longer silently opts a
/// stranger's machine into a feature tied to software they never
/// installed; it's describing where VOIDFRAME's own installer put it.
fn default_hwinfo_path() -> Option<String> {
    Some(installed_tool_path("hwinfo64.exe", r"C:\hwinfo64.exe"))
}

/// HWiNFO-based thermal cooldown is on by default for the same reason —
/// VOIDFRAME bundles and pre-configures HWiNFO64 (Shared Memory Support
/// enabled via the shipped ini), so the prerequisite this used to be
/// opt-in about is now guaranteed present on a fresh install.
fn default_thermal_cooldown_enabled() -> bool {
    true
}

/// Spec D5: Windows runs maintenance in the first minutes after boot, so
/// `--resume` waits this long before touching anything.
fn default_post_boot_settle_seconds() -> u32 {
    180
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Config {
    #[serde(default = "default_presentmon_path")]
    pub presentmon_path: String,
    #[serde(default)]
    pub dry_run_default: bool,
    #[serde(default)]
    pub last_known_cs2_build_id: Option<String>,
    /// HWiNFO64.exe's bundled install location (docs/superpowers/specs/2026-09-05-bundled-tools-alpha-install-design.md §2).
    #[serde(default = "default_hwinfo_path")]
    pub hwinfo_path: Option<String>,
    /// On by default -- see `default_thermal_cooldown_enabled`.
    #[serde(default = "default_thermal_cooldown_enabled")]
    pub thermal_cooldown_enabled: bool,
    /// Spec §7 / D3: default state of the countdown modal's "Shut down when
    /// the run completes" toggle. Off by default -- an unattended shutdown
    /// is an opt-in behavior change to what the machine does on its own.
    #[serde(default)]
    pub shutdown_when_complete_default: bool,
    /// Spec D5: default `RunConfig::post_boot_settle` in seconds.
    #[serde(default = "default_post_boot_settle_seconds")]
    pub post_boot_settle_seconds: u32,
    /// Whether the user has dismissed the one-time "custom scripts run
    /// elevated, VOIDFRAME can't verify what they do" warning -- shown once,
    /// the first time they ever add a `custom_script` module, not per-script
    /// or per-run (the per-script hash-tracking confirmation gate this used
    /// to be was ruled too unpractical for a single-user, locally-run app
    /// where the user is always the one who wrote the script in the first
    /// place).
    #[serde(default)]
    pub custom_script_warning_seen: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            presentmon_path: default_presentmon_path(),
            dry_run_default: false,
            last_known_cs2_build_id: None,
            hwinfo_path: default_hwinfo_path(),
            thermal_cooldown_enabled: default_thermal_cooldown_enabled(),
            shutdown_when_complete_default: false,
            post_boot_settle_seconds: default_post_boot_settle_seconds(),
            custom_script_warning_seen: false,
        }
    }
}

impl Config {
    /// A missing file (first run — `save_config`/`SettingsModal` has never
    /// written one yet) is not an error: returns `Config::default()`.
    ///
    /// A present file whose `presentmon_path`/`hwinfo_path` no longer
    /// resolves to a real file on disk (a stale value from an old dev
    /// build's hardcoded guess, HWiNFO64 quarantined by AV, or a reinstall
    /// to a different directory) is self-healed back to the bundled-install
    /// default rather than persisted or surfaced as a save failure —
    /// `SettingsModal` no longer renders these fields, so the user has no
    /// way to see or fix a stale value themselves. See `heal_stale_tool_paths`.
    pub fn load(path: &Path) -> Result<Config> {
        match std::fs::read(path) {
            Ok(bytes) => {
                let mut config: Config = serde_json::from_slice(&bytes)?;
                config.heal_stale_tool_paths();
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        atomic_write(path, &serde_json::to_vec_pretty(self)?)
    }

    /// Re-derives `presentmon_path`/`hwinfo_path` from the same
    /// `installed_tool_path`-based defaults `Config::default` uses,
    /// whenever the stored value no longer points at a real file. This is
    /// deliberately narrower than `save_config_impl`'s validation
    /// (`src-tauri/src/commands/config.rs`): that path stays a hard
    /// rejection for a value a user actually typed into some future UI;
    /// this one only ever fires against a value the user never typed and,
    /// with the path inputs removed from `SettingsModal`, can no longer see
    /// or correct.
    fn heal_stale_tool_paths(&mut self) {
        if !Path::new(&self.presentmon_path).is_file() {
            self.presentmon_path = default_presentmon_path();
        }
        if let Some(hwinfo_path) = &self.hwinfo_path
            && !Path::new(hwinfo_path).is_file()
        {
            self.hwinfo_path = default_hwinfo_path();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_a_real_usable_presentmon_path_guess() {
        let c = Config::default();
        assert!(!c.presentmon_path.is_empty());
        assert!(!c.dry_run_default);
    }

    #[test]
    fn tool_path_under_joins_the_exe_directory_and_exe_name() {
        let dir = std::path::Path::new(r"C:\Program Files\voidframe");
        assert_eq!(
            tool_path_under(dir, "presentmon.exe"),
            r"C:\Program Files\voidframe\presentmon.exe"
        );
    }

    #[test]
    fn default_presentmon_path_resolves_next_to_the_running_executable() {
        let path = default_presentmon_path();
        assert!(
            path.ends_with(r"presentmon.exe"),
            "expected a presentmon.exe suffix, got {path}"
        );
    }

    #[test]
    fn default_ships_a_bundled_hwinfo_path_guess_with_thermal_cooldown_on() {
        // VOIDFRAME now bundles HWiNFO64 next to its own executable (alpha
        // release scope, docs/superpowers/specs/2026-09-05-bundled-tools-alpha-install-design.md
        // §2-3) -- unlike the old BYOB era, a guessed path no longer silently
        // opts a stranger's machine into a feature they never installed
        // anything for, since VOIDFRAME shipped the thing itself.
        let c = Config::default();
        let hwinfo_path = c
            .hwinfo_path
            .expect("hwinfo_path should have a bundled default");
        assert!(
            hwinfo_path.ends_with(r"hwinfo64.exe"),
            "expected a hwinfo64.exe suffix, got {hwinfo_path}"
        );
        assert!(c.thermal_cooldown_enabled);
    }

    #[test]
    fn load_of_a_missing_file_returns_default_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let c = Config::load(&path).unwrap();
        assert_eq!(c, Config::default());
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        // A real, existing file: `Config::load` self-heals any
        // `presentmon_path` that doesn't resolve to a real file (see
        // `heal_stale_tool_paths`), so a round-trip test must use one that
        // does, or the reload would legitimately differ from `c`.
        let exe = dir.path().join("PresentMon.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let c = Config {
            presentmon_path: exe.to_string_lossy().into_owned(),
            dry_run_default: true,
            ..Default::default()
        };
        c.save(&path).unwrap();
        let reloaded = Config::load(&path).unwrap();
        assert_eq!(c, reloaded);
    }

    #[test]
    fn partial_json_fills_in_missing_fields_with_defaults() {
        // A config.json from an OLDER version of this struct (missing a
        // field this version added) must still load, not hard-fail —
        // same forward-compat convention Settings/Project already use
        // (#[serde(default = ...)] on every field).
        //
        // The stored `presentmon_path` here doesn't resolve to a real file
        // on this machine, so `Config::load`'s self-healing (see
        // `heal_stale_tool_paths`) re-derives it -- this test only cares
        // that the *other* missing fields still fell back to their
        // defaults, not that this specific path string survived verbatim.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(&path, r#"{"presentmon_path":"C:\\PresentMon.exe"}"#).unwrap();
        let c = Config::load(&path).unwrap();
        assert!(c.thermal_cooldown_enabled); // fell back to default
    }

    #[test]
    fn load_heals_a_stale_presentmon_path_that_no_longer_resolves() {
        // Finding 3 (final whole-branch review): SettingsModal no longer
        // renders `presentmon_path`, so a user has no way to see or fix a
        // stored value that stops resolving (old dev-build guess, AV
        // quarantine, reinstall elsewhere) -- `Config::load` must self-heal
        // it rather than let it persist as a silent hard-failure trap on
        // every future `save_config`.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let c = Config {
            presentmon_path: r"C:\this\path\does\not\exist\PresentMon.exe".into(),
            ..Default::default()
        };
        c.save(&path).unwrap();
        let reloaded = Config::load(&path).unwrap();
        assert_ne!(reloaded.presentmon_path, c.presentmon_path);
        assert_eq!(reloaded.presentmon_path, default_presentmon_path());
    }

    #[test]
    fn load_heals_a_stale_hwinfo_path_that_no_longer_resolves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let c = Config {
            hwinfo_path: Some(r"C:\this\path\does\not\exist\HWiNFO64.exe".into()),
            ..Default::default()
        };
        c.save(&path).unwrap();
        let reloaded = Config::load(&path).unwrap();
        assert_ne!(reloaded.hwinfo_path, c.hwinfo_path);
        assert_eq!(reloaded.hwinfo_path, default_hwinfo_path());
    }

    #[test]
    fn default_shutdown_and_post_boot_settle_match_the_spec() {
        let c = Config::default();
        assert!(!c.shutdown_when_complete_default);
        assert_eq!(c.post_boot_settle_seconds, 180);
    }

    #[test]
    fn empty_json_deserializes_shutdown_and_post_boot_settle_to_their_defaults() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(!c.shutdown_when_complete_default);
        assert_eq!(c.post_boot_settle_seconds, 180);
    }

    #[test]
    fn load_leaves_a_resolvable_presentmon_path_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let exe = dir.path().join("PresentMon.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let presentmon_path = exe.to_string_lossy().into_owned();
        let c = Config {
            presentmon_path: presentmon_path.clone(),
            ..Default::default()
        };
        c.save(&path).unwrap();
        let reloaded = Config::load(&path).unwrap();
        assert_eq!(reloaded.presentmon_path, presentmon_path);
    }

    #[test]
    fn custom_script_warning_seen_defaults_to_false_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let exe = dir.path().join("PresentMon.exe");
        std::fs::write(&exe, b"MZ").unwrap();

        let c = Config::default();
        assert!(!c.custom_script_warning_seen);

        let c2 = Config {
            presentmon_path: exe.to_string_lossy().into_owned(),
            custom_script_warning_seen: true,
            ..Default::default()
        };
        c2.save(&path).unwrap();
        let reloaded = Config::load(&path).unwrap();
        assert!(reloaded.custom_script_warning_seen);
    }

    #[test]
    fn empty_json_deserializes_custom_script_warning_seen_to_false() {
        let c: Config = serde_json::from_str("{}").unwrap();
        assert!(!c.custom_script_warning_seen);
    }
}
