//! Spec §5.3: a `.bat` a human can double-click when VOIDFRAME itself
//! cannot start. Rendered from the journal's inverses, in reverse journal
//! order (last applied, first reverted). Registry and powercfg inverses
//! map 1:1 onto `reg` / `powercfg` commands; a deleted power plan cannot
//! be recreated and is emitted as a REM.
//!
//! **Known staleness window:** [`write_all`] is only called from the run
//! loop's SNAPSHOT phase (before anything has applied -- the journal is
//! still empty then) and from `reboot_sequence` (only reached when a
//! scenario actually needs a reboot). A run whose only mutations are
//! non-rebooting modules -- `cs2_config`, most `custom_script` scenarios,
//! but also pre-existing cases like registry-without-reboot or powercfg --
//! never triggers a later regeneration, so `VOIDFRAME_RESTORE.bat` on disk
//! may be stale relative to what has actually been applied until either a
//! later reboot-requiring scenario in the same run, or the run's own end,
//! regenerates it. Pre-existing behavior for every non-rebooting module,
//! not a regression introduced by `custom_script`/`cs2_config` -- just
//! newly relevant now that these two module kinds exist and are more
//! likely to appear in a run with no reboot-requiring module at all.

use crate::error::Result;
use crate::journal::{Journal, JournalRecord, Op};
use std::path::{Path, PathBuf};

pub const RESTORE_SCRIPT_NAME: &str = "VOIDFRAME_RESTORE.bat";
/// The `.bat`'s two possible siblings, written next to every copy of it.
/// Both carry the `VOIDFRAME_RESTORE` prefix on purpose: one target is the
/// user's real Desktop, and `remove_all` deletes these names unconditionally
/// -- a bare `scripts\` there would collide with (and `write_all` would
/// merge into, then `remove_all` delete) a directory the user made
/// themselves.
pub const SCRIPTS_DIR_NAME: &str = "VOIDFRAME_RESTORE_scripts";
pub const CS2_SNAPSHOT_NAME: &str = "VOIDFRAME_RESTORE.cs2_video.snapshot.txt";

/// A literal for a double-quoted `.bat` argument: `%` doubled so `cmd`
/// doesn't expand `%SystemRoot%`-style text at run time (a `REG_EXPAND_SZ`
/// inverse is exactly that), `"` backslash-escaped so it can't end the
/// argument early.
fn bat_quote(s: &str) -> String {
    s.replace('%', "%%").replace('"', "\\\"")
}

fn reg_type_flag(t: &str) -> &'static str {
    match t {
        "DWORD" => "REG_DWORD",
        "QWORD" => "REG_QWORD",
        "SZ" => "REG_SZ",
        "EXPAND_SZ" => "REG_EXPAND_SZ",
        "MULTI_SZ" => "REG_MULTI_SZ",
        "BINARY" => "REG_BINARY",
        _ => "REG_SZ",
    }
}

fn reg_value_arg(v: &serde_json::Value) -> String {
    match v.get("type").and_then(|t| t.as_str()) {
        Some("MULTI_SZ") => v["value"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join("\\0")
            })
            .unwrap_or_default(),
        Some("DWORD") | Some("QWORD") => v["value"].to_string(),
        _ => v["value"].as_str().unwrap_or_default().to_string(),
    }
}

fn render_record(rec: &JournalRecord, out: &mut String, real_video_txt_path: Option<&Path>) {
    let inv = &rec.inverse;
    match rec.op {
        Op::RegistryWrite | Op::RegistryDelete => {
            let t = &inv["target"];
            let key = format!(
                "{}\\{}",
                t["hive"].as_str().unwrap_or("HKLM"),
                t["subkey"].as_str().unwrap_or_default()
            );
            let name = bat_quote(t["value_name"].as_str().unwrap_or_default());
            if inv["kind"].as_str() == Some("delete") {
                out.push_str(&format!("reg delete \"{key}\" /v \"{name}\" /f\r\n"));
            } else {
                let v = &inv["value"];
                let ty = v["type"].as_str().unwrap_or("SZ");
                out.push_str(&format!(
                    "reg add \"{key}\" /v \"{name}\" /t {} /d \"{}\" /f\r\n",
                    reg_type_flag(ty),
                    bat_quote(&reg_value_arg(v))
                ));
            }
        }
        Op::PowercfgWrite => {
            let sub = inv["sub"].as_str().unwrap_or_default();
            let setting = inv["setting"].as_str().unwrap_or_default();
            let value = inv["value"].as_u64().unwrap_or(0);
            out.push_str(&format!(
                "powercfg /setacvalueindex SCHEME_CURRENT {sub} {setting} {value}\r\n"
            ));
            out.push_str(&format!(
                "powercfg /setdcvalueindex SCHEME_CURRENT {sub} {setting} {value}\r\n"
            ));
        }
        Op::PowerPlanActivate => {
            out.push_str(&format!(
                "powercfg /setactive {}\r\n",
                inv["guid"].as_str().unwrap_or_default()
            ));
        }
        Op::PowerPlanCreate => {
            out.push_str(&format!(
                "powercfg /delete {}\r\n",
                inv["guid"].as_str().unwrap_or_default()
            ));
        }
        Op::PowerPlanDelete => {
            out.push_str(&format!(
                "REM seq {}: a power plan was deleted by this run and cannot be recreated by script\r\n",
                rec.seq
            ));
        }
        Op::CustomScriptApply => {
            // Same H1 threat model as `replay::validate_inverse` (which this
            // calls directly, rather than duplicating its checks): a
            // tampered journal `revert_path` must not reach the generated
            // `.bat` either, since a human is meant to trust and
            // double-click it during an emergency.
            match crate::journal::replay::validate_inverse(rec) {
                Ok(()) => {
                    let revert_path = rec.inverse["revert_path"].as_str().unwrap_or_default();
                    out.push_str(&format!(
                        "call \"%~dp0{SCRIPTS_DIR_NAME}\\{revert_path}\"\r\n"
                    ));
                }
                Err(msg) => {
                    out.push_str(&format!("REM {msg}\r\n"));
                }
            }
        }
        Op::Cs2ConfigApply => match real_video_txt_path {
            Some(p) => {
                // `real_video_txt_path` is resolved once by the engine
                // (`SystemController::steam_install_path` +
                // `vdf::find_cs2_video_config`, the same lookup apply time
                // already uses), not guessed at by the `.bat` itself. The
                // full pre-change text can't fit on one `.bat` line, so it
                // lives in a sibling snapshot file `write_all` writes next
                // to this rendered script.
                out.push_str(&format!(
                    "copy /Y \"%~dp0{CS2_SNAPSHOT_NAME}\" \"{}\" >nul\r\n",
                    p.display()
                ));
            }
            None => {
                out.push_str(&format!(
                    "REM seq {}: cs2_config revert could not resolve the real video.txt path -- re-run VOIDFRAME to restore video.txt\r\n",
                    rec.seq
                ));
            }
        },
    }
}

pub fn render(
    records: &[JournalRecord],
    run_id: &str,
    real_video_txt_path: Option<&Path>,
) -> String {
    let mut out = String::new();
    out.push_str("@echo off\r\n");
    out.push_str(&format!(
        "REM VOIDFRAME emergency restore for run {run_id}\r\n"
    ));
    out.push_str(
        "REM Reverts every journaled mutation, last applied first. Run as Administrator.\r\n",
    );
    out.push_str(
        "net session >nul 2>&1 || (echo This script must be run as Administrator. & pause & exit /b 1)\r\n",
    );
    let mut recs: Vec<&JournalRecord> = records.iter().collect();
    recs.sort_by_key(|r| std::cmp::Reverse(r.seq));
    for rec in recs {
        render_record(rec, &mut out, real_video_txt_path);
    }
    out.push_str("echo VOIDFRAME restore finished.\r\n");
    out.push_str("pause\r\n");
    out
}

/// The real Windows Desktop, unless `VOIDFRAME_DESKTOP_ROOT` is set --
/// mirrors `state.rs`'s own `VOIDFRAME_DATA_ROOT` override, for the e2e
/// suite's spawned real `voidframe.exe` (a separate process this crate's own
/// `cfg(test)` can't reach). Without this override, `targets()` below writes
/// (and can leave behind) a real `VOIDFRAME_RESTORE.bat` on the real Desktop
/// regardless of `data_root` being sandboxed for a test -- confirmed live:
/// the e2e suite's mock-backend HAGS run journals `HwSchMode` as absent
/// (`MockController`'s registry starts empty) and one spec deliberately
/// never reaches `remove_all` (`08-shutdown-and-reboot-mock.e2e.ts`'s
/// `reboot_pending` test), so every e2e run left a real, wrong "delete this
/// key" script on the developer's real Desktop with no cleanup.
///
/// `cargo test` compiles this crate itself with `cfg(test)` and links every
/// one of its own tests in-process (no separate `voidframe.exe`, no sandboxed
/// data root at all for the Desktop) -- `begin_fresh`/`reboot_sequence` calls
/// `write_all` directly, so any of `run::execute::tests::*`'s many
/// reboot-pending scenarios that don't reach `remove_all` would leave the
/// exact same real, wrong script behind on every `cargo test` run, not just
/// every e2e run. Defaulting to `None` here (rather than the real Desktop)
/// when the override isn't set makes that impossible by construction, not
/// just by every test author remembering to set the override. A test that
/// wants to exercise the Desktop-write path passes a scratch directory to
/// `write_all_with_desktop` / `remove_all_with_desktop` instead of setting
/// `VOIDFRAME_DESKTOP_ROOT`: the env var is process-wide, `cargo test` runs
/// tests concurrently, and every other `write_all` / `remove_all` caller in
/// the same process reads it through this function -- so a scratch Desktop
/// set here would leak into sibling tests and vanish under them when the
/// setting test's `TempDir` drops (observed as a NotFound in
/// `write_all_renders_only_the_live_journals`, 2026-09-10).
fn desktop_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("VOIDFRAME_DESKTOP_ROOT") {
        return Some(PathBuf::from(dir));
    }
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        dirs::desktop_dir()
    }
}

fn targets(data_root: &Path, desktop: Option<&Path>) -> Vec<PathBuf> {
    let mut t = vec![data_root.join("recovery").join(RESTORE_SCRIPT_NAME)];
    if let Some(desktop) = desktop {
        t.push(desktop.join(RESTORE_SCRIPT_NAME));
    }
    t
}

/// Flat, single-level copy of `src`'s files into `dst` (`src` never has
/// subdirectories -- see spec §3.1's own constraint on `scripts/`). A
/// missing `src` is not an error: most projects have no `custom_script`
/// module and so never created a `scripts/` directory at all.
fn copy_dir_flat(src: &Path, dst: &Path) -> Result<()> {
    if !src.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            std::fs::copy(&path, dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Warns (does not fail `write_all`) for every `CustomScriptApply` record
/// whose `revert_path` is not actually present under `scripts_dir` --
/// distinct from `copy_dir_flat`'s "no `scripts/` directory at all" no-op:
/// this is the specific file a rendered `call "%~dp0scripts\..."` line will
/// try to run, and if it's missing, that only surfaces when a human
/// double-clicks `VOIDFRAME_RESTORE.bat` during a real emergency.
fn warn_about_missing_revert_scripts(records: &[JournalRecord], scripts_dir: &Path) {
    for rec in records {
        if rec.op != Op::CustomScriptApply {
            continue;
        }
        let revert_path = rec.inverse["revert_path"].as_str().unwrap_or_default();
        if !scripts_dir.join(revert_path).exists() {
            tracing::warn!(
                seq = rec.seq,
                revert_path,
                "VOIDFRAME_RESTORE.bat will reference a revert script that is missing from scripts_dir"
            );
        }
    }
}

/// The first `Cs2ConfigApply` record's captured pre-change text, if this
/// run's journal has one -- "first" meaning the iteration order `records`
/// arrives in here (built from `live_journals`, which sorts alphabetically
/// by scenario-id filename, not by application order), not "earliest
/// applied". `cs2_config`'s own apply/revert cycle (`mutation/cs2_config.rs`)
/// always restores the true baseline before any later apply in the same
/// run, so every `Cs2ConfigApply` record in a run's journal carries the same
/// `original_text` regardless -- any one of them is the correct (and only)
/// content `cs2_video.snapshot.txt` ever needs to hold, so which one this
/// picks doesn't matter.
fn cs2_original_text(records: &[JournalRecord]) -> Option<&str> {
    records
        .iter()
        .find(|r| r.op == Op::Cs2ConfigApply)
        .and_then(|r| r.inverse["original_text"].as_str())
}

/// Renders and writes the `.bat` for the mutations still live on the machine
/// -- `live::live_journals` against `run_dir`'s own `progress.json`, which
/// every caller (`begin_fresh`, `reboot_sequence`) saves immediately before
/// calling this. A journal whose scenario already completed its own revert
/// is left out: rendering it would make the `.bat` replay already-reverted
/// `custom_script` reverts and `powercfg /delete` already-deleted plans when
/// a human runs it. With no readable `progress.json`, every journal is
/// rendered (see `live_journals`).
pub fn write_all(
    run_dir: &Path,
    data_root: &Path,
    run_id: &str,
    scripts_dir: &Path,
    real_video_txt_path: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    write_all_with_desktop(
        run_dir,
        data_root,
        run_id,
        scripts_dir,
        real_video_txt_path,
        desktop_dir().as_deref(),
    )
}

/// [`write_all`] with the Desktop resolved by the caller (see `desktop_dir`).
fn write_all_with_desktop(
    run_dir: &Path,
    data_root: &Path,
    run_id: &str,
    scripts_dir: &Path,
    real_video_txt_path: Option<&Path>,
    desktop: Option<&Path>,
) -> Result<Vec<PathBuf>> {
    let progress = crate::model::progress::RunProgress::load(run_dir).ok();
    let mut records = Vec::new();
    for p in crate::journal::live::live_journals(run_dir, progress.as_ref())? {
        records.extend(Journal::load_pending(&p)?);
    }
    warn_about_missing_revert_scripts(&records, scripts_dir);
    let text = render(&records, run_id, real_video_txt_path);
    // Only copy `scripts/` when the journal actually has a `custom_script`
    // apply to revert -- otherwise a project whose `scripts/` directory
    // happens to exist (leftover from an unrelated module type, or just
    // present on disk) would get it copied to the Desktop regardless.
    let has_custom_script_apply = records.iter().any(|r| r.op == Op::CustomScriptApply);
    // Only written when there is both a `cs2_config` apply to restore and a
    // resolved real path for the rendered `copy` line to target -- otherwise
    // `render` emitted a REM instead, and a snapshot file would have nothing
    // to be copied to.
    let snapshot_text = real_video_txt_path.and_then(|_| cs2_original_text(&records));
    let mut written = Vec::new();
    for target in targets(data_root, desktop) {
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::paths::atomic_write(&target, text.as_bytes())?;
        if let Some(parent) = target.parent() {
            if has_custom_script_apply {
                copy_dir_flat(scripts_dir, &parent.join(SCRIPTS_DIR_NAME))?;
            }
            if let Some(snapshot) = snapshot_text {
                crate::paths::atomic_write(&parent.join(CS2_SNAPSHOT_NAME), snapshot.as_bytes())?;
            }
        }
        written.push(target);
    }
    Ok(written)
}

/// Removes `VOIDFRAME_RESTORE.bat` and its two possible siblings
/// ([`SCRIPTS_DIR_NAME`], written by `write_all` when the journal has a
/// `CustomScriptApply` record; [`CS2_SNAPSHOT_NAME`], written when it has a
/// `Cs2ConfigApply` record) from every target `write_all` may have written
/// to. Without this, a clean run leaves permanent clutter behind -- most
/// visibly on the real Desktop -- with no `.bat` left to explain it. Only
/// VOIDFRAME's own names are ever touched (see the constants' doc comment).
pub fn remove_all(data_root: &Path) {
    remove_all_with_desktop(data_root, desktop_dir().as_deref());
}

/// [`remove_all`] with the Desktop resolved by the caller (see `desktop_dir`).
fn remove_all_with_desktop(data_root: &Path, desktop: Option<&Path>) {
    for target in targets(data_root, desktop) {
        let _ = std::fs::remove_file(&target);
        if let Some(parent) = target.parent() {
            let _ = std::fs::remove_dir_all(parent.join(SCRIPTS_DIR_NAME));
            let _ = std::fs::remove_file(parent.join(CS2_SNAPSHOT_NAME));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(seq: u64, op: Op, inverse: serde_json::Value) -> JournalRecord {
        JournalRecord {
            seq,
            ts_unix_ms: 0,
            run_id: "r1".into(),
            scenario_id: "s1".into(),
            step_index: 0,
            op,
            target: json!({}),
            new: json!({}),
            inverse,
            applied: true,
        }
    }

    #[test]
    fn renders_registry_powercfg_and_plan_inverses_in_reverse_order() {
        let records = vec![
            rec(
                1,
                Op::RegistryWrite,
                json!({"kind":"write","target":{"hive":"HKLM","subkey":"SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers","value_name":"HwSchMode"},"value":{"type":"DWORD","value":1}}),
            ),
            rec(
                2,
                Op::PowercfgWrite,
                json!({"sub":"sub_processor","setting":"IDLEDISABLE","value":0}),
            ),
            rec(
                3,
                Op::PowerPlanActivate,
                json!({"guid":"381b4222-f694-41f0-9685-ff5bb260df2e"}),
            ),
            rec(
                4,
                Op::RegistryDelete,
                json!({"kind":"delete","target":{"hive":"HKCU","subkey":"Software\\Test","value_name":"X"}}),
            ),
        ];
        let bat = render(&records, "r1", None);
        let lines: Vec<&str> = bat.lines().collect();
        let pos = |needle: &str| lines.iter().position(|l| l.contains(needle)).unwrap();
        assert!(
            pos("reg delete \"HKCU\\Software\\Test\" /v \"X\" /f")
                < pos("powercfg /setactive 381b4222")
        );
        assert!(pos("powercfg /setactive 381b4222") < pos("IDLEDISABLE 0"));
        assert!(
            pos("IDLEDISABLE 0")
                < pos(
                    "reg add \"HKLM\\SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers\" /v \"HwSchMode\" /t REG_DWORD /d \"1\" /f"
                )
        );
        assert!(bat.starts_with("@echo off\r\n"));
        assert!(bat.contains("net session"));
    }

    #[test]
    fn a_deleted_plan_becomes_a_rem_line() {
        let bat = render(
            &[rec(1, Op::PowerPlanDelete, json!({"guid":"x"}))],
            "r1",
            None,
        );
        assert!(bat.contains("REM seq 1"));
    }

    /// The no-Desktop default under `cfg(test)` goes through the public
    /// `write_all` / `remove_all`; the Desktop-write path gets a scratch
    /// directory passed explicitly. Never set `VOIDFRAME_DESKTOP_ROOT` from
    /// a test: it is process-wide and every concurrent `write_all` caller
    /// reads it (see `desktop_dir`).
    #[test]
    fn write_all_writes_the_recovery_copy_always_and_the_desktop_copy_only_when_given() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let mut j = Journal::open(&run_dir.join("journal-s1.jsonl")).unwrap();
        let seq = j
            .record(
                Op::PowercfgWrite,
                &crate::system::MutationCtx {
                    run_id: "r1".into(),
                    scenario_id: "s1".into(),
                    step_index: 0,
                },
                json!({}),
                json!(1),
                json!({"sub":"sub_processor","setting":"IDLEDISABLE","value":0}),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
        drop(j);

        // No override set: `desktop_dir()` must default to `None` under
        // `cfg(test)` (see its own doc comment for why) -- only the
        // `recovery\` copy gets written, never a real Desktop.
        let scripts_dir = dir.path().join("no-scripts-here");
        let written = write_all(&run_dir, dir.path(), "r1", &scripts_dir, None).unwrap();
        let recovery = dir.path().join("recovery").join(RESTORE_SCRIPT_NAME);
        assert_eq!(written, vec![recovery.clone()]);
        let text = std::fs::read_to_string(&recovery).unwrap();
        assert!(text.contains("IDLEDISABLE 0"));
        remove_all(dir.path());
        assert!(!recovery.exists());

        let desktop = tempfile::tempdir().unwrap();
        let written = write_all_with_desktop(
            &run_dir,
            dir.path(),
            "r1",
            &scripts_dir,
            None,
            Some(desktop.path()),
        )
        .unwrap();
        let desktop_copy = desktop.path().join(RESTORE_SCRIPT_NAME);
        assert!(written.contains(&desktop_copy));
        assert!(desktop_copy.exists());
        remove_all_with_desktop(dir.path(), Some(desktop.path()));
        assert!(!desktop_copy.exists());
    }

    #[test]
    fn write_all_on_a_missing_run_dir_errors_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("does-not-exist");
        let scripts_dir = dir.path().join("no-scripts-here");
        let err = write_all(&run_dir, dir.path(), "r1", &scripts_dir, None).unwrap_err();
        assert!(err.is_io());
        let recovery = dir.path().join("recovery").join(RESTORE_SCRIPT_NAME);
        assert!(!recovery.exists());
    }

    #[test]
    fn renders_a_call_to_the_recorded_revert_script() {
        let records = vec![rec(
            1,
            Op::CustomScriptApply,
            json!({
                "apply_sha256": "aaaa",
                "revert_sha256": "bbbb",
                "revert_path": "revert.ps1",
            }),
        )];
        let bat = render(&records, "r1", None);
        assert!(bat.contains("revert.ps1"), "{bat}");
    }

    /// Finding 3 (H1): a tampered `revert_path` (here, a `..` traversal
    /// segment) must never reach a rendered `call` line -- `render_record`
    /// falls back to a `REM` line instead, mirroring `validate_inverse`'s
    /// own apply-side rejection.
    #[test]
    fn renders_a_rem_line_instead_of_a_call_for_a_tampered_revert_path() {
        let records = vec![rec(
            1,
            Op::CustomScriptApply,
            json!({
                "apply_sha256": "aaaa",
                "revert_sha256": "bbbb",
                "revert_path": "..\\..\\evil.bat",
            }),
        )];
        let bat = render(&records, "r1", None);
        assert!(bat.contains("REM seq 1"), "{bat}");
        assert!(!bat.contains("call \""), "{bat}");
    }

    #[test]
    fn renders_a_copy_back_from_the_cs2_video_snapshot() {
        let records = vec![rec(
            1,
            Op::Cs2ConfigApply,
            json!({ "original_text": "setting.fullscreen 1\n" }),
        )];
        let real_path = PathBuf::from("C:\\real\\video.txt");
        let bat = render(&records, "r1", Some(&real_path));
        assert!(bat.to_lowercase().contains("copy"), "{bat}");
        assert!(bat.contains(CS2_SNAPSHOT_NAME), "{bat}");
        assert!(bat.contains("C:\\real\\video.txt"), "{bat}");
    }

    #[test]
    fn cs2_config_revert_with_no_resolved_path_falls_back_to_a_rem_line() {
        let records = vec![rec(
            1,
            Op::Cs2ConfigApply,
            json!({ "original_text": "setting.fullscreen 1\n" }),
        )];
        let bat = render(&records, "r1", None);
        assert!(bat.contains("REM seq 1"), "{bat}");
        assert!(!bat.to_lowercase().contains("copy"), "{bat}");
    }

    #[test]
    fn write_all_copies_the_scripts_directory_alongside_each_target() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let project_dir = dir.path().join("projects").join("p1");
        std::fs::create_dir_all(project_dir.join("scripts")).unwrap();
        std::fs::write(
            project_dir.join("scripts").join("revert.bat"),
            "@echo off\r\n",
        )
        .unwrap();

        let mut j = Journal::open(&run_dir.join("journal-s1.jsonl")).unwrap();
        let seq = j
            .record(
                Op::CustomScriptApply,
                &crate::system::MutationCtx {
                    run_id: "r1".into(),
                    scenario_id: "s1".into(),
                    step_index: 0,
                },
                json!({}),
                json!({}),
                json!({ "apply_sha256": "a", "revert_sha256": "b", "revert_path": "revert.bat" }),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
        drop(j);

        write_all(
            &run_dir,
            dir.path(),
            "r1",
            &project_dir.join("scripts"),
            None,
        )
        .unwrap();
        let recovery_scripts = dir
            .path()
            .join("recovery")
            .join(SCRIPTS_DIR_NAME)
            .join("revert.bat");
        assert!(recovery_scripts.exists());
    }

    /// `scripts_dir` itself exists (so `copy_dir_flat`'s missing-directory
    /// no-op doesn't apply), but the specific file the journal record
    /// references was never written into it -- e.g. deleted or renamed
    /// since apply time. `write_all` must still succeed (this is a
    /// diagnostic-only warning, not a fatal error) and still produce the
    /// `.bat`, even though the `call` line it renders now points at a file
    /// that doesn't exist.
    #[test]
    fn write_all_succeeds_when_a_referenced_revert_script_is_missing_from_scripts_dir() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let project_dir = dir.path().join("projects").join("p1");
        // scripts_dir exists, but "revert.bat" is never written into it.
        std::fs::create_dir_all(project_dir.join("scripts")).unwrap();

        let mut j = Journal::open(&run_dir.join("journal-s1.jsonl")).unwrap();
        let seq = j
            .record(
                Op::CustomScriptApply,
                &crate::system::MutationCtx {
                    run_id: "r1".into(),
                    scenario_id: "s1".into(),
                    step_index: 0,
                },
                json!({}),
                json!({}),
                json!({ "apply_sha256": "a", "revert_sha256": "b", "revert_path": "revert.bat" }),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
        drop(j);

        let written = write_all(
            &run_dir,
            dir.path(),
            "r1",
            &project_dir.join("scripts"),
            None,
        );
        assert!(written.is_ok());
        let recovery = dir.path().join("recovery").join(RESTORE_SCRIPT_NAME);
        assert!(recovery.exists());
        let text = std::fs::read_to_string(&recovery).unwrap();
        assert!(text.contains("revert.bat"));
    }

    /// Finding 4: a project's `scripts/` directory existing on disk (e.g.
    /// leftover from an unrelated module type) must NOT get copied to the
    /// Desktop when the journal has no `CustomScriptApply` record at all.
    #[test]
    fn write_all_does_not_copy_scripts_dir_when_the_journal_has_no_custom_script_apply() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let project_dir = dir.path().join("projects").join("p1");
        std::fs::create_dir_all(project_dir.join("scripts")).unwrap();
        std::fs::write(
            project_dir.join("scripts").join("leftover.bat"),
            "@echo off\r\n",
        )
        .unwrap();

        let mut j = Journal::open(&run_dir.join("journal-s1.jsonl")).unwrap();
        let seq = j
            .record(
                Op::PowercfgWrite,
                &crate::system::MutationCtx {
                    run_id: "r1".into(),
                    scenario_id: "s1".into(),
                    step_index: 0,
                },
                json!({}),
                json!(1),
                json!({"sub":"sub_processor","setting":"IDLEDISABLE","value":0}),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
        drop(j);

        write_all(
            &run_dir,
            dir.path(),
            "r1",
            &project_dir.join("scripts"),
            None,
        )
        .unwrap();
        let recovery_scripts = dir.path().join("recovery").join(SCRIPTS_DIR_NAME);
        assert!(!recovery_scripts.exists(), "{recovery_scripts:?}");
    }

    /// Finding 4: `remove_all` must clean up the sibling `scripts\` copy and
    /// `cs2_video.snapshot.txt`, not just `VOIDFRAME_RESTORE.bat` itself --
    /// otherwise a clean run leaves permanent clutter behind with no `.bat`
    /// left to explain it.
    #[test]
    fn remove_all_also_removes_the_sibling_scripts_dir_and_cs2_video_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let project_dir = dir.path().join("projects").join("p1");
        std::fs::create_dir_all(project_dir.join("scripts")).unwrap();
        std::fs::write(
            project_dir.join("scripts").join("revert.bat"),
            "@echo off\r\n",
        )
        .unwrap();

        let mut j = Journal::open(&run_dir.join("journal-s1.jsonl")).unwrap();
        let seq = j
            .record(
                Op::CustomScriptApply,
                &crate::system::MutationCtx {
                    run_id: "r1".into(),
                    scenario_id: "s1".into(),
                    step_index: 0,
                },
                json!({}),
                json!({}),
                json!({ "apply_sha256": "a", "revert_sha256": "b", "revert_path": "revert.bat" }),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
        let seq2 = j
            .record(
                Op::Cs2ConfigApply,
                &crate::system::MutationCtx {
                    run_id: "r1".into(),
                    scenario_id: "s1".into(),
                    step_index: 1,
                },
                json!({}),
                json!({}),
                json!({ "original_text": "setting.fullscreen 1\n" }),
            )
            .unwrap();
        j.mark_applied(seq2).unwrap();
        drop(j);

        let real_path = PathBuf::from("C:\\real\\video.txt");
        write_all(
            &run_dir,
            dir.path(),
            "r1",
            &project_dir.join("scripts"),
            Some(&real_path),
        )
        .unwrap();

        let recovery = dir.path().join("recovery");
        assert!(recovery.join(RESTORE_SCRIPT_NAME).exists());
        assert!(recovery.join(SCRIPTS_DIR_NAME).join("revert.bat").exists());
        assert!(recovery.join(CS2_SNAPSHOT_NAME).exists());

        remove_all(dir.path());

        assert!(!recovery.join(RESTORE_SCRIPT_NAME).exists());
        assert!(!recovery.join(SCRIPTS_DIR_NAME).exists());
        assert!(!recovery.join(CS2_SNAPSHOT_NAME).exists());
    }

    #[test]
    fn write_all_writes_the_cs2_video_snapshot_file_when_a_cs2_config_record_is_present() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        let scripts_dir = dir.path().join("no-scripts-here");

        let mut j = Journal::open(&run_dir.join("journal-s1.jsonl")).unwrap();
        let seq = j
            .record(
                Op::Cs2ConfigApply,
                &crate::system::MutationCtx {
                    run_id: "r1".into(),
                    scenario_id: "s1".into(),
                    step_index: 0,
                },
                json!({}),
                json!({}),
                json!({ "original_text": "setting.fullscreen 1\n" }),
            )
            .unwrap();
        j.mark_applied(seq).unwrap();
        drop(j);

        let real_path = PathBuf::from("C:\\real\\video.txt");
        write_all(&run_dir, dir.path(), "r1", &scripts_dir, Some(&real_path)).unwrap();

        let snapshot = dir.path().join("recovery").join(CS2_SNAPSHOT_NAME);
        assert_eq!(
            std::fs::read_to_string(&snapshot).unwrap(),
            "setting.fullscreen 1\n"
        );
    }
}

#[cfg(test)]
mod review_2026_09_10_tests {
    use super::*;
    use crate::journal::Op;
    use crate::model::progress::{Cursor, RunProgress, Stage};
    use crate::model::project::{Baseline, Project, Scenario};
    use crate::system::MutationCtx;
    use serde_json::json;

    fn rec(seq: u64, op: Op, inverse: serde_json::Value) -> JournalRecord {
        JournalRecord {
            seq,
            ts_unix_ms: 0,
            run_id: "r1".into(),
            scenario_id: "s1".into(),
            step_index: 0,
            op,
            target: json!({}),
            new: json!({}),
            inverse,
            applied: true,
        }
    }

    /// A `REG_EXPAND_SZ` inverse carries `%SystemRoot%`-style text that
    /// `cmd` would expand at run time; a `"` inside a value would end the
    /// argument early. Both must be escaped in the rendered line.
    #[test]
    fn registry_values_are_escaped_for_cmd() {
        let records = vec![rec(
            1,
            Op::RegistryWrite,
            json!({"kind":"write","target":{"hive":"HKLM","subkey":"SOFTWARE\\Test","value_name":"Path"},
                   "value":{"type":"EXPAND_SZ","value":"%SystemRoot%\\system32;\"quoted\""}}),
        )];
        let bat = render(&records, "r1", None);
        assert!(
            bat.contains(
                "reg add \"HKLM\\SOFTWARE\\Test\" /v \"Path\" /t REG_EXPAND_SZ /d \"%%SystemRoot%%\\system32;\\\"quoted\\\"\" /f"
            ),
            "{bat}"
        );
    }

    /// `write_all` renders only the journals still live per `progress.json`:
    /// scenario `a` completed its own revert before the reboot, so its
    /// records must not reach a `.bat` a human will run.
    #[test]
    fn write_all_renders_only_the_live_journals() {
        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("runs").join("r1");
        std::fs::create_dir_all(&run_dir).unwrap();
        for (id, setting) in [("a", "SETTING_A"), ("b", "SETTING_B")] {
            let mut j = Journal::open(&run_dir.join(format!("journal-{id}.jsonl"))).unwrap();
            let seq = j
                .record(
                    Op::PowercfgWrite,
                    &MutationCtx {
                        run_id: "r1".into(),
                        scenario_id: id.into(),
                        step_index: 0,
                    },
                    json!({}),
                    json!(1),
                    json!({"sub":"sub_processor","setting":setting,"value":0}),
                )
                .unwrap();
            j.mark_applied(seq).unwrap();
        }
        let scenario = |id: &str| Scenario {
            id: id.into(),
            name: id.into(),
            description: "d".into(),
            enabled: true,
            modules: vec![],
        };
        RunProgress {
            schema_version: crate::model::SCHEMA_VERSION.to_string(),
            run_id: "r1".into(),
            project: Project {
                schema_version: crate::model::SCHEMA_VERSION.to_string(),
                id: "p1".into(),
                name: "P".into(),
                description: "d".into(),
                created_at: "2026-09-10T00:00:00Z".into(),
                settings: serde_json::from_str("{}").unwrap(),
                baseline: Baseline {
                    name: "Stock".into(),
                    description: "d".into(),
                },
                scenarios: vec![scenario("a"), scenario("b")],
            },
            start_build_id: None,
            start_launch_args: String::new(),
            start_launch_args_raw: String::new(),
            start_power_plan: crate::system::PowerPlan {
                guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
                name: "Balanced".into(),
                active: true,
            },
            thermal_baseline: None,
            completed: vec![],
            unstable: vec![],
            // `b` is index 2 (`ordered_scenarios` prepends baseline).
            cursor: Cursor {
                index: 2,
                stage: Stage::Measure,
            },
            reboot: None,
            shutdown_when_complete: false,
            skip_revert_once: false,
            abort_requested: false,
        }
        .save(&run_dir)
        .unwrap();

        let scripts_dir = dir.path().join("no-scripts-here");
        write_all(&run_dir, dir.path(), "r1", &scripts_dir, None).unwrap();
        let text =
            std::fs::read_to_string(dir.path().join("recovery").join(RESTORE_SCRIPT_NAME)).unwrap();
        assert!(text.contains("SETTING_B 0"), "{text}");
        assert!(
            !text.contains("SETTING_A"),
            "an already-reverted scenario must not be replayed by the .bat: {text}"
        );
    }
}
