//! `voidframe-cli steam-launch-check` — exercises the Steam-readiness/CS2-
//! launch flow standalone against a real machine (`--controller windows`),
//! without needing a project/scenario file or the Tauri UI. Prints each
//! step's outcome and timing; non-zero exit on any step's failure.

use voidframe_engine::run::execute::steam_launch_check;

pub async fn run(controller: &str, kill_cs2_after: bool) -> anyhow::Result<()> {
    let (sys, _capture) =
        crate::harness::select_backend(controller, false, std::path::PathBuf::new())?;

    let report = steam_launch_check(sys.as_ref(), kill_cs2_after).await;

    for step in &report.steps {
        let mark = if step.ok { "OK  " } else { "FAIL" };
        println!(
            "[{mark}] {} ({} ms) — {}",
            step.name, step.duration_ms, step.detail
        );
    }

    if report.overall_ok {
        println!("\nsteam-launch-check OK");
        Ok(())
    } else {
        eprintln!("\nsteam-launch-check FAILED");
        std::process::exit(1);
    }
}
