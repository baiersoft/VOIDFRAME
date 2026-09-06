#![deny(clippy::undocumented_unsafe_blocks)]
mod calibrate_stats;
mod cmd_calibrate;
mod cmd_dry_run;
mod cmd_hwinfo_check;
mod cmd_parse_presentmon;
mod cmd_run;
mod cmd_steam_launch_check;
mod cmd_validate;
mod harness;
mod sound;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "voidframe-cli",
    version,
    about = "VOIDFRAME engine dev harness"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Print engine version and exit
    Version,
    /// Load and validate a project JSON file
    Validate { project: PathBuf },
    /// Apply every enabled scenario against a DryRun/Mock controller, then revert
    DryRun {
        project: PathBuf,
        /// Use a real (Windows) controller instead of Mock.
        #[arg(long)]
        windows: bool,
    },
    /// Parse a PresentMon CSV and print its aggregated Metrics as JSON
    ParsePresentmon { csv: PathBuf },
    /// Run the whole engine headlessly, end to end: PREFLIGHT through
    /// REPORT, printing `EngineEvent`s as JSON lines. docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §11's own named
    /// test harness — "Exercises the whole engine without the UI; the M3
    /// VM harness reuses it".
    Run {
        project: PathBuf,
        /// Log every mutation but apply none of them (wraps the chosen
        /// controller in `DryRunController`).
        #[arg(long)]
        dry_run: bool,
        /// Which `SystemController` backend to run against.
        #[arg(long, default_value = "mock")]
        controller: String,
        /// Path to `PresentMon.exe`. Defaults to the verification spike's
        /// own reference copy, `spike/tools/PresentMon.exe`, if not given.
        #[arg(long)]
        presentmon: Option<PathBuf>,
        /// Root directory for this run's journal/results files. Defaults
        /// to a `voidframe-cli`-specific temp subdirectory if not given.
        #[arg(long)]
        data_root: Option<PathBuf>,
        /// Dev/test seam: tail this file instead of resolving CS2's real
        /// `console.log` via the real Steam installation
        /// (`RunConfig::console_log_override`). Not needed for a real
        /// `--controller windows` run; useful for pointing `--controller
        /// mock` at a synthetic log to exercise detection without a real
        /// CS2 process.
        #[arg(long)]
        console_log: Option<PathBuf>,
        /// Play a distinct Windows system sound when a measure iteration's
        /// real PresentMon capture window starts and stops -- useful on a
        /// single-monitor rig with no visual feedback available to line up
        /// timing precisely.
        #[arg(long)]
        sound: bool,
    },
    /// Calibrate `warmup_loops`/`measure_loops` from a real large-N
    /// baseline-only run on the rig (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §11a, finding D9). Prints a
    /// report; does not modify `docs/`, `config.rs`, or `settings.rs`.
    Calibrate {
        /// Which `SystemController` backend to run against.
        #[arg(long, default_value = "mock")]
        controller: String,
        /// Path to `PresentMon.exe`. Defaults to the verification spike's
        /// own reference copy, `spike/tools/PresentMon.exe`, if not given.
        #[arg(long)]
        presentmon: Option<PathBuf>,
        /// Root directory for this run's journal/results files. Defaults
        /// to a `voidframe-cli`-specific temp subdirectory if not given.
        #[arg(long)]
        data_root: Option<PathBuf>,
        /// Dev/test seam: tail this file instead of resolving CS2's real
        /// `console.log` via the real Steam installation.
        #[arg(long)]
        console_log: Option<PathBuf>,
        /// Overrides `Settings::map_id` (the Steam workshop benchmark map).
        /// Defaults to the spec default (Dust2, `3240880604`).
        #[arg(long)]
        map: Option<String>,
        /// Total captured baseline iterations for this calibration session
        /// (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §11a: "≈ 25–30"). All are captured — see this command's
        /// own module doc comment for why `warmup_loops` is always 0 here.
        #[arg(long, default_value_t = 30)]
        iterations: u32,
        /// Play a distinct Windows system sound when a measure iteration's
        /// real PresentMon capture window starts and stops.
        #[arg(long)]
        sound: bool,
        /// Gives every measure iteration its own fresh CS2 launch (as its
        /// own synthetic no-op scenario) instead of one CS2 session for the
        /// whole run -- for comparing thermal behavior against the default.
        /// See this command's own module doc comment.
        #[arg(long)]
        fresh_cs2_per_iteration: bool,
        /// Cooldown, in seconds, while CS2 is closed between iterations.
        /// Only meaningful combined with --fresh-cs2-per-iteration (with it
        /// off there's only one CS2 session, so nothing to break between).
        #[arg(long, default_value_t = 0)]
        break_seconds: u32,
    },
    /// Exercises the Steam-readiness/CS2-launch flow standalone: checks
    /// Steam's status, ensures it's running and ready (waiting for both
    /// `steam.exe` and `steamwebhelper.exe`, closing its window if this
    /// call launched it), then launches CS2 and waits for it to appear.
    /// Prints each step's outcome and timing. Use `--controller windows`
    /// to verify against a real machine -- `mock` (the default) only
    /// proves the wiring, not real Steam/CS2 behavior.
    SteamLaunchCheck {
        /// Which `SystemController` backend to run against.
        #[arg(long, default_value = "mock")]
        controller: String,
        /// Gracefully close CS2 after the check succeeds, so testing this
        /// command doesn't leave a stray cs2.exe running.
        #[arg(long)]
        kill_cs2_after: bool,
    },
    /// Exercises the HWiNFO sensor-read flow standalone: checks whether
    /// HWiNFO is already running, starts it if not, reads its CPU/GPU
    /// temperature sensors, then closes HWiNFO again if this call is what
    /// started it. Prints each step's outcome, timing, and (for the sensor
    /// read) the real temperatures. Use `--controller windows` to verify
    /// against a real machine -- `mock` (the default) only proves the
    /// wiring, not real HWiNFO sensor values.
    HwinfoCheck {
        /// Which `SystemController` backend to run against.
        #[arg(long, default_value = "mock")]
        controller: String,
        /// Path to `HWiNFO64.exe`. Defaults to the verification spike's own
        /// reference copy.
        #[arg(long, default_value = "spike/tools/HWiNFO64.exe")]
        hwinfo_path: PathBuf,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_target(false).init();
    let cli = Cli::parse();
    match cli.command {
        Command::Version => println!("voidframe-engine {}", env!("CARGO_PKG_VERSION")),
        Command::Validate { project } => match cmd_validate::run(&project) {
            Ok(msg) => println!("{msg}"),
            Err(e) => {
                eprintln!("INVALID: {e}");
                std::process::exit(1);
            }
        },
        Command::DryRun { project, windows } => {
            cmd_dry_run::run(&project, windows).await?;
        }
        Command::ParsePresentmon { csv } => cmd_parse_presentmon::run(&csv)?,
        Command::Run {
            project,
            dry_run,
            controller,
            presentmon,
            data_root,
            console_log,
            sound,
        } => {
            let presentmon =
                presentmon.unwrap_or_else(|| PathBuf::from("spike/tools/PresentMon.exe"));
            let data_root =
                data_root.unwrap_or_else(|| std::env::temp_dir().join("voidframe-cli-runs"));
            cmd_run::run(
                &project,
                dry_run,
                &controller,
                presentmon,
                data_root,
                console_log,
                sound,
            )
            .await?;
        }
        Command::Calibrate {
            controller,
            presentmon,
            data_root,
            console_log,
            map,
            iterations,
            sound,
            fresh_cs2_per_iteration,
            break_seconds,
        } => {
            let presentmon =
                presentmon.unwrap_or_else(|| PathBuf::from("spike/tools/PresentMon.exe"));
            let data_root =
                data_root.unwrap_or_else(|| std::env::temp_dir().join("voidframe-cli-runs"));
            cmd_calibrate::run(
                &controller,
                presentmon,
                data_root,
                console_log,
                map,
                iterations,
                sound,
                fresh_cs2_per_iteration,
                break_seconds,
            )
            .await?;
        }
        Command::SteamLaunchCheck {
            controller,
            kill_cs2_after,
        } => {
            cmd_steam_launch_check::run(&controller, kill_cs2_after).await?;
        }
        Command::HwinfoCheck {
            controller,
            hwinfo_path,
        } => {
            cmd_hwinfo_check::run(&controller, hwinfo_path).await?;
        }
    }
    Ok(())
}
