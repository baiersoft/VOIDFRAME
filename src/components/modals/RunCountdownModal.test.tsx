import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { RunCountdownModal } from "./RunCountdownModal";
import type { Project } from "../../lib/bindings";

const PROJECT: Project = {
  schema_version: "1.0.0",
  id: "p1",
  name: "Test Project",
  description: "d",
  created_at: "2026-01-01T00:00:00Z",
  settings: {
    schema_version: "1.0.0",
    warmup_loops: 2,
    measure_loops: 3,
    capture_seconds: 60,
    map_id: "3240880604",
    watchdog_seconds: 120,
    netcon_port: null,
  },
  baseline: { name: "Stock", description: "d" },
  scenarios: [],
};

describe("RunCountdownModal", () => {
  it("renders nothing when no project is queued", () => {
    const { container } = render(
      <RunCountdownModal project={null} onCancel={vi.fn()} onConfirm={vi.fn()} />
    );
    expect(container).toBeEmptyDOMElement();
  });

  it("shows the queued project's name and the initial countdown value", () => {
    render(
      <RunCountdownModal
        project={PROJECT}
        onCancel={vi.fn()}
        onConfirm={vi.fn()}
        countdownSeconds={3}
      />
    );
    expect(screen.getByText("Test Project")).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();
  });

  it("calls onCancel and never onConfirm when Cancel is clicked, without waiting out the countdown", () => {
    const onCancel = vi.fn();
    const onConfirm = vi.fn();
    render(
      <RunCountdownModal
        project={PROJECT}
        onCancel={onCancel}
        onConfirm={onConfirm}
        countdownSeconds={30}
      />
    );
    fireEvent.click(screen.getByRole("button", { name: /cancel/i }));
    expect(onCancel).toHaveBeenCalledTimes(1);
    expect(onConfirm).not.toHaveBeenCalled();
  });

  it("calls onConfirm on its own once the countdown reaches zero, with no click needed", async () => {
    vi.useFakeTimers();
    try {
      const onConfirm = vi.fn();
      render(
        <RunCountdownModal
          project={PROJECT}
          onCancel={vi.fn()}
          onConfirm={onConfirm}
          countdownSeconds={2}
        />
      );
      expect(onConfirm).not.toHaveBeenCalled();
      // Advance one tick at a time rather than jumping the full 2000ms at
      // once: each tick's setTimeout is only scheduled once React has
      // actually run the effect from the PREVIOUS tick's state update, and
      // a single large jump can outrun that -- confirmed empirically while
      // writing this test, which flaked exactly this way at 2000ms in one
      // shot but not one second at a time.
      for (let i = 0; i < 2; i++) {
        await act(async () => {
          await vi.advanceTimersByTimeAsync(1000);
        });
      }
      expect(onConfirm).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("resets to a fresh countdown when a different project is queued after a previous one finished", async () => {
    vi.useFakeTimers();
    try {
      const onConfirm = vi.fn();
      const { rerender } = render(
        <RunCountdownModal
          project={PROJECT}
          onCancel={vi.fn()}
          onConfirm={onConfirm}
          countdownSeconds={1}
        />
      );
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
      });
      expect(onConfirm).toHaveBeenCalledTimes(1);

      const secondProject: Project = { ...PROJECT, id: "p2", name: "Second Project" };
      rerender(
        <RunCountdownModal
          project={secondProject}
          onCancel={vi.fn()}
          onConfirm={onConfirm}
          countdownSeconds={1}
        />
      );
      // Must show the fresh countdown value again, not an already-expired
      // leftover `0` that would fire `onConfirm` a second time instantly.
      expect(screen.getByText("1")).toBeInTheDocument();
      expect(onConfirm).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
