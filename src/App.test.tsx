import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import App from "./App";
import type { EngineEvent, RunSummary } from "./lib/bindings";

const mockListProjects = vi.fn();
const mockGetRunSnapshot = vi.fn();
const mockValidateScenario = vi.fn();
const mockStartRun = vi.fn();
const mockGetResults = vi.fn();
const mockSaveProject = vi.fn();
// Safe default (dry_run_default undefined -> handleRunProject's `?? false`)
// for every existing test that doesn't care about Settings -- App now reads
// this fresh on every Run Matrix click. Tests that DO care override with
// mockResolvedValueOnce/mockResolvedValue.
const mockGetConfig = vi.fn().mockResolvedValue({});
// Safe default for every existing test that doesn't care about results
// history -- App now fetches this for every loaded project, so a bare
// `vi.fn()` with no resolved value would leave those calls hanging.
// Tests that DO care override with mockResolvedValueOnce/mockResolvedValue.
const mockListResults = vi.fn().mockResolvedValue([]);
// Unlike LiveMonitor.test.tsx's single `engineEventHandler` (only one
// subscriber ever exists there), App and LiveMonitor each register their
// own, independent subscription against the same real `vf:event` channel --
// exactly like two concurrent `listen()` calls in the real app -- so this
// tracks all of them and `emit` fans an event out to whichever are
// currently registered, mirroring how a real backend emit reaches every
// live listener regardless of which components are mounted.
const engineEventHandlers: Array<(e: EngineEvent) => void> = [];
const mockSubscribeToEngineEvents = vi.fn((handler: (e: EngineEvent) => void) => {
  engineEventHandlers.push(handler);
  return () => {
    const idx = engineEventHandlers.indexOf(handler);
    if (idx !== -1) engineEventHandlers.splice(idx, 1);
  };
});
function emit(event: EngineEvent) {
  [...engineEventHandlers].forEach((handler) => handler(event));
}

vi.mock("./lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./lib/api")>();
  return {
    ...actual,
    listProjects: (...args: unknown[]) => mockListProjects(...args),
    getRunSnapshot: (...args: unknown[]) => mockGetRunSnapshot(...args),
    validateScenario: (...args: unknown[]) => mockValidateScenario(...args),
    startRun: (...args: unknown[]) => mockStartRun(...args),
    getConfig: (...args: unknown[]) => mockGetConfig(...args),
    getResults: (...args: unknown[]) => mockGetResults(...args),
    listResults: (...args: unknown[]) => mockListResults(...args),
    saveProject: (...args: unknown[]) => mockSaveProject(...args),
    subscribeToEngineEvents: (handler: (e: EngineEvent) => void) => mockSubscribeToEngineEvents(handler),
  };
});

// jsdom does not implement scrollIntoView; LiveMonitor (mounted once a run
// starts and the App navigates to the Monitor tab) calls it on every log
// update to auto-scroll the terminal, so stub it once for every test here
// (mirrors LiveMonitor.test.tsx's own stub).
Element.prototype.scrollIntoView = vi.fn();

// Applies to every test below, regardless of which `describe`'s own
// `beforeEach` also runs: RTL's automatic unmount (after each test) does
// clear out `engineEventHandlers` via each subscription's own cleanup, but
// clearing the mock's call count here too keeps `toHaveBeenCalledTimes`-style
// assertions independent of test order.
beforeEach(() => {
  engineEventHandlers.length = 0;
  mockSubscribeToEngineEvents.mockClear();
});

const REAL_PROJECT = {
  schema_version: "1.0.0",
  id: "p1",
  name: "Real Project One",
  description: "d",
  created_at: "2026-09-01T00:00:00Z",
  settings: {},
  baseline: { name: "Stock", description: "d" },
  scenarios: [],
};

describe("App real project loading", () => {
  beforeEach(() => {
    mockListProjects.mockReset();
    // Best-effort crash-recovery read on mount -- resolves `null` by default
    // (no RunState in the store, so no crash banner) unless a test wants to
    // assert the banner appears.
    mockGetRunSnapshot.mockReset().mockResolvedValue(null);
    mockValidateScenario.mockReset();
    mockStartRun.mockReset();
    mockGetResults.mockReset();
  });

  it("loads real projects from the backend on mount", async () => {
    mockListProjects.mockResolvedValue([
      {
        schema_version: "1.0.0",
        id: "p1",
        name: "Real Project One",
        description: "d",
        created_at: "2026-09-01T00:00:00Z",
        settings: {},
        baseline: { name: "Stock", description: "d" },
        scenarios: [],
      },
    ]);
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText("Real Project One")).toBeInTheDocument();
    });
    expect(mockListProjects).toHaveBeenCalledTimes(1);
  });

  it("shows an error state when listProjects fails, not a silent empty list", async () => {
    mockListProjects.mockRejectedValue(new Error("backend unreachable"));
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText(/backend unreachable/i)).toBeInTheDocument();
    });
  });

  it("shows the crash-recovery banner when getRunSnapshot resolves a non-null RunState", async () => {
    mockListProjects.mockResolvedValue([REAL_PROJECT]);
    mockGetRunSnapshot.mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "r0",
      project_id: "p1",
      phase: { kind: "scenario", id: "s1" },
      current_scenario: "s1",
      completed_scenarios: [],
      revision: 2,
    });
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText(/did not complete cleanly/i)).toBeInTheDocument();
    });
  });
});

describe("App run gating", () => {
  beforeEach(() => {
    mockListProjects.mockReset().mockResolvedValue([REAL_PROJECT]);
    mockGetRunSnapshot.mockReset().mockResolvedValue(null);
    mockValidateScenario.mockReset();
    mockStartRun.mockReset();
    mockGetResults.mockReset();
  });

  it("gates Run Matrix on validateScenario, and does not call startRun on a hard validation failure", async () => {
    mockValidateScenario.mockRejectedValue(new Error("no enabled scenario"));
    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));
    fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));
    await waitFor(() => {
      expect(screen.getByText(/no enabled scenario/i)).toBeInTheDocument();
    });
    expect(mockStartRun).not.toHaveBeenCalled();
  });

  it("calls startRun (with dryRun=false) and switches to the monitor tab once validateScenario resolves", async () => {
    mockValidateScenario.mockResolvedValue([]);
    mockStartRun.mockResolvedValue("run-123");
    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));
    fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));
    await waitFor(() => {
      expect(mockStartRun).toHaveBeenCalledWith("p1", false);
    });
    // Navigated into the Monitor tab -- LiveMonitor's own "checking for an
    // active run" / active-run UI takes over (it does its own
    // getRunSnapshot()-backed reconciliation, exercised in
    // LiveMonitor.test.tsx, not re-tested here).
    await waitFor(() => {
      expect(screen.queryByText("Real Project One")).not.toBeInTheDocument();
    });
  });

  it("passes the Settings-saved dry_run_default through to startRun", async () => {
    mockValidateScenario.mockResolvedValue([]);
    mockStartRun.mockResolvedValue("run-123");
    mockGetConfig.mockResolvedValueOnce({ dry_run_default: true });
    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));
    fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));
    await waitFor(() => {
      expect(mockStartRun).toHaveBeenCalledWith("p1", true);
    });
  });

  it("clears a stale crash-recovery banner once a new run starts successfully", async () => {
    mockGetRunSnapshot.mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "r0",
      project_id: "p1",
      phase: { kind: "rollback" },
      current_scenario: null,
      completed_scenarios: [],
      revision: 1,
    });
    mockValidateScenario.mockResolvedValue([]);
    mockStartRun.mockResolvedValue("run-123");
    render(<App />);
    await waitFor(() => {
      expect(screen.getByText(/did not complete cleanly/i)).toBeInTheDocument();
    });
    fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));
    await waitFor(() => {
      expect(mockStartRun).toHaveBeenCalled();
    });
    expect(screen.queryByText(/did not complete cleanly/i)).not.toBeInTheDocument();
  });
});

describe("App results flow", () => {
  // `handleViewRunResults` is an internal closure, not exported, and driving
  // it end-to-end from here would mean also mocking subscribeToEngineEvents
  // and getRunSnapshot to fake a live run all the way through RunComplete --
  // real coverage of that button/callback wiring already lives in
  // LiveMonitor.test.tsx ("shows a View Results button on RunComplete,
  // calling onViewResults with the real run_id" and the RunFailed negative
  // case), which mocks the engine-event stream directly and is a much
  // cheaper place to assert it. What's uniquely worth covering here, at the
  // App level, is that the Results tab now renders from real state instead
  // of the old `MOCK_PROJECT_RESULTS` import -- i.e. that
  // `docs/superpowers/plans/2026-09-02-phase-4b-4-results.md`'s removal
  // of that import actually took effect and getResults is never called
  // speculatively just by visiting the tab.
  beforeEach(() => {
    mockListProjects.mockReset().mockResolvedValue([REAL_PROJECT]);
    mockGetRunSnapshot.mockReset().mockResolvedValue(null);
    mockValidateScenario.mockReset();
    mockStartRun.mockReset();
    mockGetResults.mockReset();
  });

  it("shows the real empty state on the Results tab (not mock data) when the selected project genuinely has no history", async () => {
    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));
    // hasResults/the Results tab are now derived per-project from
    // list_results (results only mean anything in their own project's
    // context) -- with the default empty mockListResults, the tab must
    // stay hidden and nothing gets speculatively fetched just by having a
    // project selected.
    expect(screen.queryByRole("button", { name: /visualizer & charts/i })).not.toBeInTheDocument();
    expect(mockGetResults).not.toHaveBeenCalled();
  });

  it("auto-loads the most recent result once the Results tab becomes reachable after a run completes", async () => {
    // Simulates the real timing: execute() writes results.json to disk
    // before emitting RunComplete, so a fresh list_results call made once
    // that event arrives already sees the new run.
    let resultsForP1: RunSummary[] = [];
    mockListResults.mockImplementation(async (projectId: string) =>
      projectId === "p1" ? resultsForP1 : []
    );
    mockGetResults.mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "run-1",
      project_id: "p1",
      completed_at: "2026-09-04T00:00:00Z",
      detection_tier: "log_tail",
      baseline: {
        scenario_id: "baseline",
        name: "Stock",
        is_baseline: true,
        aggregated: {
          avg_fps: 400,
          median_fps: 400,
          p1_fps: 250,
          p01_fps: 180,
          frame_time_mean_ms: 2.5,
          frame_time_stddev_ms: 0.2,
          frame_time_cv: 0.08,
          adaptive_frame_time_cv: null,
          stutter_count_pct: null,
          mean_abs_animation_error_ms: null,
          gpu_busy_ms: null,
          bottleneck_ratio: null,
          render_latency_ms: null,
        },
        per_iteration: [],
        metric_deltas: [],
        wcps: 0,
        verdict: "confirmed_same",
      },
      scenarios: [],
    });

    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));
    // Select the project first -- RunComplete's own refresh only fires for
    // whichever project is currently selected, matching how a real run is
    // always started against one.
    fireEvent.click(screen.getByText("Edit Matrix"));
    await waitFor(() => screen.getByText("Matrix Builder"));

    resultsForP1 = [
      {
        run_id: "run-1",
        project_id: "p1",
        completed_at: "2026-09-04T00:00:00Z",
        scenario_count: 1,
        winner_name: null,
        winner_wcps: null,
      },
    ];
    emit({ type: "RunComplete", run_id: "run-1" });

    await waitFor(() => screen.getByRole("button", { name: /visualizer & charts/i }));
    fireEvent.click(screen.getByRole("button", { name: /visualizer & charts/i }));

    await waitFor(() => {
      expect(mockGetResults).toHaveBeenCalledWith("run-1");
      expect(screen.queryByText(/no results loaded yet/i)).not.toBeInTheDocument();
    });
  });
});

describe("App run-lifecycle state survives LiveMonitor being unmounted (tab switch)", () => {
  // Regression coverage for the H5 finding: previously, RunComplete/
  // RunFailed were only ever processed inside LiveMonitor's own
  // subscribeToEngineEvents effect. Switching tabs away from Monitor
  // unmounts LiveMonitor (tearing down that subscription in its cleanup),
  // so a terminal event delivered during that window reached nobody --
  // isRunInProgress never flipped back, and the failure reason was lost.
  // App.tsx now runs its own, separate, app-lifetime subscription (mounted
  // once, for the whole app session) that is the sole owner of
  // isRunInProgress/hasResults/runOutcome; LiveMonitor only ever reflects
  // `runOutcome` back down as a prop.
  //
  // A snapshot matching REAL_PROJECT's id is used throughout so LiveMonitor
  // (whenever it happens to be mounted) resolves `hasActiveRun` from
  // `getRunSnapshot()` as true and renders its live/terminal UI, rather than
  // "No active run for this project" -- letting the failure-reason
  // assertion below actually exercise LiveMonitor's rendering of the
  // `runOutcome` prop, not just App's own internal state.
  const ACTIVE_SNAPSHOT_FOR_P1 = {
    schema_version: "1.0.0",
    run_id: "run-123",
    project_id: "p1",
    phase: { kind: "scenario", id: "s1" },
    current_scenario: "s1",
    completed_scenarios: [],
    revision: 1,
  };

  beforeEach(() => {
    mockListProjects.mockReset().mockResolvedValue([REAL_PROJECT]);
    mockGetRunSnapshot.mockReset().mockResolvedValue(ACTIVE_SNAPSHOT_FOR_P1);
    mockValidateScenario.mockReset().mockResolvedValue([]);
    mockStartRun.mockReset().mockResolvedValue("run-123");
    mockGetResults.mockReset();
  });

  async function startRunAndNavigateAway() {
    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));
    fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));
    await waitFor(() => {
      expect(mockStartRun).toHaveBeenCalledWith("p1", false);
    });
    // isRunInProgress is now true, and the Monitor tab shows an "ACTIVE"
    // badge for it -- and LiveMonitor, now mounted, has registered its own
    // subscription alongside App's.
    await waitFor(() => {
      expect(screen.getByText("ACTIVE")).toBeInTheDocument();
      expect(engineEventHandlers.length).toBeGreaterThanOrEqual(2);
    });

    // Switch away from Monitor: unmounts LiveMonitor, tearing down its own
    // subscription. Only App's own, app-lifetime handler is left listening.
    fireEvent.click(screen.getByRole("button", { name: /project explorer/i }));
    await waitFor(() => {
      expect(engineEventHandlers.length).toBe(1);
    });
  }

  it("processes a RunFailed delivered while LiveMonitor is unmounted: isRunInProgress unblocks immediately, and the reason is not lost -- it's shown when the user returns to Monitor", async () => {
    await startRunAndNavigateAway();

    emit({ type: "RunFailed", reason: "Engine crashed mid-run" });

    // isRunInProgress flipped back even with nobody rendering LiveMonitor.
    await waitFor(() => {
      expect(screen.queryByText("ACTIVE")).not.toBeInTheDocument();
    });

    // The failure reason was captured by App, not dropped -- navigating
    // back to Monitor (a fresh LiveMonitor mount) still shows it.
    fireEvent.click(screen.getByRole("button", { name: /live monitor/i }));
    await waitFor(() => {
      // "Run Failed" legitimately appears twice in LiveMonitor (status
      // label + terminal banner heading) -- assert at least one instead of
      // the ambiguous `getByText`.
      expect(screen.getAllByText(/run failed/i).length).toBeGreaterThan(0);
      expect(screen.getByText(/engine crashed mid-run/i)).toBeInTheDocument();
    });
  });

  it("processes a RunComplete delivered while LiveMonitor is unmounted: isRunInProgress unblocks and hasResults flips on", async () => {
    await startRunAndNavigateAway();

    emit({ type: "RunComplete", run_id: "run-123" });

    await waitFor(() => {
      expect(screen.queryByText("ACTIVE")).not.toBeInTheDocument();
      // hasResults (initially false, so no badge) is now true.
      expect(screen.getByText("RESULTS")).toBeInTheDocument();
    });
  });
});

describe("App falls back off a tab that stops being valid", () => {
  beforeEach(() => {
    mockListProjects.mockReset();
    mockGetRunSnapshot.mockReset().mockResolvedValue(null);
    mockSaveProject.mockReset().mockResolvedValue(undefined);
  });

  it("returns to the dashboard when selectedProject reverts to null while on the Builder tab", async () => {
    // Mirrors a real (if rare) path: handleUpdateProject saves, then
    // re-fetches the project list and re-selects by id -- if the refetch
    // no longer contains it (e.g. a save/delete race), selectedProject
    // becomes null while the user is still sitting on the Builder tab.
    mockListProjects
      .mockResolvedValueOnce([REAL_PROJECT]) // initial mount
      .mockResolvedValueOnce([]); // refetch after the update below
    render(<App />);
    await waitFor(() => expect(screen.getByText("Real Project One")).toBeInTheDocument());

    fireEvent.click(screen.getByText("Edit Matrix"));
    await waitFor(() => expect(screen.getByText("Add Scenario")).toBeInTheDocument());
    expect(screen.getByText("Matrix Builder")).toBeInTheDocument(); // the nav tab itself

    fireEvent.click(screen.getByText("Add Scenario"));

    await waitFor(() => {
      expect(screen.queryByText("Matrix Builder")).not.toBeInTheDocument();
      // Back on the dashboard -- asserted via a marker that doesn't depend
      // on the (now-empty, per the mocked refetch) project list itself.
      expect(screen.getByText("New Benchmark Project")).toBeInTheDocument();
    });
  });
});
