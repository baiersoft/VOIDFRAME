import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";

const mockListCatalogTweaks = vi.fn();
const mockListPowerPlans = vi.fn();
const mockReadCs2LaunchOptions = vi.fn();
const mockReadCs2VideoConfig = vi.fn();
const mockReadRegistryValue = vi.fn();
const mockGetConfig = vi.fn();
const mockSaveConfig = vi.fn();
vi.mock("../../lib/api", () => ({
  listCatalogTweaks: (...args: unknown[]) => mockListCatalogTweaks(...args),
  listPowerPlans: (...args: unknown[]) => mockListPowerPlans(...args),
  readCs2LaunchOptions: (...args: unknown[]) => mockReadCs2LaunchOptions(...args),
  readCs2VideoConfig: (...args: unknown[]) => mockReadCs2VideoConfig(...args),
  readRegistryValue: (...args: unknown[]) => mockReadRegistryValue(...args),
  getConfig: (...args: unknown[]) => mockGetConfig(...args),
  saveConfig: (...args: unknown[]) => mockSaveConfig(...args),
}));

import { TweakCatalog } from "./TweakCatalog";

const HAGS_ENTRY = {
  id: "hags",
  name: "Hardware-Accelerated GPU Scheduling (HAGS)",
  category: "gpu",
  kind: "registry",
  description: "Turns HAGS on.",
  available_in: "M3",
  module_template: {
    type: "registry",
    hive: "HKLM",
    subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
    value_name: "HwSchMode",
    value_type: "DWORD",
    value: 2,
    requires_reboot: true,
  },
};

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

const CS2_CONFIG_ENTRY = {
  id: "cs2-config",
  name: "CS2 Video Config",
  category: "cs2_launch",
  kind: "cs2_config",
  description: "Edits CS2's video settings for this scenario.",
  available_in: "M3",
  singleton: true,
  module_template: { type: "cs2_config", settings: {} },
};

const CUSTOM_SCRIPT_ENTRY = {
  id: "custom-script",
  name: "Custom Script",
  category: "advanced",
  kind: "custom_script",
  description: "Runs your own apply/revert scripts for this scenario.",
  available_in: "M3",
  module_template: {
    type: "custom_script",
    apply_script: "",
    revert_script: "",
    requires_reboot: false,
    description: "",
  },
};

const BASE_CONFIG = {
  presentmon_path: "C:\\PresentMon.exe",
  dry_run_default: false,
  last_known_cs2_build_id: null,
  hwinfo_path: null,
  thermal_cooldown_enabled: true,
  shutdown_when_complete_default: false,
  post_boot_settle_seconds: 180,
  custom_script_warning_seen: false,
};

describe("TweakCatalog", () => {
  beforeEach(() => {
    mockListCatalogTweaks.mockReset();
    mockReadRegistryValue.mockReset();
    mockGetConfig.mockReset().mockResolvedValue(BASE_CONFIG);
    mockSaveConfig.mockReset().mockResolvedValue(undefined);
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
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={onSelectTweak} projectId="proj1" />);
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
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={onSelectTweak} currentModules={[]} projectId="proj1" />);
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
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={onSelectTweak} currentModules={[]} projectId="proj1" />);
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

  it("clicking Add Module for the cs2-config entry fetches the live current video settings and pre-fills the editor with them", async () => {
    mockListCatalogTweaks.mockResolvedValue([CS2_CONFIG_ENTRY]);
    mockReadCs2VideoConfig.mockResolvedValue({ "setting.mat_vsync": "1" });
    const onSelectTweak = vi.fn();
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={onSelectTweak} currentModules={[]} projectId="proj1" />);
    await waitFor(() => screen.getByText("CS2 Video Config"));
    fireEvent.click(screen.getByRole("button", { name: /add module/i }));
    expect(onSelectTweak).not.toHaveBeenCalled();
    await waitFor(() => expect(screen.getByLabelText("V-Sync")).toHaveValue("Enabled"));

    fireEvent.click(screen.getByRole("button", { name: /save/i }));
    expect(onSelectTweak).toHaveBeenCalledWith({
      type: "cs2_config",
      settings: { "setting.mat_vsync": "1" },
    });
  });

  it("shows a one-time warning before the first custom_script add, blocking the editor until acknowledged", async () => {
    mockListCatalogTweaks.mockResolvedValue([CUSTOM_SCRIPT_ENTRY]);
    mockGetConfig.mockResolvedValue({ ...BASE_CONFIG, custom_script_warning_seen: false });
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={vi.fn()} currentModules={[]} projectId="proj1" />);
    await waitFor(() => screen.getByText("Custom Script"));
    fireEvent.click(screen.getByRole("button", { name: /add module/i }));

    await waitFor(() => {
      expect(screen.getByText(/Custom Scripts Run Elevated/i)).toBeInTheDocument();
    });
    expect(screen.queryByRole("button", { name: /browse for apply script/i })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /got it/i }));

    await waitFor(() => {
      expect(mockSaveConfig).toHaveBeenCalledWith(
        expect.objectContaining({ custom_script_warning_seen: true })
      );
    });
    await waitFor(() => {
      expect(screen.getByRole("button", { name: /browse for apply script/i })).toBeInTheDocument();
    });
    expect(screen.queryByText(/Custom Scripts Run Elevated/i)).not.toBeInTheDocument();
  });

  it("opens the custom_script editor directly, with no warning, once the warning has already been seen", async () => {
    mockListCatalogTweaks.mockResolvedValue([CUSTOM_SCRIPT_ENTRY]);
    mockGetConfig.mockResolvedValue({ ...BASE_CONFIG, custom_script_warning_seen: true });
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={vi.fn()} currentModules={[]} projectId="proj1" />);
    await waitFor(() => screen.getByText("Custom Script"));
    fireEvent.click(screen.getByRole("button", { name: /add module/i }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /browse for apply script/i })).toBeInTheDocument();
    });
    expect(screen.queryByText(/Custom Scripts Run Elevated/i)).not.toBeInTheDocument();
    expect(mockSaveConfig).not.toHaveBeenCalled();
  });

  it("disables Add Module for a singleton entry whose kind is already present in the target scenario", async () => {
    mockListCatalogTweaks.mockResolvedValue([POWER_PLAN_ENTRY]);
    render(
      <TweakCatalog
        isOpen={true}
        onClose={vi.fn()}
        onSelectTweak={vi.fn()}
        currentModules={[{ type: "power_plan", plan_guid: "balanced-guid" }] as never}
        projectId="proj1"
      />
    );
    await waitFor(() => screen.getByText("Power Plan"));
    expect(screen.getByRole("button", { name: /add module/i })).toBeDisabled();
  });

  it("reads the live registry value for a registry entry and shows 'currently ON' with Add disabled when it matches the template", async () => {
    mockListCatalogTweaks.mockResolvedValue([HAGS_ENTRY]);
    mockReadRegistryValue.mockResolvedValue({ present: true, value: { type: "DWORD", value: 2 } });
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={vi.fn()} currentModules={[]} projectId="proj1" />);
    await waitFor(() => screen.getByText(/hardware-accelerated gpu scheduling/i));
    expect(mockReadRegistryValue).toHaveBeenCalledWith(
      "HKLM",
      "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
      "HwSchMode"
    );
    await waitFor(() => {
      expect(screen.getByText(/currently on/i)).toBeInTheDocument();
    });
    expect(screen.getByRole("button", { name: /add module/i })).toBeDisabled();
  });

  it("offers the OFF value instead of disabling when a registry entry with off_value is currently ON", async () => {
    const hagsWithOffValue = { ...HAGS_ENTRY, off_value: 1 };
    mockListCatalogTweaks.mockResolvedValue([hagsWithOffValue]);
    mockReadRegistryValue.mockResolvedValue({ present: true, value: { type: "DWORD", value: 2 } });
    const onSelectTweak = vi.fn();
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={onSelectTweak} currentModules={[]} projectId="proj1" />);
    await waitFor(() => screen.getByText(/hardware-accelerated gpu scheduling/i));
    await waitFor(() => {
      expect(screen.getByText(/currently on.*turn it off/i)).toBeInTheDocument();
    });
    const addButton = screen.getByRole("button", { name: /add module/i });
    expect(addButton).not.toBeDisabled();

    fireEvent.click(addButton);
    expect(onSelectTweak).toHaveBeenCalledWith(
      expect.objectContaining({ type: "registry", value: 1 })
    );
  });

  it("leaves Add Module enabled and shows no 'currently ON' hint when the live registry value is absent", async () => {
    mockListCatalogTweaks.mockResolvedValue([HAGS_ENTRY]);
    mockReadRegistryValue.mockResolvedValue({ present: false, value: null });
    render(<TweakCatalog isOpen={true} onClose={vi.fn()} onSelectTweak={vi.fn()} currentModules={[]} projectId="proj1" />);
    await waitFor(() => screen.getByText(/hardware-accelerated gpu scheduling/i));
    await waitFor(() => expect(mockReadRegistryValue).toHaveBeenCalled());
    expect(screen.queryByText(/currently on/i)).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: /add module/i })).not.toBeDisabled();
  });
});
