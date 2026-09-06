import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";

const mockPreflight = vi.fn();
vi.mock("../../lib/api", () => ({
  preflight: (...args: unknown[]) => mockPreflight(...args),
}));

import { PreflightModal } from "./PreflightModal";

describe("PreflightModal", () => {
  beforeEach(() => {
    mockPreflight.mockReset();
  });

  it("renders real checks from the backend, with status-appropriate styling", async () => {
    mockPreflight.mockResolvedValue({
      checks: [
        { id: "steam", status: "pass", detail: "Steam is running, logged in, and not elevated" },
        { id: "cs2_build", status: "warn", detail: "CS2 build changed since last run" },
        { id: "workshop_map", status: "block", detail: "Subscribe to workshop map 3240880604" },
        { id: "bitlocker", status: "deferred", detail: "BitLocker recovery-prompt risk — evaluated in milestone M3" },
      ],
    });
    render(<PreflightModal isOpen={true} onClose={vi.fn()} projectId={null} />);
    await waitFor(() => {
      expect(screen.getByText(/steam is running, logged in/i)).toBeInTheDocument();
    });
    expect(screen.getByText(/cs2 build changed/i)).toBeInTheDocument();
    expect(screen.getByText(/subscribe to workshop map/i)).toBeInTheDocument();
    expect(screen.getByText(/bitlocker recovery-prompt risk/i)).toBeInTheDocument();

    // Status-appropriate styling: assert the visible badge label for each status.
    expect(screen.getByText("PASS")).toBeInTheDocument();
    expect(screen.getByText("WARN")).toBeInTheDocument();
    expect(screen.getByText("BLOCKED")).toBeInTheDocument();
    expect(screen.getByText("M3")).toBeInTheDocument();
  });

  it("shows a blocked-run banner when any check is status=block, not a false all-clear", async () => {
    mockPreflight.mockResolvedValue({
      checks: [{ id: "disk", status: "block", detail: "Not enough free disk space for run artifacts" }],
    });
    render(<PreflightModal isOpen={true} onClose={vi.fn()} projectId={null} />);
    await waitFor(() => {
      expect(screen.getByText(/not enough free disk space/i)).toBeInTheDocument();
    });
    expect(screen.queryByText(/all .* passed/i)).not.toBeInTheDocument();
  });

  it("shows a warn-only footer (not pass, not block) when only warn checks are present", async () => {
    mockPreflight.mockResolvedValue({
      checks: [{ id: "cs2_build", status: "warn", detail: "CS2 build changed since last run" }],
    });
    render(<PreflightModal isOpen={true} onClose={vi.fn()} projectId={null} />);
    await waitFor(() => {
      expect(screen.getByText(/warnings present/i)).toBeInTheDocument();
    });
    expect(screen.queryByText(/all .* passed/i)).not.toBeInTheDocument();
    expect(screen.queryByText(/run is blocked/i)).not.toBeInTheDocument();
  });

  it("does not show a false all-clear footer when preflight() itself rejects", async () => {
    mockPreflight.mockRejectedValue(new Error("Failed to invoke preflight command"));
    render(<PreflightModal isOpen={true} onClose={vi.fn()} projectId={null} />);
    await waitFor(() => {
      expect(screen.getByText(/failed to invoke preflight command/i)).toBeInTheDocument();
    });
    expect(screen.queryByText(/all .* passed/i)).not.toBeInTheDocument();
  });

  it("passes the selected project's id through to preflight() so the power-plan-noop check has scenarios to compare", async () => {
    mockPreflight.mockResolvedValue({ checks: [] });
    render(<PreflightModal isOpen={true} onClose={vi.fn()} projectId="p1" />);
    await waitFor(() => {
      expect(mockPreflight).toHaveBeenCalledWith("p1");
    });
  });

  it("passes null to preflight() when no project is selected", async () => {
    mockPreflight.mockResolvedValue({ checks: [] });
    render(<PreflightModal isOpen={true} onClose={vi.fn()} projectId={null} />);
    await waitFor(() => {
      expect(mockPreflight).toHaveBeenCalledWith(null);
    });
  });
});
