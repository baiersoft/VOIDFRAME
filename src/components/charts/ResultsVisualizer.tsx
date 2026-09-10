import React, { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Award, BarChart3, Check, Copy, Share2, TrendingUp, Activity, ChevronDown, ChevronUp, AlertTriangle, PowerOff } from "lucide-react";
import { ResponsiveContainer, BarChart, Bar, XAxis, YAxis, Tooltip, CartesianGrid, Legend } from "recharts";
import { cancelShutdown } from "../../lib/api";
import type { RunProgress, RunResults, RunSummary, ScenarioResult, Verdict } from "../../lib/bindings";
import { SocialShareModal } from "../modals/SocialShareModal";
import { FrameTimeChart } from "./FrameTimeChart";
import { fmt } from "./formatNumber";

interface ResultsVisualizerProps {
  /** `null` once a run was recovered mid-flight and never reached REPORT --
   * `progress` (below) then carries whatever scenarios did finish. */
  results: RunResults | null;
  /** `getRunProgress(runId)`'s own result, App-fetched -- used only as the
   * `results === null` fallback source of completed/unstable scenarios;
   * ignored once `results` is present (a finished run's own file is
   * authoritative). */
  progress?: RunProgress | null;
  /** A distinct, narrower signal than `App.tsx`'s own `RunOutcome` (which
   * stays binary complete/failed for `LiveMonitor` -- see task-15's report
   * for why this isn't folded into that same union). Set once `RunComplete`
   * arrives and the run's `shutdown_when_complete` was true. */
  runOutcome?: { kind: "shutdown_requested" } | null;
  /** The current project's other runs (App.tsx's own `resultSummaries`
   * cache, already fetched for the dashboard's "N runs" pill), newest
   * first -- drives the run-switcher dropdown on the `RUN {runId}` label.
   * Omitted (rather than empty) hides the dropdown entirely, e.g. for a
   * mid-flight-recovered run this component reaches with no project
   * context to look history up in. */
  runHistory?: RunSummary[];
  /** Switches the visualizer to a different run from `runHistory`. Required
   * whenever `runHistory` is passed. */
  onSelectRun?: (runId: string) => void;
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

export const ResultsVisualizer: React.FC<ResultsVisualizerProps> = ({
  results,
  progress = null,
  runOutcome = null,
  runHistory,
  onSelectRun,
}) => {
  const [copiedMd, setCopiedMd] = useState(false);
  const [isSocialShareOpen, setIsSocialShareOpen] = useState(false);
  const [activeChartTab, setActiveChartTab] = useState<ChartTab>("percentiles");
  const [expandedScenarioId, setExpandedScenarioId] = useState<string | null>(null);
  const [isCancellingShutdown, setIsCancellingShutdown] = useState(false);
  const [cancelShutdownError, setCancelShutdownError] = useState<string | null>(null);
  const [isHistoryOpen, setIsHistoryOpen] = useState(false);
  // Viewport coordinates for the portalled dropdown below, computed from
  // the trigger button's own rect when it opens -- the header it lives in
  // is a `.glass-panel` (`contain: paint` in index.css), which clips any
  // plain `position: absolute` descendant to the panel's own box instead of
  // letting it float above the rest of the page. Portalling to `document.
  // body` (render call below) escapes that clip entirely; `position: fixed`
  // + these coordinates is what re-anchors it under the button once it's
  // no longer a DOM descendant of it.
  const [historyMenuPos, setHistoryMenuPos] = useState<{ top: number; left: number } | null>(null);
  const historyTriggerRef = useRef<HTMLButtonElement>(null);
  const historyMenuRef = useRef<HTMLDivElement>(null);

  // Closes the run-switcher dropdown on an outside click, Escape, or a
  // scroll anywhere (capture: true so this also sees scroll events from a
  // scrollable ancestor, e.g. the results page's own scroll container,
  // which don't bubble to `document`) -- a scroll would otherwise leave the
  // fixed-position portal visually detached from the button that opened it.
  // Listeners attached only while the dropdown is actually open.
  useEffect(() => {
    if (!isHistoryOpen) return;
    const handlePointerDown = (e: MouseEvent) => {
      const target = e.target as Node;
      if (
        !historyTriggerRef.current?.contains(target) &&
        !historyMenuRef.current?.contains(target)
      ) {
        setIsHistoryOpen(false);
      }
    };
    const handleKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setIsHistoryOpen(false);
    };
    const handleScroll = () => setIsHistoryOpen(false);
    document.addEventListener("mousedown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    document.addEventListener("scroll", handleScroll, true);
    window.addEventListener("resize", handleScroll);
    return () => {
      document.removeEventListener("mousedown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
      document.removeEventListener("scroll", handleScroll, true);
      window.removeEventListener("resize", handleScroll);
    };
  }, [isHistoryOpen]);

  const toggleHistoryMenu = () => {
    if (!isHistoryOpen && historyTriggerRef.current) {
      const rect = historyTriggerRef.current.getBoundingClientRect();
      setHistoryMenuPos({ top: rect.bottom + 8, left: rect.left });
    }
    setIsHistoryOpen((prev) => !prev);
  };

  // A finished run's own `results.json` is authoritative once it exists;
  // `progress.completed` (baseline first, then scenarios in run order --
  // mirrors `finish_run`'s own `completed.first()` / `completed[1..]` split
  // in crates/voidframe-engine/src/run/execute/mod.rs) is only the fallback
  // for a run recovered mid-flight, before REPORT ever wrote that file.
  const baseline: ScenarioResult | null = results?.baseline ?? progress?.completed[0] ?? null;
  const scenarios: ScenarioResult[] = results?.scenarios ?? progress?.completed.slice(1) ?? [];
  // Same rule as above: `results.json` carries its own `unstable` list
  // (copied from progress at REPORT), so a finished run -- including an
  // older one picked from the run history -- never reads the *current*
  // run's progress for it.
  const unstable = results?.unstable ?? progress?.unstable ?? [];
  const runId = results?.run_id ?? progress?.run_id ?? "—";

  const sortedScenarios = useMemo(
    () => [...scenarios].sort((a, b) => (b.wcps ?? -Infinity) - (a.wcps ?? -Infinity)),
    [scenarios]
  );
  // The top-ranked scenario by wcps is only a genuine "winner" if the
  // statistics themselves say it beat baseline -- a Worse (or the only,
  // trivially-top-ranked-by-default) scenario must never be crowned just
  // because sorting always produces *some* first element. Confirmed as a
  // real bug on alpha-tester data: study/pamuk/ab-test-analysis-report.md §7.
  //
  // Must match RunResults::summarize()'s rule on the Rust side EXACTLY:
  // filter to Better scenarios, THEN rank by wcps -- not "rank by wcps,
  // then check if the top one is Better", which disagrees with the Rust
  // side whenever a non-Better scenario (e.g. Inconclusive) outranks the
  // best genuinely-Better one by raw wcps. `sortedScenarios` is already
  // sorted descending by wcps, so `.find` on it is exactly "filter then
  // take the highest-ranked survivor." Confirmed as a real cross-
  // implementation inconsistency in this plan's own whole-branch review.
  const winner: ScenarioResult | undefined = sortedScenarios.find(
    (s) => s.verdict === "better"
  );

  const handleCancelShutdown = async () => {
    setCancelShutdownError(null);
    setIsCancellingShutdown(true);
    try {
      await cancelShutdown();
    } catch (e) {
      setCancelShutdownError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsCancellingShutdown(false);
    }
  };

  const round1 = (n: number) => Math.round(n * 10) / 10;

  const barChartData = baseline
    ? [
        {
          name: "Baseline",
          Avg: round1(baseline.aggregated.avg_fps ?? 0),
          P1: round1(baseline.aggregated.p1_fps ?? 0),
          P01: round1(baseline.aggregated.p01_fps ?? 0),
        },
        ...sortedScenarios.map((s) => ({
          name: s.name.length > 20 ? s.name.substring(0, 18) + "..." : s.name,
          Avg: round1(s.aggregated.avg_fps ?? 0),
          P1: round1(s.aggregated.p1_fps ?? 0),
          P01: round1(s.aggregated.p01_fps ?? 0),
        })),
      ]
    : [];

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
    // Only ever called from the button below, which is itself gated on
    // `results !== null` (a Markdown export of an in-progress/incomplete
    // run isn't a meaningful artifact) -- this guard just satisfies the
    // type-checker's narrowing.
    if (!results) return;
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

  const baselineCv = baseline?.aggregated.frame_time_cv ?? null;

  const toggleExpand = (scenarioId: string) => {
    setExpandedScenarioId((prev) => (prev === scenarioId ? null : scenarioId));
  };

  if (!baseline) {
    return (
      <div className="p-12 text-center text-white/50 font-mono text-sm">
        No results loaded yet.
      </div>
    );
  }

  return (
    <div className="p-8 max-w-7xl mx-auto space-y-6 w-full">
      {isHistoryOpen &&
        historyMenuPos &&
        runHistory &&
        createPortal(
          <div
            ref={historyMenuRef}
            style={{ top: historyMenuPos.top, left: historyMenuPos.left }}
            // A solid(-ish), highly opaque background rather than the
            // shared `.glass-panel` class -- that class leans on
            // `backdrop-filter: blur` with only ~1.5% white behind it,
            // which reads fine sitting over this page's own background but
            // isn't legible for a floating menu that can end up over
            // arbitrary page content once portalled (confirmed unreadable
            // live). Border/shadow/blur kept to stay visually in the same
            // "glass" family as the rest of the UI.
            className="fixed w-72 max-h-72 overflow-y-auto bg-[#08080c]/95 backdrop-blur-xl border border-white/10 rounded-xl p-1.5 z-[100] shadow-xl font-mono text-xs normal-case"
          >
            {runHistory.length === 0 ? (
              <div className="px-3 py-2 text-white/40">No runs yet.</div>
            ) : (
              runHistory.map((run) => {
                const isActive = run.run_id === runId;
                return (
                  <button
                    key={run.run_id}
                    onClick={() => {
                      setIsHistoryOpen(false);
                      if (!isActive) onSelectRun?.(run.run_id);
                    }}
                    className={`w-full text-left px-3 py-2 rounded-lg flex items-center justify-between gap-2 cursor-pointer ${
                      isActive ? "bg-white/10 text-white" : "text-white/70 hover:bg-white/5 hover:text-white"
                    }`}
                  >
                    <span className="flex flex-col">
                      <span>{new Date(run.completed_at).toLocaleString()}</span>
                      <span className="text-white/40">
                        {run.winner_name
                          ? `Winner: ${run.winner_name}${run.winner_wcps !== null ? ` (${fmt(run.winner_wcps)} WCPS)` : ""}`
                          : `${run.scenario_count} scenario${run.scenario_count === 1 ? "" : "s"}`}
                      </span>
                    </span>
                    {isActive && <Check className="w-3.5 h-3.5 text-emerald-400 shrink-0" />}
                  </button>
                );
              })
            )}
          </div>,
          document.body
        )}
      {/* Header & Winner Banner */}
      <div className="glass-panel p-6 rounded-2xl space-y-5">
        <div className="flex flex-col lg:flex-row lg:items-center justify-between gap-4">
          <div className="space-y-1">
            <div className="flex items-center gap-2 text-xs font-mono text-white/50 uppercase">
              <Award className="w-4 h-4 text-amber-400" />
              <span>BENCHMARK RESULTS //</span>
              {runHistory ? (
                <button
                  ref={historyTriggerRef}
                  onClick={toggleHistoryMenu}
                  className="flex items-center gap-1 text-[#8b5cf6] hover:text-[#a78bfa] cursor-pointer normal-case"
                >
                  <span className="uppercase">RUN {runId}</span>
                  {isHistoryOpen ? <ChevronUp className="w-3 h-3" /> : <ChevronDown className="w-3 h-3" />}
                </button>
              ) : (
                <span className="text-[#8b5cf6]">RUN {runId}</span>
              )}
            </div>
            {results ? (
              <p className="text-xs text-white/50 font-mono">
                Completed {new Date(results.completed_at).toLocaleString()} • Detection: {results.detection_tier}
              </p>
            ) : (
              <p className="text-xs text-amber-300 font-mono">Incomplete run — recovered mid-flight</p>
            )}
          </div>

          <div className="flex flex-wrap items-center gap-2.5">
            {runOutcome?.kind === "shutdown_requested" && (
              <button
                onClick={handleCancelShutdown}
                disabled={isCancellingShutdown}
                className="px-3.5 py-2 rounded-xl bg-red-500/20 hover:bg-red-500/30 text-red-300 border border-red-500/40 text-xs font-mono font-bold flex items-center gap-1.5 transition-all cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
              >
                <PowerOff className="w-3.5 h-3.5" />
                <span>Cancel shutdown</span>
              </button>
            )}
            {results && (
              <>
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
              </>
            )}
          </div>
        </div>
        {cancelShutdownError && (
          <div className="p-3 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 text-xs font-mono">
            {cancelShutdownError}
          </div>
        )}

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
        {[baseline, ...scenarios]
          .filter((s) => s.aggregated.present_mode_warning)
          .map((s) => (
            <div
              key={s.scenario_id}
              className="p-3 bg-amber-500/10 border border-amber-500/30 rounded-xl text-[11px] font-mono text-amber-300"
            >
              <strong>{s.name}:</strong> {s.aggregated.present_mode_warning}
            </div>
          ))}

        {/* Unstable scenarios (a run recovered mid-flight after a bugcheck/
            crash on one or more scenarios -- spec §4/§11) */}
        {unstable.length > 0 && (
          <div className="p-3 bg-amber-500/10 border border-amber-500/30 rounded-xl text-[11px] font-mono text-amber-300 space-y-1">
            <div className="flex items-center gap-1.5 font-bold uppercase">
              <AlertTriangle className="w-3.5 h-3.5" /> Unstable Scenarios
            </div>
            {unstable.map((u) => (
              <div key={u.scenario_id}>
                <strong>{u.scenario_id}:</strong> {u.reason}
              </div>
            ))}
          </div>
        )}
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
            <FrameTimeChart baseline={baseline} scenarios={sortedScenarios} />
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
                    <span>{baseline.name}</span>
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
                  {fmt(baseline.aggregated.avg_fps)}
                </td>
                <td className="py-3 text-right font-bold text-white/90">
                  {fmt(baseline.aggregated.p1_fps)}
                </td>
                <td className="py-3 text-right text-white/70">
                  {fmt(baseline.aggregated.p01_fps)}
                </td>
                <td className="py-3 text-right text-white/70">
                  {baseline.aggregated.adaptive_frame_time_cv !== null
                    ? `${(baseline.aggregated.adaptive_frame_time_cv * 100).toFixed(1)}%`
                    : "—"}
                </td>
                <td className="py-3 text-right text-white/70">
                  {baseline.aggregated.stutter_count_pct !== null
                    ? `${baseline.aggregated.stutter_count_pct.toFixed(1)}%`
                    : "—"}
                </td>
                <td className="py-3 text-right text-white/70">
                  {fmt(baseline.aggregated.mean_abs_animation_error_ms)}
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
                        {s.script_reverted_unverified && (
                          <span
                            title="A custom script ran this scenario's revert -- VOIDFRAME cannot verify what it actually did to your system."
                            className="ml-1.5 px-1.5 py-0.5 rounded text-[8px] font-bold border border-amber-500/40 text-amber-300 bg-amber-500/10"
                          >
                            SCRIPT-REVERTED
                          </span>
                        )}
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

      {results && (
        <SocialShareModal
          isOpen={isSocialShareOpen}
          onClose={() => setIsSocialShareOpen(false)}
          results={results}
          winner={winner}
        />
      )}
    </div>
  );
};
