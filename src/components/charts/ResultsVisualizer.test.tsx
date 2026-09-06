import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { ResultsVisualizer } from "./ResultsVisualizer";
import type { RunResults } from "../../lib/bindings";

const sampleResults: RunResults = {
  schema_version: "1.0.0",
  run_id: "r1",
  project_id: "p1",
  completed_at: "2026-09-02T12:00:00Z",
  detection_tier: "log_tail",
  baseline: {
    scenario_id: "baseline",
    name: "Stock Baseline",
    is_baseline: true,
    aggregated: {
      avg_fps: 400,
      median_fps: 402,
      p1_fps: 260,
      p01_fps: 210,
      frame_time_mean_ms: 2.5,
      frame_time_stddev_ms: 0.3,
      frame_time_cv: 0.12,
      adaptive_frame_time_cv: 0.1,
      stutter_count_pct: 1.5,
      mean_abs_animation_error_ms: 0.8,
      gpu_busy_ms: 2.1,
      bottleneck_ratio: 0.9,
      render_latency_ms: 8.0,
    },
    per_iteration: [
      { avg_fps: 398, median_fps: 400, p1_fps: 258, p01_fps: 208, frame_time_mean_ms: 2.51, frame_time_stddev_ms: 0.31, frame_time_cv: 0.13, adaptive_frame_time_cv: 0.11, stutter_count_pct: 1.6, mean_abs_animation_error_ms: 0.82, gpu_busy_ms: 2.11, bottleneck_ratio: 0.89, render_latency_ms: 8.1 },
      { avg_fps: 402, median_fps: 404, p1_fps: 262, p01_fps: 212, frame_time_mean_ms: 2.49, frame_time_stddev_ms: 0.29, frame_time_cv: 0.11, adaptive_frame_time_cv: 0.09, stutter_count_pct: 1.4, mean_abs_animation_error_ms: 0.78, gpu_busy_ms: 2.09, bottleneck_ratio: 0.91, render_latency_ms: 7.9 },
    ],
    metric_deltas: [],
    wcps: null,
    verdict: "confirmed_same",
  },
  scenarios: [
    {
      scenario_id: "s1",
      name: "Core Parking Disabled",
      is_baseline: false,
      aggregated: {
        avg_fps: 430,
        median_fps: 432,
        p1_fps: 300,
        p01_fps: 250,
        frame_time_mean_ms: 2.3,
        frame_time_stddev_ms: 0.2,
        frame_time_cv: 0.09,
        adaptive_frame_time_cv: 0.07,
        stutter_count_pct: 0.6,
        mean_abs_animation_error_ms: 0.5,
        gpu_busy_ms: 2.0,
        bottleneck_ratio: 0.95,
        render_latency_ms: 7.5,
      },
      per_iteration: [
        { avg_fps: 428, median_fps: 430, p1_fps: 298, p01_fps: 248, frame_time_mean_ms: 2.31, frame_time_stddev_ms: 0.21, frame_time_cv: 0.09, adaptive_frame_time_cv: 0.08, stutter_count_pct: 0.7, mean_abs_animation_error_ms: 0.52, gpu_busy_ms: 2.01, bottleneck_ratio: 0.94, render_latency_ms: 7.6 },
        { avg_fps: 432, median_fps: 434, p1_fps: 302, p01_fps: 252, frame_time_mean_ms: 2.29, frame_time_stddev_ms: 0.19, frame_time_cv: 0.08, adaptive_frame_time_cv: 0.06, stutter_count_pct: 0.5, mean_abs_animation_error_ms: 0.48, gpu_busy_ms: 1.99, bottleneck_ratio: 0.96, render_latency_ms: 7.4 },
      ],
      metric_deltas: [
        { metric: "avg_fps", delta_pct: 7.5 },
        { metric: "p1_fps", delta_pct: 15.4 },
      ],
      wcps: 3.2,
      verdict: "better",
    },
  ],
};

describe("ResultsVisualizer", () => {
  it("renders real scenario names and metric values from RunResults, not mock data", () => {
    render(<ResultsVisualizer results={sampleResults} />);
    expect(screen.getByText("Core Parking Disabled")).toBeInTheDocument();
    expect(screen.getByText("Stock Baseline")).toBeInTheDocument();
    // Use getAllByText, not getByText: Recharts may also render "430" as an
    // axis tick/tooltip text node in the SVG, and getByText throws on
    // multiple matches. The table cell is guaranteed to be one of the
    // matches regardless of how many chart-side nodes also show it.
    expect(screen.getAllByText(/430/).length).toBeGreaterThan(0);
  });

  it("renders a single significance verdict badge per scenario row", () => {
    render(<ResultsVisualizer results={sampleResults} />);
    expect(screen.getAllByText(/better/i).length).toBeGreaterThan(0);
  });

  it("renders the reproducibility panel using the baseline's real frame_time_cv", () => {
    render(<ResultsVisualizer results={sampleResults} />);
    expect(screen.getByText(/12(\.0)?%/)).toBeInTheDocument(); // baseline frame_time_cv 0.12 -> 12%
  });

  it("sorts the scenario table by WCPS v3 descending by default", () => {
    const twoScenarios: RunResults = {
      ...sampleResults,
      scenarios: [
        { ...sampleResults.scenarios[0], scenario_id: "s_low", name: "Low WCPS", wcps: 1.0 },
        { ...sampleResults.scenarios[0], scenario_id: "s_high", name: "High WCPS", wcps: 5.0 },
      ],
    };
    render(<ResultsVisualizer results={twoScenarios} />);
    const rows = screen.getAllByRole("row").map((r) => r.textContent ?? "");
    const highIdx = rows.findIndex((r) => r.includes("High WCPS"));
    const lowIdx = rows.findIndex((r) => r.includes("Low WCPS"));
    expect(highIdx).toBeGreaterThan(-1);
    expect(lowIdx).toBeGreaterThan(highIdx); // higher WCPS sorts first (lower row index)
  });

  it("renders the 3 new WCPS v3 pacing metrics with real values from the mock scenario", () => {
    render(<ResultsVisualizer results={sampleResults} />);
    // adaptive_frame_time_cv: 0.07 -> "7.0%"
    expect(screen.getAllByText(/7\.0%/).length).toBeGreaterThan(0);
    // stutter_count_pct: 0.6 -> "0.6%"
    expect(screen.getAllByText(/0\.6%/).length).toBeGreaterThan(0);
    // mean_abs_animation_error_ms: 0.5 -> "0.5"
    expect(screen.getAllByText(/^0\.5$/).length).toBeGreaterThan(0);
  });
});
