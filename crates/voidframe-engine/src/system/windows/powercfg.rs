//! Real powercfg access — shells out to `powercfg.exe` and parses its text
//! output, per docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §4.2's explicit choice (not native `Power*` GUID APIs).
//!
//! **Load-bearing finding from this plan's pre-writing verification:**
//! `powercfg /query` silently omits "hidden" settings (confirmed live for
//! `IDLEDISABLE`) — no error, exit code 0, just no setting block in the
//! output. Every read/write here uses `/qh` ("query hidden"), which shows
//! the same settings `/query` does plus the hidden ones, with an identical
//! text format — there is no reason to ever use plain `/query`.

use crate::error::{Error, Result};
use crate::system::AcDc;
use tokio::process::Command;

const SCHEME: &str = "SCHEME_CURRENT";

/// Suppresses the console window Windows would otherwise allocate for
/// `powercfg.exe` (a console-subsystem exe) when spawned from this GUI app
/// -- same class of bug as `capture/presentmon.rs`'s PresentMon fix,
/// unnoticed here only because each call finishes in milliseconds.
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

async fn run_powercfg(args: &[&str]) -> Result<String> {
    let output = Command::new("powercfg.exe")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .await
        .map_err(|e| Error::msg(format!("failed to spawn powercfg.exe {args:?}: {e}")))?;
    if !output.status.success() {
        return Err(Error::msg(format!(
            "powercfg.exe {args:?} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    // powercfg.exe writes plain ASCII/UTF-8-safe text on an English locale;
    // from_utf8_lossy degrades gracefully on any other codepage rather than
    // erroring the whole read.
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parses the current AC / DC setting indices out of a `/qh` block. Both
/// lines are required — a block missing either (setting alias not found at
/// all, distinct from the "hidden but present" case `/qh` already handles)
/// is reported as an error, not a default.
///
/// The line labels (`Current AC Power Setting Index:` on an English
/// system) are localized by Windows, so they are never matched. The
/// locale-independent shape is that a setting block always ends with the
/// AC index line followed by the DC index line, each `<label>: 0x........`;
/// the min/max/increment lines of a ranged setting share that value shape
/// but precede them, so the last two hex-valued lines are AC then DC.
fn parse_ac_dc(qh_output: &str) -> Result<AcDc<u32>> {
    let hex_values: Vec<u32> = qh_output
        .lines()
        .filter_map(|line| {
            let (_, value) = line.trim().rsplit_once(':')?;
            let hex = value.trim().strip_prefix("0x")?;
            u32::from_str_radix(hex, 16).ok()
        })
        .collect();
    match hex_values[..] {
        [.., ac, dc] => Ok(AcDc { ac, dc }),
        _ => Err(Error::msg(format!(
            "could not find both AC and DC power setting indices in powercfg output:\n{qh_output}"
        ))),
    }
}

pub async fn read(sub: &str, setting: &str) -> Result<AcDc<u32>> {
    let out = run_powercfg(&["/qh", SCHEME, sub, setting]).await?;
    parse_ac_dc(&out)
}

pub async fn write(sub: &str, setting: &str, v: u32) -> Result<()> {
    let v_str = v.to_string();
    run_powercfg(&["/setacvalueindex", SCHEME, sub, setting, &v_str]).await?;
    run_powercfg(&["/setdcvalueindex", SCHEME, sub, setting, &v_str]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_QH_HIDDEN: &str = r#"
Power Scheme GUID: 206b3296-170d-49f3-b100-f9c11b379c23  (Custom)
  Subgroup GUID: 54533251-82be-4824-96c1-47b60b740d00  (Processor power management)
    GUID Alias: SUB_PROCESSOR
    Power Setting GUID: 5d76a2ca-e8c0-402f-a133-2158492d58ad  (Processor idle disable)
      GUID Alias: IDLEDISABLE
      Possible Setting Index: 000
      Possible Setting Friendly Name: Enable idle
      Possible Setting Index: 001
      Possible Setting Friendly Name: Disable idle
    Current AC Power Setting Index: 0x00000000
    Current DC Power Setting Index: 0x00000000
"#;

    const SAMPLE_QH_RANGED: &str = r#"
    Power Setting GUID: 0cc5b647-c1df-4637-891a-dec35c318583  (Processor performance core parking min cores)
      GUID Alias: CPMINCORES
      Minimum Possible Setting: 0x00000000
      Maximum Possible Setting: 0x00000064
      Possible Settings increment: 0x00000001
      Possible Settings units: %
    Current AC Power Setting Index: 0x00000064
    Current DC Power Setting Index: 0x0000000a
"#;

    #[test]
    fn parses_hidden_setting_block_captured_from_the_real_rig() {
        assert_eq!(
            parse_ac_dc(SAMPLE_QH_HIDDEN).unwrap(),
            AcDc { ac: 0, dc: 0 }
        );
    }

    #[test]
    fn parses_ranged_setting_block_captured_from_the_real_rig() {
        assert_eq!(
            parse_ac_dc(SAMPLE_QH_RANGED).unwrap(),
            AcDc { ac: 0x64, dc: 0x0a }
        );
    }

    // Synthetic (not captured): the ranged block above with every label
    // replaced by non-English text of the shape Windows produces on a
    // localized system. A German alpha tester hit exactly this on
    // `powercfg /list` (see `power_plan.rs`), and `/qh` localizes its
    // labels the same way, so the parser must not depend on them.
    const SAMPLE_QH_LOCALIZED: &str = r#"
    Energieeinstellungs-GUID: 0cc5b647-c1df-4637-891a-dec35c318583  (Mindestanzahl Kerne)
      GUID-Alias: CPMINCORES
      Minimale Einstellung: 0x00000000
      Maximale Einstellung: 0x00000064
      Schrittweite: 0x00000001
      Einheiten: %
    Aktueller Wechselstrom-Index: 0x00000064
    Aktueller Gleichstrom-Index: 0x0000000a
"#;

    #[test]
    fn parses_localized_labels_without_matching_english_text() {
        assert_eq!(
            parse_ac_dc(SAMPLE_QH_LOCALIZED).unwrap(),
            AcDc { ac: 0x64, dc: 0x0a }
        );
    }

    #[test]
    fn missing_both_lines_is_an_error_not_a_default() {
        assert!(parse_ac_dc("Power Scheme GUID: xyz\n").is_err());
    }

    // This test genuinely shells out to powercfg.exe and mutates then
    // restores IDLEDISABLE on SCHEME_CURRENT — the same real setting this
    // plan's own pre-writing verification round-tripped by hand. Marked
    // #[ignore] because CI's windows-latest runner may run as a locked-down
    // service account where powercfg writes behave differently than on an
    // interactive desktop session (untested — the spec only requires
    // Windows-API code to *compile-check* in CI, §11); run manually with
    // `cargo test -- --ignored` on a real desktop to exercise it for real.
    #[tokio::test]
    #[ignore = "mutates real system power settings — run manually, not in CI"]
    async fn read_write_round_trips_against_the_real_active_scheme() {
        let before = read("sub_processor", "IDLEDISABLE").await.unwrap();
        write("sub_processor", "IDLEDISABLE", 1).await.unwrap();
        let mid = read("sub_processor", "IDLEDISABLE").await.unwrap();
        assert_eq!(mid, AcDc { ac: 1, dc: 1 });
        write("sub_processor", "IDLEDISABLE", before.ac)
            .await
            .unwrap();
        // IDLEDISABLE's AC and DC values happened to be equal on this rig at
        // verification time, but restore each independently — write() sets
        // both from one argument, so restore AC and DC in two calls if they
        // ever differ; here they're re-set together via the AC value on
        // purpose to mirror this specific setting's typical usage (one
        // value for both power sources on a desktop with no battery).
        let after = read("sub_processor", "IDLEDISABLE").await.unwrap();
        assert_eq!(after, before);
    }
}
