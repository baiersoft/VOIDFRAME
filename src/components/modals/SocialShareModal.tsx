import React, { useState } from "react";
import { X, Share2, Copy, Check, ShieldCheck } from "lucide-react";
import type { RunResults, ScenarioResult } from "../../lib/bindings";
import { fmt } from "../charts/formatNumber";

interface SocialShareModalProps {
  isOpen: boolean;
  onClose: () => void;
  results: RunResults;
  winner: ScenarioResult | undefined;
}

export const SocialShareModal: React.FC<SocialShareModalProps> = ({ isOpen, onClose, results, winner }) => {
  const [copied, setCopied] = useState(false);

  if (!isOpen) return null;

  const baseline = results.baseline;

  // WCPS v3 has no per-metric "wcps" delta the way v2's comparisons list
  // did (the score itself isn't one of the 6 metrics `metric_deltas`
  // covers) -- the headline delta is simply the winner's own p1_fps delta,
  // matching what the share text has always actually led with.
  const wcpsScore = fmt(winner?.wcps);
  const p1Fps = fmt(winner?.aggregated.p1_fps);
  const p1DeltaPct = winner?.metric_deltas.find((d) => d.metric === "p1_fps")?.delta_pct ?? null;
  const consistencyCv = winner?.aggregated.frame_time_cv;

  const handleCopyText = () => {
    const text = `🌌 baiersoft // VOIDFRAME Benchmark Report\nRun: ${results.run_id}\nWinner: ${winner?.name ?? "—"}\n\n🏆 WCPS v3 Score: ${wcpsScore}\n🔥 1% Lows: ${p1Fps} FPS${p1DeltaPct != null ? ` (${p1DeltaPct > 0 ? "+" : ""}${fmt(p1DeltaPct)}%)` : ""}\n⚡ Session Consistency (frame-time CV): ${consistencyCv != null ? `${(consistencyCv * 100).toFixed(1)}%` : "—"}\n\nVerified with Intel PresentMon ETW.\n#CS2 #VOIDFRAME #EsportsBenchmarks`;
    navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/85 backdrop-blur-xl">
      <div className="glass-panel w-full max-w-2xl rounded-2xl border border-purple-500/30 flex flex-col shadow-[0_0_70px_rgba(139,92,246,0.3)] overflow-hidden">
        {/* Header */}
        <div className="p-6 border-b border-white/10 flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-xl bg-purple-500/20 border border-purple-500/40 text-purple-300">
              <Share2 className="w-5 h-5" />
            </div>
            <div>
              <h2 className="font-sans font-bold text-lg text-white">Social Telemetry Card</h2>
              <p className="text-xs text-white/50 font-mono">Export for Reddit, X, Discord & YouTube</p>
            </div>
          </div>

          <button
            onClick={onClose}
            className="p-2 rounded-lg text-white/50 hover:text-white hover:bg-white/5 transition-colors cursor-pointer"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        {/* Shareable Graphic Card Preview */}
        <div className="p-6 space-y-6">
          <div
            id="voidframe-share-card"
            className="relative overflow-hidden rounded-2xl p-6 border border-[#06b6d4]/40 bg-gradient-to-br from-[#040409] via-[#010103] to-[#0a0515] shadow-2xl space-y-6"
          >
            {/* Ambient Corner Glows */}
            <div className="absolute top-0 right-0 w-48 h-48 bg-[#06b6d4]/10 rounded-full blur-3xl pointer-events-none" />
            <div className="absolute bottom-0 left-0 w-48 h-48 bg-[#8b5cf6]/10 rounded-full blur-3xl pointer-events-none" />

            {/* Brand Header */}
            <div className="flex items-center justify-between relative z-10">
              <div className="flex items-center gap-2.5">
                <div className="w-7 h-7 rounded-lg bg-gradient-to-br from-[#06b6d4] to-[#8b5cf6] flex items-center justify-center font-bold text-black text-xs shadow-[0_0_15px_rgba(6,182,212,0.4)]">
                  VF
                </div>
                <div>
                  <div className="text-xs font-mono font-bold tracking-widest text-white">
                    baiersoft // <span className="text-[#22d3ee]">VOIDFRAME</span>
                  </div>
                  <div className="text-[10px] font-mono text-white/40">
                    ETW Deterministic Telemetry
                  </div>
                </div>
              </div>

              <div className="px-3 py-1 rounded-full text-[10px] font-mono font-bold bg-[#8b5cf6]/20 text-purple-300 border border-[#8b5cf6]/40 flex items-center gap-1">
                <ShieldCheck className="w-3 h-3" /> VERIFIED RUN
              </div>
            </div>

            {/* Run & Winner Title */}
            <div className="space-y-1 relative z-10">
              <div className="text-[11px] font-mono text-[#06b6d4] uppercase font-bold tracking-wider">
                🏆 WINNER SCENARIO
              </div>
              <h3 className="font-sans font-bold text-2xl text-white">{winner?.name ?? "—"}</h3>
              <p className="text-xs text-white/60 font-body">Run: {results.run_id}</p>
            </div>

            {/* Metrics Showcase Grid */}
            <div className="grid grid-cols-3 gap-3 relative z-10">
              <div className="bg-purple-950/30 border border-purple-500/40 rounded-xl p-3 text-center space-y-0.5">
                <div className="text-[10px] font-mono text-purple-300 font-bold">WCPS v3 SCORE</div>
                <div className="text-2xl font-mono font-bold text-[#c084fc]">
                  {wcpsScore}
                </div>
              </div>

              <div className="bg-black/60 border border-[#06b6d4]/40 rounded-xl p-3 text-center space-y-0.5">
                <div className="text-[10px] font-mono text-[#22d3ee] font-bold">1% LOWS (P1)</div>
                <div className="text-2xl font-mono font-bold text-white">
                  {p1Fps} <span className="text-xs font-normal text-white/50">FPS</span>
                </div>
                <div className="text-[10px] font-mono text-emerald-400">
                  {p1DeltaPct != null ? `${p1DeltaPct > 0 ? "+" : ""}${fmt(p1DeltaPct)}% Gain` : "—"}
                </div>
              </div>

              <div className="bg-black/60 border border-white/10 rounded-xl p-3 text-center space-y-0.5">
                <div className="text-[10px] font-mono text-white/50 font-bold">SESSION CONSISTENCY</div>
                <div className="text-2xl font-mono font-bold text-[#a78bfa]">
                  {consistencyCv != null ? `${(consistencyCv * 100).toFixed(1)}%` : "—"}
                </div>
                <div className="text-[10px] font-mono text-white/40">Frame-time CV</div>
              </div>
            </div>

            {/* Footer Details */}
            <div className="flex items-center justify-between pt-2 border-t border-white/10 text-[10px] font-mono text-white/50 relative z-10">
              <span>Baseline: {fmt(baseline.aggregated.p1_fps)} P1 • {fmt(baseline.aggregated.avg_fps)} Avg</span>
              <span className="text-[#22d3ee]">Zero Snake Oil • 100% Proved</span>
            </div>
          </div>

          {/* Export Action Controls */}
          <div className="flex items-center justify-end gap-3 pt-2">
            <button
              onClick={handleCopyText}
              className="px-5 py-2.5 rounded-xl bg-gradient-to-r from-[#06b6d4] to-[#8b5cf6] text-white font-mono text-xs font-bold flex items-center gap-2 hover:opacity-95 shadow-[0_0_20px_rgba(6,182,212,0.3)] transition-all cursor-pointer flex-1 justify-center"
            >
              {copied ? <Check className="w-4 h-4 text-emerald-300" /> : <Copy className="w-4 h-4" />}
              <span>{copied ? "Copied Post Text" : "Copy Formatted Text"}</span>
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};
