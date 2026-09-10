//! Turns a [`Module`] into `SystemController` calls, journaling the inverse
//! before applying, and reverts a journal record back through the same
//! `SystemController`.

pub mod affinity_cpu;
pub mod cs2_config;
pub mod custom_script;
pub mod power_plan;
pub mod powercfg;
pub mod registry;

use crate::error::{Error, Result};
use crate::journal::replay::{RevertReport, revert_all};
use crate::journal::{Journal, JournalRecord, Op};
use crate::model::catalog::load_catalog;
use crate::model::module::Module;
use crate::model::project::Scenario;
use crate::system::{MutationCtx, SystemController};
use std::path::Path;

/// `Err(Error::Conflict)` if two modules in the same scenario target the same
/// registry value, or if two modules share a `Module::kind()` the catalog
/// marks `singleton: true` for (e.g. two `power_plan` modules -- a scenario
/// targets exactly one power plan, so a second one is never meaningful).
pub fn conflict_check(modules: &[Module]) -> Result<()> {
    let mut seen = std::collections::HashSet::new();
    for m in modules {
        if let Some(t) = m.registry_target()
            && !seen.insert(t.clone())
        {
            return Err(Error::conflict(format!("two modules target {t}")));
        }
    }

    // The catalog (not a hardcoded list here) is the single source of truth
    // for which module kinds are singleton, so this can't drift out of sync
    // with what the frontend's "Add Module" step allows.
    let singleton_kinds: std::collections::HashSet<String> = load_catalog()?
        .into_iter()
        .filter(|e| e.singleton)
        .map(|e| e.kind)
        .collect();
    let mut seen_kinds = std::collections::HashSet::new();
    for m in modules {
        let kind = m.kind();
        if singleton_kinds.contains(kind) && !seen_kinds.insert(kind) {
            return Err(Error::conflict(format!(
                "two modules of kind {kind} in the same scenario (at most one allowed)"
            )));
        }
    }
    Ok(())
}

/// Apply one module: journal the mutation (with its computed inverse), call
/// the matching `SystemController` method, then confirm it in the journal.
/// `AffinityCpu` / `LaunchArgs` are no-ops here — they take effect at CS2
/// launch (`docs/superpowers/plans/2026-09-01-m1-phase-3b-cs2-control-run-loop.md`),
/// not as a persisted system mutation.
///
/// `no_return` is the run's "point of no return" shield
/// (`run::execute::context::RunContext::no_return`), forwarded only to
/// [`power_plan::apply`] -- the one apply path whose OS call resolves
/// *before* its journal record can exist, so it needs the cancellation
/// window around that call closed. The Registry/Powercfg arms journal
/// before they apply and need no shield. Callers with no live run to
/// shield (dry-run, replay, tests) pass a throwaway channel's sender.
pub async fn apply_module(
    m: &Module,
    project_dir: &Path,
    sys: &dyn SystemController,
    journal: &mut Journal,
    ctx: &MutationCtx,
    no_return: &tokio::sync::watch::Sender<bool>,
) -> Result<()> {
    match m {
        Module::Registry(p) => registry::apply(p, sys, journal, ctx).await,
        Module::Powercfg {
            sub,
            setting,
            value,
        } => powercfg::apply(sub, setting, *value, sys, journal, ctx).await,
        Module::PowerPlan(p) => power_plan::apply(p, sys, journal, ctx, no_return).await,
        Module::CustomScript(p) => custom_script::apply(p, project_dir, sys, journal, ctx).await,
        Module::Cs2Config(p) => cs2_config::apply(p, sys, journal, ctx).await,
        Module::AffinityCpu(_) | Module::LaunchArgs { .. } => Ok(()),
        Module::Unsupported => Err(Error::unsupported_module(
            "unsupported".into(),
            "milestone M3".into(),
        )),
    }
}

/// Undo one journal record via the matching `SystemController` method.
pub async fn revert_record(
    rec: &JournalRecord,
    project_dir: &Path,
    sys: &dyn SystemController,
    ctx: &MutationCtx,
) -> Result<()> {
    match rec.op {
        Op::RegistryWrite | Op::RegistryDelete => registry::revert(rec, sys, ctx).await,
        Op::PowercfgWrite => powercfg::revert(rec, sys, ctx).await,
        Op::PowerPlanActivate | Op::PowerPlanCreate | Op::PowerPlanDelete => {
            power_plan::revert(rec, sys, ctx).await
        }
        Op::CustomScriptApply => custom_script::revert(rec, project_dir, sys, ctx).await,
        Op::Cs2ConfigApply => cs2_config::revert(rec, sys, ctx).await,
    }
}

/// Apply every module in a scenario, in array order, after a conflict check.
/// Stops (and returns the error) at the first module that fails to apply —
/// the caller is expected to revert whatever landed via [`revert_scenario`].
pub async fn apply_scenario(
    sc: &Scenario,
    run_id: &str,
    project_dir: &Path,
    sys: &dyn SystemController,
    journal: &mut Journal,
    no_return: &tokio::sync::watch::Sender<bool>,
) -> Result<()> {
    conflict_check(&sc.modules)?;
    for (i, m) in sc.modules.iter().enumerate() {
        m.require_m1_supported()?;
        let ctx = MutationCtx {
            run_id: run_id.to_string(),
            scenario_id: sc.id.clone(),
            step_index: i as u32,
        };
        apply_module(m, project_dir, sys, journal, &ctx, no_return).await?;
    }
    Ok(())
}

/// Reverse-replay a scenario's journal. One journal file per scenario is
/// used (`docs/superpowers/plans/2026-09-01-m1-phase-1-engine-foundation.md`),
/// opened, applied, and reverted before the next scenario starts
/// (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1), so reverting the whole file is equivalent to reverting this
/// scenario. A later phase may move to per-scenario segments within a single
/// run-long journal; `scenario_id` is accepted now so that later change is
/// call-site-transparent.
pub async fn revert_scenario(
    _scenario_id: &str,
    project_dir: &Path,
    journal_path: &Path,
    sys: &dyn SystemController,
) -> Result<RevertReport> {
    revert_all(project_dir, journal_path, sys).await
}

#[cfg(test)]
mod conflict_check_singleton_tests {
    use super::conflict_check;
    use crate::model::module::{Hive, Module, PowerPlanPayload, RegType, RegistryPayload};

    fn power_plan(guid: &str) -> Module {
        Module::PowerPlan(PowerPlanPayload {
            plan_guid: guid.into(),
            friendly_name: None,
            create_if_missing: false,
        })
    }

    fn registry_module() -> Module {
        Module::Registry(RegistryPayload {
            hive: Hive::Hklm,
            subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers".into(),
            value_name: "HwSchMode".into(),
            value_type: RegType::Dword,
            value: serde_json::json!(2),
            requires_reboot: false,
        })
    }

    #[test]
    fn rejects_two_power_plan_modules_in_the_same_scenario() {
        let e = conflict_check(&[power_plan("a"), power_plan("b")]).unwrap_err();
        assert!(e.to_string().to_lowercase().contains("conflict"));
    }

    #[test]
    fn allows_one_power_plan_plus_a_non_singleton_module() {
        conflict_check(&[power_plan("a"), registry_module()]).unwrap();
    }
}
