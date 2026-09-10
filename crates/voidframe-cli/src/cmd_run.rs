//! `voidframe-cli run <project.json>` — runs the whole engine, headless
//! (no WebView2 tree exists here, so quiet-window suspend is skipped —
//! `RunConfig::webview_root_pid` stays `None`). Prints `EngineEvent`s to
//! stdout as they arrive; exits non-zero on `RunFailed`.
//!
//! `--controller windows` is the real thing: a real `WindowsController`
//! paired with a real `RealCaptureRunner` driving the real `PresentMon.exe`
//! at `presentmon_path` against a real, VOIDFRAME-launched `cs2.exe`.
//!
//! `--controller mock` is a fully hermetic smoke test — it deliberately
//! pairs `MockController` with `MockCaptureRunner` rather than
//! `RealCaptureRunner`: `CaptureRunner` is a *separate* seam from
//! `SystemController` (see its own doc comment — this is intentional, not
//! an oversight), and `RealCaptureRunner` genuinely shells out to
//! `PresentMon.exe`, which needs an actual running `cs2.exe` process to
//! capture frames from (zero frames captured is a hard `Error`, per
//! `capture::parser::aggregate_metrics`). `MockController` never launches a
//! real process, so pairing it with `RealCaptureRunner` could only ever
//! either hang (spike's PresentMon build blocks waiting for the named
//! process) or fail — neither exercises anything real. Also pre-seeds the
//! mock's launch options so a freshly-loaded project's baseline (whose
//! reconciled launch args are always `keybind_cfg::reconcile(&[], false)`,
//! since baseline carries zero modules) doesn't immediately hit the
//! Steam-restart `OperatorPrompt` flow — `MockController::steam_status` is
//! a fixed value, so `wait_until_steam_closed` would otherwise spin for a
//! full minute and then hard-fail. A scenario that itself adds a
//! `LaunchArgs` module can still legitimately hit that prompt; this harness
//! auto-acknowledges the prompt (see below) but cannot make Steam actually
//! "close" against a static mock, so such a project will time out there —
//! a real limitation of testing that particular flow without a real Steam.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use voidframe_engine::capture::runner::CaptureRunner;
use voidframe_engine::model::project::Project;
use voidframe_engine::run::execute::RunConfig;
use voidframe_engine::run::{ControlMsg, EngineEvent, RunOutcome, RunStart};
use voidframe_engine::system::SystemController;

use crate::harness::select_backend;
use crate::sound;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    project_path: &Path,
    dry_run: bool,
    controller: &str,
    presentmon_path: PathBuf,
    data_root: PathBuf,
    console_log_override: Option<PathBuf>,
    sound_enabled: bool,
) -> anyhow::Result<()> {
    let project = Project::load(project_path)?;
    // `reboot_sequence` registers the `--resume`/`--recover` scheduled tasks
    // against `RunConfig::exe_path` -- this binary -- but `voidframe-cli`'s
    // clap parser has only subcommands and rejects that argv (exit 2,
    // "unexpected argument"). Against the real controller the machine
    // genuinely reboots, nothing ever resumes or recovers, the applied
    // scenario is never reverted, and both tasks stay registered. Refuse up
    // front rather than strand the machine; `--dry-run` and the mock
    // controller never reboot, so both stay usable for reboot projects.
    if controller == "windows"
        && !dry_run
        && let Some(sc) = project.enabled_scenarios().find(|s| s.requires_reboot())
    {
        anyhow::bail!(
            "scenario '{}' requires a reboot, which voidframe-cli cannot resume from \
             (its own binary rejects the `--resume`/`--recover` tasks the engine would \
             register); run reboot projects from the VOIDFRAME app, or pass --dry-run",
            sc.name
        );
    }
    let run_id = uuid::Uuid::new_v4().to_string();

    let (sys, capture): (Arc<dyn SystemController>, Arc<dyn CaptureRunner>) =
        select_backend(controller, dry_run, presentmon_path)?;

    let (control_tx, control_rx) = tokio::sync::mpsc::channel(16);
    // `voidframe-cli` is headless -- nothing here can ever toggle
    // `shutdown_when_complete` mid-run, so the sender is just dropped. Seeded
    // `false` to match `config.shutdown_when_complete` below.
    let (_shutdown_toggle_tx, shutdown_toggle_rx) = tokio::sync::watch::channel(false);

    let config = RunConfig {
        project,
        run_id: run_id.clone(),
        dry_run,
        data_root,
        webview_root_pid: None,
        console_log_override,
        mock_cs2_log: None,
        thermal_sample_override: None,
        inter_scenario_break_seconds: 0,
        hwinfo_path: None,
        shutdown_when_complete: false,
        post_boot_settle: std::time::Duration::from_secs(180),
        exe_path: std::env::current_exe()?,
    };

    let mut run = voidframe_engine::run::spawn_run(
        sys,
        capture,
        RunStart::Fresh(config),
        control_rx,
        shutdown_toggle_rx,
    );

    // Print events as they arrive; auto-acknowledge OperatorPrompts after
    // printing them (a headless harness has no human to click "OK" — this
    // matches docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §11's own framing of this command as a dev/test tool,
    // not the real operator-facing surface; the real Tauri UI wires
    // ControlMsg::OperatorAcknowledged to an actual button, this doesn't).
    let mut failed = false;
    while let Some(ev) = run.events.recv().await {
        println!("{}", serde_json::to_string(&ev)?);
        if sound_enabled && let Some(cue) = sound::sound_cue_for_event(&ev) {
            sound::play(cue);
        }
        match &ev {
            EngineEvent::RunFailed { .. } => failed = true,
            EngineEvent::OperatorPrompt { .. } => {
                let _ = control_tx.send(ControlMsg::OperatorAcknowledged).await;
            }
            _ => {}
        }
    }

    let result = run.join.await??;
    if failed {
        anyhow::bail!("run failed");
    }
    match result {
        RunOutcome::Complete(results) => println!(
            "\nRun complete: {} scenario(s), run_id={}",
            results.scenarios.len(),
            run_id
        ),
        RunOutcome::RebootPending(reason) => {
            println!("\nRun continues after reboot ({reason}), run_id={run_id}");
        }
        RunOutcome::ShutdownRequested(_) => {
            println!("\nRun complete, shutdown requested, run_id={run_id}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `voidframe-cli` cannot answer the `--resume`/`--recover` scheduled
    /// tasks a real reboot would register against its own binary, so a
    /// reboot project against the real controller is refused before
    /// anything is applied -- but stays runnable under `--dry-run`, which
    /// never reboots.
    #[tokio::test]
    async fn refuses_a_reboot_project_against_the_real_controller_unless_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path().join("p.json");
        std::fs::write(
            &project_path,
            serde_json::json!({
                "schema_version": voidframe_engine::model::SCHEMA_VERSION,
                "id": "p", "name": "P", "description": "d",
                "created_at": "2026-09-10T00:00:00Z", "settings": {},
                "baseline": {"name": "Stock", "description": "d"},
                "scenarios": [{
                    "id": "hags", "name": "HAGS", "description": "d", "enabled": true,
                    "modules": [{
                        "type": "registry", "hive": "HKLM",
                        "subkey": r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers",
                        "value_name": "HwSchMode", "value_type": "DWORD", "value": 2,
                        "requires_reboot": true
                    }]
                }]
            })
            .to_string(),
        )
        .unwrap();

        let err = run(
            &project_path,
            false,
            "windows",
            PathBuf::from("PresentMon.exe"),
            dir.path().join("data"),
            None,
            false,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("requires a reboot"), "{err}");
        assert!(
            !dir.path().join("data").exists(),
            "refused before the engine ever started"
        );
    }
}
