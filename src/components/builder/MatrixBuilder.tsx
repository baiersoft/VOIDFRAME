import React, { useState } from "react";
import { Plus, Play, Trash2, BookOpen, Clock, ChevronRight, Wrench, Pencil, X } from "lucide-react";
import type { Module, PowerPlan, Project, Scenario } from "../../lib/bindings";
import { PowerPlanPicker } from "../library/PowerPlanPicker";
import { LaunchArgsEditor } from "../library/LaunchArgsEditor";

interface MatrixBuilderProps {
  project: Project;
  onUpdateProject: (updated: Project) => void;
  onOpenTweakCatalog: (targetScenarioId: string) => void;
  onRunProject: (project: Project) => void;
  isStarting: boolean;
}

export const MatrixBuilder: React.FC<MatrixBuilderProps> = ({
  project,
  onUpdateProject,
  onOpenTweakCatalog,
  onRunProject,
  isStarting,
}) => {
  const [activeScenarioId, setActiveScenarioId] = useState<string>(project.scenarios?.[0]?.id || "");
  const [editingModuleIndex, setEditingModuleIndex] = useState<number | null>(null);

  // Combinatorial Calculations
  const enabledScenarios = (project.scenarios ?? []).filter((s) => s.enabled ?? true);
  const warmupLoops = project.settings.warmup_loops ?? 2;
  const measureLoops = project.settings.measure_loops ?? 3;
  const totalWarmupRuns = (enabledScenarios.length + 1) * warmupLoops;
  const totalMeasureRuns = (enabledScenarios.length + 1) * measureLoops;
  const totalRuns = totalWarmupRuns + totalMeasureRuns;
  const captureSeconds = project.settings.capture_seconds ?? 105;
  const estimatedSeconds = totalRuns * (captureSeconds + 15);
  const estimatedMinutes = Math.ceil(estimatedSeconds / 60);

  const handleToggleScenario = (scenarioId: string) => {
    onUpdateProject({
      ...project,
      scenarios: (project.scenarios ?? []).map((s) =>
        s.id === scenarioId ? { ...s, enabled: !(s.enabled ?? true) } : s
      ),
    });
  };

  const handleAddScenario = () => {
    const newId = `scen_${Date.now()}`;
    const newScenario: Scenario = {
      id: newId,
      name: `Scenario ${(project.scenarios ?? []).length + 1}`,
      description: "Custom tweak combination matrix",
      enabled: true,
      modules: [],
    };
    onUpdateProject({ ...project, scenarios: [...(project.scenarios ?? []), newScenario] });
    setActiveScenarioId(newId);
  };

  const handleDeleteScenario = (scenarioId: string) => {
    const updated = {
      ...project,
      scenarios: (project.scenarios ?? []).filter((s) => s.id !== scenarioId),
    };
    onUpdateProject(updated);
    if (activeScenarioId === scenarioId) {
      setActiveScenarioId(updated.scenarios[0]?.id ?? "");
    }
  };

  const handleRemoveModule = (scenarioId: string, moduleIndex: number) => {
    const updatedScenarios = (project.scenarios ?? []).map((s) => {
      if (s.id === scenarioId) {
        return { ...s, modules: (s.modules ?? []).filter((_, i) => i !== moduleIndex) };
      }
      return s;
    });
    onUpdateProject({ ...project, scenarios: updatedScenarios });
  };

  const handleUpdateModule = (scenarioId: string, moduleIndex: number, updated: Module) => {
    const updatedScenarios = (project.scenarios ?? []).map((s) => {
      if (s.id === scenarioId) {
        const modules = [...(s.modules ?? [])];
        modules[moduleIndex] = updated;
        return { ...s, modules };
      }
      return s;
    });
    onUpdateProject({ ...project, scenarios: updatedScenarios });
  };

  const handlePickPowerPlanForModule = (scenarioId: string, moduleIndex: number, plan: PowerPlan) => {
    handleUpdateModule(scenarioId, moduleIndex, {
      type: "power_plan",
      plan_guid: plan.guid,
      friendly_name: plan.name,
      create_if_missing: false,
    });
    setEditingModuleIndex(null);
  };

  const handleSaveLaunchArgsForModule = (scenarioId: string, moduleIndex: number, args: string) => {
    handleUpdateModule(scenarioId, moduleIndex, { type: "launch_args", args });
    setEditingModuleIndex(null);
  };

  const currentScenario = (project.scenarios ?? []).find((s) => s.id === activeScenarioId);
  const modules = currentScenario?.modules ?? [];

  return (
    <div className="p-8 max-w-7xl mx-auto space-y-8 w-full">
      {/* Header Info & Matrix Estimator */}
      <div className="glass-panel p-6 rounded-2xl space-y-6">
        <div className="flex flex-col lg:flex-row lg:items-center justify-between gap-4">
          <div>
            <div className="flex items-center gap-2 text-xs font-mono text-white/50 uppercase">
              <span>ACTIVE MATRIX //</span>
              <span className="text-[#06b6d4]">PROJECT CONFIG</span>
            </div>
            <h2 className="font-sans font-bold text-2xl text-white mt-1">{project.name}</h2>
            <p className="text-xs text-white/60 font-body mt-1 max-w-3xl leading-relaxed">
              {project.description}
            </p>
          </div>

          <div className="flex flex-col items-end gap-1.5">
            <button
              onClick={() => onRunProject(project)}
              disabled={isStarting}
              className="px-6 py-3 rounded-xl bg-gradient-to-r from-[#06b6d4] to-[#8b5cf6] text-white font-mono text-xs font-bold flex items-center gap-2 hover:opacity-95 shadow-[0_0_30px_rgba(6,182,212,0.4)] transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
            >
              <Play className="w-4 h-4 fill-current" />
              <span>Launch Autonomous Pipeline</span>
            </button>
            <span className="text-[10px] text-white/40 font-mono text-right max-w-xs">
              This starts a real run — it applies the configured tweaks, benchmarks, then automatically rolls everything back when it finishes.
            </span>
          </div>
        </div>

        {/* Combinatorial Estimation Bar */}
        <div className="grid grid-cols-2 md:grid-cols-3 gap-3 pt-4 border-t border-white/10">
          <div className="bg-black/30 border border-white/5 rounded-xl p-3">
            <div className="text-[10px] text-white/40 font-mono">SCENARIOS ACTIVE</div>
            <div className="font-mono text-lg font-bold text-white mt-0.5">
              {enabledScenarios.length} / {(project.scenarios ?? []).length}
            </div>
          </div>

          <div className="bg-black/30 border border-white/5 rounded-xl p-3">
            <div className="text-[10px] text-white/40 font-mono">TOTAL BENCH RUNS</div>
            <div className="font-mono text-lg font-bold text-white mt-0.5">
              {totalRuns} <span className="text-xs text-white/40 font-normal">({totalMeasureRuns}m + {totalWarmupRuns}w)</span>
            </div>
          </div>

          <div className="bg-black/30 border border-white/5 rounded-xl p-3">
            <div className="text-[10px] text-white/40 font-mono">ESTIMATED TIME</div>
            <div className="font-mono text-lg font-bold text-[#22d3ee] mt-0.5 flex items-center gap-1.5">
              <Clock className="w-4 h-4" /> ~{estimatedMinutes} min
            </div>
          </div>
        </div>
      </div>

      {/* Main Builder Grid: Left Scenarios List, Right Active Scenario Editor */}
      <div className="grid grid-cols-1 lg:grid-cols-3 gap-6">
        {/* Left Column: Scenario Selector & Baseline */}
        <div className="space-y-4">
          <div className="flex items-center justify-between">
            <h3 className="font-mono text-xs font-bold text-white/60 tracking-wider uppercase">
              Matrix Pipelines
            </h3>
            <button
              onClick={handleAddScenario}
              className="glass-pill px-2.5 py-1 rounded-lg text-xs font-mono text-[#22d3ee] hover:text-white flex items-center gap-1 cursor-pointer"
            >
              <Plus className="w-3.5 h-3.5" /> Add Scenario
            </button>
          </div>

          {/* Baseline Card (Permanent) */}
          <div className="glass-panel p-4 rounded-xl border-[#8b5cf6]/30 bg-purple-950/10 space-y-2">
            <div className="flex items-center justify-between">
              <div className="flex items-center gap-2">
                <span className="w-2 h-2 rounded-full bg-[#8b5cf6] animate-pulse" />
                <span className="font-mono text-xs font-bold text-purple-300 uppercase">
                  Phase 0: Baseline
                </span>
              </div>
              <span className="text-[10px] font-mono bg-purple-500/20 text-purple-300 px-2 py-0.5 rounded border border-purple-500/30">
                REQUIRED
              </span>
            </div>
            <h4 className="font-sans font-bold text-sm text-white">{project.baseline.name}</h4>
            <p className="text-[11px] text-white/50 leading-relaxed font-body">
              {project.baseline.description}
            </p>
          </div>

          {/* Scenario List */}
          <div className="space-y-2.5">
            {(project.scenarios ?? []).map((scen, idx) => {
              const isSelected = scen.id === activeScenarioId;

              return (
                <div
                  key={scen.id}
                  onClick={() => {
                    setActiveScenarioId(scen.id);
                    setEditingModuleIndex(null);
                  }}
                  className={`p-4 rounded-xl transition-all cursor-pointer flex items-center justify-between border ${
                    isSelected
                      ? "bg-white/[0.05] border-[#06b6d4]/50 shadow-[0_0_20px_rgba(6,182,212,0.15)]"
                      : "glass-card border-white/5 hover:border-white/20"
                  }`}
                >
                  <div className="space-y-1 pr-2">
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-[10px] text-white/40">#{idx + 1}</span>
                      <span className="font-sans font-bold text-sm text-white">{scen.name}</span>
                    </div>
                    <div className="flex flex-wrap items-center gap-2 text-[10px] font-mono text-white/50">
                      <span>{(scen.modules ?? []).length} modules</span>
                    </div>
                  </div>

                  <div className="flex items-center gap-2">
                    <input
                      type="checkbox"
                      checked={scen.enabled ?? true}
                      onChange={(e) => {
                        e.stopPropagation();
                        handleToggleScenario(scen.id);
                      }}
                      className="rounded bg-black/50 border-white/20 text-[#06b6d4] focus:ring-0 cursor-pointer w-4 h-4"
                    />
                    <ChevronRight className={`w-4 h-4 ${isSelected ? "text-[#22d3ee]" : "text-white/20"}`} />
                  </div>
                </div>
              );
            })}
          </div>
        </div>

        {/* Right Column: Scenario Detail & Tweak Modular Blocks */}
        <div className="lg:col-span-2 space-y-6">
          {currentScenario ? (
            <div className="glass-panel p-6 rounded-2xl space-y-6">
              {/* Scenario Detail Header */}
              <div className="flex items-start justify-between gap-4 pb-4 border-b border-white/10">
                <div className="space-y-1">
                  <div className="flex items-center gap-2">
                    <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-[#06b6d4]/10 text-[#22d3ee] border border-[#06b6d4]/30">
                      ACTIVE SCENARIO
                    </span>
                  </div>
                  <h3 className="font-sans font-bold text-xl text-white">{currentScenario.name}</h3>
                  <p className="text-xs text-white/60 font-body">{currentScenario.description}</p>
                </div>

                <div className="flex items-center gap-2">
                  <button
                    onClick={() => handleDeleteScenario(currentScenario.id)}
                    title="Delete Scenario"
                    className="p-2 rounded-lg text-white/40 hover:text-red-400 hover:bg-red-500/10 border border-transparent hover:border-red-500/20 transition-colors cursor-pointer"
                  >
                    <Trash2 className="w-4 h-4" />
                  </button>
                </div>
              </div>

              {/* Module Blocks List */}
              <div className="space-y-3">
                <div className="flex items-center justify-between">
                  <h4 className="font-mono text-xs font-bold text-white/70 uppercase">
                    Configured Modules ({modules.length})
                  </h4>

                  <div className="flex items-center gap-2">
                    <button
                      onClick={() => onOpenTweakCatalog(currentScenario.id)}
                      className="px-3 py-1.5 rounded-lg bg-[#06b6d4]/15 hover:bg-[#06b6d4]/25 text-[#22d3ee] border border-[#06b6d4]/30 font-mono text-xs flex items-center gap-1.5 transition-colors cursor-pointer"
                    >
                      <BookOpen className="w-3.5 h-3.5" />
                      <span>Browse Catalog</span>
                    </button>
                  </div>
                </div>

                {modules.length === 0 ? (
                  <div className="border border-dashed border-white/10 rounded-xl p-8 text-center space-y-3">
                    <Wrench className="w-8 h-8 mx-auto text-white/20" />
                    <div className="text-xs font-mono text-white/40">No tweaks added to this scenario yet.</div>
                    <div className="flex items-center justify-center gap-2">
                      <button
                        onClick={() => onOpenTweakCatalog(currentScenario.id)}
                        className="px-3.5 py-2 rounded-lg glass-pill font-mono text-xs text-[#22d3ee] inline-flex items-center gap-1.5 cursor-pointer"
                      >
                        <Plus className="w-3.5 h-3.5" /> Add from Catalog
                      </button>
                    </div>
                  </div>
                ) : (
                  <div className="space-y-3">
                    {modules.map((module, idx) => {
                      return (
                        <div key={idx} className="glass-card p-4 rounded-xl border-white/10 space-y-2 relative group">
                          <div className="flex items-start justify-between gap-2">
                            <div>
                              <span className="px-2 py-0.5 rounded text-[9px] font-mono uppercase bg-white/5 text-white/60 border border-white/10">
                                {module.type}
                              </span>
                            </div>
                            <div className="flex items-center gap-1">
                              {(module.type === "power_plan" || module.type === "launch_args") && (
                                <button
                                  onClick={() => setEditingModuleIndex(idx)}
                                  title="Edit Module"
                                  className="text-white/30 hover:text-[#22d3ee] p-1.5 rounded transition-colors cursor-pointer"
                                >
                                  <Pencil className="w-3.5 h-3.5" />
                                </button>
                              )}
                              <button
                                onClick={() => handleRemoveModule(currentScenario.id, idx)}
                                title="Remove Module"
                                className="text-white/30 hover:text-red-400 p-1.5 rounded transition-colors cursor-pointer"
                              >
                                <Trash2 className="w-3.5 h-3.5" />
                              </button>
                            </div>
                          </div>
                          <div className="bg-black/50 border border-white/5 rounded-lg p-2.5 text-[11px] font-mono text-white/70 space-y-1">
                            {module.type === "registry" && (
                              <div className="truncate text-white/50">
                                <span className="text-[#06b6d4]">Registry:</span> {module.hive}\{module.subkey}\
                                <span className="text-white font-semibold">{module.value_name}</span> ({module.value_type}) ={" "}
                                {String(module.value)}
                              </div>
                            )}
                            {module.type === "powercfg" && (
                              <div className="truncate text-white/50">
                                <span className="text-[#8b5cf6]">Powercfg:</span> {module.sub} / {module.setting} = {module.value}
                              </div>
                            )}
                            {module.type === "power_plan" && (
                              <div className="truncate text-white/50">
                                <span className="text-emerald-400">Power Plan:</span>{" "}
                                {module.friendly_name ?? module.plan_guid}
                                {module.create_if_missing && <span className="text-white/40"> (create if missing)</span>}
                              </div>
                            )}
                            {editingModuleIndex === idx && module.type === "power_plan" && (
                              <div className="pt-2 mt-2 border-t border-white/10 space-y-2">
                                <div className="flex items-center justify-between">
                                  <span className="text-[10px] uppercase text-white/40">Pick a power plan</span>
                                  <button
                                    onClick={() => setEditingModuleIndex(null)}
                                    className="text-white/40 hover:text-white p-1 rounded cursor-pointer"
                                  >
                                    <X className="w-3.5 h-3.5" />
                                  </button>
                                </div>
                                <PowerPlanPicker
                                  currentPlanGuid={module.plan_guid}
                                  onSelect={(plan) =>
                                    handlePickPowerPlanForModule(currentScenario.id, idx, plan)
                                  }
                                />
                              </div>
                            )}
                            {module.type === "affinity_cpu" && (
                              <div className="truncate text-white/50">
                                <span className="text-amber-400">Affinity:</span> {module.mode}
                                {module.mask_hex && <span> ({module.mask_hex})</span>}
                              </div>
                            )}
                            {module.type === "launch_args" && (
                              <div className="truncate text-white/50">
                                <span className="text-[#22d3ee]">Launch Args:</span>{" "}
                                {module.args || <span className="text-white/30">(none)</span>}
                              </div>
                            )}
                            {editingModuleIndex === idx && module.type === "launch_args" && (
                              <div className="pt-2 mt-2 border-t border-white/10 space-y-2">
                                <div className="flex items-center justify-between">
                                  <span className="text-[10px] uppercase text-white/40">
                                    Edit launch options
                                  </span>
                                  <button
                                    onClick={() => setEditingModuleIndex(null)}
                                    className="text-white/40 hover:text-white p-1 rounded cursor-pointer"
                                  >
                                    <X className="w-3.5 h-3.5" />
                                  </button>
                                </div>
                                <LaunchArgsEditor
                                  initialValue={module.args}
                                  onSave={(args) =>
                                    handleSaveLaunchArgsForModule(currentScenario.id, idx, args)
                                  }
                                />
                              </div>
                            )}
                          </div>
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            </div>
          ) : (
            <div className="glass-panel p-12 rounded-2xl text-center text-white/40 font-mono text-xs">
              Select or create a scenario to begin configuring the matrix.
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
