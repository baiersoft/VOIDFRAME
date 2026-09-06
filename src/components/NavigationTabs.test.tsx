import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { NavigationTabs, isTabVisible } from "./NavigationTabs";

describe("isTabVisible", () => {
  const noneVisible = { hasProject: false, hasRun: false, hasResults: false };

  it("dashboard and library are always visible", () => {
    expect(isTabVisible("dashboard", noneVisible)).toBe(true);
    expect(isTabVisible("library", noneVisible)).toBe(true);
  });

  it("builder requires hasProject", () => {
    expect(isTabVisible("builder", noneVisible)).toBe(false);
    expect(isTabVisible("builder", { ...noneVisible, hasProject: true })).toBe(true);
  });

  it("monitor requires hasRun, not hasProject", () => {
    expect(isTabVisible("monitor", { ...noneVisible, hasProject: true })).toBe(false);
    expect(isTabVisible("monitor", { ...noneVisible, hasRun: true })).toBe(true);
  });

  it("results requires hasResults", () => {
    expect(isTabVisible("results", noneVisible)).toBe(false);
    expect(isTabVisible("results", { ...noneVisible, hasResults: true })).toBe(true);
  });
});

describe("NavigationTabs", () => {
  it("shows only Project Explorer and Verified Tweaks when no project, run, or results exist", () => {
    render(<NavigationTabs activeTab="dashboard" onSelectTab={vi.fn()} />);
    expect(screen.getByText("Project Explorer")).toBeInTheDocument();
    expect(screen.getByText("Verified Tweaks")).toBeInTheDocument();
    expect(screen.queryByText("Matrix Builder")).not.toBeInTheDocument();
    expect(screen.queryByText("Live Monitor")).not.toBeInTheDocument();
    expect(screen.queryByText("Visualizer & Charts")).not.toBeInTheDocument();
  });

  it("shows Matrix Builder once a project is selected", () => {
    render(<NavigationTabs activeTab="dashboard" onSelectTab={vi.fn()} hasProject={true} />);
    expect(screen.getByText("Matrix Builder")).toBeInTheDocument();
  });

  it("does not show Live Monitor just because a project is selected -- only once a run has actually started", () => {
    render(<NavigationTabs activeTab="dashboard" onSelectTab={vi.fn()} hasProject={true} hasRun={false} />);
    expect(screen.queryByText("Live Monitor")).not.toBeInTheDocument();
  });

  it("shows Live Monitor once a run has started", () => {
    render(<NavigationTabs activeTab="dashboard" onSelectTab={vi.fn()} hasRun={true} />);
    expect(screen.getByText("Live Monitor")).toBeInTheDocument();
  });

  it("keeps Live Monitor visible after the run finishes, until the user leaves it -- isRunning alone must not hide it", () => {
    // isRunning flips false the instant RunComplete/RunFailed fires, but the
    // user is likely still looking at the terminal banner at that exact
    // moment -- the tab must not vanish out from under them. hasRun (App.tsx:
    // activeRunId !== null) only clears once they explicitly leave via
    // "Back to Builder".
    render(<NavigationTabs activeTab="monitor" onSelectTab={vi.fn()} hasRun={true} isRunning={false} />);
    expect(screen.getByText("Live Monitor")).toBeInTheDocument();
  });

  it("shows Visualizer & Charts once results exist", () => {
    render(<NavigationTabs activeTab="dashboard" onSelectTab={vi.fn()} hasResults={true} />);
    expect(screen.getByText("Visualizer & Charts")).toBeInTheDocument();
  });

  it("calls onSelectTab with the clicked tab's id", () => {
    const onSelectTab = vi.fn();
    render(<NavigationTabs activeTab="dashboard" onSelectTab={onSelectTab} hasProject={true} />);
    fireEvent.click(screen.getByText("Matrix Builder"));
    expect(onSelectTab).toHaveBeenCalledWith("builder");
  });
});
