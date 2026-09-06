//! `voidframe-cli parse-presentmon <csv-path>` — parse a PresentMon CSV
//! and print its aggregated Metrics as JSON (manual smoke test).

use std::path::Path;
use voidframe_engine::capture::{aggregate_metrics, parse_presentmon_csv};

pub fn run(csv_path: &Path) -> anyhow::Result<()> {
    let samples = parse_presentmon_csv(csv_path)?;
    println!(
        "Parsed {} frame samples from {}",
        samples.len(),
        csv_path.display()
    );
    let metrics = aggregate_metrics(&samples)?;
    println!("{}", serde_json::to_string_pretty(&metrics)?);
    Ok(())
}
