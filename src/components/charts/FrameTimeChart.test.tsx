import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { FrameTimeChart } from "./FrameTimeChart";
import type { ScenarioResult } from "../../lib/bindings";

function makeScenario(name: string, frameTimesMs: number[]): ScenarioResult {
  return {
    scenario_id: name.toLowerCase().replace(/\s+/g, "_"),
    name,
    is_baseline: name === "Baseline",
    aggregated: {
      avg_fps: null, median_fps: null, p1_fps: null, p01_fps: null,
      frame_time_mean_ms: frameTimesMs[0] ?? null, frame_time_stddev_ms: null, frame_time_cv: null,
      adaptive_frame_time_cv: null, stutter_count_pct: null, mean_abs_animation_error_ms: null,
      gpu_busy_ms: null, bottleneck_ratio: null, render_latency_ms: null,
    },
    per_iteration: frameTimesMs.map((ft) => ({
      avg_fps: null, median_fps: null, p1_fps: null, p01_fps: null,
      frame_time_mean_ms: ft, frame_time_stddev_ms: null, frame_time_cv: null,
      adaptive_frame_time_cv: null, stutter_count_pct: null, mean_abs_animation_error_ms: null,
      gpu_busy_ms: null, bottleneck_ratio: null, render_latency_ms: null,
    })),
    metric_deltas: [],
    wcps: null,
    verdict: "confirmed_same",
  };
}

// NOTE on assertions: jsdom does not implement a functional 2D canvas context
// (HTMLCanvasElement#getContext("2d") returns null) and also lacks the global
// `Path2D` uPlot's paint routines depend on. FrameTimeChart probes for this
// upfront and skips mounting uPlot when canvas rendering is unusable (see
// FrameTimeChart.tsx), so under jsdom no <canvas> element is ever created by
// uPlot. These assertions therefore check for the chart's container <div>
// rather than a <canvas> element -- see task-2-report.md for the full
// jsdom/canvas investigation this deviation is based on.
describe("FrameTimeChart", () => {
  it("renders without crashing given a baseline and one scenario with matching iteration counts", () => {
    const baseline = makeScenario("Baseline", [2.5, 2.4, 2.6]);
    const scenario = makeScenario("Tweak A", [2.2, 2.1, 2.3]);
    const { container } = render(<FrameTimeChart baseline={baseline} scenarios={[scenario]} />);
    expect(container.querySelector("div")).not.toBeNull();
  });

  it("renders without crashing given zero iterations (empty per_iteration arrays)", () => {
    const baseline = makeScenario("Baseline", []);
    const { container } = render(<FrameTimeChart baseline={baseline} scenarios={[]} />);
    // uPlot needs at least an x-axis array; an empty run must not throw during mount.
    expect(container).toBeTruthy();
  });

  it("renders without crashing when scenarios have a different iteration count than baseline", () => {
    const baseline = makeScenario("Baseline", [2.5, 2.4, 2.6, 2.5]);
    const scenario = makeScenario("Short Run", [2.2, 2.1]);
    const { container } = render(<FrameTimeChart baseline={baseline} scenarios={[scenario]} />);
    expect(container.querySelector("div")).not.toBeNull();
  });
});
