import type { Phase } from "./bindings";

/** Human-readable label for the run monitor heading. */
export function phaseLabel(phase: Phase): string {
  switch (phase.kind) {
    case "preflight":
      return "preflight";
    case "snapshot":
      return "snapshot";
    case "thermal_baseline":
      return "thermal_baseline";
    case "thermal_cooldown":
      return "thermal_cooldown";
    case "baseline":
      return "baseline";
    case "scenario":
      return `scenario:${phase.id}`;
    case "aborting":
      return "aborting";
    case "reboot_pending":
      return `reboot_pending:${phase.reason}`;
    case "boot_resume":
      return "boot_resume";
    case "rollback":
      return "rollback";
    case "report":
      return "report";
    default: {
      const _exhaustive: never = phase;
      return String(_exhaustive);
    }
  }
}

/** Mirrors `Phase::scenario_id` in crates/voidframe-engine/src/run/phase.rs. */
export function phaseScenarioId(phase: Phase): string | null {
  if (phase.kind === "baseline") return "baseline";
  if (phase.kind === "scenario") return phase.id;
  return null;
}

/** Whether the operator control row (Pause/Abort, shutdown toggle) is
 * meaningful in this phase. Mirrors the engine's no-return rule: once a
 * reboot is initiated or cleanup/finalization is underway, an Abort can no
 * longer cancel anything. `null` = run starting, controls available. */
export function canOperatorControl(phase: Phase | null): boolean {
  if (phase === null) return true;
  switch (phase.kind) {
    case "aborting":
    case "rollback":
    case "report":
    case "reboot_pending":
      return false;
    default:
      return true;
  }
}
