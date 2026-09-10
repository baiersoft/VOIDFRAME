//! Runs a `.bat`/`.cmd`/`.ps1` script to completion and captures its output.
//! `.bat`/`.cmd` run directly (`CreateProcess` handles batch files natively);
//! `.ps1` needs an explicit `powershell.exe -File` wrapper, since PowerShell
//! scripts aren't directly executable via `CreateProcess`. Same
//! `tokio::process::Command` + `CREATE_NO_WINDOW` pattern as
//! `system/windows/powercfg.rs::run_powercfg` -- this app is a GUI process,
//! so a console-subsystem child would otherwise flash a window.

use super::console_text::decode_console_output;
use crate::error::{Error, Result};
use crate::system::ScriptOutput;
use std::path::Path;
use std::time::Duration;
use tokio::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub async fn run(path: &Path, timeout: Duration) -> Result<ScriptOutput> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_lowercase)
        .unwrap_or_default();
    let mut cmd = match ext.as_str() {
        "ps1" => {
            let mut c = Command::new("powershell.exe");
            c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
                .arg(path);
            c
        }
        "bat" | "cmd" => Command::new(path),
        other => {
            return Err(Error::msg(format!(
                "unsupported script extension {other:?} for {}",
                path.display()
            )));
        }
    };
    cmd.creation_flags(CREATE_NO_WINDOW).kill_on_drop(true);

    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| {
            Error::msg(format!(
                "script {} timed out after {:?}",
                path.display(),
                timeout
            ))
        })?
        .map_err(|e| Error::msg(format!("failed to spawn {}: {e}", path.display())))?;

    Ok(ScriptOutput {
        exit_code: output.status.code(),
        stdout: decode_console_output(&output.stdout),
        stderr: decode_console_output(&output.stderr),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_batch_script_that_exits_2_reports_that_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fail.bat");
        std::fs::write(&script, "@echo off\r\nexit /b 2\r\n").unwrap();
        let out = run(&script, Duration::from_secs(10)).await.unwrap();
        assert_eq!(out.exit_code, Some(2));
    }

    #[tokio::test]
    async fn stdout_is_captured() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("echo.bat");
        std::fs::write(&script, "@echo off\r\necho hello-from-script\r\n").unwrap();
        let out = run(&script, Duration::from_secs(10)).await.unwrap();
        assert!(out.stdout.contains("hello-from-script"), "{:?}", out.stdout);
    }

    #[tokio::test]
    async fn an_unsupported_extension_is_rejected_before_spawning() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("nope.exe");
        std::fs::write(&script, b"").unwrap();
        let err = run(&script, Duration::from_secs(10)).await.unwrap_err();
        assert!(err.to_string().contains("unsupported script extension"));
    }

    #[tokio::test]
    async fn a_script_that_outlives_its_timeout_is_killed_and_reported() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("hang.bat");
        // `ping` with a count is a portable "sleep" on Windows; 20 pings at
        // ~1s each comfortably outlives the 1s timeout below.
        std::fs::write(&script, "@echo off\r\nping -n 20 127.0.0.1 >nul\r\n").unwrap();
        let err = run(&script, Duration::from_millis(500)).await.unwrap_err();
        assert!(err.to_string().contains("timed out"), "{err}");
    }
}
