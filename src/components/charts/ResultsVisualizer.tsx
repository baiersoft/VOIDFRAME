import React, { useMemo, useState } from "react";
import { Award, BarChart3, Check, Copy, Share2, TrendingUp, Activity, ChevronDown, ChevronUp } from "lucide-react";
import { ResponsiveContainer, BarChart, Bar, XAxis, YAxis, Tooltip, CartesianGrid, Legend } from "recharts";
import type { RunResults, ScenarioResult, Verdict } from "../../lib/bindings";
import { SocialShareModal } from "../modals/SocialShareModal";
import { FrameTimeChart } from "./FrameTimeChart";
import { fmt } from "./formatNumber";

interface ResultsVisualizerProps {
  results: RunResults;
}

const VERDICT_STYLES: Record<Verdict, { label: string; className: string }> = {
  better: { label: "BETTER", className: "bg-emerald-500/20 text-emerald-300 border-emerald-500/40" },
  worse: { label: "WORSE", className: "bg-red-500/20 text-red-300 border-red-500/40" },
  confirmed_same: { label: "CONFIRMED SAME", className: "bg-white/10 text-white/50 border-white/20" },
  inconclusive: { label: "INCONCLUSIVE", className: "bg-amber-500/15 text-amber-300/80 border-amber-500/30" },
  no_measurable_difference: { label: "NO DIFFERENCE", className: "bg-white/10 text-white/50 border-white/20" },
};

type ChartTab = "percentiles" | "timeline" | "pacing" | "wcps";

function getMetricDelta(scenario: ScenarioResult, metricName: string): number | null {
  const item = scenario.metric_deltas.find((d) => d.metric === metricName);
  return item?.delta_pct ?? null;
}

function renderDeltaInline(delta: number | null, invertColor = false) {
  if (delta === null) return null;
  const isGood = invertColor ? delta < 0 : delta > 0;
  const isBad = invertColor ? delta > 0 : delta < 0;
  const colorClass = isGood ? "text-emerald-400" : isBad ? "text-red-400" : "text-white/40";
  const sign = delta > 0 ? "+" : "";
  return (
    <span className={`text-[10px] font-mono ${colorClass} font-semibold ml-1.5`}>
      {sign}{fmt(delta)}%
    </span>
  );
}

export const ResultsVisualizer: React.FC<ResultsVisualizerProps> = ({ results }) => {
  const [copiedMd, setCopiedMd] = useState(false);
  const [isSocialShareOpen, setIsSocialShareOpen] = useState(false);
  const [activeChartTab, setActiveChartTab] = useState<ChartTab>("percentiles");
  const [expandedScenarioId, setExpandedScenarioId] = useState<string | null>(null);

  const sortedScenarios = useMemo(
    () =>
      [...results.scenarios].sort(
        (a, b) => (b.wcps ?? -Infinity) - (a.wcps ?? -Infinity)
      ),
    [results.scenarios]
  );
  const winner: ScenarioResult | undefined = sortedScenarios[0];

  const round1 = (n: number) => Math.round(n * 10) / 10;

  const barChartData = [
    {
      name: "Baseline",
      Avg: round1(results.baseline.aggregated.avg_fps ?? 0),
      P1: round1(results.baseline.aggregated.p1_fps ?? 0),
      P01: round1(results.baseline.aggregated.p01_fps ?? 0),
    },
    ...sortedScenarios.map((s) => ({
      name: s.name.length > 20 ? s.name.substring(0, 18) + "..." : s.name,
      Avg: round1(s.aggregated.avg_fps ?? 0),
      P1: round1(s.aggregated.p1_fps ?? 0),
      P01: round1(s.aggregated.p01_fps ?? 0),
    })),
  ];

  const wcpsChartData = sortedScenarios.map((s) => ({
    name: s.name.length > 20 ? s.name.substring(0, 18) + "..." : s.name,
    WCPS: round1(s.wcps ?? 0),
  }));

  const pacingChartData = sortedScenarios.map((s) => ({
    name: s.name.length > 20 ? s.name.substring(0, 18) + "..." : s.name,
    StutterPct: round1(s.aggregated.stutter_count_pct ?? 0),
    AdaptiveCv: round1((s.aggregated.adaptive_frame_time_cv ?? 0) * 100),
  }));

  const handleCopyMarkdown = () => {
    const rows = [
      `| **${results.baseline.name}** | ${fmt(results.baseline.aggregated.avg_fps)} | ${fmt(results.baseline.aggregated.p1_fps)} | ${fmt(results.baseline.aggregated.p01_fps)} | ${fmt(results.baseline.aggregated.adaptive_frame_time_cv !== null ? results.baseline.aggregated.adaptive_frame_time_cv * 100 : null)}% | ${fmt(results.baseline.aggregated.stutter_count_pct)}% | ${fmt(results.baseline.aggregated.mean_abs_animation_error_ms)} | — | Baseline |`,
      ...sortedScenarios.map((s) => {
        const verdict = s.verdict;
        return `| **${s.name}** | ${fmt(s.aggregated.avg_fps)} | ${fmt(s.aggregated.p1_fps)} | ${fmt(s.aggregated.p01_fps)} | ${fmt(s.aggregated.adaptive_frame_time_cv !== null ? s.aggregated.adaptive_frame_time_cv * 100 : null)}% | ${fmt(s.aggregated.stutter_count_pct)}% | ${fmt(s.aggregated.mean_abs_animation_error_ms)} | ${fmt(s.wcps)} | ${VERDICT_STYLES[verdict].label} |`;
      }),
    ];
    const md = `### Benchmark Report — run ${results.run_id}\n\n| Scenario | Avg FPS | 1% Low (P1) | 0.1% Low (P0.1) | Adaptive CV | Stutter % | Anim Err | WCPS v3 | Verdict |\n| :--- | :---: | :---: | :---: | :---: | :---: | :---: | :---: | :---: |\n${rows.join("\n")}\n`;
    navigator.clipboard.writeText(md);
    setCopiedMd(true);
    setTimeout(() => setCopiedMd(false), 2000);
  };

  const baselineCv = results.baseline.aggregated.frame_time_cv;

  const toggleExpand = (scenarioId: string) => {
    setExpandedScenarioId((prev) => (prev === scenarioId ? null : scenarioId));
  };

  return (
    <div className="p-8 max-w-7xl mx-auto space-y-6 w-full">
      {/* Header & Winner Banner */}
      <div className="glass-panel p-6 rounded-2xl space-y-5">
        <div className="flex flex-col lg:flex-row lg:items-center justify-between gap-4">
          <div className="space-y-1">
            <div className="flex items-center gap-2 text-xs font-mono text-white/50 uppercase">
              <Award className="w-4 h-4 text-amber-400" />
              <span>BENCHMARK RESULTS //</span>
              <span className="text-[#8b5cf6]">RUN {results.run_id}</span>
            </div>
            <p className="text-xs text-white/50 font-mono">
              Completed {new Date(results.completed_at).toLocaleString()} • Detection: {results.detection_tier}
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2.5">
            <button
              onClick={() => setIsSocialShareOpen(true)}
              className="px-3.5 py-2 rounded-xl bg-purple-500/20 hover:bg-purple-500/30 text-purple-300 border border-purple-500/40 text-xs font-mono font-bold flex items-center gap-1.5 transition-all cursor-pointer shadow-[0_0_15px_rgba(139,92,246,0.15)]"
            >
              <Share2 className="w-3.5 h-3.5" />
              <span>Social Share Card</span>
            </button>
            <button
              onClick={handleCopyMarkdown}
              className="glass-pill px-3.5 py-2 rounded-xl text-xs font-mono text-white/80 hover:text-white flex items-center gap-1.5 cursor-pointer"
            >
              {copiedMd ? <Check className="w-3.5 h-3.5 text-emerald-400" /> : <Copy className="w-3.5 h-3.5" />}
              <span>{copiedMd ? "Copied Markdown" : "Markdown Table"}</span>
            </button>
          </div>
        </div>

        {/* Winner Highlight Card */}
        {winner && (
          <div className="p-4 rounded-xl bg-gradient-to-r from-emerald-500/10 via-[#040409] to-cyan-500/10 border border-emerald-500/30 flex flex-col sm:flex-row sm:items-center justify-between gap-4">
            <div className="flex items-center gap-3">
              <div className="p-2.5 rounded-xl bg-emerald-500/20 text-emerald-300 border border-emerald-500/40">
                <TrendingUp className="w-5 h-5" />
              </div>
              <div>
                <div className="flex items-center gap-2">
                  <span className="text-[10px] font-mono font-bold uppercase tracking-wider text-emerald-400">
                    Highest Ranked Scenario
                  </span>
                  <span className={`px-2 py-0.5 rounded text-[9px] font-bold border ${VERDICT_STYLES[winner.verdict].className}`}>
                    {VERDICT_STYLES[winner.verdict].label}
                  </span>
                </div>
                <h4 className="font-sans font-bold text-lg text-white mt-0.5">Winner: {winner.name}</h4>
              </div>
            </div>

            <div className="flex items-center gap-4 text-xs font-mono">
              <div className="text-right">
                <div className="text-[10px] text-white/40 uppercase">1% Low (P1)</div>
                <div className="text-base font-bold text-[#22d3ee]">
                  {fmt(winner.aggregated.p1_fps)} FPS
                </div>
              </div>
              <div className="text-right border-l border-white/10 pl-4">
                <div className="text-[10px] text-white/40 uppercase">WCPS v3</div>
                <div className="text-base font-bold text-[#c084fc]">
                  {fmt(winner.wcps)}
                </div>
              </div>
            </div>
          </div>
        )}

        {/* Reproducibility panel */}
        <div className="p-3 bg-black/40 border border-white/5 rounded-xl text-[11px] font-mono text-white/60">
          <strong className="text-white/80">Baseline reproducibility:</strong>{" "}
          frame-time CV {baselineCv !== null ? `${(baselineCv * 100).toFixed(1)}%` : "unavailable"} — results with a
          smaller effect than this may not be reliably distinguishable from run-to-run noise.
        </div>

        {/* Present-mode warnings */}
        {[results.baseline, ...results.scenarios]
          .filter((s) => s.aggregated.present_mode_warning)
          .map((s) => (
            <div
              key={s.scenario_id}
              className="p-3 bg-amber-500/10 border border-amber-500/30 rounded-xl text-[11px] font-mono text-amber-300"
            >
              <strong>{s.name}:</strong> {s.aggregated.present_mode_warning}
            </div>
          ))}
      </div>

      {/* Segmented Chart View */}
      <div className="glass-panel p-6 rounded-2xl space-y-6">
        <div className="flex flex-col sm:flex-row sm:items-center justify-between gap-4 border-b border-white/10 pb-4">
          <h3 className="font-mono text-xs font-bold text-white uppercase tracking-wider flex items-center gap-2">
            <Activity className="w-4 h-4 text-[#06b6d4]" /> Telemetry & Performance Charts
          </h3>

          <div className="flex items-center gap-1.5 p-1 bg-black/40 border border-white/10 rounded-xl overflow-x-auto">
            <button
              onClick={() => setActiveChartTab("percentiles")}
              className={`px-3 py-1.5 rounded-lg text-xs font-mono font-medium transition-all cursor-pointer whitespace-nowrap ${
                activeChartTab === "percentiles"
                  ? "bg-[#06b6d4]/20 text-[#22d3ee] border border-[#06b6d4]/40 font-bold"
                  : "text-white/50 hover:text-white"
              }`}
            >
              Percentiles (FPS)
            </button>
            <button
              onClick={() => setActiveChartTab("timeline")}
              className={`px-3 py-1.5 rounded-lg text-xs font-mono font-medium transition-all cursor-pointer whitespace-nowrap ${
                activeChartTab === "timeline"
                  ? "bg-[#06b6d4]/20 text-[#22d3ee] border border-[#06b6d4]/40 font-bold"
                  : "text-white/50 hover:text-white"
              }`}
            >
              Frame-Time Timeline
            </button>
            <button
              onClick={() => setActiveChartTab("pacing")}
              className={`px-3 py-1.5 rounded-lg text-xs font-mono font-medium transition-all cursor-pointer whitespace-nowrap ${
                activeChartTab === "pacing"
                  ? "bg-[#06b6d4]/20 text-[#22d3ee] border border-[#06b6d4]/40 font-bold"
                  : "text-white/50 hover:text-white"
              }`}
            >
              Pacing & Stability
            </button>
            <button
              onClick={() => setActiveChartTab("wcps")}
              className={`px-3 py-1.5 rounded-lg text-xs font-mono font-medium transition-all cursor-pointer whitespace-nowrap ${
                activeChartTab === "wcps"
                  ? "bg-[#06b6d4]/20 text-[#22d3ee] border border-[#06b6d4]/40 font-bold"
                  : "text-white/50 hover:text-white"
              }`}
            >
              WCPS v3 Ranking
            </button>
          </div>
        </div>

        {/* Tab 1: Percentiles */}
        {activeChartTab === "percentiles" && (
          <div className="space-y-2">
            <div className="text-[11px] text-white/50 font-mono">
              Average FPS, 1% Lows (P1), and 0.1% Lows (P0.1) across scenarios.
            </div>
            <div className="h-80 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={barChartData} margin={{ top: 20, right: 30, left: 20, bottom: 5 }}>
                  <CartesianGrid strokeDasharray="3 3" stroke="rgba(255,255,255,0.05)" />
                  <XAxis dataKey="name" stroke="rgba(255,255,255,0.3)" tick={{ fontSize: 11, fill: "rgba(255,255,255,0.6)" }} />
                  <YAxis stroke="rgba(255,255,255,0.3)" unit=" FPS" tick={{ fontSize: 10, fill: "rgba(255,255,255,0.4)" }} />
                  <Tooltip contentStyle={{ backgroundColor: "rgba(1,1,3,0.95)", borderColor: "rgba(255,255,255,0.15)", borderRadius: "12px", fontSize: "11px" }} />
                  <Legend wrapperStyle={{ fontSize: "11px" }} />
                  <Bar dataKey="Avg" name="Average FPS" fill="#EDEDED" radius={[4, 4, 0, 0]} />
                  <Bar dataKey="P1" name="1% Low (P1)" fill="#06b6d4" radius={[4, 4, 0, 0]} />
                  <Bar dataKey="P01" name="0.1% Low (P0.1)" fill="#8b5cf6" radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            </div>
          </div>
        )}

        {/* Tab 2: Timeline */}
        {activeChartTab === "timeline" && (
          <div className="space-y-2">
            <div className="text-[11px] text-white/50 font-mono">
              High-resolution uPlot canvas overlay of frame times across recorded iterations.
            </div>
            <FrameTimeChart baseline={results.baseline} scenarios={sortedScenarios} />
          </div>
        )}

        {/* Tab 3: Pacing */}
        {activeChartTab === "pacing" && (
          <div className="space-y-2">
            <div className="text-[11px] text-white/50 font-mono">
              Stutter count percentage and Adaptive Frame-Time CV (lower is smoother).
            </div>
            <div className="h-80 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={pacingChartData} margin={{ top: 20, right: 30, left: 20, bottom: 5 }}>
                  <CartesianGrid strokeDasharray="3 3" stroke="rgba(255,255,255,0.05)" />
                  <XAxis dataKey="name" stroke="rgba(255,255,255,0.3)" tick={{ fontSize: 11, fill: "rgba(255,255,255,0.6)" }} />
                  <YAxis stroke="rgba(255,255,255,0.3)" unit="%" tick={{ fontSize: 10, fill: "rgba(255,255,255,0.4)" }} />
                  <Tooltip contentStyle={{ backgroundColor: "rgba(1,1,3,0.95)", borderColor: "rgba(255,255,255,0.15)", borderRadius: "12px", fontSize: "11px" }} />
                  <Legend wrapperStyle={{ fontSize: "11px" }} />
                  <Bar dataKey="StutterPct" name="Stutter %" fill="#f472b6" radius={[4, 4, 0, 0]} />
                  <Bar dataKey="AdaptiveCv" name="Adaptive Frame-Time CV %" fill="#facc15" radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            </div>
          </div>
        )}

        {/* Tab 4: WCPS */}
        {activeChartTab === "wcps" && (
          <div className="space-y-2">
            <div className="text-[11px] text-white/50 font-mono">
              Weighted Competitive Performance Score (WCPS v3) dimensionless ranking.
            </div>
            <div className="h-80 w-full">
              <ResponsiveContainer width="100%" height="100%">
                <BarChart data={wcpsChartData} margin={{ top: 20, right: 30, left: 20, bottom: 5 }}>
                  <CartesianGrid strokeDasharray="3 3" stroke="rgba(255,255,255,0.05)" />
                  <XAxis dataKey="name" stroke="rgba(255,255,255,0.3)" tick={{ fontSize: 11, fill: "rgba(255,255,255,0.6)" }} />
                  <YAxis stroke="rgba(255,255,255,0.3)" tick={{ fontSize: 10, fill: "rgba(255,255,255,0.4)" }} />
                  <Tooltip contentStyle={{ backgroundColor: "rgba(1,1,3,0.95)", borderColor: "rgba(255,255,255,0.15)", borderRadius: "12px", fontSize: "11px" }} />
                  <Bar dataKey="WCPS" name="WCPS v3 Score" fill="#8b5cf6" radius={[4, 4, 0, 0]} />
                </BarChart>
              </ResponsiveContainer>
            </div>
          </div>
        )}
      </div>

      {/* Decluttered & Streamlined Table */}
      <div className="glass-panel p-6 rounded-2xl space-y-4">
        <div className="flex items-center justify-between">
          <h3 className="font-mono text-xs font-bold text-white uppercase tracking-wider flex items-center gap-2">
            <BarChart3 className="w-4 h-4 text-[#06b6d4]" /> Scenario Results (sorted by WCPS v3)
          </h3>
          <span className="text-[10px] font-mono text-white/40 hidden sm:inline">
            Deltas shown relative to Stock Baseline
          </span>
        </div>

        <div className="overflow-x-auto">
          <table className="w-full text-left font-mono text-xs">
            <thead>
              <tr className="border-b border-white/10 text-white/40 text-[10px]">
                <th className="pb-3 pl-3 text-left">SCENARIO</th>
                <th className="pb-3 text-center">VERDICT</th>
                <th className="pb-3 text-right">WCPS</th>
                <th className="pb-3 text-right">AVG FPS</th>
                <th className="pb-3 text-right">1% LOW (P1)</th>
                <th className="pb-3 text-right">0.1% LOW</th>
                <th className="pb-3 text-right">ADAPTIVE CV</th>
                <th className="pb-3 text-right">STUTTER %</th>
                <th className="pb-3 text-right">ANIM ERR</th>
                <th className="pb-3 pr-3 text-center">DETAILS</th>
              </tr>
            </thead>
            <tbody className="divide-y divide-white/5">
              {/* Baseline Row */}
              <tr className="text-white/70 bg-purple-950/10 border-l-2 border-[#8b5cf6]/60">
                <td className="py-3 pl-3 font-bold text-purple-300">
                  <div className="flex items-center gap-2">
                    <span className="w-1.5 h-1.5 rounded-full bg-[#8b5cf6]" />
                    <span>{results.baseline.name}</span>
                    <span className="text-[9px] px-1.5 py-0.2 rounded bg-purple-500/20 text-purple-300 font-normal">
                      BASELINE
                    </span>
                  </div>
                </td>
                <td className="py-3 text-center">
                  <span className="text-[10px] text-white/40 font-bold uppercase tracking-wider">
                    Reference
                  </span>
                </td>
                <td className="py-3 text-right font-bold text-white/40">—</td>
                <td className="py-3 text-right font-bold text-white/90">
                  {fmt(results.baseline.aggregated.avg_fps)}
                </td>
                <td className="py-3 text-right font-bold text-white/90">
                  {fmt(results.baseline.aggregated.p1_fps)}
                </td>
                <td className="py-3 text-right text-white/70">
                  {fmt(results.baseline.aggregated.p01_fps)}
                </td>
                <td className="py-3 text-right text-white/70">
                  {results.baseline.aggregated.adaptive_frame_time_cv !== null
                    ? `${(results.baseline.aggregated.adaptive_frame_time_cv * 100).toFixed(1)}%`
                    : "—"}
                </td>
                <td className="py-3 text-right text-white/70">
                  {results.baseline.aggregated.stutter_count_pct !== null
                    ? `${results.baseline.aggregated.stutter_count_pct.toFixed(1)}%`
                    : "—"}
                </td>
                <td className="py-3 text-right text-white/70">
                  {fmt(results.baseline.aggregated.mean_abs_animation_error_ms)}
                </td>
                <td className="py-3 pr-3 text-center text-white/30 text-[10px]">—</td>
              </tr>

              {/* Scenario Rows */}
              {sortedScenarios.map((s) => {
                const isExpanded = expandedScenarioId === s.scenario_id;
                const avgDelta = getMetricDelta(s, "avg_fps");
                const p1Delta = getMetricDelta(s, "p1_fps");
                const p01Delta = getMetricDelta(s, "p01_fps");

                return (
                  <React.Fragment key={s.scenario_id}>
                    <tr
                      className={`text-white transition-colors cursor-pointer hover:bg-white/[0.03] ${
                        isExpanded ? "bg-white/[0.04]" : ""
                      }`}
                      onClick={() => toggleExpand(s.scenario_id)}
                    >
                      <td className="py-3 pl-3 font-semibold">
                        <div className="flex items-center gap-2">
                          <span className="text-white/40 hover:text-white">
                            {isExpanded ? (
                              <ChevronUp className="w-3.5 h-3.5 text-[#06b6d4]" />
                            ) : (
                              <ChevronDown className="w-3.5 h-3.5 text-white/40" />
                            )}
                          </span>
                          <span>{s.name}</span>
                        </div>
                      </td>

                      <td className="py-3 text-center">
                        <span
                          className={`px-2 py-0.5 rounded text-[9px] font-bold border ${VERDICT_STYLES[s.verdict].className}`}
                        >
                          {VERDICT_STYLES[s.verdict].label}
                        </span>
                      </td>

                      <td className="py-3 text-right font-bold text-[#c084fc]">
                        {fmt(s.wcps)}
                      </td>

                      <td className="py-3 text-right">
                        <span>{fmt(s.aggregated.avg_fps)}</span>
                        {renderDeltaInline(avgDelta)}
                      </td>

                      <td className="py-3 text-right font-bold text-[#22d3ee]">
                        <span>{fmt(s.aggregated.p1_fps)}</span>
                        {renderDeltaInline(p1Delta)}
                      </td>

                      <td className="py-3 text-right text-white/80">
                        <span>{fmt(s.aggregated.p01_fps)}</span>
                        {renderDeltaInline(p01Delta)}
                      </td>

                      <td className="py-3 text-right text-white/80">
                        {s.aggregated.adaptive_frame_time_cv !== null
                          ? `${(s.aggregated.adaptive_frame_time_cv * 100).toFixed(1)}%`
                          : "—"}
                      </td>

                      <td className="py-3 text-right text-white/80">
                        {s.aggregated.stutter_count_pct !== null
                          ? `${s.aggregated.stutter_count_pct.toFixed(1)}%`
                          : "—"}
                      </td>

                      <td className="py-3 text-right text-white/80">
                        {fmt(s.aggregated.mean_abs_animation_error_ms)}
                      </td>

                      <td className="py-3 pr-3 text-center">
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            toggleExpand(s.scenario_id);
                          }}
                          className={`px-2 py-0.5 rounded text-[10px] font-mono transition-all cursor-pointer ${
                            isExpanded
                              ? "bg-[#06b6d4]/20 text-[#22d3ee] border border-[#06b6d4]/40"
                              : "text-white/40 hover:text-white bg-white/5 border border-white/10"
                          }`}
                        >
                          {isExpanded ? "Hide" : "Inspect"}
                        </button>
                      </td>
                    </tr>

                    {/* Expandable Telemetry Drawer */}
                    {isExpanded && (
                      <tr className="bg-black/40">
                        <td colSpan={10} className="p-4 border-b border-white/10">
                          <div className="glass-card p-4 rounded-xl space-y-3 border-white/10">
                            <div className="flex items-center justify-between">
                              <span className="text-[11px] font-mono text-white/60 font-semibold uppercase tracking-wider flex items-center gap-1.5">
                                <Activity className="w-3.5 h-3.5 text-[#06b6d4]" />
                                Telemetry Deltas vs Baseline ({s.name})
                              </span>
                              <span className="text-[10px] font-mono text-white/40">
                                {s.metric_deltas.length} statistical deltas evaluated
                              </span>
                            </div>

                            <div className="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-6 gap-2">
                              {s.metric_deltas.map((d) => {
                                const isPacing = [
                                  "adaptive_frame_time_cv",
                                  "stutter_count_pct",
                                  "mean_abs_animation_error_ms",
                                ].includes(d.metric);
                                const isGood = isPacing
                                  ? (d.delta_pct ?? 0) < 0
                                  : (d.delta_pct ?? 0) > 0;
                                const isBad = isPacing
                                  ? (d.delta_pct ?? 0) > 0
                                  : (d.delta_pct ?? 0) < 0;
                                const sign = (d.delta_pct ?? 0) > 0 ? "+" : "";
                                return (
                                  <div
                                    key={d.metric}
                                    className="bg-black/50 border border-white/5 rounded-lg p-2.5 text-center"
                                  >
                                    <div
                                      className="text-[9px] font-mono text-white/40 uppercase truncate"
                                      title={d.metric}
                                    >
                                      {d.metric.replace(/_/g, " ")}
                                    </div>
                                    <div
                                      className={`text-xs font-mono font-bold mt-1 ${
                                        isGood
                                          ? "text-emerald-400"
                                          : isBad
                                          ? "text-red-400"
                                          : "text-white/60"
                                      }`}
                                    >
                                      {sign}{fmt(d.delta_pct)}%
                                    </div>
                                  </div>
                                );
                              })}
                            </div>

                            {/* Additional Telemetry Details */}
                            <div className="pt-2 border-t border-white/5 flex flex-wrap gap-4 text-[11px] font-mono text-white/50">
                              {s.aggregated.render_latency_ms !== null && (
                                <span>
                                  Render Latency:{" "}
                                  <strong className="text-white/80">
                                    {fmt(s.aggregated.render_latency_ms)} ms
                                  </strong>
                                </span>
                              )}
                              {s.aggregated.gpu_busy_ms !== null && (
                                <span>
                                  GPU Busy:{" "}
                                  <strong className="text-white/80">
                                    {fmt(s.aggregated.gpu_busy_ms)} ms
                                  </strong>
                                </span>
                              )}
                              {s.aggregated.bottleneck_ratio !== null && (
                                <span>
                                  GPU Bound:{" "}
                                  <strong className="text-white/80">
                                    {(s.aggregated.bottleneck_ratio * 100).toFixed(0)}%
                                  </strong>
                                </span>
                              )}
                              {s.aggregated.frame_time_mean_ms !== null && (
                                <span>
                                  Mean Frame Time:{" "}
                                  <strong className="text-white/80">
                                    {fmt(s.aggregated.frame_time_mean_ms)} ms
                                  </strong>
                                </span>
                              )}
                            </div>
                          </div>
                        </td>
                      </tr>
                    )}
                  </React.Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
      </div>

      <SocialShareModal
        isOpen={isSocialShareOpen}
        onClose={() => setIsSocialShareOpen(false)}
        results={results}
        winner={winner}
      />
    </div>
  );
};
