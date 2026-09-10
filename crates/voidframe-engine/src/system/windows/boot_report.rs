//! Boot facts for classification (spec §4): Kernel-Power 41 and BugCheck
//! 1001 from the System log, new minidumps, and the BCD safeboot option.
//!
//! The event log is read with `wevtutil qe System /f:xml` and a
//! server-side XPath filter, then scanned for `<EventID>` elements. XML is
//! structural, not localized, so this is locale-safe (unlike `/f:text`)
//! and far smaller than driving `EvtQuery`/`EvtRender` by hand. The BCD
//! read scans `bcdedit /enum {current}` output **only** for the literal
//! `safeboot` element name, which bcdedit prints identically in every
//! locale (element names are not translated; only headings are).

use crate::error::{Error, Result};
use crate::system::BootReport;
use std::path::PathBuf;
use std::time::SystemTime;
use tokio::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub(crate) fn iso_utc(t: SystemTime) -> String {
    let secs = t
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days as i64);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

// Howard Hinnant's days-to-civil algorithm (public domain).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

async fn run(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .await
        .map_err(|e| Error::msg(format!("failed to spawn {program} {args:?}: {e}")))?;
    if !output.status.success() {
        return Err(Error::msg(format!(
            "{program} {args:?} exited with {}: {}",
            output.status,
            super::console_text::decode_console_output(&output.stderr)
        )));
    }
    Ok(super::console_text::decode_console_output(&output.stdout))
}

/// Parses `wevtutil` XML output: returns (saw 41, saw 1001).
pub(crate) fn scan_event_ids(xml: &str) -> (bool, bool) {
    let mut saw41 = false;
    let mut saw1001 = false;
    for chunk in xml.split("<EventID").skip(1) {
        let Some(gt) = chunk.find('>') else { continue };
        let body = &chunk[gt + 1..];
        let Some(end) = body.find('<') else { continue };
        match body[..end].trim() {
            "41" => saw41 = true,
            "1001" => saw1001 = true,
            _ => {}
        }
    }
    (saw41, saw1001)
}

/// Finds a `safeboot` line in `bcdedit /enum {current}` output; the value
/// token (`Minimal` / `Network`) follows on the same line.
pub(crate) fn scan_safeboot(bcd: &str) -> Option<String> {
    bcd.lines()
        .map(str::trim)
        .find(|l| l.to_ascii_lowercase().starts_with("safeboot"))
        .and_then(|l| l.split_whitespace().nth(1).map(str::to_string))
}

async fn minidumps_since(since: SystemTime) -> Vec<PathBuf> {
    let dir = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Minidump");
    let Ok(mut rd) = tokio::fs::read_dir(&dir).await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        let p = e.path();
        let is_dmp = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dmp"));
        let is_new = e
            .metadata()
            .await
            .and_then(|m| m.modified())
            .is_ok_and(|t| t >= since);
        if is_dmp && is_new {
            out.push(p);
        }
    }
    out
}

pub async fn report(since: SystemTime) -> Result<BootReport> {
    let query = format!(
        "*[System[(EventID=41 or EventID=1001) and TimeCreated[@SystemTime>='{}']]]",
        iso_utc(since)
    );
    let q = format!("/q:{query}");
    let xml = run("wevtutil.exe", &["qe", "System", "/f:xml", "/c:200", &q]).await?;
    let (kernel_power_41, bugcheck_1001) = scan_event_ids(&xml);
    let bcd = run("bcdedit.exe", &["/enum", "{current}"])
        .await
        .unwrap_or_default();
    Ok(BootReport {
        kernel_power_41,
        bugcheck_1001,
        new_minidumps: minidumps_since(since).await,
        safeboot_option: scan_safeboot(&bcd),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_event_ids_out_of_wevtutil_xml() {
        let xml = r#"<Event><System><EventID>41</EventID></System></Event><Event><System><EventID Qualifiers="16384">1001</EventID></System></Event>"#;
        assert_eq!(scan_event_ids(xml), (true, true));
        assert_eq!(scan_event_ids("<Events/>"), (false, false));
        assert_eq!(
            scan_event_ids("<Event><System><EventID>6005</EventID></System></Event>"),
            (false, false)
        );
    }

    #[test]
    fn scans_safeboot_from_bcdedit_output_in_any_locale() {
        let de = "Windows-Startladeprogramm\r\n---------\r\nbezeichner    {current}\r\nsafeboot      Minimal\r\n";
        assert_eq!(scan_safeboot(de).as_deref(), Some("Minimal"));
        assert_eq!(
            scan_safeboot("identifier {current}\r\ndescription Windows 11\r\n"),
            None
        );
    }

    #[test]
    fn iso_utc_formats_the_epoch_and_a_known_date() {
        assert_eq!(iso_utc(SystemTime::UNIX_EPOCH), "1970-01-01T00:00:00Z");
        // 1_788_652_800 is 2026-09-06T00:00:00Z (verified independently via
        // `date -u -d @1788652800`) -- the brief's original literal,
        // 1_788_998_400, is actually 2026-09-10T00:00:00Z, a four-day
        // arithmetic slip against its own "a known date" comment.
        let t = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_788_652_800);
        assert_eq!(iso_utc(t), "2026-09-06T00:00:00Z");
    }

    #[tokio::test]
    #[ignore = "reads the real System event log and BCD -- run manually: `cargo test -p voidframe-engine --lib system::windows::boot_report::tests::live_report_since_epoch_parses -- --ignored --exact --nocapture`"]
    async fn live_report_since_epoch_parses() {
        let r = report(SystemTime::UNIX_EPOCH).await.unwrap();
        println!("{r:?}");
        assert!(r.safeboot_option.is_none(), "dev rig is not in safe mode");
    }
}
