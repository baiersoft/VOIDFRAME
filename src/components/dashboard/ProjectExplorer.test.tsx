import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { ProjectExplorer } from "./ProjectExplorer";
import type { Project } from "../../lib/bindings";

function project(overrides: Partial<Project> = {}): Project {
  return {
    schema_version: "1.0.0",
    id: "p1",
    name: "Test Project",
    description: "A test project",
    created_at: "2026-09-01T00:00:00Z",
    settings: {},
    baseline: { name: "Stock", description: "d" },
    scenarios: [],
    ...overrides,
  };
}

describe("ProjectExplorer", () => {
  it("renders real project names and scenario counts, no fake stat values", () => {
    render(
      <ProjectExplorer
        projects={[project({ scenarios: [{ id: "s1", name: "S1", description: "d", enabled: true, modules: [] }] })]}
        onSelectProject={vi.fn()}
        onEditMatrix={vi.fn()}
        onRunProject={vi.fn()}
        onViewResults={vi.fn()}
        onCreateNew={vi.fn()}
        isStarting={false}
      />
    );
    expect(screen.getByText("Test Project")).toBeInTheDocument();
    // The old mockup hardcoded "325.4 pts" / "+14.8%" regardless of real
    // data -- neither must appear now that this renders real projects.
    expect(screen.queryByText(/325\.4/)).not.toBeInTheDocument();
    expect(screen.queryByText(/\+14\.8%/)).not.toBeInTheDocument();
  });

  it("shows a minimal run-count pill only for a project that has results, and clicking it calls onViewResults", () => {
    const onViewResults = vi.fn();
    render(
      <ProjectExplorer
        projects={[project({ id: "p1", name: "Has Results" }), project({ id: "p2", name: "No Results" })]}
        onSelectProject={vi.fn()}
        onEditMatrix={vi.fn()}
        onRunProject={vi.fn()}
        onViewResults={onViewResults}
        onCreateNew={vi.fn()}
        isStarting={false}
        resultSummaries={{
          p1: [
            { run_id: "r1", project_id: "p1", completed_at: "2026-09-04T00:00:00Z", scenario_count: 2, winner_name: null, winner_wcps: null },
            { run_id: "r2", project_id: "p1", completed_at: "2026-09-03T00:00:00Z", scenario_count: 2, winner_name: null, winner_wcps: null },
          ],
        }}
      />
    );
    // No winner name on the card -- that detail stays behind the Results tab.
    const pill = screen.getByRole("button", { name: /2 runs/i });
    expect(pill).toBeInTheDocument();
    expect(screen.queryByText(/winner/i)).not.toBeInTheDocument();

    fireEvent.click(pill);
    expect(onViewResults).toHaveBeenCalledWith(
      expect.objectContaining({ id: "p1", name: "Has Results" })
    );
  });

  it("shows the best WCPS across a project's runs, or an em dash when none have scored yet", () => {
    render(
      <ProjectExplorer
        projects={[
          project({ id: "p1", name: "Scored" }),
          project({ id: "p2", name: "Unscored" }),
          project({ id: "p3", name: "Never Run" }),
        ]}
        onSelectProject={vi.fn()}
        onEditMatrix={vi.fn()}
        onRunProject={vi.fn()}
        onViewResults={vi.fn()}
        onCreateNew={vi.fn()}
        isStarting={false}
        resultSummaries={{
          p1: [
            { run_id: "r1", project_id: "p1", completed_at: "2026-09-04T00:00:00Z", scenario_count: 2, winner_name: "A", winner_wcps: 87.3 },
            { run_id: "r2", project_id: "p1", completed_at: "2026-09-03T00:00:00Z", scenario_count: 2, winner_name: "B", winner_wcps: 91.6 },
          ],
          p2: [
            { run_id: "r3", project_id: "p2", completed_at: "2026-09-02T00:00:00Z", scenario_count: 1, winner_name: null, winner_wcps: null },
          ],
        }}
      />
    );
    expect(screen.getByText("91.6")).toBeInTheDocument();
    expect(screen.getAllByText("—")).toHaveLength(2);
  });

  it("shows no run-count pill for a project with no results", () => {
    render(
      <ProjectExplorer
        projects={[project({ id: "p2", name: "No Results" })]}
        onSelectProject={vi.fn()}
        onEditMatrix={vi.fn()}
        onRunProject={vi.fn()}
        onViewResults={vi.fn()}
        onCreateNew={vi.fn()}
        isStarting={false}
        resultSummaries={{}}
      />
    );
    expect(screen.queryByRole("button", { name: /\d+ runs?/i })).not.toBeInTheDocument();
  });
});

