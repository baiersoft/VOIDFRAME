import React, { useEffect, useRef } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import type { ScenarioResult } from "../../lib/bindings";

interface FrameTimeChartProps {
  baseline: ScenarioResult;
  scenarios: ScenarioResult[];
}

const SERIES_COLORS = ["#06b6d4", "#8b5cf6", "#22d3ee", "#a78bfa", "#f472b6", "#facc15"];

/**
 * Iteration-indexed frame-time chart, not a continuous per-frame timeline --
 * the real `RunResults.scenarios[].per_iteration` only carries one aggregated
 * `Metrics` value per measurement iteration, not raw per-frame samples. See
 * this plan's own Global Constraints for why.
 */
export const FrameTimeChart: React.FC<FrameTimeChartProps> = ({ baseline, scenarios }) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const plotRef = useRef<uPlot | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;

    const maxLen = Math.max(
      baseline.per_iteration.length,
      ...scenarios.map((s) => s.per_iteration.length),
      1
    );
    const xValues = Array.from({ length: maxLen }, (_, i) => i + 1);

    const toSeries = (sr: ScenarioResult) =>
      xValues.map((_, i) => sr.per_iteration[i]?.frame_time_mean_ms ?? null);

    const data: uPlot.AlignedData = [
      xValues,
      toSeries(baseline),
      ...scenarios.map((s) => toSeries(s)),
    ];

    const series: uPlot.Series[] = [
      {},
      { label: baseline.name, stroke: "#ffffff66", width: 1.5, dash: [4, 4] },
      ...scenarios.map((s, i) => ({
        label: s.name,
        stroke: SERIES_COLORS[i % SERIES_COLORS.length],
        width: 2,
      })),
    ];

    const opts: uPlot.Options = {
      width: containerRef.current.clientWidth || 600,
      height: 320,
      series,
      axes: [
        { label: "Iteration", stroke: "#ffffff66" },
        { label: "Frame Time (ms)", stroke: "#ffffff66" },
      ],
      scales: { x: { time: false } },
      legend: { show: true },
    };

    plotRef.current?.destroy();

    // jsdom (used under vitest) does not implement a functional 2D canvas
    // context -- getContext("2d") returns null, and jsdom also lacks the
    // global `Path2D` uPlot's paint routines depend on. uPlot schedules its
    // first paint asynchronously (post-microtask), so a try/catch around the
    // constructor call below cannot catch the resulting failure -- it fires
    // after this effect has already returned. Probe canvas support upfront
    // instead and skip mounting the chart entirely when it's unusable,
    // rather than crashing the component tree. The real Tauri webview has
    // full canvas support, so this only ever engages under jsdom.
    const probeCtx = document.createElement("canvas").getContext("2d");
    const canvasSupported = probeCtx !== null && typeof Path2D !== "undefined";

    if (!canvasSupported) {
      console.warn("FrameTimeChart: canvas 2D rendering unavailable in this environment, skipping chart render");
      return;
    }

    try {
      plotRef.current = new uPlot(opts, data, containerRef.current);
    } catch (err) {
      // Defense-in-depth: any other synchronous construction-time failure
      // (e.g. a genuinely broken canvas in some embedding) should also fail
      // soft rather than crash the tree.
      console.warn("FrameTimeChart: uPlot failed to construct, skipping render", err);
      plotRef.current = null;
    }

    return () => {
      plotRef.current?.destroy();
      plotRef.current = null;
    };
  }, [baseline, scenarios]);

  return <div ref={containerRef} className="w-full" />;
};
