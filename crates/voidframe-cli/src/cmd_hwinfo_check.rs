//! `voidframe-cli hwinfo-check` — exercises the HWiNFO sensor-read flow
//! standalone against a real machine (`--controller windows`), without
//! needing a project/scenario file or the Tauri UI. Prints each step's
//! outcome and timing (including the real CPU/GPU temperatures read);
//! non-zero exit on any step's failure.

use std::path::PathBuf;
use voidframe_engine::run::execute::hwinfo_check;

pub async fn run(controller: &str, hwinfo_path: PathBuf) -> anyhow::Result<()> {
    let (sys, _capture) = crate::harness::select_backend(controller, false, PathBuf::new())?;

    let report = hwinfo_check(sys.as_ref(), &hwinfo_path).await;

    for step in &report.steps {
        let mark = if step.ok { "OK  " } else { "FAIL" };
        println!(
            "[{mark}] {} ({} ms) — {}",
            step.name, step.duration_ms, step.detail
        );
    }

    if report.overall_ok {
        println!("\nhwinfo-check OK");
        Ok(())
    } else {
        eprintln!("\nhwinfo-check FAILED");
        std::process::exit(1);
    }
}
