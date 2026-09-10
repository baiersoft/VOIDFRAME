//! Pre-flight facts and the pure evaluator that turns them into a report.
//! `docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md` ships
//! the M1 subset from docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §4.4: Steam / CS2-install / map /
//! disk (blocking), build-id drift and stale ETW (warning). M3
//! (`docs/superpowers/sdd/2026-09-06-m3-reboot-core/`) adds AutoLogon /
//! DefaultPassword / BitLocker / Secure Boot / registry-no-op / sleep, gated
//! by `has_reboot_scenarios` so a project with no reboot scenarios never
//! pays their extra registry/WMI calls. `anticheat` / `thermal` stay
//! `Deferred` for the UI to show greyed out until milestone M4. Fact
//! *gathering* is
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
    /// Whether the Dust2 Workshop map is installed -- `None` when the
    /// benchmark kind never loads it (`AveYoCfgV2`), so `evaluate` can say
    /// "not needed" instead of asserting a Steam fact that was never read.
    pub workshop_map_subscribed: Option<bool>,
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
    /// Whether any enabled scenario in this project needs a reboot
    /// (`Scenario::requires_reboot`) -- gates whether AutoLogon/BitLocker/
    /// Secure Boot facts are gathered at all, so a project with no reboot
    /// scenarios never pays the extra registry/WMI calls.
    pub has_reboot_scenarios: bool,
    /// `None` when not gathered (no reboot scenarios in this project).
    pub logon: Option<crate::logon::LogonConfig>,
    pub bitlocker: Option<crate::system::BitlockerStatus>,
    /// Outer `None`: not gathered. Inner `None`: the Secure Boot state key
    /// is absent (legacy BIOS).
    pub secure_boot: Option<Option<bool>>,
    /// (scenario name, would need a reboot) for enabled scenarios whose
    /// `Module::Registry` value already matches the live registry value.
    pub noop_registry_scenarios: Vec<(String, bool)>,
    pub sleep_inhibited: bool,
}

impl PreflightFacts {
    pub fn all_ok() -> Self {
        Self {
            steam_running: true,
            steam_logged_in: true,
            steam_elevated: false,
            cs2_installed: true,
            cs2_state_flags: 4,
            workshop_map_subscribed: Some(true),
            cs2_build_id: Some("199".into()),
            last_known_build_id: Some("199".into()),
            stale_etw_session: false,
            free_disk_bytes: 50_000_000_000,
            min_free_bytes: 2_000_000_000,
            noop_power_plan_scenarios: vec![],
            noop_launch_args_scenarios: vec![],
            has_reboot_scenarios: false,
            logon: None,
            bitlocker: None,
            secure_boot: None,
            noop_registry_scenarios: vec![],
            sleep_inhibited: true,
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

    checks.push(match f.workshop_map_subscribed {
        Some(true) => check(
            "workshop_map",
            CheckStatus::Pass,
            "Benchmark workshop map is subscribed",
        ),
        Some(false) => check(
            "workshop_map",
            CheckStatus::Block,
            "Subscribe to workshop map 3240880604",
        ),
        // Spec D4: the check is skipped for a kind that never loads the
        // Workshop map. Still listed, so the report doesn't silently lose a
        // row between kinds, but as an honest "not needed" rather than a
        // green PASS about a subscription nobody looked up.
        None => check(
            "workshop_map",
            CheckStatus::Pass,
            "Not needed for the AveYo benchmark.cfg v2 benchmark",
        ),
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

    if !f.has_reboot_scenarios {
        for id in ["autologon", "default_password", "bitlocker", "secure_boot"] {
            checks.push(check(
                id,
                CheckStatus::Pass,
                "no reboot scenarios in this project",
            ));
        }
    } else {
        let logon = f.logon.clone().unwrap_or(crate::logon::LogonConfig {
            auto_admin_logon: false,
            default_user_name: None,
            default_password_present: false,
            microsoft_account: false,
        });
        checks.push(if !logon.auto_admin_logon {
            check(
                "autologon",
                CheckStatus::Warn,
                "AutoLogon is not configured -- you'll need to manually log back in after each reboot \
                 to resume the run (the deadman recovery will still safely revert everything if you \
                 don't within ~10 minutes). See docs/guides/autologon-setup.md to set up unattended \
                 AutoLogon instead.",
            )
        } else if logon.microsoft_account {
            check(
                "autologon",
                CheckStatus::Warn,
                "AutoLogon is configured for a Microsoft account -- Windows Hello or a password change \
                 can break unattended logon; a local account is safer for overnight runs",
            )
        } else {
            check(
                "autologon",
                CheckStatus::Pass,
                format!(
                    "AutoLogon configured for {}",
                    logon.default_user_name.as_deref().unwrap_or("the current user")
                ),
            )
        });
        checks.push(if logon.default_password_present {
            check(
                "default_password",
                CheckStatus::Warn,
                "DefaultPassword is stored in plain text in the registry -- use Sysinternals Autologon, \
                 which stores it as an LSA secret, and delete the plain-text value",
            )
        } else {
            check(
                "default_password",
                CheckStatus::Pass,
                "No plain-text DefaultPassword in the registry",
            )
        });
        checks.push(match f.bitlocker {
            Some(crate::system::BitlockerStatus::On) => check(
                "bitlocker",
                CheckStatus::Warn,
                "BitLocker protects the system volume -- a boot-configuration change could trigger the \
                 recovery prompt and stall an overnight run. HAGS does not change boot configuration; \
                 keep your recovery key at hand anyway",
            ),
            Some(crate::system::BitlockerStatus::Unknown) => {
                check("bitlocker", CheckStatus::Warn, "BitLocker status could not be determined")
            }
            _ => check("bitlocker", CheckStatus::Pass, "BitLocker is off on the system volume"),
        });
        checks.push(match f.secure_boot {
            Some(Some(true)) => check(
                "secure_boot",
                CheckStatus::Warn,
                "Secure Boot is enabled -- boot-configuration changes may be rejected; HAGS is unaffected",
            ),
            _ => check("secure_boot", CheckStatus::Pass, "Secure Boot is off or not applicable"),
        });
    }

    checks.push(if f.noop_registry_scenarios.is_empty() {
        check(
            "registry_noop_vs_active",
            CheckStatus::Pass,
            "No registry module already matches the live value",
        )
    } else if f.noop_registry_scenarios.iter().any(|(_, reboot)| *reboot) {
        let names: Vec<&str> = f
            .noop_registry_scenarios
            .iter()
            .filter(|(_, r)| *r)
            .map(|(n, _)| n.as_str())
            .collect();
        check(
            "registry_noop_vs_active",
            CheckStatus::Block,
            format!(
                "{} would reboot only to set a registry value that is already set -- that is your \
                 baseline; remove the module or choose the other value",
                names.join(", ")
            ),
        )
    } else {
        let names: Vec<&str> = f.noop_registry_scenarios.iter().map(|(n, _)| n.as_str()).collect();
        check(
            "registry_noop_vs_active",
            CheckStatus::Warn,
            format!(
                "{} already match the live registry value -- they won't differ from baseline on that dimension",
                names.join(", ")
            ),
        )
    });

    checks.push(if f.sleep_inhibited {
        check(
            "sleep",
            CheckStatus::Pass,
            "Sleep and display timeout are inhibited for the run",
        )
    } else {
        check(
            "sleep",
            CheckStatus::Warn,
            "Could not inhibit sleep -- set the power plan's sleep timeout to Never before an overnight run",
        )
    });

    for (id, label) in [
        ("anticheat", "Third-party kernel anti-cheat conflicts"),
        ("thermal", "Thermal baseline calibration"),
    ] {
        checks.push(check(
            id,
            CheckStatus::Deferred,
            format!("{label} — evaluated in milestone M4"),
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
    benchmark_kind: crate::model::BenchmarkKind,
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

    let has_reboot_scenarios = scenarios
        .iter()
        .filter(|s| s.enabled)
        .any(crate::model::project::Scenario::requires_reboot);

    let (logon, bitlocker, secure_boot) = if has_reboot_scenarios {
        (
            Some(crate::logon::read_logon_config(sys).await?),
            Some(
                sys.bitlocker_protection()
                    .await
                    .unwrap_or(crate::system::BitlockerStatus::Unknown),
            ),
            Some(crate::logon::secure_boot_enabled(sys).await?),
        )
    } else {
        (None, None, None)
    };

    // Spec §8: the block condition is "every reboot-required registry module
    // in the scenario is a no-op", not "any one module happens to match" --
    // a scenario can carry more than one `Module::Registry` entry, and one
    // matching value must not hide another that genuinely differs.
    let mut noop_registry_scenarios = Vec::new();
    for s in scenarios.iter().filter(|s| s.enabled) {
        let mut has_reg = false;
        let mut has_reboot_reg = false;
        let mut all_noop = true;
        let mut reboot_all_noop = true;
        for m in &s.modules {
            if let crate::model::module::Module::Registry(p) = m {
                has_reg = true;
                let live = sys
                    .read_registry(&crate::mutation::registry::regkey(p))
                    .await?;
                let is_noop = live == crate::mutation::registry::regvalue_from_payload(p)?;
                all_noop &= is_noop;
                if p.requires_reboot {
                    has_reboot_reg = true;
                    reboot_all_noop &= is_noop;
                }
            }
        }
        if has_reboot_reg && reboot_all_noop {
            noop_registry_scenarios.push((s.name.clone(), true));
        } else if has_reg && all_noop {
            noop_registry_scenarios.push((s.name.clone(), false));
        }
    }

    Ok(PreflightFacts {
        steam_running: steam.running,
        steam_logged_in: steam.running, // Steam being findable+running implies a logged-in session for M1's purposes — a running-but-logged-out Steam is not a state this engine distinguishes; documented limitation, not a gap: Steam's own UI blocks most actions while logged out anyway.
        steam_elevated: steam.elevated,
        cs2_installed: state_flags != 0,
        cs2_state_flags: state_flags,
        workshop_map_subscribed: match benchmark_kind {
            crate::model::BenchmarkKind::WorkshopDust2 => {
                Some(sys.workshop_item_installed(730, "3240880604").await?)
            }
            // AveYo's cfg-triggered benchmark never loads a Workshop item --
            // the fact is never read at all (spec D4), and `evaluate` says
            // so rather than reporting a subscription as satisfied.
            crate::model::BenchmarkKind::AveYoCfgV2 => None,
        },
        cs2_build_id: build_id,
        last_known_build_id: last_known_build_id.map(str::to_string),
        stale_etw_session: false, // M1: not evaluated — no PresentMon ETW session tracking exists yet; docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §4.4 lists this as a "warn", not "block", so a permanently-false default here costs nothing until a real check is added
        free_disk_bytes,
        min_free_bytes,
        noop_power_plan_scenarios,
        noop_launch_args_scenarios,
        has_reboot_scenarios,
        logon,
        bitlocker,
        secure_boot,
        noop_registry_scenarios,
        // Pre-flight reports the intent; the engine actually inhibits sleep
        // and asserts it at SNAPSHOT, logging a failure there if it didn't
        // take (docs/superpowers/sdd/2026-09-06-m3-reboot-core/).
        sleep_inhibited: true,
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
        let facts = gather_facts(
            &sys,
            Some("199"),
            1,
            &[],
            crate::model::BenchmarkKind::WorkshopDust2,
        )
        .await
        .unwrap();
        assert!(facts.steam_running);
        assert!(facts.steam_elevated);
        assert!(facts.cs2_installed);
        assert_eq!(facts.cs2_state_flags, 4);
        assert_eq!(facts.cs2_build_id.as_deref(), Some("200"));
        assert_eq!(facts.last_known_build_id.as_deref(), Some("199"));
        assert_eq!(facts.workshop_map_subscribed, Some(false));
        assert_eq!(facts.free_disk_bytes, 123);
    }

    #[tokio::test]
    async fn gather_facts_reports_cs2_not_installed_when_there_is_no_manifest() {
        let sys = crate::system::MockController::new().with_app_manifest(None);
        let facts = gather_facts(
            &sys,
            None,
            1,
            &[],
            crate::model::BenchmarkKind::WorkshopDust2,
        )
        .await
        .unwrap();
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

        let facts = gather_facts(
            &sys,
            None,
            1,
            &scenarios,
            crate::model::BenchmarkKind::WorkshopDust2,
        )
        .await
        .unwrap();
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

        let facts = gather_facts(
            &sys,
            None,
            1,
            &scenarios,
            crate::model::BenchmarkKind::WorkshopDust2,
        )
        .await
        .unwrap();
        assert_eq!(
            facts.noop_launch_args_scenarios,
            vec!["Matches Live".to_string()]
        );
    }

    #[tokio::test]
    async fn gather_facts_flags_a_hags_scenario_that_already_matches_the_live_registry_value() {
        use crate::model::module::{Hive, Module, RegType, RegistryPayload};
        use crate::model::project::Scenario;

        let sys = crate::system::MockController::new().with_registry(
            &crate::system::RegKey {
                hive: Hive::Hklm,
                subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers".into(),
                value_name: "HwSchMode".into(),
            },
            crate::system::RegValue::Dword(2),
        );
        let scenarios = vec![Scenario {
            id: "s1".into(),
            name: "HAGS on".into(),
            description: "d".into(),
            enabled: true,
            modules: vec![Module::Registry(RegistryPayload {
                hive: Hive::Hklm,
                subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers".into(),
                value_name: "HwSchMode".into(),
                value_type: RegType::Dword,
                value: serde_json::json!(2),
                requires_reboot: true,
            })],
        }];

        let facts = gather_facts(
            &sys,
            None,
            1,
            &scenarios,
            crate::model::BenchmarkKind::WorkshopDust2,
        )
        .await
        .unwrap();
        assert_eq!(
            facts.noop_registry_scenarios,
            vec![("HAGS on".to_string(), true)]
        );
    }

    #[tokio::test]
    async fn gather_facts_requires_every_reboot_registry_module_to_be_a_noop_not_just_one() {
        use crate::model::module::{Hive, Module, RegType, RegistryPayload};
        use crate::model::project::Scenario;

        fn module(subkey: &str, value_name: &str, value: i64, requires_reboot: bool) -> Module {
            Module::Registry(RegistryPayload {
                hive: Hive::Hklm,
                subkey: subkey.into(),
                value_name: value_name.into(),
                value_type: RegType::Dword,
                value: serde_json::json!(value),
                requires_reboot,
            })
        }

        const HAGS_SUBKEY: &str = "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers";
        const MSI_SUBKEY: &str = "SYSTEM\\CurrentControlSet\\Control\\Class\\Display\\0000";

        let sys = crate::system::MockController::new().with_registry(
            &crate::system::RegKey {
                hive: Hive::Hklm,
                subkey: HAGS_SUBKEY.into(),
                value_name: "HwSchMode".into(),
            },
            crate::system::RegValue::Dword(2),
        );
        let scenarios = vec![Scenario {
            id: "s1".into(),
            name: "HAGS + MSI".into(),
            description: "d".into(),
            enabled: true,
            modules: vec![
                module(HAGS_SUBKEY, "HwSchMode", 2, true),
                module(MSI_SUBKEY, "MSISupported", 1, true),
            ],
        }];

        // Only HwSchMode matches the live registry; MSISupported (also
        // requires_reboot) is absent from the mock -- one matching module
        // must not hide another that genuinely differs.
        let facts = gather_facts(
            &sys,
            None,
            1,
            &scenarios,
            crate::model::BenchmarkKind::WorkshopDust2,
        )
        .await
        .unwrap();
        assert!(
            facts.noop_registry_scenarios.is_empty(),
            "a partially-matching scenario must not be flagged as a no-op"
        );

        // Now both match -- the whole reboot would be wasted.
        let sys = sys.with_registry(
            &crate::system::RegKey {
                hive: Hive::Hklm,
                subkey: MSI_SUBKEY.into(),
                value_name: "MSISupported".into(),
            },
            crate::system::RegValue::Dword(1),
        );
        let facts = gather_facts(
            &sys,
            None,
            1,
            &scenarios,
            crate::model::BenchmarkKind::WorkshopDust2,
        )
        .await
        .unwrap();
        assert_eq!(
            facts.noop_registry_scenarios,
            vec![("HAGS + MSI".to_string(), true)]
        );
    }

    fn reboot_facts() -> PreflightFacts {
        let mut f = PreflightFacts::all_ok();
        f.has_reboot_scenarios = true;
        f.logon = Some(crate::logon::LogonConfig {
            auto_admin_logon: true,
            default_user_name: Some("bench".into()),
            default_password_present: false,
            microsoft_account: false,
        });
        f.bitlocker = Some(crate::system::BitlockerStatus::Off);
        f.secure_boot = Some(Some(false));
        f
    }

    fn status_of<'a>(r: &'a PreflightReport, id: &str) -> &'a Check {
        r.checks
            .iter()
            .find(|c| c.id == id)
            .unwrap_or_else(|| panic!("no check {id}"))
    }

    #[test]
    fn reboot_checks_pass_when_autologon_is_configured_the_safe_way() {
        let r = evaluate(&reboot_facts());
        for id in [
            "autologon",
            "default_password",
            "bitlocker",
            "secure_boot",
            "sleep",
        ] {
            assert_eq!(status_of(&r, id).status, CheckStatus::Pass, "{id}");
        }
        assert!(!r.is_blocked());
    }

    #[test]
    fn missing_autologon_warns_on_a_reboot_run() {
        // The deadman recovery task is a pure boot-delay timer, independent
        // of whether anyone logs back in, and the resume task's "at logon"
        // trigger fires on any logon (manual or automatic) -- so a missing
        // AutoLogon only costs the user a manual login after each reboot,
        // not a stalled/unrecoverable run. See this check's own message.
        let mut f = reboot_facts();
        f.logon.as_mut().unwrap().auto_admin_logon = false;
        let r = evaluate(&f);
        assert_eq!(status_of(&r, "autologon").status, CheckStatus::Warn);
        assert!(!r.is_blocked());
    }

    #[test]
    fn microsoft_account_with_autologon_warns_and_plain_text_password_warns() {
        let mut f = reboot_facts();
        f.logon.as_mut().unwrap().microsoft_account = true;
        f.logon.as_mut().unwrap().default_password_present = true;
        let r = evaluate(&f);
        assert_eq!(status_of(&r, "autologon").status, CheckStatus::Warn);
        assert_eq!(status_of(&r, "default_password").status, CheckStatus::Warn);
    }

    #[test]
    fn bitlocker_on_and_secure_boot_on_warn_only() {
        let mut f = reboot_facts();
        f.bitlocker = Some(crate::system::BitlockerStatus::On);
        f.secure_boot = Some(Some(true));
        let r = evaluate(&f);
        assert_eq!(status_of(&r, "bitlocker").status, CheckStatus::Warn);
        assert_eq!(status_of(&r, "secure_boot").status, CheckStatus::Warn);
        assert!(!r.is_blocked());
    }

    #[test]
    fn reboot_checks_pass_with_an_explanation_when_there_are_no_reboot_scenarios() {
        let r = evaluate(&PreflightFacts::all_ok());
        let c = status_of(&r, "autologon");
        assert_eq!(c.status, CheckStatus::Pass);
        assert!(c.detail.contains("no reboot scenarios"));
    }

    #[test]
    fn registry_noop_warns_and_blocks_when_it_would_burn_a_reboot() {
        let mut f = PreflightFacts::all_ok();
        f.noop_registry_scenarios = vec![("Scenario A".into(), false)];
        assert_eq!(
            status_of(&evaluate(&f), "registry_noop_vs_active").status,
            CheckStatus::Warn
        );
        f.noop_registry_scenarios = vec![("HAGS on".into(), true)];
        let r = evaluate(&f);
        assert_eq!(
            status_of(&r, "registry_noop_vs_active").status,
            CheckStatus::Block
        );
        assert!(
            status_of(&r, "registry_noop_vs_active")
                .detail
                .contains("HAGS on")
        );
    }

    #[test]
    fn anticheat_and_thermal_stay_deferred_to_m4() {
        let r = evaluate(&PreflightFacts::all_ok());
        for id in ["anticheat", "thermal"] {
            let c = status_of(&r, id);
            assert_eq!(c.status, CheckStatus::Deferred);
            assert!(c.detail.contains("M4"));
        }
        assert!(r.checks.iter().all(|c| {
            !["bitlocker", "secure_boot", "autologon"].contains(&c.id.as_str())
                || c.status != CheckStatus::Deferred
        }));
    }

    #[tokio::test]
    async fn aveyo_kind_never_reads_the_workshop_subscription_and_reports_it_as_not_needed() {
        let sys = crate::system::MockController::new()
            .with_steam_status(crate::system::SteamStatus {
                running: true,
                elevated: false,
            })
            .with_workshop_item_installed(false);
        let facts = gather_facts(&sys, None, 1, &[], crate::model::BenchmarkKind::AveYoCfgV2)
            .await
            .unwrap();
        assert_eq!(
            facts.workshop_map_subscribed, None,
            "AveYo projects don't use the Workshop map at all -- the fact is never read"
        );
        let report = evaluate(&facts);
        let check = status_of(&report, "workshop_map");
        assert!(
            matches!(check.status, CheckStatus::Pass),
            "the check must never block an AveYo run"
        );
        assert!(
            check.detail.contains("Not needed"),
            "must not claim a subscription nobody looked up: {}",
            check.detail
        );
    }

    #[tokio::test]
    async fn workshop_dust2_kind_still_reports_the_real_subscription_state() {
        let sys = crate::system::MockController::new()
            .with_steam_status(crate::system::SteamStatus {
                running: true,
                elevated: false,
            })
            .with_workshop_item_installed(false);
        let facts = gather_facts(
            &sys,
            None,
            1,
            &[],
            crate::model::BenchmarkKind::WorkshopDust2,
        )
        .await
        .unwrap();
        assert_eq!(facts.workshop_map_subscribed, Some(false));
    }
}
