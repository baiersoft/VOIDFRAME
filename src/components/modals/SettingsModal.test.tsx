import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";

const mockGetConfig = vi.fn();
const mockSaveConfig = vi.fn();
const mockGetCpuModel = vi.fn();
const mockGetGpuModel = vi.fn();
const mockOpenUrl = vi.fn();
vi.mock("../../lib/api", () => ({
  getConfig: (...args: unknown[]) => mockGetConfig(...args),
  saveConfig: (...args: unknown[]) => mockSaveConfig(...args),
  getCpuModel: (...args: unknown[]) => mockGetCpuModel(...args),
  getGpuModel: (...args: unknown[]) => mockGetGpuModel(...args),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: (...args: unknown[]) => mockOpenUrl(...args),
}));

import { SettingsModal } from "./SettingsModal";

const baseConfig = {
  presentmon_path: "C:\\Program Files\\voidframe\\PresentMon\\PresentMon.exe",
  hwinfo_path: "C:\\Program Files\\voidframe\\HWiNFO64\\HWiNFO64.exe",
  thermal_cooldown_enabled: true,
  dry_run_default: false,
  last_known_cs2_build_id: null,
  shutdown_when_complete_default: false,
  post_boot_settle_seconds: 180,
};

describe("SettingsModal", () => {
  beforeEach(() => {
    mockGetConfig.mockReset();
    mockSaveConfig.mockReset();
    mockGetCpuModel.mockReset();
    mockGetGpuModel.mockReset();
    mockOpenUrl.mockReset();
    mockGetCpuModel.mockResolvedValue("AMD Ryzen 7 9800X3D 8-Core Processor");
    mockGetGpuModel.mockResolvedValue("NVIDIA GeForce RTX 4090");
  });

  async function renderLoaded(overrides: Partial<typeof baseConfig> = {}) {
    mockGetConfig.mockResolvedValue({ ...baseConfig, ...overrides });
    render(<SettingsModal isOpen={true} onClose={vi.fn()} />);
    await waitFor(() =>
      expect(screen.getByRole("button", { name: /save preferences/i })).not.toBeDisabled()
    );
  }

  it("does not render editable PresentMon or HWiNFO path fields", async () => {
    await renderLoaded();
    expect(screen.queryByText(/presentmon executable path/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/hwinfo executable path/i)).not.toBeInTheDocument();
  });

  it("shows the backend's error on save failure", async () => {
    await renderLoaded();
    mockSaveConfig.mockRejectedValue(new Error("presentmon path could not be read"));
    fireEvent.click(screen.getByRole("button", { name: /save preferences/i }));
    await waitFor(() => {
      expect(screen.getByText(/presentmon path could not be read/i)).toBeInTheDocument();
    });
  });

  it("fetches and renders the real CPU and GPU model once, when opened", async () => {
    await renderLoaded();
    expect(screen.getByText("AMD Ryzen 7 9800X3D 8-Core Processor")).toBeInTheDocument();
    expect(screen.getByText("NVIDIA GeForce RTX 4090")).toBeInTheDocument();
    expect(mockGetCpuModel).toHaveBeenCalledTimes(1);
    expect(mockGetGpuModel).toHaveBeenCalledTimes(1);
  });

  it("shows the backend's error if hardware detection fails, without breaking config editing", async () => {
    mockGetCpuModel.mockRejectedValue(new Error("registry value ProcessorNameString is absent"));
    await renderLoaded();
    await waitFor(() => {
      expect(screen.getByText(/processornamestring is absent/i)).toBeInTheDocument();
    });
    expect(screen.getByLabelText(/post-boot settle/i)).toBeInTheDocument();
  });

  it("still shows a successful GPU read when the CPU read fails, and vice versa", async () => {
    mockGetCpuModel.mockRejectedValue(new Error("registry value ProcessorNameString is absent"));
    await renderLoaded();
    await waitFor(() => {
      expect(screen.getByText(/processornamestring is absent/i)).toBeInTheDocument();
      expect(screen.getByText("NVIDIA GeForce RTX 4090")).toBeInTheDocument();
    });
  });

  it("keeps the thermal cooldown toggle, defaulting to checked", async () => {
    await renderLoaded({ thermal_cooldown_enabled: true });
    const toggle = screen.getByRole("checkbox", { name: /thermal cooldown between iterations/i });
    expect(toggle).toBeChecked();
    expect(screen.queryByText(/requires the hwinfo path above/i)).not.toBeInTheDocument();
  });

  it("round-trips the shutdown-when-complete default and post-boot settle seconds into saveConfig", async () => {
    await renderLoaded({ shutdown_when_complete_default: false, post_boot_settle_seconds: 180 });
    const shutdownToggle = screen.getByRole("checkbox", { name: /shut down when a run completes/i });
    expect(shutdownToggle).not.toBeChecked();
    fireEvent.click(shutdownToggle);

    const settleInput = screen.getByLabelText(/post-boot settle/i);
    fireEvent.change(settleInput, { target: { value: "240" } });

    fireEvent.click(screen.getByRole("button", { name: /save preferences/i }));
    await waitFor(() => {
      expect(mockSaveConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          shutdown_when_complete_default: true,
          post_boot_settle_seconds: 240,
        })
      );
    });
  });

  it("shows HWiNFO's attribution link before PresentMon's, and opens the right URL", async () => {
    await renderLoaded();
    const links = screen.getAllByRole("button", { name: /hwinfo|presentmon/i });
    expect(links[0]).toHaveTextContent(/hwinfo/i);
    expect(links[1]).toHaveTextContent(/presentmon/i);

    fireEvent.click(screen.getByRole("button", { name: /hwinfo/i }));
    expect(mockOpenUrl).toHaveBeenCalledWith("https://www.hwinfo.com/");

    fireEvent.click(screen.getByRole("button", { name: /presentmon/i }));
    expect(mockOpenUrl).toHaveBeenCalledWith("https://www.presentmon.com/");
  });
});
