import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

const mockListPowerPlans = vi.fn();
vi.mock("../../lib/api", () => ({
  listPowerPlans: (...args: unknown[]) => mockListPowerPlans(...args),
}));

import { MatrixBuilder } from "./MatrixBuilder";
import type { Project } from "../../lib/bindings";

function project(overrides: Partial<Project> = {}): Project {
  return {
    schema_version: "1.0.0",
    id: "p1",
    name: "Test Project",
    description: "d",
    created_at: "2026-09-01T00:00:00Z",
    settings: { warmup_loops: 2, measure_loops: 3, capture_seconds: 60 },
    baseline: { name: "Stock", description: "Baseline description" },
    scenarios: [
      {
        id: "s1",
        name: "Scenario 1",
        description: "d",
        enabled: true,
        modules: [{ type: "powercfg", sub: "sub_processor", setting: "CPMINCORES", value: 100 }],
      },
    ],
    ...overrides,
  };
}

describe("MatrixBuilder", () => {
  beforeEach(() => {
    mockListPowerPlans.mockReset();
  });

  it("shows an Edit button only for a power_plan module, and picking a plan calls onUpdateProject with the module replaced", async () => {
    mockListPowerPlans.mockResolvedValue([
      { guid: "balanced-guid", name: "Balanced", active: true },
      { guid: "high-perf-guid", name: "High performance", active: false },
    ]);
    const onUpdateProject = vi.fn();
    const proj = project({
      scenarios: [
        {
          id: "s1",
          name: "Scenario 1",
          description: "d",
          enabled: true,
          modules: [
            { type: "powercfg", sub: "sub_processor", setting: "CPMINCORES", value: 100 },
            { type: "power_plan", plan_guid: "balanced-guid", create_if_missing: false },
          ],
        },
      ],
    });
    render(
      <MatrixBuilder
        project={proj}
        onUpdateProject={onUpdateProject}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={false}
      />
    );

    const editButtons = screen.getAllByRole("button", { name: /edit/i });
    expect(editButtons).toHaveLength(1); // only the power_plan module gets one, not the powercfg module

    fireEvent.click(editButtons[0]);
    await waitFor(() => screen.getByText("High performance"));
    fireEvent.click(screen.getByRole("button", { name: /high performance/i }));

    expect(onUpdateProject).toHaveBeenCalledWith(
      expect.objectContaining({
        scenarios: [
          expect.objectContaining({
            modules: [
              { type: "powercfg", sub: "sub_processor", setting: "CPMINCORES", value: 100 },
              {
                type: "power_plan",
                plan_guid: "high-perf-guid",
                friendly_name: "High performance",
                create_if_missing: false,
              },
            ],
          }),
        ],
      })
    );
  });

  it("shows an Edit button for a launch_args module too, and saving new text calls onUpdateProject with the module replaced", async () => {
    const onUpdateProject = vi.fn();
    const proj = project({
      scenarios: [
        {
          id: "s1",
          name: "Scenario 1",
          description: "d",
          enabled: true,
          modules: [{ type: "launch_args", args: "-novid" }],
        },
      ],
    });
    render(
      <MatrixBuilder
        project={proj}
        onUpdateProject={onUpdateProject}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={false}
      />
    );

    fireEvent.click(screen.getByRole("button", { name: /edit/i }));
    const textarea = screen.getByRole("textbox");
    expect(textarea).toHaveValue("-novid");
    fireEvent.change(textarea, { target: { value: "-novid -high" } });
    fireEvent.click(screen.getByRole("button", { name: /save/i }));

    expect(onUpdateProject).toHaveBeenCalledWith(
      expect.objectContaining({
        scenarios: [
          expect.objectContaining({
            modules: [{ type: "launch_args", args: "-novid -high" }],
          }),
        ],
      })
    );
  });

  it("renders real modules with real field data, not TweakItem-shaped fields", () => {
    render(
      <MatrixBuilder
        project={project()}
        onUpdateProject={vi.fn()}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={false}
      />
    );
    expect(screen.getByText(/sub_processor/)).toBeInTheDocument();
    expect(screen.getByText(/CPMINCORES/)).toBeInTheDocument();
  });

  it("toggling a scenario calls onUpdateProject with the real Scenario shape (enabled flipped)", () => {
    const onUpdateProject = vi.fn();
    render(
      <MatrixBuilder
        project={project()}
        onUpdateProject={onUpdateProject}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={false}
      />
    );
    fireEvent.click(screen.getByRole("checkbox"));
    expect(onUpdateProject).toHaveBeenCalledWith(
      expect.objectContaining({
        scenarios: [expect.objectContaining({ id: "s1", enabled: false })],
      })
    );
  });

  it("clicking Rename Scenario reveals an editable input pre-filled with the current name", () => {
    render(
      <MatrixBuilder
        project={project()}
        onUpdateProject={vi.fn()}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={false}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: /rename scenario/i }));
    const input = screen.getByRole("textbox", { name: /scenario name/i });
    expect(input).toHaveValue("Scenario 1");
  });

  it("typing a new name and pressing Enter calls onUpdateProject with the scenario's name updated and every other field unchanged", () => {
    const onUpdateProject = vi.fn();
    const proj = project();
    render(
      <MatrixBuilder
        project={proj}
        onUpdateProject={onUpdateProject}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={false}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: /rename scenario/i }));
    const input = screen.getByRole("textbox", { name: /scenario name/i });
    fireEvent.change(input, { target: { value: "Renamed Scenario" } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onUpdateProject).toHaveBeenCalledWith({
      ...proj,
      scenarios: [{ ...proj.scenarios![0], name: "Renamed Scenario" }],
    });
  });

  it("typing a new name and blurring calls onUpdateProject with the scenario's name updated", () => {
    const onUpdateProject = vi.fn();
    const proj = project();
    render(
      <MatrixBuilder
        project={proj}
        onUpdateProject={onUpdateProject}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={false}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: /rename scenario/i }));
    const input = screen.getByRole("textbox", { name: /scenario name/i });
    fireEvent.change(input, { target: { value: "Blur Renamed" } });
    fireEvent.blur(input);

    expect(onUpdateProject).toHaveBeenCalledWith({
      ...proj,
      scenarios: [{ ...proj.scenarios![0], name: "Blur Renamed" }],
    });
  });

  it("pressing Escape cancels the rename without calling onUpdateProject and reverts the displayed name", () => {
    const onUpdateProject = vi.fn();
    render(
      <MatrixBuilder
        project={project()}
        onUpdateProject={onUpdateProject}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={false}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: /rename scenario/i }));
    const input = screen.getByRole("textbox", { name: /scenario name/i });
    fireEvent.change(input, { target: { value: "Should Not Save" } });
    fireEvent.keyDown(input, { key: "Escape" });

    expect(onUpdateProject).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: "Scenario 1" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /scenario name/i })).not.toBeInTheDocument();
  });

  it("committing an empty/whitespace-only name does not save it and reverts to the original name", () => {
    const onUpdateProject = vi.fn();
    render(
      <MatrixBuilder
        project={project()}
        onUpdateProject={onUpdateProject}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={false}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: /rename scenario/i }));
    const input = screen.getByRole("textbox", { name: /scenario name/i });
    fireEvent.change(input, { target: { value: "   " } });
    fireEvent.keyDown(input, { key: "Enter" });

    expect(onUpdateProject).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { name: "Scenario 1" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /scenario name/i })).not.toBeInTheDocument();
  });

  it("adding a scenario produces a real Scenario shape (empty modules array, no rebootRequired/tweaks fields)", () => {
    const onUpdateProject = vi.fn();
    render(
      <MatrixBuilder
        project={project()}
        onUpdateProject={onUpdateProject}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={false}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: /add scenario/i }));
    const call = onUpdateProject.mock.calls[0][0];
    const newScenario = call.scenarios[call.scenarios.length - 1];
    expect(newScenario.modules).toEqual([]);
    expect(newScenario).not.toHaveProperty("rebootRequired");
    expect(newScenario).not.toHaveProperty("tweaks");
  });
});

describe("MatrixBuilder run trigger", () => {
  // Validation now happens one level up, in App.tsx's handleRunProject (the
  // single choke point shared by both the Builder's Launch button and
  // ProjectExplorer's Run Matrix button) -- see App.test.tsx's "App run
  // gating" suite for validate-then-run coverage. MatrixBuilder itself now
  // just forwards the click straight through, with no validation gate of
  // its own.
  it("calls onRunProject directly on click, with no validation gate of its own", () => {
    const onRunProject = vi.fn();
    const sampleProject = project();
    render(
      <MatrixBuilder
        project={sampleProject}
        onUpdateProject={vi.fn()}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={onRunProject}
        isStarting={false}
      />
    );
    fireEvent.click(screen.getByText(/launch autonomous pipeline/i));
    expect(onRunProject).toHaveBeenCalledWith(sampleProject);
  });

  it("disables the Launch button while isStarting is true", () => {
    render(
      <MatrixBuilder
        project={project()}
        onUpdateProject={vi.fn()}
        onOpenTweakCatalog={vi.fn()}
        onRunProject={vi.fn()}
        isStarting={true}
      />
    );
    expect(screen.getByText(/launch autonomous pipeline/i).closest("button")).toBeDisabled();
  });
});

describe("MatrixBuilder estimator math", () => {
  it("computes total runs as (enabledScenarios+1) * (warmup+measure), matching the M1 no-reboot formula", () => {
    const estimatorProject: Project = {
      schema_version: "1.0.0",
      id: "p1",
      name: "Estimator Test",
      description: "d",
      created_at: "2026-09-01T00:00:00Z",
      settings: { warmup_loops: 2, measure_loops: 3, capture_seconds: 60 },
      baseline: { name: "Stock", description: "d" },
      scenarios: [
        { id: "s1", name: "S1", description: "d", enabled: true, modules: [] },
        { id: "s2", name: "S2", description: "d", enabled: true, modules: [] },
        { id: "s3", name: "S3", description: "d", enabled: false, modules: [] }, // disabled -- must not count
      ],
    };
    render(
      <MatrixBuilder project={estimatorProject} onUpdateProject={vi.fn()} onOpenTweakCatalog={vi.fn()} onRunProject={vi.fn()} isStarting={false} />
    );
    // 2 enabled scenarios + 1 baseline = 3 units; warmup = 3*2 = 6, measure = 3*3 = 9, total = 15.
    // The component renders `{totalRuns} <span>({totalMeasureRuns}m + {totalWarmupRuns}w)</span>` --
    // "15" and "(9m + 6w)" are SIBLING text/element nodes inside the same div, so no single
    // element's own textContent is exactly "15" (getByText("15") would find nothing) and a bare
    // regex like /15/ would match every ancestor up the tree (getByText would throw on multiple
    // matches). Use an explicit function matcher against the one div whose full normalized
    // textContent is the complete combined string -- the RTL-documented pattern for text split
    // across sibling elements.
    expect(
      screen.getByText((_, element) => element?.tagName.toLowerCase() === "div" && element.textContent === "15 (9m + 6w)")
    ).toBeInTheDocument();
    // Estimated seconds: 15 * (60 + 15) = 1125s -> ceil(1125/60) = 19 minutes.
    // This one IS a single clean text node (the Clock icon contributes no text), so an exact
    // string match against its own div is safe and won't ambiguously match an ancestor (the
    // ancestor also contains the "ESTIMATED TIME" label text, so its full textContent differs).
    expect(screen.getByText("~19 min")).toBeInTheDocument();
  });

  it("falls back to the documented defaults (warmup 2, measure 3, capture 105s) when settings omits them", () => {
    const defaultsProject: Project = {
      schema_version: "1.0.0",
      id: "p1",
      name: "Defaults Test",
      description: "d",
      created_at: "2026-09-01T00:00:00Z",
      settings: {}, // no warmup_loops/measure_loops/capture_seconds at all
      baseline: { name: "Stock", description: "d" },
      scenarios: [{ id: "s1", name: "S1", description: "d", enabled: true, modules: [] }],
    };
    render(
      <MatrixBuilder project={defaultsProject} onUpdateProject={vi.fn()} onOpenTweakCatalog={vi.fn()} onRunProject={vi.fn()} isStarting={false} />
    );
    // (1 enabled + 1 baseline) = 2 units; warmup = 2*2 = 4, measure = 2*3 = 6, total = 10.
    // Same sibling-text-node situation as the previous test -- use the same function matcher.
    expect(
      screen.getByText((_, element) => element?.tagName.toLowerCase() === "div" && element.textContent === "10 (6m + 4w)")
    ).toBeInTheDocument();
    // Estimated seconds: 10 * (105 + 15) = 1200s -> ceil(1200/60) = 20 minutes.
    // This directly exercises the `capture_seconds ?? 105` fallback (unlike totalRuns/
    // totalMeasureRuns/totalWarmupRuns above, which don't depend on capture_seconds at all).
    expect(screen.getByText("~20 min")).toBeInTheDocument();
  });

  it("shows a reboot badge on a HAGS scenario and a reboot count in the estimate area", () => {
    const rebootProject: Project = {
      schema_version: "1.0.0",
      id: "p1",
      name: "Reboot Test",
      description: "d",
      created_at: "2026-09-01T00:00:00Z",
      settings: { warmup_loops: 2, measure_loops: 3, capture_seconds: 60 },
      baseline: { name: "Stock", description: "d" },
      scenarios: [
        {
          id: "s1",
          name: "HAGS On",
          description: "d",
          enabled: true,
          modules: [
            {
              type: "registry",
              hive: "HKLM",
              subkey: "SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
              value_name: "HwSchMode",
              value_type: "DWORD",
              value: 2,
              requires_reboot: true,
            },
          ],
        },
      ],
    };
    render(
      <MatrixBuilder project={rebootProject} onUpdateProject={vi.fn()} onOpenTweakCatalog={vi.fn()} onRunProject={vi.fn()} isStarting={false} />
    );
    expect(screen.getByText(/reboot/i, { selector: "span" })).toBeInTheDocument();
    // countReboots([true]) === 2 (apply-reboot, then a final revert-reboot).
    expect(screen.getByText(/2 reboots/i)).toBeInTheDocument();
  });
});
