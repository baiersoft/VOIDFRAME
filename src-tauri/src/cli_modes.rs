//! `voidframe.exe --resume` and `voidframe.exe --recover --data-root <p>`
//! (spec §3.4, §5.2). Parsed before Tauri starts: `--recover` never opens
//! a window (it runs as SYSTEM from the deadman task); `--resume` starts
//! the GUI and tells `setup` to call `resume_run` immediately.

use std::path::PathBuf;
use voidframe_engine::system::WindowsController;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliMode {
    Gui,
    Resume,
    Recover { data_root: PathBuf },
}

pub fn parse(args: &[String]) -> CliMode {
    let mut iter = args.iter().skip(1);
    let mut recover = false;
    let mut data_root: Option<PathBuf> = None;
    let mut resume = false;
    while let Some(a) = iter.next() {
        match a.as_str() {
            "--resume" => resume = true,
            "--recover" => recover = true,
            "--data-root" => data_root = iter.next().map(PathBuf::from),
            _ => {}
        }
    }
    match (recover, data_root, resume) {
        (true, Some(root), _) => CliMode::Recover { data_root: root },
        (true, None, _) => CliMode::Recover {
            data_root: voidframe_engine::paths::DataRoot::resolve(false)
                .map(|d| d.path().to_path_buf())
                .unwrap_or_default(),
        },
        (false, _, true) => CliMode::Resume,
        _ => CliMode::Gui,
    }
}

/// Appends a short trace to `<data_root>\logs\recover.log`. The Tauri log
/// plugin never starts under `--recover` (no `tauri::Builder::run()`), so
/// `log::*` has no sink here -- this is the only record the deadman leaves.
fn append_recover_log(data_root: &std::path::Path, line: &str) {
    use std::io::Write;
    let logs_dir = data_root.join("logs");
    if std::fs::create_dir_all(&logs_dir).is_err() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(logs_dir.join("recover.log"))
    {
        let _ = writeln!(
            f,
            "{} {line}",
            voidframe_engine::model::results::utc_timestamp_now()
        );
    }
}

pub fn run_recover(data_root: PathBuf) -> i32 {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("voidframe --recover: tokio runtime: {e}");
            return 2;
        }
    };
    let sys = WindowsController::new();
    match rt.block_on(voidframe_engine::recover::recover(
        &sys,
        &data_root,
        std::time::SystemTime::now(),
    )) {
        Ok(report) => {
            log::info!("deadman recovery: {report:?}");
            append_recover_log(&data_root, &format!("deadman recovery: {report:?}"));
            if report.verify_failures.is_empty() {
                0
            } else {
                1
            }
        }
        Err(e) => {
            log::error!("deadman recovery failed: {e}");
            append_recover_log(&data_root, &format!("deadman recovery failed: {e}"));
            eprintln!("voidframe --recover: {e}");
            3
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("voidframe.exe")
            .chain(list.iter().copied())
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn parses_the_three_modes() {
        assert_eq!(parse(&args(&[])), CliMode::Gui);
        assert_eq!(parse(&args(&["--resume"])), CliMode::Resume);
        assert_eq!(
            parse(&args(&["--recover", "--data-root", r"C:\d"])),
            CliMode::Recover {
                data_root: PathBuf::from(r"C:\d")
            }
        );
    }
}
