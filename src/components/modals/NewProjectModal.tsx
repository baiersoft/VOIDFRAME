import React, { useState } from "react";
import { X, Plus, Sparkles } from "lucide-react";
import type { Project } from "../../lib/bindings";

interface NewProjectModalProps {
  isOpen: boolean;
  onClose: () => void;
  onCreateProject: (project: Project) => Promise<void>;
}

export const NewProjectModal: React.FC<NewProjectModalProps> = ({
  isOpen,
  onClose,
  onCreateProject,
}) => {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [warmupLoops, setWarmupLoops] = useState(2);
  const [measureLoops, setMeasureLoops] = useState(3);

  if (!isOpen) return null;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;

    const newProject: Project = {
      schema_version: "2.0.0",
      id: `proj_${Date.now()}`,
      name,
      description: description.trim() || "Autonomous benchmark matrix",
      created_at: new Date().toISOString(),
      settings: {
        schema_version: "2.0.0",
        warmup_loops: warmupLoops,
        measure_loops: measureLoops,
        capture_seconds: 105,
        map_id: "3240880604",
        watchdog_seconds: 120,
        netcon_port: null,
      },
      baseline: {
        name: "Stock System Baseline",
        description: "Initial un-modified system state before applying scenario permutations.",
      },
      scenarios: [],
    };

    await onCreateProject(newProject);
    onClose();
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/80 backdrop-blur-xl">
      <div className="glass-panel w-full max-w-xl rounded-2xl border border-white/10 flex flex-col shadow-[0_0_50px_rgba(0,0,0,0.8)] overflow-hidden">
        {/* Header */}
        <div className="p-6 border-b border-white/10 flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-xl bg-[#06b6d4]/10 border border-[#06b6d4]/30 text-[#06b6d4]">
              <Plus className="w-5 h-5" />
            </div>
            <div>
              <h2 className="font-sans font-bold text-lg text-white">Create Benchmark Project</h2>
              <p className="text-xs text-white/50 font-mono">New Autonomous DAG Matrix</p>
            </div>
          </div>

          <button
            onClick={onClose}
            className="p-2 rounded-lg text-white/50 hover:text-white hover:bg-white/5 transition-colors cursor-pointer"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Form */}
        <form onSubmit={handleSubmit} className="p-6 space-y-4">
          <div className="space-y-1.5">
            <label className="text-xs font-mono text-white/70">Project Name</label>
            <input
              type="text"
              required
              placeholder="e.g., BYOB Driver Branch vs. GPU Interrupt Matrix"
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="w-full px-3.5 py-2.5 rounded-xl glass-input text-xs font-mono"
            />
          </div>

          <div className="space-y-1.5">
            <label className="text-xs font-mono text-white/70">Description / Hypothesis</label>
            <textarea
              rows={2}
              placeholder="e.g., Testing if NVIDIA 537.58 Golden Branch + NPI Latency Extreme improves Dust2 smoke 1% low frame pacing."
              value={description}
              onChange={(e) => setDescription(e.target.value)}
              className="w-full px-3.5 py-2 rounded-xl glass-input text-xs font-body resize-none"
            />
          </div>

          <div className="grid grid-cols-2 gap-3 pt-1">
            <div className="space-y-1.5">
              <label className="text-xs font-mono text-white/70">Warmup Loops</label>
              <select
                value={warmupLoops}
                onChange={(e) => setWarmupLoops(Number(e.target.value))}
                className="w-full px-3 py-2 rounded-xl glass-input text-xs font-mono cursor-pointer"
              >
                <option value={1} className="bg-[#040409]">1 Loop (Quick Shader Cache)</option>
                <option value={2} className="bg-[#040409]">2 Loops (Recommended)</option>
                <option value={3} className="bg-[#040409]">3 Loops (Mandatory for Drivers)</option>
              </select>
            </div>

            <div className="space-y-1.5">
              <label className="text-xs font-mono text-white/70">Measurement Loops</label>
              <select
                value={measureLoops}
                onChange={(e) => setMeasureLoops(Number(e.target.value))}
                className="w-full px-3 py-2 rounded-xl glass-input text-xs font-mono cursor-pointer"
              >
                <option value={3} className="bg-[#040409]">3 Loops (Standard)</option>
                <option value={5} className="bg-[#040409]">5 Loops (High Precision)</option>
              </select>
            </div>
          </div>

          {/* Footer Actions */}
          <div className="pt-4 border-t border-white/10 flex items-center justify-end gap-3">
            <button
              type="button"
              onClick={onClose}
              className="glass-pill px-4 py-2 rounded-xl text-xs font-mono text-white/60 hover:text-white cursor-pointer"
            >
              Cancel
            </button>

            <button
              type="submit"
              className="px-5 py-2 rounded-xl bg-gradient-to-r from-[#06b6d4] to-[#8b5cf6] text-white font-mono text-xs font-bold flex items-center gap-1.5 cursor-pointer shadow-[0_0_20px_rgba(6,182,212,0.3)]"
            >
              <Sparkles className="w-4 h-4" />
              <span>Create Matrix</span>
            </button>
          </div>
        </form>
      </div>
    </div>
  );
};
