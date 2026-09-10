import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent, act } from "@testing-library/react";
import App from "./App";
import type { EngineEvent, RunSummary } from "./lib/bindings";

const mockListProjects = vi.fn();
const mockGetRunSnapshot = vi.fn();
const mockValidateScenario = vi.fn();
const mockStartRun = vi.fn();
const mockGetResults = vi.fn();
const mockSaveProject = vi.fn();
const mockResumeRun = vi.fn();
// Safe default (normal launch, not `--resume`) for every existing test that
// doesn't care about the M3 autonomous-reboot resume-launch routing -- App
// now fetches this alongside getRunSnapshot() on mount to decide whether a
// non-null snapshot means "route straight into Monitor" (a real resume
// already in progress) or the legacy crash banner. Tests that DO care
// override with mockResolvedValueOnce/mockResolvedValue.
const mockIsResumeLaunch = vi.fn().mockResolvedValue(false);
// Safe default (no active-run progress) for every existing test that
// doesn't care about the M3 reboot/progress wiring -- App now fetches this
// once a run becomes active and again on PhaseChanged/ScenarioComplete.
const mockGetRunProgress = vi.fn().mockResolvedValue(null);
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
    isResumeLaunch: (...args: unknown[]) => mockIsResumeLaunch(...args),
    validateScenario: (...args: unknown[]) => mockValidateScenario(...args),
    startRun: (...args: unknown[]) => mockStartRun(...args),
    getConfig: (...args: unknown[]) => mockGetConfig(...args),
    getResults: (...args: unknown[]) => mockGetResults(...args),
    listResults: (...args: unknown[]) => mockListResults(...args),
    saveProject: (...args: unknown[]) => mockSaveProject(...args),
    resumeRun: (...args: unknown[]) => mockResumeRun(...args),
    getRunProgress: (...args: unknown[]) => mockGetRunProgress(...args),
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
  mockResumeRun.mockReset();
  mockGetRunProgress.mockReset().mockResolvedValue(null);
  mockIsResumeLaunch.mockReset().mockResolvedValue(false);
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
      expect(mockStartRun).toHaveBeenCalledWith("p1", false, false);
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
      expect(mockStartRun).toHaveBeenCalledWith("p1", true, false);
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
      expect(mockStartRun).toHaveBeenCalledWith("p1", false, false);
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

describe("App reboot-core wiring (M3)", () => {
  const PROGRESS_FOR_RUN_123 = {
    schema_version: "1.0.0",
    run_id: "run-123",
    project: REAL_PROJECT,
    start_build_id: null,
    start_launch_args: "",
    start_launch_args_raw: "",
    start_power_plan: { guid: "g", name: "Balanced", active: true },
    thermal_baseline: null,
    completed: [],
    unstable: [],
    cursor: { index: 0, stage: "apply" },
    reboot: null,
    shutdown_when_complete: true,
  };

  beforeEach(() => {
    mockListProjects.mockReset().mockResolvedValue([REAL_PROJECT]);
    mockGetRunSnapshot.mockReset().mockResolvedValue(null);
    mockValidateScenario.mockReset().mockResolvedValue([]);
    mockStartRun.mockReset().mockResolvedValue("run-123");
    mockGetResults.mockReset();
    mockGetConfig.mockReset().mockResolvedValue({});
  });

  it("fetches getRunProgress once a run becomes active, and again on PhaseChanged", async () => {
    mockGetRunProgress.mockResolvedValue(PROGRESS_FOR_RUN_123);
    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));
    fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));
    await waitFor(() => {
      expect(mockGetRunProgress).toHaveBeenCalledWith("run-123");
    });
    const callsAfterStart = mockGetRunProgress.mock.calls.length;

    emit({ type: "PhaseChanged", phase: { kind: "baseline" } });
    await waitFor(() => {
      expect(mockGetRunProgress.mock.calls.length).toBeGreaterThan(callsAfterStart);
    });
  });

  it("passes the config's shutdown_when_complete_default through the countdown modal into startRun's third argument", async () => {
    mockGetConfig.mockResolvedValue({ shutdown_when_complete_default: true });
    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));
    fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));
    await waitFor(() => {
      expect(mockStartRun).toHaveBeenCalledWith("p1", false, true);
    });
  });

  it("shows a Resume Run button on the dashboard for a reboot_pending crashed run, and resuming switches to the monitor tab", async () => {
    mockGetRunSnapshot.mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "run-999",
      project_id: "p1",
      phase: { kind: "reboot_pending", reason: "apply_next" },
      current_scenario: "s1",
      completed_scenarios: [],
      revision: 2,
    });
    mockResumeRun.mockResolvedValue("run-999");
    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));

    fireEvent.click(screen.getByRole("button", { name: /resume run/i }));
    await waitFor(() => {
      expect(mockResumeRun).toHaveBeenCalledTimes(1);
    });
    await waitFor(() => {
      expect(screen.queryByText("Real Project One")).not.toBeInTheDocument();
    });
  });

  it("shows a Cancel shutdown button on the Results tab once RunComplete arrives and the run's progress had shutdown_when_complete set", async () => {
    mockGetRunProgress.mockResolvedValue(PROGRESS_FOR_RUN_123);
    mockListResults.mockResolvedValue([
      { run_id: "run-123", project_id: "p1", completed_at: "2026-09-04T00:00:00Z", scenario_count: 0, winner_name: null, winner_wcps: null },
    ]);
    mockGetResults.mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "run-123",
      project_id: "p1",
      completed_at: "2026-09-04T00:00:00Z",
      detection_tier: "log_tail",
      baseline: {
        scenario_id: "baseline",
        name: "Stock",
        is_baseline: true,
        aggregated: {
          avg_fps: 400, median_fps: 400, p1_fps: 250, p01_fps: 180,
          frame_time_mean_ms: 2.5, frame_time_stddev_ms: 0.2, frame_time_cv: 0.08,
          adaptive_frame_time_cv: null, stutter_count_pct: null, mean_abs_animation_error_ms: null,
          gpu_busy_ms: null, bottleneck_ratio: null, render_latency_ms: null,
        },
        per_iteration: [], metric_deltas: [], wcps: 0, verdict: "confirmed_same",
      },
      scenarios: [],
    });
    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));
    fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));
    await waitFor(() => {
      expect(mockGetRunProgress).toHaveBeenCalledWith("run-123");
    });

    emit({ type: "RunComplete", run_id: "run-123" });
    await waitFor(() => screen.getByRole("button", { name: /visualizer & charts/i }));
    fireEvent.click(screen.getByRole("button", { name: /visualizer & charts/i }));

    await waitFor(() => {
      expect(screen.getByRole("button", { name: /cancel shutdown/i })).toBeInTheDocument();
    });
  });

  it("does not let a stale getRunProgress response from a previous run overwrite a newer run's progress", async () => {
    mockStartRun.mockReset().mockResolvedValueOnce("run-A").mockResolvedValueOnce("run-B");
    // Matches REAL_PROJECT's id ("p1") so LiveMonitor's own `hasActiveRun`
    // check resolves true and it renders its live view (including the
    // `progress.unstable` list used below) instead of "No active run".
    mockGetRunSnapshot.mockReset().mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "run-A",
      project_id: "p1",
      phase: { kind: "scenario", id: "s1" },
      current_scenario: "s1",
      completed_scenarios: [],
      revision: 1,
    });

    // run-A's fetch never resolves on its own -- resolved manually below,
    // after run-B has already become active, to simulate a PhaseChanged
    // response arriving late.
    let resolveStaleProgress: (value: unknown) => void = () => {};
    const staleProgressPromise = new Promise((resolve) => {
      resolveStaleProgress = resolve;
    });
    mockGetRunProgress.mockImplementation((runId: string) => {
      if (runId === "run-A") return staleProgressPromise;
      return Promise.resolve({
        ...PROGRESS_FOR_RUN_123,
        run_id: "run-B",
        unstable: [{ scenario_id: "fresh-scenario", reason: "fresh" }],
      });
    });

    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));

    fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));
    await waitFor(() => {
      expect(mockGetRunProgress).toHaveBeenCalledWith("run-A");
    });

    // A PhaseChanged event while run-A is still active starts a second,
    // also-pending fetch for run-A via `onEngineEvent`'s own switch case --
    // the code path finding 1 fixes.
    emit({ type: "PhaseChanged", phase: { kind: "baseline" } });

    // Back to the dashboard, then start a second run -- activeRunId flips
    // to run-B while run-A's getRunProgress is still unresolved.
    fireEvent.click(screen.getByRole("button", { name: /project explorer/i }));
    await waitFor(() => screen.getByRole("button", { name: /run matrix/i }));
    fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));

    await waitFor(() => {
      expect(screen.getByText("fresh-scenario")).toBeInTheDocument();
    });

    // Now the stale run-A response lands -- it must not clobber run-B's
    // already-rendered progress.
    resolveStaleProgress({
      ...PROGRESS_FOR_RUN_123,
      run_id: "run-A",
      unstable: [{ scenario_id: "stale-scenario", reason: "stale" }],
    });

    await waitFor(() => {
      expect(screen.getByText("fresh-scenario")).toBeInTheDocument();
    });
    expect(screen.queryByText("stale-scenario")).not.toBeInTheDocument();
  });

  it("retries the initial getRunProgress fetch when it races progress.json not existing yet, instead of leaving runProgress stuck at null", async () => {
    // Mirrors the real race this test covers: startRun's command returns
    // (and activeRunId is set) before the engine's PREFLIGHT+SNAPSHOT
    // phases have written progress.json for the first time, so the very
    // first getRunProgress call resolves `null` -- that is what the backend
    // actually returns for an absent progress.json (`Ok(None)`), never a
    // rejection. A matching snapshot lets LiveMonitor
    // (rendered once the run starts) resolve `hasActiveRun` true and show
    // its live view -- including the shutdown checkbox this test reads to
    // prove `runProgress` isn't stuck at `null`/stale after the retry.
    mockGetRunSnapshot.mockReset().mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "run-123",
      project_id: "p1",
      phase: { kind: "scenario", id: "s1" },
      current_scenario: "s1",
      completed_scenarios: [],
      revision: 1,
    });

    let callCount = 0;
    mockGetRunProgress.mockReset().mockImplementation((runId: string) => {
      if (runId !== "run-123") return Promise.resolve(null);
      callCount += 1;
      // The very first call loses the race (progress.json doesn't exist
      // yet, so the backend answers `null`); every later call (the retry)
      // succeeds.
      if (callCount === 1) return Promise.resolve(null);
      return Promise.resolve(PROGRESS_FOR_RUN_123);
    });

    render(<App />);
    await waitFor(() => screen.getByText("Real Project One"));

    vi.useFakeTimers();
    try {
      fireEvent.click(screen.getByRole("button", { name: /run matrix/i }));

      // Advance in small steps rather than one large jump: the
      // validateScenario -> getConfig -> startRun -> countdown modal's
      // zero-second auto-confirm -> handleRunProject chain (all real
      // Promise resolutions), the first failing getRunProgress attempt,
      // and its 500ms-later retry each only progress once React has
      // actually committed the previous step's state update (mirrors
      // RunCountdownModal.test.tsx's own fake-timer test, which found a
      // single large jump flakes for exactly this reason).
      for (let i = 0; i < 60; i++) {
        await act(async () => {
          await vi.advanceTimersByTimeAsync(10);
        });
      }

      // The retry happened (more than the one, losing attempt)...
      expect(callCount).toBeGreaterThanOrEqual(2);
      // ...and its successful response reached `runProgress` (and, through
      // it, LiveMonitor's checkbox) instead of leaving it stuck at `null`.
      expect(
        screen.getByRole("checkbox", { name: /shut down when the run completes/i })
      ).toBeChecked();
    } finally {
      vi.useRealTimers();
    }
  });

  it("routes straight into Monitor for a real --resume launch, without showing the crash banner or calling resumeRun again", async () => {
    mockGetRunSnapshot.mockReset().mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "resumed-run-1",
      project_id: "p1",
      phase: { kind: "boot_resume" },
      current_scenario: "s1",
      completed_scenarios: [],
      revision: 4,
    });
    mockIsResumeLaunch.mockReset().mockResolvedValue(true);

    render(<App />);

    // Landed straight on Monitor for the resumed run -- the nav bar's
    // "ACTIVE" badge is up, LiveMonitor rendered for the right project (it
    // resolves the same activeRunId via its own getRunSnapshot() check),
    // and the dashboard's project list is gone.
    await waitFor(() => {
      expect(screen.getByText("ACTIVE")).toBeInTheDocument();
      expect(screen.getByText(/Project: Real Project One/)).toBeInTheDocument();
    });
    expect(screen.queryByText("Real Project One", { selector: "h3" })).not.toBeInTheDocument();
    expect(screen.queryByText(/did not complete cleanly/i)).not.toBeInTheDocument();
    // The backend's own --resume setup hook already resumed this run --
    // the frontend must not call resumeRun() a second time.
    expect(mockResumeRun).not.toHaveBeenCalled();
  });

  it("still shows the crash banner for a non-null snapshot when isResumeLaunch resolves false (a normal launch)", async () => {
    mockGetRunSnapshot.mockReset().mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "stranded-run-1",
      project_id: "p1",
      phase: { kind: "boot_resume" },
      current_scenario: "s1",
      completed_scenarios: [],
      revision: 4,
    });
    mockIsResumeLaunch.mockReset().mockResolvedValue(false);

    render(<App />);

    await waitFor(() => {
      expect(screen.getByText(/did not complete cleanly/i)).toBeInTheDocument();
    });
    expect(screen.queryByText("ACTIVE")).not.toBeInTheDocument();
    expect(mockResumeRun).not.toHaveBeenCalled();
  });
});

