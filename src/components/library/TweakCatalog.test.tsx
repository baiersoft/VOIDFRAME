import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";

const mockListCatalogTweaks = vi.fn();
const mockListPowerPlans = vi.fn();
const mockReadCs2LaunchOptions = vi.fn();
vi.mock("../../lib/api", () => ({
  listCatalogTweaks: (...args: unknown[]) => mockListCatalogTweaks(...args),
  listPowerPlans: (...args: unknown[]) => mockListPowerPlans(...args),
  readCs2LaunchOptions: (...args: unknown[]) => mockReadCs2LaunchOptions(...args),
}));

import { TweakCatalog } from "./TweakCatalog";

const POWER_PLAN_ENTRY = {
  id: "power-plan",
  name: "Power Plan",
  category: "cpu",
  kind: "power_plan",
  description: "Switches the active Windows power plan for this scenario.",
  available_in: "M1",
  singleton: true,
  module_template: { type: "power_plan", plan_guid: "", create_if_missing: false },
};

const LAUNCH_OPTIONS_ENTRY = {
  id: "launch-options",
  name: "Launch Options",
  category: "cs2_launch",
  kind: "launch_args",
  description: "Edits CS2's launch options for this scenario.",
  available_in: "M1",
  singleton: true,
  module_template: { type: "launch_args", args: "" },
};

describe("TweakCatalog", () => {
  beforeEach(() => {
    mockListCatalogTweaks.mockReset();
  });

  it("renders real catalog entries and passes the real module_template through on add", async () => {
    mockListCatalogTweaks.mockResolvedValue([
      {
        id: "core-parking-disable",
        name: "Disable Core Parking",
        category: "cpu",
        kind: "powercfg",
        description: "Sets CPMINCORES to 100%.",
        available_in: "M1",
        module_template: { type: "powercfg", sub: "sub_processor", setting: "CPMINCORES", value: 100 },
      },
    ]);
    const onSelectTweak = vi.fn();
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={onSelectTweak} />);
    await waitFor(() => {
      expect(screen.getByText("Disable Core Parking")).toBeInTheDocument();
    });
    fireEvent.click(screen.getByRole("button", { name: /add module/i }));
    expect(onSelectTweak).toHaveBeenCalledWith({
      type: "powercfg",
      sub: "sub_processor",
      setting: "CPMINCORES",
      value: 100,
    });
  });

  it("clicking Add Module for the power-plan entry opens a live picker instead of adding immediately", async () => {
    mockListCatalogTweaks.mockResolvedValue([POWER_PLAN_ENTRY]);
    mockListPowerPlans.mockResolvedValue([
      { guid: "balanced-guid", name: "Balanced", active: true },
      { guid: "high-perf-guid", name: "High performance", active: false },
    ]);
    const onSelectTweak = vi.fn();
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={onSelectTweak} currentModules={[]} />);
    await waitFor(() => screen.getByText("Power Plan"));
    fireEvent.click(screen.getByRole("button", { name: /add module/i }));
    expect(onSelectTweak).not.toHaveBeenCalled();
    await waitFor(() => screen.getByText("High performance"));

    fireEvent.click(screen.getByRole("button", { name: /high performance/i }));
    expect(onSelectTweak).toHaveBeenCalledWith({
      type: "power_plan",
      plan_guid: "high-perf-guid",
      friendly_name: "High performance",
      create_if_missing: false,
    });
  });

  it("clicking Add Module for the launch-options entry fetches the live current options and opens an editor pre-filled with them", async () => {
    mockListCatalogTweaks.mockResolvedValue([LAUNCH_OPTIONS_ENTRY]);
    mockReadCs2LaunchOptions.mockResolvedValue("-high -threads 8");
    const onSelectTweak = vi.fn();
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={onSelectTweak} currentModules={[]} />);
    await waitFor(() => screen.getByText("Launch Options"));
    fireEvent.click(screen.getByRole("button", { name: /add module/i }));
    expect(onSelectTweak).not.toHaveBeenCalled();
    const textarea = () => screen.getByRole("textbox", { name: /launch options/i });
    await waitFor(() => expect(textarea()).toHaveValue("-high -threads 8"));

    fireEvent.change(textarea(), { target: { value: "-high -threads 8 -novid" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(onSelectTweak).toHaveBeenCalledWith({
      type: "launch_args",
      args: "-high -threads 8 -novid",
    });
  });

  it("disables Add Module for a singleton entry whose kind is already present in the target scenario", async () => {
    mockListCatalogTweaks.mockResolvedValue([POWER_PLAN_ENTRY]);
    render(
      <TweakCatalog
        isOpen={true}
        onClose={vi.fn()}
        onSelectTweak={vi.fn()}
        currentModules={[{ type: "power_plan", plan_guid: "balanced-guid" }] as never}
      />
    );
    await waitFor(() => screen.getByText("Power Plan"));
    expect(screen.getByRole("button", { name: /add module/i })).toBeDisabled();
  });
});
