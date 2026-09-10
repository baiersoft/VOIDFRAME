import type { Scenario } from "./bindings";

/** Mirrors `Scenario::requires_reboot` in the engine: every module kind that
 * carries a `requires_reboot` flag (`registry`, `custom_script`) counts. */
export function scenarioRequiresReboot(s: Scenario): boolean {
  return (s.modules ?? []).some(
    (m) => (m.type === "registry" || m.type === "custom_script") && m.requires_reboot === true
  );
}

type Transition = "none" | "reboot_then_continue" | "apply_next_then_reboot";

function planTransition(prev: boolean, next: boolean | null): Transition {
  if (next === true) return "apply_next_then_reboot";
  if (prev) return "reboot_then_continue";
  return "none";
}

/** Mirrors crates/voidframe-engine/src/run/transition.rs::count_reboots. */
export function countReboots(needs: boolean[]): number {
  let reboots = 0;
  let prev = false;
  needs.forEach((need, i) => {
    if (i === 0 && need) reboots += 1;
    else if (i > 0 && planTransition(prev, need) !== "none") reboots += 1;
    prev = need;
  });
  if (prev) reboots += 1;
  return reboots;
}

export function estimateSeconds(runs: number, captureSeconds: number, reboots: number, settleSeconds: number): number {
  return runs * (captureSeconds + 15) + reboots * (settleSeconds + 90);
}
