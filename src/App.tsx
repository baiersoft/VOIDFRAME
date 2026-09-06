import React, { useState, useEffect, useEffectEvent } from "react";
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
  getRunSnapshot,
  rollbackNow,
  startRun,
  subscribeToEngineEvents,
  validateScenario,
} from "./lib/api";
import type { EngineEvent, Module, Project, RunResults, RunState, RunSummary } from "./lib/bindings";
import { phaseLabel } from "./lib/phase";
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
  const [runStartError, setRunStartError] = useState<string | null>(null);
  const [runValidationErrors, setRunValidationErrors] = useState<string[] | null>(null);
  const [isRunInProgress, setIsRunInProgress] = useState(false);
  const [isStartingRun, setIsStartingRun] = useState(false);
  const [pendingRunProject, setPendingRunProject] = useState<Project | null>(null);
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

  const [crashedRun, setCrashedRun] = useState<RunState | null>(null);
  const [isRollingBackCrash, setIsRollingBackCrash] = useState(false);
  const [crashRollbackError, setCrashRollbackError] = useState<string | null>(null);
  const [isEmergencyRestoreOpen, setIsEmergencyRestoreOpen] = useState(false);

  useEffect(() => {
    getRunSnapshot()
      .then(setCrashedRun)
      .catch(() => {});
  }, []);

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
    setPendingRunProject(project);
  };

  const handleCancelPendingRun = () => {
    setPendingRunProject(null);
    setIsStartingRun(false);
  };

  const handleRunProject = async (project: Project) => {
    try {
      // Read fresh rather than cached: reflects whatever the user last
      // saved in Settings, including a change made after this app session
      // started.
      const config = await getConfig();
      const runId = await startRun(project.id, config.dry_run_default ?? false);
      setActiveRunId(runId);
      setCrashedRun(null);
      setViewedResults(null);
      setResultsError(null);
      setRunOutcome(null);
      setIsRunInProgress(true);
      setSelectedProject(project);
      setActiveTab("monitor");
    } catch (e) {
      setRunStartError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsStartingRun(false);
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
              <ResultsVisualizer results={viewedResults} />
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
        onCancel={handleCancelPendingRun}
        onConfirm={() => {
          const project = pendingRunProject;
          setPendingRunProject(null);
          if (project) handleRunProject(project);
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
      />

      <EmergencyRestoreModal
        isOpen={isEmergencyRestoreOpen}
        onClose={() => setIsEmergencyRestoreOpen(false)}
      />
    </div>
  );
};

export default App;
