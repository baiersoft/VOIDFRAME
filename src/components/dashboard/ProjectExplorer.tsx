import React, { useState } from "react";
import { Plus, Play, Sliders, Search, BarChart3, Trash2, X, Check, Shield } from "lucide-react";
import type { Project, RunSummary } from "../../lib/bindings";
import { fmt } from "../charts/formatNumber";

interface ProjectExplorerProps {
  projects: Project[];
  onSelectProject: (project: Project) => void;
  onEditMatrix: (project: Project) => void;
  onRunProject: (project: Project) => void;
  onViewResults: (project: Project) => void;
  onCreateNew: () => void;
  isStarting: boolean;
  /** Per-project results history -- used for the run-count pill and, since
   * WCPS v3 shipped real `winner_wcps` per run, the BEST WCPS stat (the max
   * across the project's runs). */
  resultSummaries?: Record<string, RunSummary[]>;
  onDeleteProject?: (projectId: string) => void;
}

/** Highest `winner_wcps` across a project's run history, or null when there
 * are no runs yet or none produced a WCPS (e.g. every run pre-dates WCPS
 * v3, or errored before scoring). */
function bestWcps(summaries: RunSummary[] | undefined): number | null {
  if (!summaries) return null;
  return summaries.reduce<number | null>((best, r) => {
    if (r.winner_wcps === null) return best;
    return best === null ? r.winner_wcps : Math.max(best, r.winner_wcps);
  }, null);
}

export const ProjectExplorer: React.FC<ProjectExplorerProps> = ({
  projects,
  onEditMatrix,
  onRunProject,
  onViewResults,
  onCreateNew,
  isStarting,
  resultSummaries = {},
  onDeleteProject,
}) => {
  const [searchTerm, setSearchTerm] = useState("");
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);

  const filteredProjects = projects.filter(
    (p) =>
      p.name.toLowerCase().includes(searchTerm.toLowerCase()) ||
      p.description.toLowerCase().includes(searchTerm.toLowerCase())
  );

  const totalScenarios = projects.reduce((acc, p) => acc + (p.scenarios?.length ?? 0), 0);
  const totalRuns = Object.values(resultSummaries).reduce((acc, list) => acc + list.length, 0);

  const handleDelete = (projectId: string) => {
    if (onDeleteProject) {
      onDeleteProject(projectId);
    }
    setConfirmDeleteId(null);
  };

  return (
    <div className="p-8 max-w-7xl mx-auto space-y-6 w-full relative">
      {/* Overview Stat Cards */}
      <div className="grid grid-cols-1 sm:grid-cols-3 gap-4">
        <div className="glass-panel p-5 rounded-2xl space-y-1">
          <div className="flex items-center justify-between text-white/40 text-xs font-mono">
            <span>TOTAL MATRICES</span>
            <Sliders className="w-4 h-4 text-[#06b6d4]" />
          </div>
          <div className="text-2xl font-mono font-bold text-white">{projects.length}</div>
          <div className="text-[11px] text-white/50 font-body">Configured test projects</div>
        </div>

        <div className="glass-panel p-5 rounded-2xl space-y-1">
          <div className="flex items-center justify-between text-white/40 text-xs font-mono">
            <span>ACTIVE SCENARIOS</span>
            <span className="w-2 h-2 rounded-full bg-[#8b5cf6]" />
          </div>
          <div className="text-2xl font-mono font-bold text-[#c084fc]">{totalScenarios}</div>
          <div className="text-[11px] text-white/50 font-body">Total tweak permutations</div>
        </div>

        <div className="glass-panel p-5 rounded-2xl space-y-1">
          <div className="flex items-center justify-between text-white/40 text-xs font-mono">
            <span>MEASURED RUNS</span>
            <BarChart3 className="w-4 h-4 text-[#22d3ee]" />
          </div>
          <div className="text-2xl font-mono font-bold text-[#22d3ee]">{totalRuns}</div>
          <div className="text-[11px] text-white/50 font-body">Historical benchmark sessions</div>
        </div>
      </div>

      {/* Action Bar: Search & New Project */}
      <div className="flex flex-col sm:flex-row items-center justify-between gap-4">
        <div className="relative w-full sm:w-80">
          <Search className="w-4 h-4 absolute left-3.5 top-1/2 -translate-y-1/2 text-white/40" />
          <input
            type="text"
            placeholder="Search matrices or descriptions..."
            value={searchTerm}
            onChange={(e) => setSearchTerm(e.target.value)}
            className="w-full pl-10 pr-9 py-2.5 rounded-xl glass-input text-xs font-mono"
          />
          {searchTerm && (
            <button
              onClick={() => setSearchTerm("")}
              className="absolute right-3 top-1/2 -translate-y-1/2 text-white/40 hover:text-white cursor-pointer"
            >
              <X className="w-3.5 h-3.5" />
            </button>
          )}
        </div>

        <div className="flex items-center gap-3 w-full sm:w-auto justify-end">
          <button
            onClick={onCreateNew}
            className="w-full sm:w-auto px-5 py-2.5 rounded-xl bg-gradient-to-r from-[#06b6d4] to-[#8b5cf6] text-white font-mono text-xs font-bold flex items-center justify-center gap-2 hover:opacity-95 shadow-[0_0_25px_rgba(6,182,212,0.35)] transition-all cursor-pointer"
          >
            <Plus className="w-4 h-4" />
            <span>New Benchmark Project</span>
          </button>
        </div>
      </div>

      {/* Safety Notice */}
      <div className="flex items-center gap-2 text-[11px] text-white/50 font-mono px-1">
        <Shield className="w-3.5 h-3.5 text-[#06b6d4]" />
        <span>
          Autonomous Execution: Every run benchmarks your scenarios and automatically restores the baseline when finished.
        </span>
      </div>

      {/* Projects Grid or Empty States */}
      {projects.length === 0 ? (
        <div className="glass-panel p-12 rounded-2xl text-center space-y-4 max-w-lg mx-auto">
          <Sliders className="w-12 h-12 mx-auto text-[#06b6d4]/40" />
          <div className="space-y-1">
            <h3 className="font-sans font-bold text-lg text-white">No Benchmark Matrices Yet</h3>
            <p className="text-xs text-white/50 font-body">
              Create your first project to define stock baseline settings and test system tweak permutations.
            </p>
          </div>
          <button
            onClick={onCreateNew}
            className="px-5 py-2.5 rounded-xl bg-gradient-to-r from-[#06b6d4] to-[#8b5cf6] text-white font-mono text-xs font-bold inline-flex items-center gap-2 hover:opacity-95 cursor-pointer shadow-[0_0_20px_rgba(6,182,212,0.3)]"
          >
            <Plus className="w-4 h-4" />
            <span>Create First Project</span>
          </button>
        </div>
      ) : filteredProjects.length === 0 ? (
        <div className="glass-panel p-10 rounded-2xl text-center space-y-3">
          <p className="text-xs font-mono text-white/50">
            No matrices match your search for &quot;{searchTerm}&quot;.
          </p>
          <button
            onClick={() => setSearchTerm("")}
            className="glass-pill px-4 py-1.5 rounded-lg text-xs font-mono text-[#22d3ee] cursor-pointer"
          >
            Clear search
          </button>
        </div>
      ) : (
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
          {filteredProjects.map((project) => {
            const totalTweaks = (project.scenarios ?? []).reduce(
              (acc, s) => acc + (s.modules ?? []).length,
              0
            );

            return (
              <div
                key={project.id}
                className="glass-card p-6 rounded-2xl space-y-5 flex flex-col justify-between group"
              >
                <div className="space-y-3">
                  {/* Project Title & Description */}
                  <div className="flex items-start justify-between gap-3">
                    <div>
                      <h3 className="font-sans font-bold text-lg text-white group-hover:text-[#22d3ee] transition-colors">
                        {project.name}
                      </h3>
                      <p className="text-xs text-white/60 font-body mt-1 line-clamp-2 leading-relaxed">
                        {project.description}
                      </p>
                    </div>

                    {onDeleteProject && (
                      <div className="shrink-0">
                        {confirmDeleteId === project.id ? (
                          <div className="flex items-center gap-1 bg-red-500/10 border border-red-500/30 p-1 rounded-lg">
                            <button
                              onClick={() => handleDelete(project.id)}
                              title="Confirm delete project"
                              className="p-1 rounded text-red-400 hover:text-red-200 hover:bg-red-500/20 cursor-pointer"
                            >
                              <Check className="w-3.5 h-3.5" />
                            </button>
                            <button
                              onClick={() => setConfirmDeleteId(null)}
                              title="Cancel"
                              className="p-1 rounded text-white/40 hover:text-white cursor-pointer"
                            >
                              <X className="w-3.5 h-3.5" />
                            </button>
                          </div>
                        ) : (
                          <button
                            onClick={() => setConfirmDeleteId(project.id)}
                            title="Delete Project"
                            className="opacity-0 group-hover:opacity-100 p-1.5 rounded-lg text-white/30 hover:text-red-400 hover:bg-red-500/10 transition-all cursor-pointer"
                          >
                            <Trash2 className="w-3.5 h-3.5" />
                          </button>
                        )}
                      </div>
                    )}
                  </div>

                  {/* Metrics pill */}
                  <div className="grid grid-cols-3 gap-2 pt-2">
                    <div className="bg-black/40 border border-white/5 rounded-lg p-2.5 text-center">
                      <div className="text-[10px] text-white/40 font-mono">SCENARIOS</div>
                      <div className="font-mono text-sm font-bold text-white mt-0.5">
                        {(project.scenarios ?? []).length}
                      </div>
                    </div>
                    <div className="bg-black/40 border border-white/5 rounded-lg p-2.5 text-center">
                      <div className="text-[10px] text-white/40 font-mono">TOTAL TWEAKS</div>
                      <div className="font-mono text-sm font-bold text-[#8b5cf6] mt-0.5">{totalTweaks}</div>
                    </div>
                    <div className="bg-black/40 border border-white/5 rounded-lg p-2.5 text-center">
                      <div className="text-[10px] text-white/40 font-mono">BEST WCPS</div>
                      <div className="font-mono text-sm font-bold text-[#22d3ee] mt-0.5">
                        {fmt(bestWcps(resultSummaries[project.id]))}
                      </div>
                    </div>
                  </div>
                </div>

                {/* Card Actions */}
                <div className="border-t border-white/10 pt-4 flex items-center justify-between gap-3">
                  <div className="flex items-center gap-2">
                    <button
                      onClick={() => onEditMatrix(project)}
                      className="glass-pill px-3 py-1.5 rounded-lg text-xs font-mono text-white/70 hover:text-white flex items-center gap-1.5 cursor-pointer"
                    >
                      <Sliders className="w-3.5 h-3.5 text-white/50" />
                      <span>Edit Matrix</span>
                    </button>
                    {(resultSummaries[project.id]?.length ?? 0) > 0 && (
                      <button
                        onClick={() => onViewResults(project)}
                        className="glass-pill px-3 py-1.5 rounded-lg text-xs font-mono text-white/70 hover:text-white flex items-center gap-1.5 cursor-pointer"
                      >
                        <BarChart3 className="w-3.5 h-3.5 text-white/50" />
                        <span>
                          {resultSummaries[project.id]!.length} run
                          {resultSummaries[project.id]!.length === 1 ? "" : "s"}
                        </span>
                      </button>
                    )}
                  </div>

                  <button
                    onClick={() => onRunProject(project)}
                    disabled={isStarting}
                    className="px-4 py-1.5 rounded-lg bg-[#06b6d4]/20 hover:bg-[#06b6d4]/30 text-[#22d3ee] hover:text-white border border-[#06b6d4]/40 font-mono text-xs font-semibold flex items-center gap-1.5 transition-all shadow-[0_0_15px_rgba(6,182,212,0.15)] cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                  >
                    <Play className="w-3.5 h-3.5 fill-current" />
                    <span>Run Matrix</span>
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
};
