import { describe, expect, it } from "vitest";
import fixture from "./module-wire.fixture.json";
import type { Module } from "../bindings";

// The fixture is written by `model::module::tests::module_wire_fixture_is_current`
// (crates/voidframe-engine/src/model/module.rs) straight from serde. The JSON
// import types each `type` field as plain `string`, so this cast only widens
// that to the generated literal union — it asserts no new shape, and does not
// stand in for real validation. `_exhaustive` below is the actual type-level
// guard: it is typed as `Record<Module["type"], true>`, so the file fails to
// type-check if a variant is either added to or removed from `Module`
// without updating this object.
const modules = fixture as Module[];
const _exhaustive: Record<Module["type"], true> = {
  registry: true,
  powercfg: true,
  power_plan: true,
  affinity_cpu: true,
  launch_args: true,
  custom_script: true,
  cs2_config: true,
  unsupported: true,
};

describe("Module wire contract", () => {
  it("covers every variant the engine can emit", () => {
    const types = modules.map((m) => m.type);
    expect(types).toEqual([
      "registry",
      "powercfg",
      "power_plan",
      "affinity_cpu",
      "launch_args",
      "custom_script",
      "cs2_config",
      "unsupported",
    ]);
  });

  it("narrows by the `type` discriminant without casts", () => {
    const registry = modules.find((m) => m.type === "registry");
    expect(registry && registry.type === "registry" ? registry.value_name : null).toBe("HwSchMode");
    const launch = modules.find((m) => m.type === "launch_args");
    expect(launch && launch.type === "launch_args" ? launch.args : null).toBe("-novid -high");
  });
});
