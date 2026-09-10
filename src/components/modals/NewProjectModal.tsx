import React, { useState } from "react";
import { X, Plus, Sparkles } from "lucide-react";
import type { BenchmarkKind, Project } from "../../lib/bindings";

interface NewProjectModalProps {
  isOpen: boolean;
  onClose: () => void;
  onCreateProject: (project: Project) => Promise<void>;
}

/** Per-kind defaults, in one place. `capture_seconds` mirrors the engine's
 * own bound: Dust2's serde default (105) and AveYo's
 * `AVEYO_MAX_CAPTURE_SECONDS` (57) in
 * crates/voidframe-engine/src/model/settings.rs -- `Settings::validate`
 * rejects an AveYo project above it, so a drift here fails loudly. `map_id`
 * stays the real Dust2 addon id for every kind: it is inert for AveYo (spec
 * 2026-09-09 D4) but must remain a valid id so a project whose kind is
 * ever switched back to Dust2 still passes `Settings::validate`.
 * `measure_loops` is each kind's calibrated default (see
 * study/real-runs/3f3130b7-aveyo-measure-loops-15/measure-loops-15-verification-report.md):
 * Dust2 stays 3, AveYo's default is 5 -- n=3 showed real reliability gaps for
 * AveYo. It seeds the Measurement Loops select when a kind is picked but
 * (unlike capture_seconds) remains independently overridable afterward. */
const BENCHMARK_KINDS: ReadonlyArray<{
  value: BenchmarkKind;
  label: string;
  capture_seconds: number;
  measure_loops: number;
}> = [
  { value: "workshop_dust2", label: "Dust2 Workshop (default)", capture_seconds: 105, measure_loops: 3 },
  { value: "aveyo_cfg_v2", label: "AveYo benchmark.cfg v2", capture_seconds: 57, measure_loops: 5 },
];
const DUST2_MAP_ID = "3240880604";

export const NewProjectModal: React.FC<NewProjectModalProps> = ({
  isOpen,
  onClose,
  onCreateProject,
}) => {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [warmupLoops, setWarmupLoops] = useState(2);
  const [measureLoops, setMeasureLoops] = useState(BENCHMARK_KINDS[0].measure_loops);
  const [benchmarkKind, setBenchmarkKind] = useState<BenchmarkKind>("workshop_dust2");

  if (!isOpen) return null;

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (!name.trim()) return;

    const kind = BENCHMARK_KINDS.find((k) => k.value === benchmarkKind) ?? BENCHMARK_KINDS[0];
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
        capture_seconds: kind.capture_seconds,
        map_id: DUST2_MAP_ID,
        watchdog_seconds: 120,
        netcon_port: null,
        benchmark_kind: kind.value,
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

          <div className="space-y-1.5">
            <label className="text-xs font-mono text-white/70">Benchmark</label>
            <div className="grid grid-cols-2 gap-2">
              {BENCHMARK_KINDS.map((kind) => (
                <label
                  key={kind.value}
                  className={`px-3 py-2 rounded-xl text-xs font-mono cursor-pointer border ${
                    benchmarkKind === kind.value
                      ? "border-[#06b6d4]/60 bg-[#06b6d4]/10 text-white"
                      : "border-white/10 text-white/60"
                  }`}
                >
                  <input
                    type="radio"
                    name="benchmarkKind"
                    value={kind.value}
                    checked={benchmarkKind === kind.value}
                    onChange={() => {
                      setBenchmarkKind(kind.value);
                      setMeasureLoops(kind.measure_loops);
                    }}
                    className="sr-only"
                  />
                  {kind.label}
                </label>
              ))}
            </div>
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
                <option value={8} className="bg-[#040409]">8 Loops (Best Reliability)</option>
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
