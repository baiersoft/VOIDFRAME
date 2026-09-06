//! Real power-plan enumeration/activation via `powercfg.exe`. Output format
//! confirmed live (`powercfg /list`) during this plan's pre-writing
//! verification — one `Power Scheme GUID: <guid>  (<name>)` line per plan,
//! a trailing ` *` on the active one.

use crate::error::{Error, Result};
use crate::system::PowerPlan;
use tokio::process::Command;

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
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parses every `Power Scheme GUID: <guid>  (<name>)[ *]` line out of a
/// `powercfg /list` (or `/duplicatescheme`) block.
fn parse_plans(list_output: &str) -> Vec<PowerPlan> {
    let mut plans = Vec::new();
    for line in list_output.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("Power Scheme GUID: ") else {
            continue;
        };
        let active = rest.trim_end().ends_with('*');
        let rest = rest.trim_end().trim_end_matches('*').trim_end();
        let Some(open) = rest.find('(') else { continue };
        let Some(close) = rest.rfind(')') else {
            continue;
        };
        if close <= open {
            continue;
        }
        let guid = rest[..open].trim().to_string();
        let name = rest[open + 1..close].to_string();
        plans.push(PowerPlan { guid, name, active });
    }
    plans
}

pub async fn list() -> Result<Vec<PowerPlan>> {
    let out = run_powercfg(&["/list"]).await?;
    Ok(parse_plans(&out))
}

pub async fn active() -> Result<PowerPlan> {
    list()
        .await?
        .into_iter()
        .find(|p| p.active)
        .ok_or_else(|| Error::msg("powercfg /list reported no active scheme".into()))
}

pub async fn set_active(guid: &str) -> Result<()> {
    run_powercfg(&["/setactive", guid]).await?;
    Ok(())
}

pub async fn duplicate(template_guid: &str) -> Result<PowerPlan> {
    let out = run_powercfg(&["/duplicatescheme", template_guid]).await?;
    let plans = parse_plans(&out);
    plans.into_iter().next().ok_or_else(|| {
        Error::msg(format!(
            "powercfg /duplicatescheme {template_guid} produced no parseable output:\n{out}"
        ))
    })
}

pub async fn delete(guid: &str) -> Result<()> {
    run_powercfg(&["/delete", guid]).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Captured verbatim from this rig via `powercfg /list` during this
    // plan's pre-writing verification.
    const SAMPLE_LIST: &str = "Existing Power Schemes (* Active)\r\n-----------------------------------\r\nPower Scheme GUID: 206b3296-170d-49f3-b100-f9c11b379c23  (FrameSync Labs Boost (X3D)) *\r\nPower Scheme GUID: 31693169-3169-3169-3169-316931693169  (imribiy Power Plan)\r\nPower Scheme GUID: 381b4222-f694-41f0-9685-ff5bb260df2e  (Balanced)\r\nPower Scheme GUID: 8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c  (High performance)\r\nPower Scheme GUID: a1841308-3541-4fab-bc81-f71556f20b4a  (Power saver)\r\n";

    #[test]
    fn parses_five_plans_with_one_active_captured_from_the_real_rig() {
        let plans = parse_plans(SAMPLE_LIST);
        assert_eq!(plans.len(), 5);
        assert_eq!(
            plans.iter().filter(|p| p.active).count(),
            1,
            "exactly one plan should be marked active"
        );
        let active = plans.iter().find(|p| p.active).unwrap();
        assert_eq!(active.guid, "206b3296-170d-49f3-b100-f9c11b379c23");
        assert_eq!(active.name, "FrameSync Labs Boost (X3D)");
        let balanced = plans.iter().find(|p| p.name == "Balanced").unwrap();
        assert!(!balanced.active);
        assert_eq!(balanced.guid, "381b4222-f694-41f0-9685-ff5bb260df2e");
    }

    #[test]
    fn empty_input_parses_to_no_plans() {
        assert!(parse_plans("").is_empty());
        assert!(parse_plans("Existing Power Schemes (* Active)\r\n---\r\n").is_empty());
    }

    // Genuinely shells out and mutates power-plan state — but only ever a
    // freshly-*duplicated* scratch plan, never the machine's real original
    // active plan: duplicate -> activate the duplicate -> restore the
    // original active plan -> delete the duplicate. Zero residue and the
    // real active plan is back to what it was before this test ran, even
    // if an assertion fails partway through (the restore step runs before
    // any assertion that could panic). #[ignore]d for the same
    // CI-service-account-uncertainty reason as `powercfg.rs`'s round-trip test.
    #[tokio::test]
    #[ignore = "mutates real system power-plan state — run manually, not in CI"]
    async fn duplicate_activate_restore_delete_round_trips() {
        let original_active = active().await.unwrap();

        let dup = duplicate(&original_active.guid).await.unwrap();
        assert_ne!(dup.guid, original_active.guid);
        assert!(list().await.unwrap().iter().any(|p| p.guid == dup.guid));

        set_active(&dup.guid).await.unwrap();
        assert_eq!(active().await.unwrap().guid, dup.guid);

        // Restore before any further assertions, so a later failure still
        // leaves the machine's real plan active.
        set_active(&original_active.guid).await.unwrap();
        let restored = active().await.unwrap();

        delete(&dup.guid).await.unwrap();
        assert!(!list().await.unwrap().iter().any(|p| p.guid == dup.guid));

        assert_eq!(restored.guid, original_active.guid);
    }
}
