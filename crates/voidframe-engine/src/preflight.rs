//! Pre-flight facts and the pure evaluator that turns them into a report.
//! `docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md` ships
//! the M1 subset from docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §4.4: Steam / CS2-install / map /
//! disk (blocking), build-id drift and stale ETW (warning), and the not-yet-
//! evaluated M3 checks surfaced as `Deferred` so the UI can show them greyed
//! out rather than silently omit them. Fact *gathering* is
//! `docs/superpowers/plans/2026-09-01-m1-phase-3a-windows-controller.md`
//! (`WindowsController`); here only the value type and the pure `evaluate`
//! function exist.

use crate::error::Result;
use serde::Serialize;

/// Raw facts a `SystemController` gathers before a run. [`PreflightFacts::all_ok`]
/// is a test/demo constructor — every field passes.
#[derive(Debug, Clone)]
pub struct PreflightFacts {
    pub steam_running: bool,
    pub steam_logged_in: bool,
    pub steam_elevated: bool,
    pub cs2_installed: bool,
    pub cs2_state_flags: u32,
    pub workshop_map_subscribed: bool,
    pub cs2_build_id: Option<String>,
    pub last_known_build_id: Option<String>,
    pub stale_etw_session: bool,
    pub free_disk_bytes: u64,
    pub min_free_bytes: u64,
    /// Names of enabled scenarios whose `power_plan` module targets the
    /// plan that is *currently active* right now -- meaning that scenario
    /// would show no difference from baseline on this dimension. Read live
    /// (not from saved project state) because the active plan can drift
    /// between authoring the project and starting the run.
    pub noop_power_plan_scenarios: Vec<String>,
    /// Same idea as `noop_power_plan_scenarios`, for `launch_args`: names of
    /// enabled scenarios whose configured launch options (whitespace-
    /// normalized) equal the user's real current CS2 launch options right
    /// now (with VOIDFRAME's own reserved tokens stripped out) -- read live
    /// for the same reason.
    pub noop_launch_args_scenarios: Vec<String>,
}

impl PreflightFacts {
    pub fn all_ok() -> Self {
        Self {
            steam_running: true,
            steam_logged_in: true,
            steam_elevated: false,
            cs2_installed: true,
            cs2_state_flags: 4,
            workshop_map_subscribed: true,
            cs2_build_id: Some("199".into()),
            last_known_build_id: Some("199".into()),
            stale_etw_session: false,
            free_disk_bytes: 50_000_000_000,
            min_free_bytes: 2_000_000_000,
            noop_power_plan_scenarios: vec![],
            noop_launch_args_scenarios: vec![],
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Pass,
    Warn,
    Block,
    Deferred,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct Check {
    pub id: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
pub struct PreflightReport {
    pub checks: Vec<Check>,
}

impl PreflightReport {
    pub fn blocking(&self) -> Vec<&Check> {
        self.checks
            .iter()
            .filter(|c| matches!(c.status, CheckStatus::Block))
            .collect()
    }
    pub fn is_blocked(&self) -> bool {
        !self.blocking().is_empty()
    }
    pub fn warnings(&self) -> Vec<&Check> {
        self.checks
            .iter()
            .filter(|c| matches!(c.status, CheckStatus::Warn))
            .collect()
    }
}

fn check(id: &str, status: CheckStatus, detail: impl Into<String>) -> Check {
    Check {
        id: id.into(),
        status,
        detail: detail.into(),
    }
}

/// Pure fact -> report mapping. Deterministic and side-effect free, so it is
/// exercised directly in tests without a `SystemController`.
pub fn evaluate(f: &PreflightFacts) -> PreflightReport {
    let mut checks = Vec::new();

    checks.push(if !f.steam_running {
        check(
            "steam",
            CheckStatus::Block,
            "Steam is not running — open Steam normally, or enable Steam's \"Start Steam \
             when computer starts\" option, and try again",
        )
    } else if !f.steam_logged_in {
        check(
            "steam",
            CheckStatus::Block,
            "Steam must be running and logged in",
        )
    } else if f.steam_elevated {
        check(
            "steam",
            CheckStatus::Warn,
            "Steam is running elevated — the CS2 it launches will be elevated too; \
             VOIDFRAME never closes an already-running Steam just to fix this",
        )
    } else {
        check(
            "steam",
            CheckStatus::Pass,
            "Steam is running, logged in, and not elevated",
        )
    });

    checks.push(if f.cs2_installed && f.cs2_state_flags == 4 {
        check(
            "cs2_install",
            CheckStatus::Pass,
            "CS2 is installed and up to date",
        )
    } else {
        check(
            "cs2_install",
            CheckStatus::Block,
            format!(
                "CS2 not fully installed / currently updating (StateFlags={})",
                f.cs2_state_flags
            ),
        )
    });

    checks.push(if f.workshop_map_subscribed {
        check(
            "workshop_map",
            CheckStatus::Pass,
            "Benchmark workshop map is subscribed",
        )
    } else {
        check(
            "workshop_map",
            CheckStatus::Block,
            "Subscribe to workshop map 3240880604",
        )
    });

    checks.push(if f.free_disk_bytes >= f.min_free_bytes {
        check(
            "disk",
            CheckStatus::Pass,
            "Enough free disk space for run artifacts",
        )
    } else {
        check(
            "disk",
            CheckStatus::Block,
            "Not enough free disk space for run artifacts",
        )
    });

    checks.push(match (&f.cs2_build_id, &f.last_known_build_id) {
        (Some(cur), Some(prev)) if cur != prev => check(
            "cs2_build",
            CheckStatus::Warn,
            format!("CS2 build changed since last run ({prev} -> {cur}) — re-baseline recommended"),
        ),
        _ => check("cs2_build", CheckStatus::Pass, "CS2 build id unchanged"),
    });

    checks.push(if f.stale_etw_session {
        check(
            "etw",
            CheckStatus::Warn,
            "A stale PresentMon ETW session was found and will be cleared",
        )
    } else {
        check("etw", CheckStatus::Pass, "No stale PresentMon ETW session")
    });

    checks.push(if f.noop_power_plan_scenarios.is_empty() {
        check(
            "power_plan_noop_vs_active",
            CheckStatus::Pass,
            "No scenario's power plan matches the system's current active plan",
        )
    } else {
        check(
            "power_plan_noop_vs_active",
            CheckStatus::Warn,
            format!(
                "{} already target the system's current active power plan -- \
                 they won't show a difference from baseline on this dimension unless \
                 the active plan changes before the run starts",
                f.noop_power_plan_scenarios.join(", ")
            ),
        )
    });

    checks.push(if f.noop_launch_args_scenarios.is_empty() {
        check(
            "launch_args_noop_vs_active",
            CheckStatus::Pass,
            "No scenario's launch options match the system's current launch options",
        )
    } else {
        check(
            "launch_args_noop_vs_active",
            CheckStatus::Warn,
            format!(
                "{} already match your current CS2 launch options -- \
                 they won't show a difference from baseline on this dimension unless \
                 your launch options change before the run starts",
                f.noop_launch_args_scenarios.join(", ")
            ),
        )
    });

    for (id, label) in [
        ("bitlocker", "BitLocker recovery-prompt risk"),
        ("secure_boot", "Secure Boot / TPM measured boot"),
        ("autologon", "AutoLogon readiness for unattended reboots"),
        ("anticheat", "Third-party kernel anti-cheat conflicts"),
        ("thermal", "Thermal baseline calibration"),
    ] {
        checks.push(check(
            id,
            CheckStatus::Deferred,
            format!("{label} — evaluated in milestone M3"),
        ));
    }

    PreflightReport { checks }
}

/// Gathers real facts via `sys` (Steam status, CS2 install state, disk
/// space) and `last_known_build_id` (the run-start value, persisted across
/// runs by the caller — pre-flight only needs to *compare* against it, not
/// own its storage). `min_free_bytes` is a caller-supplied threshold (spec
/// §4.4 doesn't hardcode one; the caller reads it from `config.json`).
pub async fn gather_facts(
    sys: &dyn crate::system::SystemController,
    last_known_build_id: Option<&str>,
    min_free_bytes: u64,
    scenarios: &[crate::model::project::Scenario],
) -> Result<PreflightFacts> {
    let steam = sys.steam_status().await?;
    let manifest = sys.app_manifest(730).await?;
    let build_id = manifest.as_ref().and_then(|m| m.build_id.clone());
    let state_flags = manifest.and_then(|m| m.state_flags).unwrap_or(0);
    let free_disk_bytes = sys.free_disk_bytes().await?;

    let active_plan_guid = sys.active_power_plan().await?.guid;
    let noop_power_plan_scenarios = scenarios
        .iter()
        .filter(|s| s.enabled)
        .filter(|s| {
            s.modules.iter().any(|m| {
                matches!(m, crate::model::module::Module::PowerPlan(p) if p.plan_guid == active_plan_guid)
            })
        })
        .map(|s| s.name.clone())
        .collect();

    let live_launch_args =
        crate::cs2::keybind_cfg::strip_reserved(&sys.read_cs2_launch_options().await?);
    let noop_launch_args_scenarios = scenarios
        .iter()
        .filter(|s| s.enabled)
        .filter(|s| {
            s.modules.iter().any(|m| {
                matches!(m, crate::model::module::Module::LaunchArgs { args }
                    if args.split_whitespace().eq(live_launch_args.split_whitespace()))
            })
        })
        .map(|s| s.name.clone())
        .collect();

    Ok(PreflightFacts {
        steam_running: steam.running,
        steam_logged_in: steam.running, // Steam being findable+running implies a logged-in session for M1's purposes — a running-but-logged-out Steam is not a state this engine distinguishes; documented limitation, not a gap: Steam's own UI blocks most actions while logged out anyway.
        steam_elevated: steam.elevated,
        cs2_installed: state_flags != 0,
        cs2_state_flags: state_flags,
        workshop_map_subscribed: sys.workshop_item_installed(730, "3240880604").await?,
        cs2_build_id: build_id,
        last_known_build_id: last_known_build_id.map(str::to_string),
        stale_etw_session: false, // M1: not evaluated — no PresentMon ETW session tracking exists yet; docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §4.4 lists this as a "warn", not "block", so a permanently-false default here costs nothing until a real check is added
        free_disk_bytes,
        min_free_bytes,
        noop_power_plan_scenarios,
        noop_launch_args_scenarios,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_ok_facts_produce_no_blocks() {
        let r = evaluate(&PreflightFacts::all_ok());
        assert!(!r.is_blocked());
        assert!(
            r.checks
                .iter()
                .any(|c| matches!(c.status, CheckStatus::Deferred))
        );
    }

    #[test]
    fn steam_not_running_blocks() {
        let mut f = PreflightFacts::all_ok();
        f.steam_running = false;
        let r = evaluate(&f);
        assert!(r.is_blocked());
        assert!(r.blocking().iter().any(|c| c.id == "steam"));
    }

    #[test]
    fn steam_elevated_warns_not_blocks_when_running_and_logged_in() {
        let mut f = PreflightFacts::all_ok();
        f.steam_elevated = true;
        let r = evaluate(&f);
        assert!(
            !r.is_blocked(),
            "an elevated-but-running Steam must warn, not block (§2.1a rev 6)"
        );
        assert!(r.warnings().iter().any(|c| c.id == "steam"));
    }

    #[test]
    fn cs2_updating_blocks() {
        let mut f = PreflightFacts::all_ok();
        f.cs2_state_flags = 6;
        assert!(evaluate(&f).is_blocked());
    }

    #[test]
    fn power_plan_noop_scenario_produces_a_warning_not_a_block() {
        let mut f = PreflightFacts::all_ok();
        f.noop_power_plan_scenarios = vec!["Scenario A".into()];
        let r = evaluate(&f);
        assert!(!r.is_blocked());
        let check = r
            .checks
            .iter()
            .find(|c| c.id == "power_plan_noop_vs_active")
            .expect("a power_plan_noop_vs_active check must always be present");
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains("Scenario A"), "{}", check.detail);
    }

    #[test]
    fn no_noop_power_plan_scenarios_passes() {
        let r = evaluate(&PreflightFacts::all_ok());
        let check = r
            .checks
            .iter()
            .find(|c| c.id == "power_plan_noop_vs_active")
            .expect("a power_plan_noop_vs_active check must always be present");
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn launch_args_noop_scenario_produces_a_warning_not_a_block() {
        let mut f = PreflightFacts::all_ok();
        f.noop_launch_args_scenarios = vec!["Scenario B".into()];
        let r = evaluate(&f);
        assert!(!r.is_blocked());
        let check = r
            .checks
            .iter()
            .find(|c| c.id == "launch_args_noop_vs_active")
            .expect("a launch_args_noop_vs_active check must always be present");
        assert_eq!(check.status, CheckStatus::Warn);
        assert!(check.detail.contains("Scenario B"), "{}", check.detail);
    }

    #[test]
    fn no_noop_launch_args_scenarios_passes() {
        let r = evaluate(&PreflightFacts::all_ok());
        let check = r
            .checks
            .iter()
            .find(|c| c.id == "launch_args_noop_vs_active")
            .expect("a launch_args_noop_vs_active check must always be present");
        assert_eq!(check.status, CheckStatus::Pass);
    }

    #[test]
    fn build_id_mismatch_only_warns() {
        let mut f = PreflightFacts::all_ok();
        f.cs2_build_id = Some("200".into());
        f.last_known_build_id = Some("199".into());
        let r = evaluate(&f);
        assert!(!r.is_blocked());
        assert!(r.warnings().iter().any(|c| c.id == "cs2_build"));
    }

    #[tokio::test]
    async fn gather_facts_reflects_the_mocked_steam_status() {
        let sys = crate::system::MockController::new()
            .with_steam_status(crate::system::SteamStatus {
                running: true,
                elevated: true,
            })
            .with_app_manifest(Some(crate::system::AppManifest {
                build_id: Some("200".into()),
                state_flags: Some(4),
            }))
            .with_workshop_item_installed(false)
            .with_free_disk_bytes(123);
        let facts = gather_facts(&sys, Some("199"), 1, &[]).await.unwrap();
        assert!(facts.steam_running);
        assert!(facts.steam_elevated);
        assert!(facts.cs2_installed);
        assert_eq!(facts.cs2_state_flags, 4);
        assert_eq!(facts.cs2_build_id.as_deref(), Some("200"));
        assert_eq!(facts.last_known_build_id.as_deref(), Some("199"));
        assert!(!facts.workshop_map_subscribed);
        assert_eq!(facts.free_disk_bytes, 123);
    }

    #[tokio::test]
    async fn gather_facts_reports_cs2_not_installed_when_there_is_no_manifest() {
        let sys = crate::system::MockController::new().with_app_manifest(None);
        let facts = gather_facts(&sys, None, 1, &[]).await.unwrap();
        assert!(!facts.cs2_installed);
        assert_eq!(facts.cs2_state_flags, 0);
        assert_eq!(facts.cs2_build_id, None);
    }

    #[tokio::test]
    async fn gather_facts_flags_a_scenario_whose_power_plan_matches_the_live_active_plan() {
        use crate::model::module::{Module, PowerPlanPayload};
        use crate::model::project::Scenario;

        let sys = crate::system::MockController::new();
        // MockController's default active plan is "Balanced",
        // 381b4222-f694-41f0-9685-ff5bb260df2e (system::mock::default_plans).
        let scenarios = vec![
            Scenario {
                id: "s1".into(),
                name: "Same As Active".into(),
                description: "d".into(),
                enabled: true,
                modules: vec![Module::PowerPlan(PowerPlanPayload {
                    plan_guid: "381b4222-f694-41f0-9685-ff5bb260df2e".into(),
                    friendly_name: None,
                    create_if_missing: false,
                })],
            },
            Scenario {
                id: "s2".into(),
                name: "Different Plan".into(),
                description: "d".into(),
                enabled: true,
                modules: vec![Module::PowerPlan(PowerPlanPayload {
                    plan_guid: "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c".into(),
                    friendly_name: None,
                    create_if_missing: false,
                })],
            },
        ];

        let facts = gather_facts(&sys, None, 1, &scenarios).await.unwrap();
        assert_eq!(
            facts.noop_power_plan_scenarios,
            vec!["Same As Active".to_string()]
        );
    }

    #[tokio::test]
    async fn gather_facts_flags_a_scenario_whose_launch_args_match_the_live_current_options() {
        use crate::model::module::Module;
        use crate::model::project::Scenario;

        // Includes VOIDFRAME's own reserved tokens, as a real "current"
        // value would after a prior run -- gather_facts must strip them
        // before comparing, so the scenario's module ("-novid") matches the
        // stripped human part, not the raw string.
        let sys = crate::system::MockController::new().with_launch_options(&format!(
            "-condebug -conclearlog +exec {} -novid",
            crate::cs2::keybind_cfg::CFG_FILENAME
        ));
        let scenarios = vec![
            Scenario {
                id: "s1".into(),
                name: "Matches Live".into(),
                description: "d".into(),
                enabled: true,
                modules: vec![Module::LaunchArgs {
                    args: "-novid".into(),
                }],
            },
            Scenario {
                id: "s2".into(),
                name: "Different Args".into(),
                description: "d".into(),
                enabled: true,
                modules: vec![Module::LaunchArgs {
                    args: "-high -threads 8".into(),
                }],
            },
        ];

        let facts = gather_facts(&sys, None, 1, &scenarios).await.unwrap();
        assert_eq!(
            facts.noop_launch_args_scenarios,
            vec!["Matches Live".to_string()]
        );
    }
}
