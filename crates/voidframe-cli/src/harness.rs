//! Shared `SystemController`/`CaptureRunner` backend selection for this
//! crate's dev-harness subcommands (`run`, `calibrate`) — factored out of
//! `cmd_run` once `cmd_calibrate` needed the identical match block.

use std::path::PathBuf;
use std::sync::Arc;
use voidframe_engine::capture::runner::{CaptureRunner, MockCaptureRunner, RealCaptureRunner};
use voidframe_engine::mock_harness::{mock_metrics, mock_ready_controller};
use voidframe_engine::system::SystemController;

/// Selects the `SystemController`/`CaptureRunner` pair for `controller`
/// ("mock" or, on Windows builds, "windows"), wrapping in `DryRunController`
/// when `dry_run` is set. Shared by `cmd_run` and `cmd_calibrate` — both
/// dev-harness commands needed the identical selection logic.
///
/// The "mock" backend itself (`mock_ready_controller`/`mock_metrics`) lives
/// in `voidframe_engine::mock_harness` -- shared with `src-tauri`'s own
/// `mock-run` Cargo feature, so both consumers of "a fully synthetic run
/// backend" stay in sync automatically.
pub fn select_backend(
    controller: &str,
    dry_run: bool,
    presentmon_path: PathBuf,
) -> anyhow::Result<(Arc<dyn SystemController>, Arc<dyn CaptureRunner>)> {
    match controller {
        "mock" => Ok((
            voidframe_engine::system::select(dry_run, Arc::new(mock_ready_controller())),
            Arc::new(MockCaptureRunner::new(vec![mock_metrics()])),
        )),
        #[cfg(windows)]
        "windows" => Ok((
            voidframe_engine::system::select(
                dry_run,
                Arc::new(voidframe_engine::system::WindowsController::new()),
            ),
            Arc::new(RealCaptureRunner::new(presentmon_path)),
        )),
        other => anyhow::bail!("unknown or unavailable controller: {other}"),
    }
}
