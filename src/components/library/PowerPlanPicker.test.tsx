import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";

const mockListPowerPlans = vi.fn();
vi.mock("../../lib/api", () => ({
  listPowerPlans: (...args: unknown[]) => mockListPowerPlans(...args),
}));

import { PowerPlanPicker } from "./PowerPlanPicker";

const PLANS = [
  { guid: "balanced-guid", name: "Balanced", active: true },
  { guid: "high-perf-guid", name: "High performance", active: false },
  { guid: "saver-guid", name: "Power saver", active: false },
];

describe("PowerPlanPicker", () => {
  beforeEach(() => {
    mockListPowerPlans.mockReset();
  });

  it("renders every plan the system reports", async () => {
    mockListPowerPlans.mockResolvedValue(PLANS);
    render(<PowerPlanPicker onSelect={vi.fn()} />);
    await waitFor(() => {
      expect(screen.getByText("Balanced")).toBeInTheDocument();
    });
    expect(screen.getByText("High performance")).toBeInTheDocument();
    expect(screen.getByText("Power saver")).toBeInTheDocument();
  });

  it("disables the currently active plan and labels it, rather than letting it be picked", async () => {
    mockListPowerPlans.mockResolvedValue(PLANS);
    render(<PowerPlanPicker onSelect={vi.fn()} />);
    await waitFor(() => screen.getByText("Balanced"));
    const activeRow = screen.getByRole("button", { name: /balanced/i });
    expect(activeRow).toBeDisabled();
    expect(screen.getByText(/currently active/i)).toBeInTheDocument();
  });

  it("calls onSelect with the plan when a non-active row is clicked", async () => {
    mockListPowerPlans.mockResolvedValue(PLANS);
    const onSelect = vi.fn();
    render(<PowerPlanPicker onSelect={onSelect} />);
    await waitFor(() => screen.getByText("High performance"));
    fireEvent.click(screen.getByRole("button", { name: /high performance/i }));
    expect(onSelect).toHaveBeenCalledWith(PLANS[1]);
  });

  it("clicking the disabled active row does not call onSelect", async () => {
    mockListPowerPlans.mockResolvedValue(PLANS);
    const onSelect = vi.fn();
    render(<PowerPlanPicker onSelect={onSelect} />);
    await waitFor(() => screen.getByText("Balanced"));
    fireEvent.click(screen.getByRole("button", { name: /balanced/i }));
    expect(onSelect).not.toHaveBeenCalled();
  });
});
