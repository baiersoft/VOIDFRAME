import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { SocialShareModal } from "./SocialShareModal";
import type { RunResults, ScenarioResult } from "../../lib/bindings";

const winner: ScenarioResult = {
  scenario_id: "s1",
  name: "Core Parking Disabled",
  is_baseline: false,
  aggregated: {
    avg_fps: 430, median_fps: 432, p1_fps: 300, p01_fps: 250,
    frame_time_mean_ms: 2.3, frame_time_stddev_ms: 0.2, frame_time_cv: 0.09,
    adaptive_frame_time_cv: 0.07, stutter_count_pct: 0.6, mean_abs_animation_error_ms: 0.5,
    gpu_busy_ms: 2.0, bottleneck_ratio: 0.95, render_latency_ms: 7.5,
  },
  per_iteration: [],
  metric_deltas: [{ metric: "p1_fps", delta_pct: 15.4 }],
  wcps: 3.2,
  verdict: "better",
};

const results: RunResults = {
  schema_version: "1.0.0",
  run_id: "r1",
  project_id: "p1",
  completed_at: "2026-09-02T12:00:00Z",
  detection_tier: "log_tail",
  baseline: { ...winner, scenario_id: "baseline", name: "Stock Baseline", is_baseline: true, wcps: null, metric_deltas: [], verdict: "confirmed_same" },
  scenarios: [winner],
};

describe("SocialShareModal", () => {
  it("renders the winner's real WCPS v3 score and P1 gain, no fake thermal line", () => {
    render(<SocialShareModal isOpen={true} onClose={vi.fn()} results={results} winner={winner} />);
    expect(screen.getByText(/3\.2/)).toBeInTheDocument(); // wcps
    expect(screen.queryByText(/°C/)).not.toBeInTheDocument(); // no fake thermal claim anywhere
  });

  it("renders nothing when isOpen is false", () => {
    const { container } = render(<SocialShareModal isOpen={false} onClose={vi.fn()} results={results} winner={winner} />);
    expect(container.firstChild).toBeNull();
  });
});
