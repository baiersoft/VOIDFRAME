import { describe, expect, it } from "vitest";
import { phaseLabel, phaseScenarioId } from "./phase";

describe("phase helpers", () => {
  it("labels every phase the way the engine's Display impl does", () => {
    expect(phaseLabel({ kind: "thermal_baseline" })).toBe("thermal_baseline");
    expect(phaseLabel({ kind: "scenario", id: "s1" })).toBe("scenario:s1");
  });
  it("derives the current scenario like Phase::scenario_id", () => {
    expect(phaseScenarioId({ kind: "baseline" })).toBe("baseline");
    expect(phaseScenarioId({ kind: "scenario", id: "s1" })).toBe("s1");
    expect(phaseScenarioId({ kind: "rollback" })).toBeNull();
  });
});
