import { describe, expect, it } from "vitest";
import { countReboots, estimateSeconds, scenarioRequiresReboot } from "./reboots";
import type { Scenario } from "./bindings";

const hags = (id: string): Scenario => ({
  id, name: id, description: "", enabled: true,
  modules: [{ type: "registry", hive: "HKLM", subkey: "SYSTEM\\X", value_name: "HwSchMode", value_type: "DWORD", value: 2, requires_reboot: true }],
});
const plain = (id: string): Scenario => ({
  id, name: id, description: "", enabled: true,
  modules: [{ type: "powercfg", sub: "sub_processor", setting: "IDLEDISABLE", value: 1 }],
});

const script = (id: string, requires_reboot: boolean): Scenario => ({
  id, name: id, description: "", enabled: true,
  modules: [{ type: "custom_script", apply_script: "apply.bat", revert_script: "revert.bat", requires_reboot, description: "" }],
});

describe("reboots", () => {
  it("detects reboot scenarios", () => {
    expect(scenarioRequiresReboot(hags("a"))).toBe(true);
    expect(scenarioRequiresReboot(plain("p"))).toBe(false);
  });
  it("counts a custom_script module's requires_reboot like the engine does", () => {
    expect(scenarioRequiresReboot(script("s", true))).toBe(true);
    expect(scenarioRequiresReboot(script("s", false))).toBe(false);
  });
  it("counts reboots like the engine", () => {
    expect(countReboots([false, true, false, true, true, false])).toBe(5);
    expect(countReboots([false, false, false])).toBe(0);
    expect(countReboots([false, true])).toBe(2);
    expect(countReboots([false, true, true])).toBe(3);
  });
  it("estimates wall clock with reboots and settle", () => {
    expect(estimateSeconds(4, 105, 2, 180)).toBe(4 * 120 + 2 * 270);
  });
});
