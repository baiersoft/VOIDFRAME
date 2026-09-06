//! Where VOIDFRAME keeps its files, how they are written safely, and the
//! single-instance guard.

use crate::error::{Error, Result};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// The root of all mutable VOIDFRAME data.
///
/// On Windows this is `%LOCALAPPDATA%\baiersoft\VOIDFRAME\`; with `--portable`
/// it is a `voidframe-data` directory next to the executable.
#[derive(Debug, Clone)]
pub struct DataRoot(PathBuf);

impl DataRoot {
    /// Resolve and create the data-root directory tree.
    pub fn resolve(portable: bool) -> Result<DataRoot> {
        let base = if portable {
            std::env::current_exe()?
                .parent()
                .ok_or_else(|| Error::msg("current_exe has no parent".into()))?
                .join("voidframe-data")
        } else {
            dirs::data_local_dir()
                .ok_or_else(|| Error::msg("no LocalAppData directory".into()))?
                .join("baiersoft")
                .join("VOIDFRAME")
        };
        Self::with_base(base)
    }

    /// Test / explicit constructor. Creates the standard sub-directories.
    ///
    /// VOIDFRAME runs fully elevated (see design decision D4). If a
    /// lower-privileged process pre-creates `base` — or any directory this
    /// function is about to create — as a symlink/junction before the first
    /// elevated launch, a naive `create_dir_all` would silently follow it and
    /// place elevated-written files at an attacker-chosen location. Refuse
    /// instead: reject if `base` or any of its already-existing ancestors is
    /// a symlink/reparse point, and reject if any of the sub-directories we
    /// are about to create already exists as one — checked again right after
    /// `create_dir_all` to narrow (see `reject_symlink_ancestors` for why
    /// it cannot, in `docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md`,
    /// be closed entirely) the window in which a race
    /// could plant one between the pre-check and the actual creation.
    pub fn with_base(base: PathBuf) -> Result<DataRoot> {
        reject_symlink_ancestors(&base)?;
        let root = DataRoot(base);
        for d in [
            root.projects_dir(),
            root.runs_dir(),
            root.state_dir(),
            root.0.join("recovery"),
            root.0.join("schemas"),
            root.0.join("logs"),
        ] {
            reject_existing_symlink(&d)?;
            fs::create_dir_all(&d)?;
            reject_existing_symlink(&d)?;
        }
        Ok(root)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
    pub fn projects_dir(&self) -> PathBuf {
        self.0.join("projects")
    }
    pub fn runs_dir(&self) -> PathBuf {
        self.0.join("runs")
    }
    pub fn run_dir(&self, run_id: &str) -> PathBuf {
        self.runs_dir().join(run_id)
    }
    pub fn state_dir(&self) -> PathBuf {
        self.0.join("state")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.0.join("logs")
    }
    pub fn config_path(&self) -> PathBuf {
        self.0.join("config.json")
    }
}

/// Free bytes remaining on the drive that would host the (non-portable)
/// data root, i.e. `%LOCALAPPDATA%\baiersoft\VOIDFRAME`'s drive. Used by
/// pre-flight's disk-space check (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §4.4). Resolves the same base path
/// [`DataRoot::resolve`]'s non-portable branch would, but only needs to
/// know which *drive* that is — not the fully created directory tree — so
/// this works even before `DataRoot::resolve` has ever run.
pub(crate) fn free_disk_bytes_for_data_root() -> Result<u64> {
    let base = dirs::data_local_dir()
        .ok_or_else(|| Error::msg("no LocalAppData directory".into()))?
        .join("baiersoft")
        .join("VOIDFRAME");
    free_disk_bytes(&base)
}

/// Real Win32 body, gated to this one function only — `paths.rs` is
/// otherwise free of Win32 dependencies. Unlike `system::windows`, this
/// module isn't a pure OS-abstraction seam, so a single-function `#[cfg]`
/// gate here is enough; no need for a whole platform submodule.
#[cfg(windows)]
fn free_disk_bytes(path: &Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;

    // GetDiskFreeSpaceExW only needs to resolve the *volume* — using the
    // drive root (e.g. "C:\") instead of `path` itself sidesteps any
    // question of whether every intermediate directory in `path` already
    // exists (it usually doesn't, on a fresh machine's first run).
    let prefix = path
        .components()
        .find_map(|c| match c {
            std::path::Component::Prefix(p) => Some(p.as_os_str().to_owned()),
            _ => None,
        })
        .ok_or_else(|| Error::msg(format!("{} has no drive prefix", path.display())))?;
    let mut root_wide: Vec<u16> = prefix.encode_wide().collect();
    root_wide.push(b'\\' as u16); // "C:" -> "C:\"
    root_wide.push(0);

    let mut free_bytes_available: u64 = 0;
    // SAFETY: `root_wide` is NUL-terminated (pushed above) and outlives this
    // call; `free_bytes_available` is a `&mut` local.
    unsafe {
        GetDiskFreeSpaceExW(
            PCWSTR(root_wide.as_ptr()),
            Some(&mut free_bytes_available),
            None,
            None,
        )
    }
    .map_err(|e| Error::msg(format!("GetDiskFreeSpaceExW({}): {e}", path.display())))?;
    Ok(free_bytes_available)
}

#[cfg(not(windows))]
fn free_disk_bytes(_path: &Path) -> Result<u64> {
    Err(Error::msg(
        "free disk space query is only implemented on Windows".into(),
    ))
}

/// `true` if `meta` describes a symlink, an NTFS junction, or any other
/// reparse point. `std::fs::FileType::is_symlink()` alone is not guaranteed
/// to classify every Windows reparse-point kind (junctions in particular) as
/// a symlink across all targets/versions, so on Windows this additionally
/// checks the raw `FILE_ATTRIBUTE_REPARSE_POINT` bit — the superset that
/// covers every reparse-point type. Nothing legitimate should ever place a
/// reparse point inside VOIDFRAME's own data tree, so treating any of them
/// as suspect (not just literal symlinks) costs nothing.
fn is_symlink_or_reparse_point(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Refuse a path that is itself an existing symlink/junction/reparse point.
///
/// Fails **closed**: an unexpected filesystem error (anything other than
/// "does not exist yet") is propagated rather than treated as "safe to
/// proceed". Silently swallowing e.g. a permission error here would let an
/// attacker who can make a path component unstatable bypass the check
/// entirely — the previous version did exactly that via `if let Ok(meta) =
/// fs::symlink_metadata(path)`, which is a fail-open pattern.
fn reject_existing_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if is_symlink_or_reparse_point(&meta) => Err(Error::msg(format!(
            "refusing to use symlinked or reparse-point path: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Best-effort defense against a symlink/junction attack: walk `dir` and its
/// ancestors (only the ones that currently exist — most of a fresh data root
/// does not yet exist on first run) and refuse if any is a symlink/reparse
/// point. Bounded to a shallow depth since VOIDFRAME's own tree is only a
/// handful of levels deep; this is not meant to (and does not need to)
/// re-validate the OS-owned directories further up the chain
/// (`%LOCALAPPDATA%`, the user profile, …).
///
/// This is a check-then-act (TOCTOU) mitigation, not an atomic guarantee: a
/// racing process could still plant a symlink between this check and the
/// `create_dir_all` call that follows it. Fully closing that gap needs
/// directory-relative creation (`openat`-style / Win32 `NtCreateFile` with
/// `FILE_OPEN_REPARSE_POINT` at every path component), which requires
/// platform FFI that `docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md`
/// deliberately does not introduce (see that plan's Global Constraints —
/// `voidframe-engine` has zero Windows FFI until `WindowsController` lands in
/// `docs/superpowers/plans/2026-09-01-m1-phase-3a-windows-controller.md`, where this should
/// be revisited). [`atomic_write`] closes the equivalent gap for the actual
/// file *write* without needing FFI, by using a uniquely-named, `create_new`
/// temp file instead — see its doc comment.
fn reject_symlink_ancestors(dir: &Path) -> Result<()> {
    let mut cur = Some(dir);
    let mut depth = 0;
    while let Some(d) = cur {
        if depth > 16 {
            break;
        }
        reject_existing_symlink(d)?;
        cur = d.parent();
        depth += 1;
    }
    Ok(())
}

/// Write `bytes` to `path` atomically: a uniquely-named sibling temp file is
/// written and fsync'd, then renamed over the target.
///
/// The temp file is opened with `create_new` (`O_CREAT|O_EXCL` / Win32
/// `CREATE_NEW`) under a random, never-before-used name. That is atomic with
/// respect to symlinks at the OS level: if *anything* — a real file, or a
/// symlink an attacker planted in the race window — already occupies that
/// exact name, creation fails outright instead of following it. An attacker
/// cannot pre-plant a symlink under a name they cannot predict, so this
/// closes the TOCTOU gap for the write itself, which is the dominant risk.
/// The much narrower gap around `create_dir_all` on the *parent* directory
/// is covered by `reject_symlink_ancestors` (see its doc comment for why
/// that part remains best-effort in `docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md`).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::msg("path has no parent".into()))?;
    reject_symlink_ancestors(parent)?;
    fs::create_dir_all(parent)?;
    reject_symlink_ancestors(parent)?;

    let file_name = path
        .file_name()
        .ok_or_else(|| Error::msg("path has no file name".into()))?
        .to_string_lossy()
        .into_owned();
    let tmp = parent.join(format!("{file_name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    drop(f);
    fs::rename(&tmp, path)?;
    Ok(())
}

/// RAII single-instance guard. Dropping it releases the lock.
#[derive(Debug)]
pub struct InstanceLock {
    path: PathBuf,
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Take the process-level single-instance lock (`state/instance.lock`).
///
/// The lock file itself is opened with `create_new`, which is already
/// atomic w.r.t. a pre-planted symlink at that exact path (see
/// [`atomic_write`]'s doc comment for the same reasoning); the pre-checks
/// below exist to fail with a clearer "symlink" error instead of a generic
/// `AlreadyExists`/OS error, and to catch a symlinked ancestor directory.
///
/// The Tauri layer layers a `Global\VOIDFRAME_SINGLETON` named mutex on top in
/// a later phase; this file lock is the cross-platform engine-level guard.
///
/// If a lock file already exists, its recorded PID is checked before
/// refusing outright: `InstanceLock::drop` only removes the file on a clean
/// Rust unwind, which in practice covers neither a crash/hard-kill NOR (as
/// confirmed live) this app's own normal exit — Tauri's event loop tears the
/// process down directly rather than returning cleanly from `main()`, so a
/// stale lock file is the expected common case, not a rare edge case, and
/// treating every leftover file as a genuine second instance would
/// permanently strand every future launch until someone finds and deletes
/// it by hand.
pub fn acquire_instance_lock(root: &DataRoot) -> Result<InstanceLock> {
    let path = root.state_dir().join("instance.lock");
    if let Some(parent) = path.parent() {
        reject_symlink_ancestors(parent)?;
    }
    reject_existing_symlink(&path)?;
    match create_lock_file(&path) {
        Ok(()) => Ok(InstanceLock { path }),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if !lock_file_names_a_live_process(&path) {
                let _ = fs::remove_file(&path);
                create_lock_file(&path)?;
                return Ok(InstanceLock { path });
            }
            Err(Error::already_running())
        }
        Err(e) => Err(e.into()),
    }
}

fn create_lock_file(path: &Path) -> std::io::Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    let _ = write!(f, "{}", std::process::id());
    Ok(())
}

/// `false` if the PID recorded in `path` is not currently a running
/// process (including if the file is unreadable or its content isn't a
/// plain PID — a corrupted lock file must not permanently strand every
/// future launch either).
fn lock_file_names_a_live_process(path: &Path) -> bool {
    let Some(pid) = fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
    else {
        return false;
    };
    process_is_alive(pid)
}

/// Not image-name-checked against this exe specifically. A PID being
/// reused by an unrelated process in the narrow window between this app's
/// last exit and the next launch is possible on a real, busy desktop —
/// empirically confirmed while writing this fix's own test, where even a
/// plain `cmd.exe /C exit`, fully waited on, sometimes had its pid claimed
/// by a different live process within ~200ms. The consequence of getting
/// that race wrong here is narrow either way (one extra failed launch that
/// needs a retry, or — no worse than today's status quo — a manual lock
/// deletion), and this is already documented above as the pragmatic
/// interim check ahead of the real named-mutex guard, not the final word.
#[cfg(windows)]
fn process_is_alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    // SAFETY: `OpenProcess` is called with a plain numeric pid (no
    // pointers); its `Result` is checked before the handle is used for
    // anything, and the handle is closed exactly once, immediately, on the
    // only path that obtains one.
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(not(windows))]
fn process_is_alive(_pid: u32) -> bool {
    // Conservative: without a real liveness check, never treat an existing
    // lock as stale.
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn free_disk_bytes_for_data_root_returns_a_plausible_value() {
        let bytes = free_disk_bytes_for_data_root().unwrap();
        assert!(bytes > 0);
    }

    /// Create a directory symlink for a test. Returns `false` (and the caller
    /// should skip the test) if this environment does not permit creating one
    /// — e.g. no `SeCreateSymbolicLinkPrivilege` and no Developer Mode.
    fn try_symlink_dir(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(target, link).is_ok()
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = (target, link);
            false
        }
    }

    #[test]
    fn atomic_write_replaces_contents() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("f.json");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn instance_lock_is_exclusive_then_released() {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let l1 = acquire_instance_lock(&root).unwrap();
        assert!(acquire_instance_lock(&root).is_err_and(|e| e.is_already_running()));
        drop(l1);
        let _l2 = acquire_instance_lock(&root).unwrap();
    }

    /// Regression test: `InstanceLock::drop` only fires on a clean Rust
    /// unwind, which a hard-killed process (a crash) never reaches -- and,
    /// confirmed live, neither does this app's own normal exit (Tauri's
    /// event loop tears the process down directly rather than returning
    /// from `main()`). A stale lock file naming an already-exited process
    /// must not permanently strand every future launch.
    #[test]
    #[cfg(windows)]
    fn acquire_instance_lock_recovers_from_a_stale_lock_naming_an_exited_process() {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::with_base(dir.path().to_path_buf()).unwrap();

        // Deliberately not a real spawn-then-kill: on a genuinely busy
        // desktop, a just-exited pid's number can be claimed by a
        // different, unrelated live process within a couple hundred
        // milliseconds -- confirmed empirically while writing this test
        // (both `notepad.exe`, which is itself a launcher with an
        // ambiguous captured-pid lifetime, and a plain `cmd.exe /C exit`,
        // fully waited on, hit this in practice). A pid this large is
        // never a real Windows process id, so this proves the exact same
        // "not alive -> treat as stale" code path with zero race window.
        let never_alive_pid: u32 = 0xFFFF_FFF0;
        assert!(!process_is_alive(never_alive_pid));

        std::fs::write(
            root.state_dir().join("instance.lock"),
            never_alive_pid.to_string(),
        )
        .unwrap();

        // Must succeed despite the file already existing -- proving the
        // stale lock was detected and cleared, not just coincidentally
        // absent.
        let _lock = acquire_instance_lock(&root).unwrap();
    }

    /// The inverse: a lock file naming a genuinely still-running process
    /// (this test process itself) must still be refused.
    #[test]
    #[cfg(windows)]
    fn acquire_instance_lock_refuses_a_lock_naming_a_live_process() {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        std::fs::write(
            root.state_dir().join("instance.lock"),
            std::process::id().to_string(),
        )
        .unwrap();
        assert!(acquire_instance_lock(&root).is_err_and(|e| e.is_already_running()));
    }

    #[test]
    fn acquire_instance_lock_recovers_from_an_unreadable_or_corrupt_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        std::fs::write(root.state_dir().join("instance.lock"), "not-a-pid").unwrap();
        let _lock = acquire_instance_lock(&root).unwrap();
    }

    #[test]
    fn with_base_creates_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        assert!(root.projects_dir().is_dir());
        assert!(root.runs_dir().is_dir());
        assert!(root.state_dir().is_dir());
        assert!(root.logs_dir().is_dir());
    }

    #[test]
    fn atomic_write_refuses_to_follow_a_symlinked_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let real_target = dir.path().join("real_target");
        std::fs::create_dir_all(&real_target).unwrap();
        let link = dir.path().join("linked");

        if !try_symlink_dir(&real_target, &link) {
            eprintln!("skipping: could not create a symlink in this environment");
            return;
        }

        let p = link.join("sub").join("f.json");
        let err = atomic_write(&p, b"x").unwrap_err();
        assert!(err.to_string().contains("symlink"));
        assert!(!real_target.join("sub").join("f.json").exists());
    }

    #[test]
    fn with_base_refuses_when_an_ancestor_is_a_symlink() {
        let outer = tempfile::tempdir().unwrap();
        let real_elsewhere = outer.path().join("elsewhere");
        std::fs::create_dir_all(&real_elsewhere).unwrap();
        // Simulate an attacker having pre-planted `<LocalAppData>\baiersoft`
        // as a symlink before VOIDFRAME's first elevated launch.
        let planted = outer.path().join("baiersoft");

        if !try_symlink_dir(&real_elsewhere, &planted) {
            eprintln!("skipping: could not create a symlink in this environment");
            return;
        }

        let base = planted.join("VOIDFRAME");
        let err = DataRoot::with_base(base).unwrap_err();
        assert!(err.to_string().contains("symlink"));
        // Nothing was created through the link.
        assert!(!real_elsewhere.join("VOIDFRAME").exists());
    }

    #[test]
    fn instance_lock_refuses_a_pre_existing_symlinked_lock_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::with_base(dir.path().to_path_buf()).unwrap();
        let elsewhere = dir.path().join("elsewhere.lock");
        std::fs::write(&elsewhere, b"").unwrap();
        let lock_path = root.state_dir().join("instance.lock");

        #[cfg(unix)]
        let created = std::os::unix::fs::symlink(&elsewhere, &lock_path).is_ok();
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_file(&elsewhere, &lock_path).is_ok();
        #[cfg(not(any(unix, windows)))]
        let created = false;

        if !created {
            eprintln!("skipping: could not create a symlink in this environment");
            return;
        }

        let err = acquire_instance_lock(&root).unwrap_err();
        assert!(err.to_string().contains("symlink"));
    }

    /// Regression test for the fail-open bug: an unexpected `symlink_metadata`
    /// error (here: search permission removed on a parent directory, so even
    /// `stat` on the leaf fails) must be propagated, not silently treated as
    /// "path doesn't exist, proceed".
    #[cfg(unix)]
    #[test]
    fn reject_existing_symlink_fails_closed_on_unexpected_stat_error() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let blocked = dir.path().join("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        let leaf = blocked.join("leaf.json");
        std::fs::write(&leaf, b"x").unwrap();

        // Remove search (execute) permission on `blocked` so stat-ing
        // anything inside it fails with PermissionDenied, not NotFound.
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = reject_existing_symlink(&leaf);
        // Restore permissions unconditionally so tempdir cleanup can proceed.
        std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(result.is_err());
    }
}
