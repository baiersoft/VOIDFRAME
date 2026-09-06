use std::path::{Component, Path, Prefix};
use voidframe_engine::model::Config;

#[allow(dead_code)]
pub(crate) fn get_config_impl(path: &Path) -> Result<Config, String> {
    Config::load(path).map_err(|e| e.to_string())
}

/// Gate on `Config.presentmon_path` before it is ever persisted or spawned.
///
/// This is a privilege-escalation guard, not a UX nicety. `config.json`
/// lives under `%LOCALAPPDATA%`, which the *unelevated* user account can
/// write freely — but VOIDFRAME itself runs as `requireAdministrator` and
/// hands this exact string to `RealCaptureRunner::new`, which spawns it as a
/// subprocess. Without a check, any unprivileged process on the machine
/// could rewrite this field to point at an executable it controls and have
/// VOIDFRAME launch it with administrator rights on the next benchmark run.
///
/// What is enforced:
/// - **absolute, with a real drive-letter prefix** — a relative path would
///   resolve against whatever the elevated process's cwd happens to be, and
///   a UNC path (`\\host\share\x.exe`, which `Path::is_absolute` accepts on
///   Windows) would let an attacker host the payload on a remote share.
/// - **exists and is a regular file** — a directory or a dangling path can
///   only ever produce a confusing spawn failure much later, mid-run.
/// - **has an `.exe` extension** (case-insensitive) — cheap, and it stops
///   the obvious "point it at a `.bat`/`.cmd`/`.ps1`" variants.
///
/// Known residual limitation, deliberately not addressed here: this cannot
/// prove the file is *trustworthy*, only that it is a plausible local
/// executable. A user (or malware running as that user) who drops an
/// arbitrary `.exe` at an absolute local path and points this at it still
/// gets it launched elevated. Closing that fully needs either a signature
/// check or a requirement that the path sit in an admin-only-writable
/// directory; both are out of scope for this fix wave and are noted in the
/// final-review fix report.
pub(crate) fn validate_presentmon_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);

    let has_disk_prefix = matches!(
        p.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    );
    if !p.is_absolute() || !has_disk_prefix {
        return Err(format!(
            "PresentMon path must be an absolute local path with a drive letter \
             (e.g. C:\\PresentMon\\PresentMon.exe), got {path:?}"
        ));
    }

    let meta = std::fs::metadata(p)
        .map_err(|e| format!("PresentMon path {path:?} could not be read: {e}"))?;
    if !meta.is_file() {
        return Err(format!("PresentMon path {path:?} is not a file"));
    }

    let is_exe = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if !is_exe {
        return Err(format!(
            "PresentMon path {path:?} must point at an .exe (VOIDFRAME launches it as an \
             elevated subprocess)"
        ));
    }

    Ok(())
}

/// Gate on `Config.hwinfo_path` before it is ever persisted or spawned.
///
/// Structurally identical to [`validate_presentmon_path`] and exists for the
/// same reason: `start_hwinfo` spawns this exact path as a subprocess from
/// this elevated process, so the same privilege-escalation guard applies.
/// See that function's doc comment for the full rationale; only the tool
/// name differs here.
pub(crate) fn validate_hwinfo_path(path: &str) -> Result<(), String> {
    let p = Path::new(path);

    let has_disk_prefix = matches!(
        p.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_))
    );
    if !p.is_absolute() || !has_disk_prefix {
        return Err(format!(
            "HWiNFO path must be an absolute local path with a drive letter \
             (e.g. C:\\Program Files\\HWiNFO64\\HWiNFO64.exe), got {path:?}"
        ));
    }

    let meta =
        std::fs::metadata(p).map_err(|e| format!("HWiNFO path {path:?} could not be read: {e}"))?;
    if !meta.is_file() {
        return Err(format!("HWiNFO path {path:?} is not a file"));
    }

    let is_exe = p
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    if !is_exe {
        return Err(format!(
            "HWiNFO path {path:?} must point at an .exe (VOIDFRAME launches it as an elevated \
             subprocess)"
        ));
    }

    Ok(())
}

pub(crate) fn save_config_impl(path: &Path, config: &Config) -> Result<(), String> {
    validate_presentmon_path(&config.presentmon_path)?;
    if let Some(hwinfo_path) = &config.hwinfo_path {
        validate_hwinfo_path(hwinfo_path)?;
    }
    config.save(path).map_err(|e| e.to_string())
}

/// Shared by every `state.config.lock()` call site in this file and in
/// `commands/preflight.rs` -- matches `commands/run.rs`'s own `start_run`
/// wording for the identical condition (a previous operation panicked while
/// holding the same `std::sync::Mutex<Config>`), rather than letting
/// `.lock().unwrap()` panic here too.
pub(crate) const CONFIG_POISONED_MSG: &str = "configuration state is poisoned (a previous operation panicked while holding it) -- restart \
     VOIDFRAME";

/// Clones `Config` out of `mutex`, or a poisoning error. Split out from
/// [`get_config`] so the poisoning path is unit-testable against a plain
/// `std::sync::Mutex<Config>`, without a live `tauri::State`.
pub(crate) fn get_config_from_mutex(mutex: &std::sync::Mutex<Config>) -> Result<Config, String> {
    mutex
        .lock()
        .map(|cfg| cfg.clone())
        .map_err(|_| CONFIG_POISONED_MSG.to_string())
}

/// Overwrites `*mutex` with `config`, or a poisoning error. Split out from
/// [`save_config`] for the same reason as [`get_config_from_mutex`].
pub(crate) fn save_config_to_mutex(
    mutex: &std::sync::Mutex<Config>,
    config: Config,
) -> Result<(), String> {
    match mutex.lock() {
        Ok(mut guard) => {
            *guard = config;
            Ok(())
        }
        Err(_) => Err(CONFIG_POISONED_MSG.to_string()),
    }
}

#[specta::specta]
#[tauri::command]
pub async fn get_config(state: tauri::State<'_, crate::state::AppState>) -> Result<Config, String> {
    get_config_from_mutex(&state.config)
}

#[specta::specta]
#[tauri::command]
pub async fn save_config(
    state: tauri::State<'_, crate::state::AppState>,
    config: Config,
) -> Result<(), String> {
    let config_path = state.data_root.config_path();
    let config_for_disk = config.clone();
    tokio::task::spawn_blocking(move || save_config_impl(&config_path, &config_for_disk))
        .await
        .map_err(|e| super::join_error_to_string("save_config", e))??;
    save_config_to_mutex(&state.config, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use voidframe_engine::model::Config;

    /// A real, existing `.exe` file at an absolute local path — the only
    /// shape `validate_presentmon_path` accepts. Returns the tempdir too so
    /// the caller keeps it alive for the duration of the test.
    fn fake_presentmon() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("PresentMon.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let s = exe.to_string_lossy().into_owned();
        (dir, s)
    }

    #[test]
    fn get_then_save_round_trips() {
        let (_pm_dir, presentmon_path) = fake_presentmon();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut c = get_config_impl(&path).unwrap(); // default, file absent
        assert_eq!(c, Config::default());
        c.dry_run_default = true;
        // The shipped default `presentmon_path` (and, likewise,
        // `hwinfo_path`) is only a *guess* at an install location (see
        // `Config::default`), so it does not survive save-time validation
        // on a machine without the tool there. Point `presentmon_path` at a
        // real file, which is what the Settings modal makes the user do
        // anyway, and clear `hwinfo_path` since this test isn't exercising
        // HWiNFO validation.
        c.presentmon_path = presentmon_path;
        c.hwinfo_path = None;
        save_config_impl(&path, &c).unwrap();
        assert_eq!(get_config_impl(&path).unwrap(), c);
    }

    #[test]
    fn save_config_rejects_a_presentmon_path_that_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let c = Config {
            presentmon_path: r"C:\definitely\not\here\evil.exe".into(),
            ..Default::default()
        };
        let err = save_config_impl(&path, &c).unwrap_err();
        assert!(err.contains("could not be read"), "{err}");
        // Critically: nothing was persisted, so the next `start_run` cannot
        // pick up the rejected value.
        assert!(!path.exists());
    }

    #[test]
    fn save_config_rejects_a_relative_presentmon_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let c = Config {
            presentmon_path: r"..\..\evil.exe".into(),
            ..Default::default()
        };
        let err = save_config_impl(&path, &c).unwrap_err();
        assert!(err.contains("absolute local path"), "{err}");
    }

    #[test]
    fn save_config_rejects_a_unc_presentmon_path() {
        // `Path::is_absolute` says yes to a UNC path on Windows, so this
        // needs its own explicit rejection: an unelevated attacker who can
        // write config.json could otherwise host the payload on a remote
        // share and have this elevated process fetch and run it.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let c = Config {
            presentmon_path: r"\\attacker\share\PresentMon.exe".into(),
            ..Default::default()
        };
        let err = save_config_impl(&path, &c).unwrap_err();
        assert!(err.contains("absolute local path"), "{err}");
    }

    #[test]
    fn save_config_rejects_a_directory_and_a_non_exe() {
        let (pm_dir, _exe) = fake_presentmon();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");

        let as_dir = Config {
            presentmon_path: pm_dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let err = save_config_impl(&path, &as_dir).unwrap_err();
        assert!(err.contains("is not a file"), "{err}");

        let bat = pm_dir.path().join("PresentMon.bat");
        std::fs::write(&bat, b"echo").unwrap();
        let as_bat = Config {
            presentmon_path: bat.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let err = save_config_impl(&path, &as_bat).unwrap_err();
        assert!(err.contains("must point at an .exe"), "{err}");
    }

    #[test]
    fn a_real_exe_is_accepted_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("PresentMon.EXE");
        std::fs::write(&exe, b"MZ").unwrap();
        validate_presentmon_path(&exe.to_string_lossy()).unwrap();
    }

    /// A real, existing `.exe` file at an absolute local path -- the only
    /// shape `validate_hwinfo_path` accepts. Mirrors `fake_presentmon`
    /// above.
    fn fake_hwinfo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("HWiNFO64.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        let s = exe.to_string_lossy().into_owned();
        (dir, s)
    }

    #[test]
    fn validate_hwinfo_path_accepts_a_real_exe() {
        let (_dir, path) = fake_hwinfo();
        validate_hwinfo_path(&path).unwrap();
    }

    #[test]
    fn validate_hwinfo_path_accepts_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("HWiNFO64.EXE");
        std::fs::write(&exe, b"MZ").unwrap();
        validate_hwinfo_path(&exe.to_string_lossy()).unwrap();
    }

    #[test]
    fn validate_hwinfo_path_rejects_a_relative_path() {
        let err = validate_hwinfo_path(r"..\..\evil.exe").unwrap_err();
        assert!(err.contains("absolute local path"), "{err}");
    }

    #[test]
    fn validate_hwinfo_path_rejects_a_unc_path() {
        let err = validate_hwinfo_path(r"\\attacker\share\HWiNFO64.exe").unwrap_err();
        assert!(err.contains("absolute local path"), "{err}");
    }

    #[test]
    fn validate_hwinfo_path_rejects_a_missing_file() {
        let err = validate_hwinfo_path(r"C:\definitely\not\here\evil.exe").unwrap_err();
        assert!(err.contains("could not be read"), "{err}");
    }

    #[test]
    fn validate_hwinfo_path_rejects_a_directory_and_a_non_exe() {
        let (hwinfo_dir, _exe) = fake_hwinfo();

        let err = validate_hwinfo_path(&hwinfo_dir.path().to_string_lossy()).unwrap_err();
        assert!(err.contains("is not a file"), "{err}");

        let bat = hwinfo_dir.path().join("HWiNFO64.bat");
        std::fs::write(&bat, b"echo").unwrap();
        let err = validate_hwinfo_path(&bat.to_string_lossy()).unwrap_err();
        assert!(err.contains("must point at an .exe"), "{err}");
    }

    #[test]
    fn save_config_skips_hwinfo_validation_when_not_configured() {
        let (_pm_dir, presentmon_path) = fake_presentmon();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let c = Config {
            presentmon_path,
            hwinfo_path: None,
            ..Default::default()
        };
        save_config_impl(&path, &c).unwrap();
    }

    #[test]
    fn save_config_rejects_a_bad_hwinfo_path_when_configured() {
        let (_pm_dir, presentmon_path) = fake_presentmon();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let c = Config {
            presentmon_path,
            hwinfo_path: Some(r"C:\definitely\not\here\evil.exe".into()),
            ..Default::default()
        };
        let err = save_config_impl(&path, &c).unwrap_err();
        assert!(err.contains("could not be read"), "{err}");
        assert!(!path.exists());
    }

    #[test]
    fn save_config_accepts_a_good_hwinfo_path_when_configured() {
        let (_pm_dir, presentmon_path) = fake_presentmon();
        let (_hw_dir, hwinfo_path) = fake_hwinfo();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let c = Config {
            presentmon_path,
            hwinfo_path: Some(hwinfo_path),
            thermal_cooldown_enabled: true,
            ..Default::default()
        };
        save_config_impl(&path, &c).unwrap();
        assert_eq!(get_config_impl(&path).unwrap(), c);
    }

    /// A `std::sync::Mutex` is poisoned the moment a thread panics while
    /// holding its guard -- that happens during the guard's `Drop`, which
    /// runs as part of stack unwinding, so catching the panic with
    /// `catch_unwind` right here (no separate thread needed) still leaves
    /// `mutex` poisoned afterward.
    fn poisoned_config_mutex() -> std::sync::Mutex<Config> {
        let mutex = std::sync::Mutex::new(Config::default());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock().unwrap();
            panic!("intentional panic to poison the mutex for a test");
        }));
        assert!(mutex.is_poisoned());
        mutex
    }

    #[test]
    fn get_config_from_mutex_returns_an_error_instead_of_panicking_when_poisoned() {
        // Regression for the audit's Medium finding: `get_config` (and, via
        // this same helper, `preflight`) used to call `.lock().unwrap()`,
        // which panics on a poisoned mutex instead of surfacing a clear
        // error -- inconsistent with `commands/run.rs`'s own
        // `map`/`map_err` handling of the identical `Mutex<Config>`.
        let mutex = poisoned_config_mutex();
        let err = get_config_from_mutex(&mutex).unwrap_err();
        assert_eq!(err, CONFIG_POISONED_MSG);
    }

    #[test]
    fn save_config_to_mutex_returns_an_error_instead_of_panicking_when_poisoned() {
        let mutex = poisoned_config_mutex();
        let err = save_config_to_mutex(&mutex, Config::default()).unwrap_err();
        assert_eq!(err, CONFIG_POISONED_MSG);
    }
}
