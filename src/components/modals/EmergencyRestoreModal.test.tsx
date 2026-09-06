import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";

const mockEmergencyRollback = vi.fn();
const mockOpenDataDir = vi.fn();
const mockRevealRestoreBat = vi.fn();
vi.mock("../../lib/api", () => ({
  emergencyRollback: (...args: unknown[]) => mockEmergencyRollback(...args),
  openDataDir: (...args: unknown[]) => mockOpenDataDir(...args),
  revealRestoreBat: (...args: unknown[]) => mockRevealRestoreBat(...args),
}));

import { EmergencyRestoreModal } from "./EmergencyRestoreModal";

describe("EmergencyRestoreModal", () => {
  beforeEach(() => {
    mockEmergencyRollback.mockReset();
    mockOpenDataDir.mockReset();
    mockRevealRestoreBat.mockReset();
  });

  it("does not call emergencyRollback until the user confirms", () => {
    render(<EmergencyRestoreModal isOpen={true} onClose={vi.fn()} />);
    expect(mockEmergencyRollback).not.toHaveBeenCalled();
    expect(screen.getByText(/execute emergency restore/i)).toBeInTheDocument();
  });

  it("on confirm, calls emergencyRollback and renders each real RevertReport", async () => {
    mockEmergencyRollback.mockResolvedValue([
      { reverted: 3, verify_failures: [] },
      { reverted: 1, verify_failures: ["HKLM\\...\\Foo did not verify"] },
    ]);
    render(<EmergencyRestoreModal isOpen={true} onClose={vi.fn()} />);
    fireEvent.click(screen.getByText(/execute emergency restore/i));
    await waitFor(() => {
      expect(screen.getByText(/3 mutation\(s\) reverted/i)).toBeInTheDocument();
    });
    expect(screen.getByText(/1 mutation\(s\) reverted/i)).toBeInTheDocument();
    expect(screen.getByText(/did not verify/i)).toBeInTheDocument();
  });

  it("shows a real error, not a silent failure, when emergencyRollback rejects", async () => {
    mockEmergencyRollback.mockRejectedValue(new Error("no runs found to roll back"));
    render(<EmergencyRestoreModal isOpen={true} onClose={vi.fn()} />);
    fireEvent.click(screen.getByText(/execute emergency restore/i));
    await waitFor(() => {
      expect(screen.getByText(/no runs found to roll back/i)).toBeInTheDocument();
    });
  });

  it("shows a visible error, not a silent failure, when openDataDir rejects", async () => {
    mockOpenDataDir.mockRejectedValue(new Error("data directory not found"));
    render(<EmergencyRestoreModal isOpen={true} onClose={vi.fn()} />);
    fireEvent.click(screen.getByText(/open data folder/i));
    await waitFor(() => {
      expect(screen.getByText(/data directory not found/i)).toBeInTheDocument();
    });
  });

  it("shows a visible error, not a silent failure, when revealRestoreBat rejects", async () => {
    mockRevealRestoreBat.mockRejectedValue(new Error("restore script not found"));
    render(<EmergencyRestoreModal isOpen={true} onClose={vi.fn()} />);
    fireEvent.click(screen.getByText(/reveal restore script/i));
    await waitFor(() => {
      expect(screen.getByText(/restore script not found/i)).toBeInTheDocument();
    });
  });
});
