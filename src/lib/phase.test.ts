import { describe, expect, it } from "vitest";
import { canOperatorControl, phaseLabel, phaseScenarioId } from "./phase";

describe("canOperatorControl", () => {
  it("disables controls for phases past the point of no return", () => {
    expect(canOperatorControl({ kind: "aborting" })).toBe(false);
    expect(canOperatorControl({ kind: "rollback" })).toBe(false);
    expect(canOperatorControl({ kind: "report" })).toBe(false);
    expect(canOperatorControl({ kind: "reboot_pending", reason: "apply_next" })).toBe(false);
  });
  it("enables controls for cancellable phases and before the first phase", () => {
    expect(canOperatorControl(null)).toBe(true);
    expect(canOperatorControl({ kind: "preflight" })).toBe(true);
    expect(canOperatorControl({ kind: "thermal_baseline" })).toBe(true);
    expect(canOperatorControl({ kind: "scenario", id: "s1" })).toBe(true);
  });
});

describe("phase helpers", () => {
  it("labels every phase the way the engine's Display impl does", () => {
    expect(phaseLabel({ kind: "thermal_baseline" })).toBe("thermal_baseline");
    expect(phaseLabel({ kind: "scenario", id: "s1" })).toBe("scenario:s1");
  });
  it("labels the aborting phase", () => {
    expect(phaseLabel({ kind: "aborting" })).toBe("aborting");
  });
  it("derives the current scenario like Phase::scenario_id", () => {
    expect(phaseScenarioId({ kind: "baseline" })).toBe("baseline");
    expect(phaseScenarioId({ kind: "scenario", id: "s1" })).toBe("s1");
    expect(phaseScenarioId({ kind: "rollback" })).toBeNull();
  });
});
