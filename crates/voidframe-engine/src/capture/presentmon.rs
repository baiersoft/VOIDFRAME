//! Builds and runs the PresentMon v2.5.1 capture invocation confirmed
//! working in the verification spike
//! (`docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md` §12;
//! `spike/findings.md` §3).

use crate::error::{Error, Result};
use std::path::PathBuf;

/// Suppresses the console window Windows would otherwise allocate for
/// PresentMon (a console-subsystem exe) when spawned from this GUI app --
/// confirmed live: without it, a visible terminal popped up in the
/// foreground for the whole capture window. Piped stdout/stderr capture
/// below still works normally with no console attached.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// One capture window's parameters.
#[derive(Debug, Clone, PartialEq)]
pub struct PmArgs {
    pub process_name: String,
    pub output_file: PathBuf,
    pub timed_seconds: u32,
}

/// Builds the exact confirmed CLI invocation:
/// `--process_name <name> --output_file <path> --timed <seconds> --terminate_after_timed
/// --set_circular_buffer_size 4096`. The buffer size bump (PresentMon's default is smaller)
/// was added after real captures at the current, longer `capture_seconds` window logged
/// PresentMon buffer-overflow warnings in the terminal.
pub fn build_presentmon_args(args: &PmArgs) -> Vec<String> {
    vec![
        "--process_name".to_string(),
        args.process_name.clone(),
        "--output_file".to_string(),
        args.output_file.to_string_lossy().into_owned(),
        "--timed".to_string(),
        args.timed_seconds.to_string(),
        "--terminate_after_timed".to_string(),
        "--set_circular_buffer_size".to_string(),
        "4096".to_string(),
    ]
}

/// Thin wrapper around spawning the real `PresentMon.exe`. Not unit tested
/// beyond argument-building (Step above) — correctness of the actual
/// subprocess invocation needs a real PresentMon binary, which isn't
/// available in CI (same reasoning
/// `docs/superpowers/plans/2026-09-01-m1-phase-1-status.md` applied to
/// deferring `WindowsController`'s real implementation to
/// `docs/superpowers/plans/2026-09-01-m1-phase-3a-windows-controller.md`).
pub struct PresentMonController {
    exe_path: PathBuf,
}

impl PresentMonController {
    pub fn new(exe_path: PathBuf) -> Self {
        Self { exe_path }
    }

    /// Runs one capture window and returns the CSV path on success.
    /// `--terminate_after_timed` means PresentMon exits on its own after
    /// `timed_seconds` — this call returns once that happens.
    ///
    /// PresentMon's own stdout+stderr (e.g. ETW buffer-overflow/event-loss
    /// warnings) previously only reached a human if a terminal happened to
    /// be attached and watching at the exact moment — invisible in the
    /// rotating log file, and in the one real case this was observed
    /// (2026-09-04), the only diagnostic evidence of *why* a capture had
    /// silently produced no output. Captured here unconditionally (`.output()`
    /// pipes both streams, same convention as `powercfg::run_powercfg`) and
    /// logged, so a post-mortem never needs the run reproduced live to see it.
    pub async fn capture(&self, args: &PmArgs) -> Result<PathBuf> {
        let cli_args = build_presentmon_args(args);
        // `kill_on_drop(true)`: makes an operator Abort mid-capture (`run/
        // execute/scenario.rs` races this future against the abort signal
        // via `tokio::select!`) actually terminate the real PresentMon
        // process when this future is dropped, instead of leaving it
        // running detached until its own `--terminate_after_timed` fires.
        let output = tokio::process::Command::new(&self.exe_path)
            .args(&cli_args)
            .kill_on_drop(true)
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .await?;

        let pm_output = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let pm_output = pm_output.trim();
        if !pm_output.is_empty() {
            tracing::info!(target: "presentmon", "{pm_output}");
        }
        let output_suffix = if pm_output.is_empty() {
            String::new()
        } else {
            format!(" -- PresentMon output: {pm_output}")
        };

        if !output.status.success() {
            return Err(Error::msg(format!(
                "PresentMon exited with status {} (invocation: {} {}){output_suffix}",
                output.status,
                self.exe_path.display(),
                cli_args.join(" "),
            )));
        }

        // A non-zero exit above already covers the "PresentMon itself
        // failed" case -- this covers the distinct case observed live: a
        // clean exit status with no output file at all, which happens when
        // PresentMon's capture target (`args.process_name`) exits or was
        // never captured during the timed window. Left unchecked, this
        // surfaced as a bare, contextless `io: file not found` from the CSV
        // parser several call frames away, with nothing pointing at the
        // real cause.
        if !tokio::fs::try_exists(&args.output_file)
            .await
            .unwrap_or(false)
        {
            return Err(Error::msg(format!(
                "PresentMon exited successfully but never wrote its output file ({}) -- its \
                 capture target (\"{}\") likely exited before or during the {}s capture \
                 window{output_suffix}",
                args.output_file.display(),
                args.process_name,
                args.timed_seconds,
            )));
        }

        Ok(args.output_file.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_the_confirmed_invocation_exactly() {
        let args = PmArgs {
            process_name: "cs2.exe".into(),
            output_file: std::path::PathBuf::from(r"C:\data\runs\r1\scenario-s1\iter-01.csv"),
            timed_seconds: 60,
        };
        let cli = build_presentmon_args(&args);
        let expected: Vec<String> = vec![
            "--process_name",
            "cs2.exe",
            "--output_file",
            r"C:\data\runs\r1\scenario-s1\iter-01.csv",
            "--timed",
            "60",
            "--terminate_after_timed",
            "--set_circular_buffer_size",
            "4096",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        assert_eq!(cli, expected);
    }

    /// Real round trip against `PresentMon.exe`, exercising the exact case
    /// observed live (2026-09-04) that this file's `capture` doc comment
    /// describes: PresentMon exits successfully but writes no output file
    /// because its capture target never actually matched a running process.
    /// A bogus, guaranteed-never-to-exist `process_name` reproduces that
    /// deterministically without needing CS2 or any other real target
    /// running. Not portable/CI-safe (spawns a real `PresentMon.exe`), so
    /// `#[ignore]`d and run manually, same precedent as this session's other
    /// live tests.
    #[tokio::test]
    #[ignore = "spawns a real PresentMon.exe -- run manually: `cargo test -p voidframe-engine \
                --lib capture::presentmon::tests::live_capture_errors_clearly_on_a_clean_exit_with_no_output_file \
                -- --ignored --exact --nocapture`"]
    async fn live_capture_errors_clearly_on_a_clean_exit_with_no_output_file() {
        let dir = tempfile::tempdir().unwrap();
        let output_file = dir.path().join("should-not-exist.csv");
        let controller = PresentMonController::new(PathBuf::from(r"C:\PresentMon\PresentMon.exe"));
        let args = PmArgs {
            process_name: "voidframe-test-process-that-will-never-exist-8f3a2c91.exe".into(),
            output_file: output_file.clone(),
            timed_seconds: 2,
        };
        let err = controller.capture(&args).await.unwrap_err();
        let msg = err.to_string();
        println!("live capture error (expected): {msg}");
        assert!(
            msg.contains("never wrote its output file"),
            "error should name the clean-exit-no-file case, was: {msg}"
        );
        assert!(!output_file.exists());
    }
}
