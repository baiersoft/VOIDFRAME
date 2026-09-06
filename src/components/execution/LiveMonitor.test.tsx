import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import type { Project, EngineEvent } from "../../lib/bindings";
import type { RunOutcome } from "../../App";

const mockGetRunSnapshot = vi.fn();
const mockSendControl = vi.fn();
let engineEventHandler: ((e: EngineEvent) => void) | null = null;
const mockSubscribeToEngineEvents = vi.fn((handler: (e: EngineEvent) => void) => {
  engineEventHandler = handler;
  return () => {
    engineEventHandler = null;
  };
});

vi.mock("../../lib/api", () => ({
  getRunSnapshot: (...args: unknown[]) => mockGetRunSnapshot(...args),
  sendControl: (...args: unknown[]) => mockSendControl(...args),
  subscribeToEngineEvents: (handler: (e: EngineEvent) => void) => mockSubscribeToEngineEvents(handler),
}));

import { LiveMonitor } from "./LiveMonitor";

// jsdom does not implement scrollIntoView; LiveMonitor calls it on every log
// update to auto-scroll the terminal, so stub it once for every test here.
Element.prototype.scrollIntoView = vi.fn();

const sampleProject: Project = {
  schema_version: "1.0.0",
  id: "p1",
  name: "Test Project",
  description: "d",
  created_at: "2026-09-01T00:00:00Z",
  settings: {},
  baseline: { name: "Stock", description: "d" },
  scenarios: [{ id: "s1", name: "S1", description: "d", enabled: true, modules: [] }],
};

function emit(event: EngineEvent) {
  engineEventHandler?.(event);
}

// Matches sampleProject.id ("p1") so `getRunSnapshot`'s resolved snapshot
// reflects a genuinely live run for the currently-selected project -- since
// LiveMonitor's "active" determination is now `snapMatches` alone (a stale
// or wrong-project runId must NOT present as live), most tests below need
// this rather than a bare non-null runId to be considered active.
const activeSnapshot = {
  schema_version: "1.0.0",
  run_id: "r1",
  project_id: "p1",
  phase: { kind: "preflight" },
  current_scenario: null,
  completed_scenarios: [],
  revision: 1,
};

describe("LiveMonitor", () => {
  beforeEach(() => {
    mockGetRunSnapshot.mockReset().mockResolvedValue(activeSnapshot);
    mockSendControl.mockReset().mockResolvedValue(undefined);
    mockSubscribeToEngineEvents.mockClear();
    engineEventHandler = null;
  });

  it("shows an empty state and does not subscribe when there is no active run", async () => {
    mockGetRunSnapshot.mockResolvedValue(null);
    render(
      <LiveMonitor project={sampleProject} runId={null} runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => {
      expect(screen.getByText(/no active run/i)).toBeInTheDocument();
    });
    expect(mockSubscribeToEngineEvents).not.toHaveBeenCalled();
  });

  it("subscribes and renders real events as they arrive", async () => {
    render(
      <LiveMonitor project={sampleProject} runId="r1" runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => expect(mockSubscribeToEngineEvents).toHaveBeenCalled());

    emit({ type: "PhaseChanged", phase: { kind: "baseline" } });
    emit({ type: "LogLine", text: "Waiting for map load" });
    // "baseline" now legitimately appears in two places (the phase heading
    // and the derived current-scenario line -- see the PhaseChanged case
    // below), so scope to the heading specifically.
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: /phase: baseline/i })).toBeInTheDocument();
    });
    // PhaseChanged with phase "baseline" also derives currentScenario as
    // "baseline" (mirrors the engine's `Phase::scenario_id` / this file's own
    // `phaseScenarioId` rule).
    expect(screen.getByText(/current scenario: baseline/i)).toBeInTheDocument();
    expect(screen.getByText(/waiting for map load/i)).toBeInTheDocument();
  });

  it("derives currentScenario from PhaseChanged per phaseScenarioId's rule", async () => {
    render(
      <LiveMonitor project={sampleProject} runId="r1" runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => expect(mockSubscribeToEngineEvents).toHaveBeenCalled());

    emit({ type: "PhaseChanged", phase: { kind: "scenario", id: "s1" } });
    await waitFor(() => {
      expect(screen.getByText(/current scenario: s1/i)).toBeInTheDocument();
    });

    // A run-scoped phase (e.g. "rollback") identifies no scenario and must
    // leave currentScenario untouched -- a crash during rollback still
    // needs to know which scenario was last active.
    emit({ type: "PhaseChanged", phase: { kind: "rollback" } });
    await waitFor(() => {
      expect(screen.getByRole("heading", { name: /phase: rollback/i })).toBeInTheDocument();
    });
    expect(screen.getByText(/current scenario: s1/i)).toBeInTheDocument();
  });

  it("renders a distinct, human-readable label with a progress indicator for the thermal_baseline phase", async () => {
    render(
      <LiveMonitor project={sampleProject} runId="r1" runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => expect(mockSubscribeToEngineEvents).toHaveBeenCalled());

    emit({ type: "PhaseChanged", phase: { kind: "thermal_baseline" } });
    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: /collecting thermal baseline/i })
      ).toBeInTheDocument();
    });
    // Raw "Phase: thermal_baseline" text must not also be shown -- the
    // heading is fully replaced by the human-readable label for this phase.
    expect(screen.queryByText(/phase: thermal_baseline/i)).not.toBeInTheDocument();

    // Indeterminate spinner (no exact elapsed-time signal is available from
    // existing events without new plumbing -- see PhaseChanged/LogLine).
    const heading = screen.getByRole("heading", { name: /collecting thermal baseline/i });
    expect(heading.querySelector(".animate-spin")).toBeInTheDocument();
  });

  it("regression: a live PhaseChanged event received before getRunSnapshot() resolves is not clobbered once the view becomes visible", async () => {
    // getRunSnapshot() and the live event subscription both start when
    // `runId` is already known -- the subscription is live immediately, but
    // `hasActiveRun` (which gates whether the real view or the "Checking
    // for an active run…" loading branch renders) only flips true once the
    // snapshot resolves. A live event that arrives *before* that resolution
    // still updates `phase` correctly, but invisibly (the loading branch is
    // still showing) -- and before this fix, the snapshot's own `.then()`
    // unconditionally called setPhase(snap.phase) as it flips the view
    // visible, silently overwriting that already-current phase with
    // whatever stale value the snapshot read captured. This exactly matches
    // what was observed live: HWiNFO visibly running (thermal baseline
    // collection genuinely already in progress) while the screen, the
    // moment it became visible, was stuck showing "preflight".
    let resolveSnapshot: (value: typeof activeSnapshot) => void = () => {};
    mockGetRunSnapshot.mockReturnValue(
      new Promise<typeof activeSnapshot>((resolve) => {
        resolveSnapshot = resolve;
      })
    );

    render(
      <LiveMonitor project={sampleProject} runId="r1" runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => expect(mockSubscribeToEngineEvents).toHaveBeenCalled());

    // Arrives while the snapshot fetch is still in flight -- the component
    // is still showing the loading branch at this point.
    emit({ type: "PhaseChanged", phase: { kind: "thermal_baseline" } });

    // The snapshot now resolves, carrying a stale phase captured before the
    // live event above -- it must not win once the view becomes visible.
    resolveSnapshot({ ...activeSnapshot, phase: { kind: "preflight" } });

    await waitFor(() => {
      expect(
        screen.getByRole("heading", { name: /collecting thermal baseline/i })
      ).toBeInTheDocument();
    });
    expect(screen.queryByRole("heading", { name: /phase: preflight/i })).not.toBeInTheDocument();
  });

  it("sends Pause/Resume via sendControl and tracks pause as local-only optimistic state", async () => {
    render(
      <LiveMonitor project={sampleProject} runId="r1" runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => expect(mockSubscribeToEngineEvents).toHaveBeenCalled());
    fireEvent.click(screen.getByText(/^pause$/i));
    await waitFor(() => {
      expect(mockSendControl).toHaveBeenCalledWith("Pause");
    });
    expect(screen.getByText(/^resume$/i)).toBeInTheDocument();
    fireEvent.click(screen.getByText(/^resume$/i));
    await waitFor(() => {
      expect(mockSendControl).toHaveBeenCalledWith("Resume");
    });
  });

  it("renders an OperatorPrompt and sends OperatorAcknowledged on click", async () => {
    render(
      <LiveMonitor project={sampleProject} runId="r1" runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => expect(mockSubscribeToEngineEvents).toHaveBeenCalled());
    emit({ type: "OperatorPrompt", text: "Please close Steam, then click Acknowledge" });
    await waitFor(() => {
      expect(screen.getByText(/please close steam/i)).toBeInTheDocument();
    });
    fireEvent.click(screen.getByRole("button", { name: /acknowledge/i }));
    await waitFor(() => {
      expect(mockSendControl).toHaveBeenCalledWith("OperatorAcknowledged");
    });
  });

  it("treats RunComplete/RunFailed on its own subscription as a no-op -- the terminal banner comes only from the runOutcome prop (App.tsx owns run-lifecycle state)", async () => {
    // This is the regression test for the tab-switch bug (see App.test.tsx
    // for the App-level half of the fix): LiveMonitor used to derive its
    // own terminal state directly from RunComplete/RunFailed on its own
    // subscription, which meant a terminal event delivered while this
    // component was unmounted (a tab switch away from Monitor) was lost
    // entirely -- nothing else was listening. Now App.tsx's own,
    // app-lifetime subscription is the only thing that processes these
    // events, and LiveMonitor only ever reflects that via the `runOutcome`
    // prop -- so its own subscription receiving the same event must change
    // nothing by itself.
    const { rerender } = render(
      <LiveMonitor project={sampleProject} runId="r1" runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => expect(mockSubscribeToEngineEvents).toHaveBeenCalled());

    emit({ type: "RunComplete", run_id: "r1" });
    expect(screen.queryByText(/run complete/i)).not.toBeInTheDocument();
    expect(screen.getByText(/autonomous pipeline active/i)).toBeInTheDocument();

    // Simulates App.tsx's separate, app-lifetime subscription having
    // processed the same event and passed the result down as a prop.
    rerender(
      <LiveMonitor
        project={sampleProject}
        runId="r1"
        runOutcome={{ kind: "complete", runId: "r1" }}
        onBackToBuilder={vi.fn()}
      />
    );
    await waitFor(() => {
      // "Run Complete" now legitimately appears twice (the status label and
      // the terminal banner's own heading) -- assert at least one instead
      // of the ambiguous `getByText`.
      expect(screen.getAllByText(/run complete/i).length).toBeGreaterThan(0);
    });
  });

  it("shows the correct terminal state (including the failure reason) on mount from the runOutcome prop alone, with no live event required", async () => {
    // Covers the actual bug scenario end-to-end from LiveMonitor's side: a
    // RunFailed delivered while this component was unmounted is processed
    // by App.tsx and handed down as `runOutcome` -- a fresh mount (the user
    // switching back to the Monitor tab) must show the real outcome and
    // reason immediately, not the live "Autonomous Pipeline Active" state.
    render(
      <LiveMonitor
        project={sampleProject}
        runId="r1"
        runOutcome={{ kind: "failed", runId: "r1", reason: "CS2 did not respond" }}
        onBackToBuilder={vi.fn()}
      />
    );
    await waitFor(() => {
      // "Run Failed" legitimately appears twice (status label + banner
      // heading) -- assert at least one instead of the ambiguous `getByText`.
      expect(screen.getAllByText(/run failed/i).length).toBeGreaterThan(0);
      expect(screen.getByText(/cs2 did not respond/i)).toBeInTheDocument();
    });
    expect(screen.queryByText(/autonomous pipeline active/i)).not.toBeInTheDocument();
  });

  it("reconciles against a snapshot from a prior mount (tab revisit, runId=null) instead of showing empty state", async () => {
    // Regression test for Critical finding 1: a user who navigates away from
    // the Monitor tab and back gets a fresh component instance with no
    // runId, but the backend's own RunState may still be tracking a
    // genuinely active run. This must render the reconciled state (not the
    // empty state) AND actually subscribe to live events -- otherwise the
    // user sees a frozen snapshot with no further updates.
    mockGetRunSnapshot.mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "r1",
      project_id: "p1",
      phase: { kind: "scenario", id: "s1" },
      current_scenario: "s1",
      completed_scenarios: [],
      revision: 4,
    });
    render(
      <LiveMonitor project={sampleProject} runId={null} runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => {
      expect(screen.getByText(/scenario:s1/i)).toBeInTheDocument();
    });
    expect(mockSubscribeToEngineEvents).toHaveBeenCalled();
  });

  it("regression (C1): subscribes to live events on the null-runId reconnect path, and events flow through afterwards", async () => {
    mockGetRunSnapshot.mockResolvedValue({
      schema_version: "1.0.0",
      run_id: "r1",
      project_id: "p1",
      phase: { kind: "baseline" },
      current_scenario: null,
      completed_scenarios: [],
      revision: 1,
    });
    render(
      <LiveMonitor project={sampleProject} runId={null} runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => {
      expect(mockSubscribeToEngineEvents).toHaveBeenCalled();
    });
    // Prove the subscription is live, not just established: an event
    // emitted after reconnect must actually update the UI.
    emit({ type: "LogLine", text: "Reconnected and streaming" });
    await waitFor(() => {
      expect(screen.getByText(/reconnected and streaming/i)).toBeInTheDocument();
    });
  });

  it("surfaces a sendControl rejection as visible error text", async () => {
    render(
      <LiveMonitor project={sampleProject} runId="r1" runOutcome={null} onBackToBuilder={vi.fn()} />
    );
    await waitFor(() => expect(mockSubscribeToEngineEvents).toHaveBeenCalled());
    mockSendControl.mockRejectedValueOnce(new Error("Engine unreachable"));
    fireEvent.click(screen.getByText(/^pause$/i));
    await waitFor(() => {
      expect(screen.getByText(/engine unreachable/i)).toBeInTheDocument();
    });
  });

  it("shows a View Results button when runOutcome is complete, calling onViewResults with the run_id", async () => {
    const onViewResults = vi.fn();
    const completeOutcome: RunOutcome = { kind: "complete", runId: "r1" };
    render(
      <LiveMonitor
        project={sampleProject}
        runId="r1"
        runOutcome={completeOutcome}
        onBackToBuilder={vi.fn()}
        onViewResults={onViewResults}
      />
    );
    await waitFor(() => {
      expect(screen.getByText(/view results/i)).toBeInTheDocument();
    });
    fireEvent.click(screen.getByText(/view results/i));
    expect(onViewResults).toHaveBeenCalledWith("r1");
  });

  it("does not show a View Results button when runOutcome is failed", async () => {
    const failedOutcome: RunOutcome = { kind: "failed", runId: "r1", reason: "CS2 crashed" };
    render(
      <LiveMonitor
        project={sampleProject}
        runId="r1"
        runOutcome={failedOutcome}
        onBackToBuilder={vi.fn()}
        onViewResults={vi.fn()}
      />
    );
    await waitFor(() => {
      expect(screen.getByText(/cs2 crashed/i)).toBeInTheDocument();
    });
    expect(screen.queryByText(/view results/i)).not.toBeInTheDocument();
  });
});
