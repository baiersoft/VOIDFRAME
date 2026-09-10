import React, { useState, useEffect, useEffectEvent, useRef } from "react";
import { BackgroundGlow } from "./components/BackgroundGlow";
import { TitleBar } from "./components/TitleBar";
import { HeaderBar } from "./components/HeaderBar";
import { NavigationTabs, NavTab, isTabVisible } from "./components/NavigationTabs";
import { ProjectExplorer } from "./components/dashboard/ProjectExplorer";
import { MatrixBuilder } from "./components/builder/MatrixBuilder";
import { LiveMonitor } from "./components/execution/LiveMonitor";
import { ResultsVisualizer } from "./components/charts/ResultsVisualizer";
import { TweakCatalog } from "./components/library/TweakCatalog";
import { PreflightModal } from "./components/modals/PreflightModal";
import { SettingsModal } from "./components/modals/SettingsModal";
import { NewProjectModal } from "./components/modals/NewProjectModal";
import { RunCountdownModal } from "./components/modals/RunCountdownModal";
import { AlertCircle, AlertTriangle, X } from "lucide-react";
import {
  listProjects,
  saveProject,
  deleteProject,
  getConfig,
  getResults,
  listResults,
  getRunProgress,
  getRunSnapshot,
  isResumeLaunch,
  resumeRun,
  rollbackNow,
  startRun,
  subscribeToEngineEvents,
  validateScenario,
} from "./lib/api";
import type { Config, EngineEvent, Module, Project, RunProgress, RunResults, RunState, RunSummary } from "./lib/bindings";
import { phaseLabel } from "./lib/phase";
import { countReboots, scenarioRequiresReboot } from "./lib/reboots";
import { EmergencyRestoreModal } from "./components/modals/EmergencyRestoreModal";

// The run-lifecycle *outcome* of the most recently started run, as reported
// by the `RunComplete`/`RunFailed` engine events. Owned by App (see the
// app-lifetime subscription effect below) rather than by LiveMonitor, so a
// terminal event delivered while the user has navigated away from the
// Monitor tab (which unmounts LiveMonitor and its own event subscription)
// is not lost -- switching back to Monitor still reflects the real outcome,
// and `isRunInProgress`-gated navigation/buttons unblock correctly even if
// the user never had LiveMonitor mounted when the run actually ended.
export interface RunOutcome {
  kind: "complete" | "failed";
  runId: string | null;
  reason?: string;
}

export const App: React.FC = () => {
  const [activeTab, setActiveTab] = useState<NavTab>("dashboard");
  const [projects, setProjects] = useState<Project[]>([]);
  const [selectedProject, setSelectedProject] = useState<Project | null>(null);
  const [isLoadingProjects, setIsLoadingProjects] = useState(true);
  const [projectsError, setProjectsError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [activeRunId, setActiveRunId] = useState<string | null>(null);
  // Seeds LiveMonitor's shutdown checkbox (the countdown modal's choice)
  // until the first progress.json exists -- see LiveMonitor's own
  // `initialShutdownWhenComplete` prop doc comment. The resume path leaves
  // this false; a resumed run has progress.json on disk, which takes
  // precedence anyway.
  const [startedShutdownWhenComplete, setStartedShutdownWhenComplete] = useState(false);
  const [runStartError, setRunStartError] = useState<string | null>(null);
  const [runValidationErrors, setRunValidationErrors] = useState<string[] | null>(null);
  const [isRunInProgress, setIsRunInProgress] = useState(false);
  const [isStartingRun, setIsStartingRun] = useState(false);
  const [pendingRunProject, setPendingRunProject] = useState<Project | null>(null);
  // Read once validateScenario passes (handleRequestRunProject) -- feeds
  // both the countdown modal's `shutdownDefault` and the eventual
  // `startRun`'s `dryRun` argument, so both reflect the exact same fetch
  // (see the comment on `handleRequestRunProject` for why this isn't
  // fetched twice).
  const [pendingConfig, setPendingConfig] = useState<Config | null>(null);
  // Per-project results history (spec: results only mean anything in their
  // own project's context -- a WCPS score is meaningless without knowing
  // which baseline it was measured against, so this is deliberately never a
  // single global flag/list). Newest-first, matching list_results' own sort.
  const [resultSummaries, setResultSummaries] = useState<Record<string, RunSummary[]>>({});
  const hasResults = selectedProject
    ? (resultSummaries[selectedProject.id]?.length ?? 0) > 0
    : false;
  const [runOutcome, setRunOutcome] = useState<RunOutcome | null>(null);
  const [viewedResults, setViewedResults] = useState<RunResults | null>(null);
  const [isLoadingResults, setIsLoadingResults] = useState(false);
  const [resultsError, setResultsError] = useState<string | null>(null);
  // Fetched when a run becomes active (`activeRunId`) and refreshed on every
  // `PhaseChanged`/`ScenarioComplete` the app-lifetime subscription below
  // already handles -- passed straight down to LiveMonitor, which does no
  // polling of its own (see LiveMonitor's own `progress` prop doc comment).
  const [runProgress, setRunProgress] = useState<RunProgress | null>(null);
  // Mirrors `activeRunId` for `onEngineEvent` below (a `useEffectEvent`,
  // called once per event with no re-run of its own to hang a `cancelled`
  // closure off) so its PhaseChanged/ScenarioComplete getRunProgress fetch
  // can tell, once the promise resolves, whether a newer run has since
  // started and discard a stale response instead of overwriting it.
  const activeRunIdRef = useRef<string | null>(null);
  // Set from the `RunComplete` case below when the run's own progress had
  // `shutdown_when_complete` true -- drives the Results tab's "Cancel
  // shutdown" button. Distinct from `RunOutcome` (which stays binary
  // complete/failed for LiveMonitor's terminal banner) on purpose; see
  // ResultsVisualizer's own `runOutcome` prop doc comment for why.
  const [shutdownRequested, setShutdownRequested] = useState(false);

  const refreshProjects = async () => {
    setIsLoadingProjects(true);
    setProjectsError(null);
    try {
      const real = await listProjects();
      setProjects(real);
      return real;
    } catch (e) {
      setProjectsError(e instanceof Error ? e.message : String(e));
      return [];
    } finally {
      setIsLoadingProjects(false);
    }
  };

  useEffect(() => {
    refreshProjects();
  }, []);

  // Best-effort: one project's result history failing to load must not
  // block the rest of the app -- it just won't show a preview/tab for that
  // one project until it succeeds.
  const refreshResultSummaries = async (projectId: string) => {
    try {
      const summaries = await listResults(projectId);
      setResultSummaries((prev) => ({ ...prev, [projectId]: summaries }));
    } catch {
      // swallowed deliberately -- see comment above
    }
  };

  useEffect(() => {
    projects.forEach((p) => {
      refreshResultSummaries(p.id);
    });
  }, [projects]);

  // Auto-defaults the Results tab to the selected project's own most recent
  // run (list_results is already sorted newest-first) -- results only mean
  // anything in their own project's context, so there is no cross-project
  // "last result" to fall back to. A no-op once the right run is already
  // loaded, so this can't fight the explicit `handleViewRunResults` call
  // LiveMonitor's own "View Results" button makes.
  const loadLatestResultForSelectedProject = useEffectEvent(() => {
    if (!selectedProject) return;
    const latest = resultSummaries[selectedProject.id]?.[0];
    if (!latest || viewedResults?.run_id === latest.run_id) return;
    handleViewRunResults(latest.run_id);
  });

  useEffect(() => {
    if (activeTab === "results") {
      loadLatestResultForSelectedProject();
    }
  }, [activeTab, selectedProject, resultSummaries]);

  // Keeps `activeTab` from ever pointing at a tab that isn't currently shown
  // in the nav bar -- e.g. `selectedProject` reverting to `null` after a
  // save/refresh mismatch (handleUpdateProject/handleCreateNewProject) while
  // the user is on the Builder tab. Falls back to the dashboard, the one
  // tab that's always valid. Uses the exact same `isTabVisible` gating
  // NavigationTabs itself renders from, so there is only one source of
  // truth for "is this tab currently meaningful."
  useEffect(() => {
    const flags = { hasProject: selectedProject !== null, hasRun: activeRunId !== null, hasResults };
    if (!isTabVisible(activeTab, flags)) {
      setActiveTab("dashboard");
    }
  }, [activeTab, selectedProject, activeRunId, hasResults]);

  // Subscribes to the engine's event stream for the app's full lifetime --
  // not just while LiveMonitor happens to be mounted -- so a RunComplete/
  // RunFailed delivered while the user has navigated to another tab (which
  // unmounts LiveMonitor and its own, separate subscription) is not lost.
  // This is the ONLY place run-lifecycle state (isRunInProgress, runOutcome)
  // is updated; LiveMonitor keeps its own subscription purely for its own
  // live progress display (phase/log lines/current scenario/etc.) and
  // renders its terminal banner from the `runOutcome` state passed down as
  // a prop instead of tracking outcome itself.
  const onEngineEvent = useEffectEvent((event: EngineEvent) => {
    switch (event.type) {
      case "RunComplete":
        setIsRunInProgress(false);
        setRunOutcome({ kind: "complete", runId: event.run_id });
        // `shutdown_when_complete` is a live value the user can toggle
        // mid-run (see `drain_shutdown_toggle` in the engine's run/execute)
        // -- `runProgress` is kept current by the polling below (including
        // its own retry for the very first fetch), so whatever it holds
        // right now is the most recent value the engine actually reported.
        setShutdownRequested(runProgress?.shutdown_when_complete === true);
        // execute() writes results.json before emitting RunComplete, so a
        // fresh list_results call made right now already sees the new run.
        if (selectedProject) {
          refreshResultSummaries(selectedProject.id);
        }
        break;
      case "RunFailed":
        setIsRunInProgress(false);
        setRunOutcome({ kind: "failed", runId: activeRunId, reason: event.reason });
        break;
      case "PhaseChanged":
      case "ScenarioComplete":
      case "ScenarioScored":
        // Keeps LiveMonitor's `progress` prop current without LiveMonitor
        // polling itself -- see its own prop doc comment. Guarded against
        // a new run starting (activeRunId changing) before this fetch
        // resolves -- see the `activeRunIdRef` doc comment above.
        if (activeRunId) {
          const fetchedForRunId = activeRunId;
          getRunProgress(fetchedForRunId)
            .then((p) => {
              if (activeRunIdRef.current === fetchedForRunId) setRunProgress(p);
            })
            .catch(() => {});
        }
        break;
      default:
        break;
    }
  });

  useEffect(() => {
    const unsubscribe = subscribeToEngineEvents(onEngineEvent);
    return () => {
      unsubscribe();
    };
  }, []);

  // Fetches the run's own progress once it becomes active (a fresh
  // `startRun`/`resumeRun`), in addition to the PhaseChanged/ScenarioComplete
  // refreshes above -- covers the window between "run started" and the
  // first such event, and the resume path (which may not emit PhaseChanged
  // immediately).
  //
  // `startRun`'s command returns as soon as the run is spawned as a
  // background task -- well before the engine's PREFLIGHT+SNAPSHOT phases
  // have actually run and written `progress.json` to disk for the first
  // time. That means this very first fetch (unlike the PhaseChanged/
  // ScenarioComplete-triggered ones above, which fire later in the run's
  // life once the file reliably exists) can easily race a file that isn't
  // there yet and fail. A single-shot fetch with no retry would then leave
  // `runProgress` stuck at `null` -- visibly stale in the UI (e.g. Live
  // Monitor's shutdown checkbox) -- until some later, unrelated
  // PhaseChanged/ScenarioComplete event happens to fire. So retry every
  // 500ms while the fetch either rejects or resolves `null` (the backend's
  // `get_run_progress` answers an absent `progress.json` with `Ok(None)`,
  // not an error -- the race shows up as `null`, so retrying only from
  // `.catch` never fired), bounded to roughly how long PREFLIGHT+SNAPSHOT
  // could plausibly take; if it's still missing after that, the
  // PhaseChanged-triggered effect above will pick it up once the engine
  // progresses, so there's no need to retry forever.
  useEffect(() => {
    activeRunIdRef.current = activeRunId;
    if (!activeRunId) {
      setRunProgress(null);
      return;
    }
    let cancelled = false;
    let timeoutId: ReturnType<typeof setTimeout> | undefined;
    const retryDelayMs = 500;
    const maxAttempts = 30; // ~15s of retries at 500ms apart

    const attempt = (attemptsSoFar: number) => {
      const scheduleRetry = () => {
        if (cancelled) return;
        if (attemptsSoFar + 1 >= maxAttempts) return;
        timeoutId = setTimeout(() => attempt(attemptsSoFar + 1), retryDelayMs);
      };
      getRunProgress(activeRunId)
        .then((p) => {
          if (cancelled) return;
          setRunProgress(p);
          if (p === null) scheduleRetry();
        })
        .catch(scheduleRetry);
    };
    attempt(0);

    return () => {
      cancelled = true;
      if (timeoutId !== undefined) clearTimeout(timeoutId);
    };
  }, [activeRunId]);

  const [crashedRun, setCrashedRun] = useState<RunState | null>(null);
  const [isRollingBackCrash, setIsRollingBackCrash] = useState(false);
  const [crashRollbackError, setCrashRollbackError] = useState<string | null>(null);
  const [isEmergencyRestoreOpen, setIsEmergencyRestoreOpen] = useState(false);

  // On-disk run snapshot found at mount, paired with whether THIS process
  // launch was itself a `--resume` invocation -- both facts are needed
  // before the routing effect below can decide anything (see its own
  // comment for why neither alone is enough), so they're captured together
  // once both promises have resolved rather than branching off of either
  // one's resolution alone.
  const [resumeLaunchInfo, setResumeLaunchInfo] = useState<{
    snapshot: RunState | null;
    isResume: boolean;
  } | null>(null);

  useEffect(() => {
    let cancelled = false;
    Promise.all([getRunSnapshot().catch(() => null), isResumeLaunch().catch(() => false)]).then(
      ([snapshot, isResume]) => {
        if (!cancelled) setResumeLaunchInfo({ snapshot, isResume });
      }
    );
    return () => {
      cancelled = true;
    };
  }, []);

  // Guards the routing effect below from re-running once it has already
  // acted. `resumeLaunchInfo` itself is set once (on mount) and never
  // changes again, but the effect also needs `projects` loaded to resolve
  // the resumed run's `project_id` into a real `Project` -- and `projects`
  // legitimately changes many times over the app's life (every later
  // save/delete). Without this guard, the first `projects` update that
  // lands after `resumeLaunchInfo` would route correctly, but every LATER
  // unrelated `projects` refresh would replay the same routing and clobber
  // whatever run/tab state the user has since moved to.
  const hasRoutedResumeLaunchRef = useRef(false);

  useEffect(() => {
    if (!resumeLaunchInfo || hasRoutedResumeLaunchRef.current) return;
    const { snapshot, isResume } = resumeLaunchInfo;
    if (snapshot && isResume) {
      // The backend's `--resume` setup hook already resumed this exact run
      // in the background -- route straight into Monitor to reflect that,
      // without calling `resumeRun()` again (that's `handleResumeCrashedRun`'s
      // job, for the genuinely-stranded case where no `--resume` launch
      // happened at all).
      if (isLoadingProjects) return; // wait for `projects` before resolving project_id
      const resumedProject = projects.find((p) => p.id === snapshot.project_id) ?? null;
      if (resumedProject) setSelectedProject(resumedProject);
      setActiveRunId(snapshot.run_id);
      setIsRunInProgress(true);
      setActiveTab("monitor");
    } else {
      // Either no on-disk run at all, or one exists but this launch wasn't
      // a `--resume` invocation -- today's unchanged crash-banner behavior
      // (a `null` snapshot here is a no-op, matching the old
      // `getRunSnapshot().then(setCrashedRun)`).
      setCrashedRun(snapshot);
    }
    hasRoutedResumeLaunchRef.current = true;
  }, [resumeLaunchInfo, projects, isLoadingProjects]);

  const handleRollbackCrashedRun = async () => {
    setIsRollingBackCrash(true);
    setCrashRollbackError(null);
    try {
      await rollbackNow();
      setCrashedRun(null);
    } catch (e) {
      setCrashRollbackError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsRollingBackCrash(false);
    }
  };

  // Modal States
  const [isPreflightOpen, setIsPreflightOpen] = useState(false);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [isNewProjectOpen, setIsNewProjectOpen] = useState(false);
  const [isCatalogOpen, setIsCatalogOpen] = useState(false);
  const [catalogTargetScenarioId, setCatalogTargetScenarioId] = useState<string>("");

  // Handler Actions
  const handleSelectProject = (project: Project) => {
    setSelectedProject(project);
    setActiveTab("builder");
  };

  const handleEditMatrix = (project: Project) => {
    setSelectedProject(project);
    setActiveTab("builder");
  };

  const handleRequestRunProject = async (project: Project) => {
    setIsStartingRun(true);
    setRunStartError(null);
    setRunValidationErrors(null);
    try {
      await validateScenario(project);
    } catch (e) {
      setRunValidationErrors([e instanceof Error ? e.message : String(e)]);
      setIsStartingRun(false);
      return;
    }
    try {
      // Read fresh rather than cached: reflects whatever the user last
      // saved in Settings, including a change made after this app session
      // started. Fetched once here (not again in handleRunProject) so the
      // countdown modal's `shutdownDefault` and the eventual `startRun`
      // dryRun argument both come from the exact same read.
      const config = await getConfig();
      setPendingConfig(config);
      setPendingRunProject(project);
    } catch (e) {
      setRunStartError(e instanceof Error ? e.message : String(e));
      setIsStartingRun(false);
    }
  };

  const handleCancelPendingRun = () => {
    setPendingRunProject(null);
    setPendingConfig(null);
    setIsStartingRun(false);
  };

  const handleRunProject = async (project: Project, opts: { shutdownWhenComplete: boolean }) => {
    try {
      setStartedShutdownWhenComplete(opts.shutdownWhenComplete);
      const runId = await startRun(project.id, pendingConfig?.dry_run_default ?? false, opts.shutdownWhenComplete);
      setActiveRunId(runId);
      setCrashedRun(null);
      setViewedResults(null);
      setResultsError(null);
      setRunOutcome(null);
      setShutdownRequested(false);
      setIsRunInProgress(true);
      setSelectedProject(project);
      setActiveTab("monitor");
    } catch (e) {
      setRunStartError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsStartingRun(false);
    }
  };

  const handleResumeCrashedRun = async () => {
    try {
      const runId = await resumeRun();
      setActiveRunId(runId);
      const resumedProject = projects.find((p) => p.id === crashedRun?.project_id) ?? null;
      if (resumedProject) setSelectedProject(resumedProject);
      setCrashedRun(null);
      setViewedResults(null);
      setResultsError(null);
      setRunOutcome(null);
      setShutdownRequested(false);
      setIsRunInProgress(true);
      setActiveTab("monitor");
    } catch (e) {
      setRunStartError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleViewResults = (project: Project) => {
    setSelectedProject(project);
    setActiveTab("results");
  };

  const handleViewRunResults = async (runId: string) => {
    setResultsError(null);
    setActiveTab("results");
    setIsLoadingResults(true);
    try {
      const results = await getResults(runId);
      setViewedResults(results);
    } catch (e) {
      setResultsError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsLoadingResults(false);
    }
  };

  const handleBackToBuilder = () => {
    setActiveRunId(null);
    setActiveTab("builder");
  };

  const handleOpenCatalogForScenario = (scenarioId: string) => {
    setCatalogTargetScenarioId(scenarioId);
    setIsCatalogOpen(true);
  };

  const handleAddModuleToActiveScenario = (module: Module, scenarioId: string) => {
    if (!selectedProject || !scenarioId) return;
    const updatedScenarios = (selectedProject.scenarios ?? []).map((scen) => {
      if (scen.id === scenarioId) {
        const updatedModules = [...(scen.modules ?? []), module];
        return { ...scen, modules: updatedModules };
      }
      return scen;
    });
    const updatedProj = { ...selectedProject, scenarios: updatedScenarios };
    handleUpdateProject(updatedProj);
  };

  const handleUpdateProject = async (updated: Project) => {
    try {
      setSaveError(null);
      await saveProject(updated);
      const refreshed = await refreshProjects();
      setSelectedProject(refreshed.find((p) => p.id === updated.id) ?? null);
    } catch (e) {
      setSaveError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleCreateNewProject = async (newProj: Project) => {
    try {
      setSaveError(null);
      await saveProject(newProj);
      const refreshed = await refreshProjects();
      setSelectedProject(refreshed.find((p) => p.id === newProj.id) ?? null);
      setActiveTab("builder");
    } catch (e) {
      setSaveError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleDeleteProject = async (projectId: string) => {
    try {
      setSaveError(null);
      await deleteProject(projectId);
      await refreshProjects();
      if (selectedProject?.id === projectId) {
        setSelectedProject(null);
      }
    } catch (e) {
      setSaveError(e instanceof Error ? e.message : String(e));
    }
  };

  const handleEmergencyRestore = () => {
    setIsEmergencyRestoreOpen(true);
  };

  return (
    <div className="relative w-screen h-screen overflow-hidden bg-[#010103] flex flex-col select-none border border-white/10 rounded-none shadow-2xl">
      {/* Custom Frameless Draggable TitleBar */}
      <TitleBar />

      {/* Dynamic Ambient Void Lighting */}
      <BackgroundGlow />

      {/* Streamlined Top Control Header */}
      <HeaderBar
        onOpenSettings={() => setIsSettingsOpen(true)}
        onOpenPreflight={() => setIsPreflightOpen(true)}
        onEmergencyRestore={handleEmergencyRestore}
      />

      {/* Navigation Tabs Bar */}
      <NavigationTabs
        activeTab={activeTab}
        onSelectTab={setActiveTab}
        hasProject={selectedProject !== null}
        hasRun={activeRunId !== null}
        isRunning={isRunInProgress}
        hasResults={hasResults}
      />

      {/* Main Viewport Workspace */}
      <main className="relative z-10 flex-1 overflow-y-auto w-full">
        {crashedRun && (
          <div className="p-4 m-4 rounded-xl bg-amber-500/10 border border-amber-500/30 text-amber-200 font-mono text-xs flex items-center justify-between gap-4">
            <div className="flex items-center gap-2.5">
              <AlertTriangle className="w-4 h-4 text-amber-400 shrink-0" />
              <span>
                A previous run (project {crashedRun.project_id}, phase &quot;{phaseLabel(crashedRun.phase)}&quot;) did
                not complete cleanly.
                {crashRollbackError && (
                  <span className="block text-red-400 mt-1">Rollback failed: {crashRollbackError}</span>
                )}
              </span>
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <button
                onClick={handleRollbackCrashedRun}
                disabled={isRollingBackCrash || isRunInProgress}
                className="px-3 py-1.5 rounded-lg bg-amber-500/20 hover:bg-amber-500/30 text-amber-200 border border-amber-500/40 text-xs font-mono cursor-pointer disabled:opacity-50"
              >
                {isRollingBackCrash ? "Rolling back…" : "Roll Back Now"}
              </button>
              <button
                onClick={() => setCrashedRun(null)}
                disabled={isRunInProgress}
                className="px-3 py-1.5 rounded-lg glass-pill text-xs font-mono text-white/70 cursor-pointer disabled:opacity-50"
              >
                Dismiss
              </button>
            </div>
          </div>
        )}

        {/* Actionable Error Banners */}
        {saveError && (
          <div className="mx-6 mt-4 p-3.5 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 font-mono text-xs flex items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <AlertCircle className="w-4 h-4 text-red-400 shrink-0" />
              <span>Could not save: {saveError}</span>
            </div>
            <button onClick={() => setSaveError(null)} className="text-white/40 hover:text-white cursor-pointer">
              <X className="w-4 h-4" />
            </button>
          </div>
        )}

        {runStartError && (
          <div className="mx-6 mt-4 p-3.5 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 font-mono text-xs flex items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <AlertCircle className="w-4 h-4 text-red-400 shrink-0" />
              <span>Could not start run: {runStartError}</span>
            </div>
            <button onClick={() => setRunStartError(null)} className="text-white/40 hover:text-white cursor-pointer">
              <X className="w-4 h-4" />
            </button>
          </div>
        )}

        {runValidationErrors && (
          <div className="mx-6 mt-4 p-3.5 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 font-mono text-xs flex items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <AlertCircle className="w-4 h-4 text-red-400 shrink-0" />
              <span>Run blocked by validation: {runValidationErrors.join("; ")}</span>
            </div>
            <button onClick={() => setRunValidationErrors(null)} className="text-white/40 hover:text-white cursor-pointer">
              <X className="w-4 h-4" />
            </button>
          </div>
        )}

        {isLoadingProjects && (
          <div className="p-12 text-center text-white/50 font-mono text-sm">Loading projects…</div>
        )}

        {projectsError && !isLoadingProjects && (
          <div className="mx-6 mt-4 p-4 rounded-xl bg-red-500/10 border border-red-500/30 text-red-400 font-mono text-xs flex items-center gap-2">
            <AlertCircle className="w-4 h-4 shrink-0" />
            <span>Could not load projects: {projectsError}</span>
          </div>
        )}

        {!isLoadingProjects && !projectsError && activeTab === "dashboard" && (
          <ProjectExplorer
            projects={projects}
            onSelectProject={handleSelectProject}
            onEditMatrix={handleEditMatrix}
            onRunProject={handleRequestRunProject}
            onViewResults={handleViewResults}
            onCreateNew={() => setIsNewProjectOpen(true)}
            isStarting={isStartingRun}
            resultSummaries={resultSummaries}
            onDeleteProject={handleDeleteProject}
            crashedRun={crashedRun}
            onResumeRun={handleResumeCrashedRun}
          />
        )}

        {activeTab === "builder" && selectedProject && (
          <MatrixBuilder
            project={selectedProject}
            onUpdateProject={handleUpdateProject}
            onOpenTweakCatalog={handleOpenCatalogForScenario}
            onRunProject={handleRequestRunProject}
            isStarting={isStartingRun}
          />
        )}

        {activeTab === "monitor" && selectedProject && (
          <LiveMonitor
            project={selectedProject}
            runId={activeRunId}
            runOutcome={runOutcome}
            onBackToBuilder={handleBackToBuilder}
            onViewResults={handleViewRunResults}
            progress={runProgress}
            initialShutdownWhenComplete={startedShutdownWhenComplete}
          />
        )}

        {activeTab === "results" && (
          <>
            {isLoadingResults && (
              <div className="p-12 text-center text-white/50 font-mono text-sm">Loading results…</div>
            )}
            {resultsError && !isLoadingResults && (
              <div className="mx-6 mt-4 p-4 rounded-xl bg-red-500/10 border border-red-500/30 text-red-400 font-mono text-xs flex items-center gap-2">
                <AlertCircle className="w-4 h-4 shrink-0" />
                <span>Could not load results: {resultsError}</span>
              </div>
            )}
            {!isLoadingResults && !resultsError && viewedResults && (
              <ResultsVisualizer
                results={viewedResults}
                progress={runProgress}
                runOutcome={shutdownRequested ? { kind: "shutdown_requested" } : null}
                runHistory={selectedProject ? resultSummaries[selectedProject.id] : undefined}
                onSelectRun={handleViewRunResults}
              />
            )}
            {!isLoadingResults && !resultsError && !viewedResults && (
              <div className="p-12 text-center text-white/50 font-mono text-sm">
                No results loaded yet — finish a run and click &quot;View Results&quot;.
              </div>
            )}
          </>
        )}

        {activeTab === "library" && (
          <div className="p-8 max-w-6xl mx-auto w-full">
            <TweakCatalog
              isOpen={true}
              isModal={false}
              onClose={() => setActiveTab("builder")}
              onSelectTweak={(module) => {
                if (selectedProject?.scenarios?.[0]) {
                  handleAddModuleToActiveScenario(module, selectedProject.scenarios[0].id);
                }
              }}
              targetScenarioName={selectedProject?.scenarios?.[0]?.name}
              currentModules={selectedProject?.scenarios?.[0]?.modules ?? []}
              projectId={selectedProject?.id ?? ""}
            />
          </div>
        )}
      </main>

      {/* Modals & Dialog Overlays */}
      <PreflightModal
        isOpen={isPreflightOpen}
        onClose={() => setIsPreflightOpen(false)}
        projectId={selectedProject?.id ?? null}
      />

      <SettingsModal
        isOpen={isSettingsOpen}
        onClose={() => setIsSettingsOpen(false)}
      />

      <NewProjectModal
        isOpen={isNewProjectOpen}
        onClose={() => setIsNewProjectOpen(false)}
        onCreateProject={handleCreateNewProject}
      />

      <RunCountdownModal
        project={pendingRunProject}
        shutdownDefault={pendingConfig?.shutdown_when_complete_default ?? false}
        rebootCount={countReboots(
          (pendingRunProject?.scenarios ?? [])
            .filter((s) => s.enabled ?? true)
            .map(scenarioRequiresReboot)
        )}
        onCancel={handleCancelPendingRun}
        onConfirm={(opts) => {
          const project = pendingRunProject;
          setPendingRunProject(null);
          if (project) handleRunProject(project, opts);
        }}
      />

      <TweakCatalog
        isOpen={isCatalogOpen}
        isModal={true}
        onClose={() => setIsCatalogOpen(false)}
        onSelectTweak={(module) => handleAddModuleToActiveScenario(module, catalogTargetScenarioId)}
        targetScenarioName={
          selectedProject?.scenarios?.find((s) => s.id === catalogTargetScenarioId)?.name
        }
        currentModules={
          selectedProject?.scenarios?.find((s) => s.id === catalogTargetScenarioId)?.modules ?? []
        }
        projectId={selectedProject?.id ?? ""}
      />

      <EmergencyRestoreModal
        isOpen={isEmergencyRestoreOpen}
        onClose={() => setIsEmergencyRestoreOpen(false)}
      />
    </div>
  );
};

export default App;
