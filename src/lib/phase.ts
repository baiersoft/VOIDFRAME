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
    case "baseline":
      return "baseline";
    case "scenario":
      return `scenario:${phase.id}`;
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
