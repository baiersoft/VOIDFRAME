import React, { useEffect, useState } from "react";
import { Rocket, X } from "lucide-react";
import type { Project } from "../../lib/bindings";

// 0 under Vitest (vite's standard test-mode flag) so integration-style
// tests elsewhere (e.g. App.test.tsx) that click through "Run" don't have
// to wait out a real countdown or fight fake timers -- the effect below
// sees `secondsLeft <= 0` immediately and calls `onConfirm()` on the first
// render, with zero behavior difference in an actual build. Overridable
// via the `countdownSeconds` prop specifically so this component's own
// test file can still exercise a real, short countdown (including
// cancelling mid-way) despite this default.
const DEFAULT_COUNTDOWN_SECONDS = import.meta.env.MODE === "test" ? 0 : 5;

interface RunCountdownModalProps {
  project: Project | null;
  onCancel: () => void;
  onConfirm: () => void;
  countdownSeconds?: number;
}

// Once a run actually starts, `ControlMsg::Abort` is only checked between
// iterations (docs/superpowers/specs/2026-09-01-m1-benchmark-engine-design.md §7.1) -- an iteration already in progress (map load,
// warmup, a measured capture) cannot be interrupted, and the "quiet window"
// suspends the app's own UI for the duration of a capture, so there is no
// responsive Abort button to click even if there were one. This modal is
// the actual escape hatch: a fixed window to cancel BEFORE any of that
// starts, while cancelling is still free.
//
// Just a null-check + a `key`-remount wrapper around `Countdown` — keying
// on `project.id` forces a fresh mount (and therefore a fresh
// `useState(countdownSeconds)`) every time a different project is queued,
// rather than trying to coordinate a "reset" effect against a "tick /
// confirm" effect racing over the same state in one commit. That
// coordination was tried first and had a real bug: on a project switch,
// the confirm-check effect could still read the OLD project's already-
// expired `secondsLeft <= 0` before the reset effect's state update had
// applied, firing `onConfirm` a second, spurious time for the new project.
export const RunCountdownModal: React.FC<RunCountdownModalProps> = ({
  project,
  onCancel,
  onConfirm,
  countdownSeconds = DEFAULT_COUNTDOWN_SECONDS,
}) => {
  if (!project) return null;
  return (
    <Countdown
      key={project.id}
      project={project}
      onCancel={onCancel}
      onConfirm={onConfirm}
      countdownSeconds={countdownSeconds}
    />
  );
};

interface CountdownProps {
  project: Project;
  onCancel: () => void;
  onConfirm: () => void;
  countdownSeconds: number;
}

const Countdown: React.FC<CountdownProps> = ({
  project,
  onCancel,
  onConfirm,
  countdownSeconds,
}) => {
  const [secondsLeft, setSecondsLeft] = useState(countdownSeconds);

  useEffect(() => {
    if (secondsLeft <= 0) {
      onConfirm();
      return;
    }
    const id = setTimeout(() => setSecondsLeft((s) => s - 1), 1000);
    return () => clearTimeout(id);
  }, [secondsLeft, onConfirm]);

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center p-4 bg-black/85 backdrop-blur-xl">
      <div className="glass-panel w-full max-w-md rounded-2xl border border-[#06b6d4]/30 flex flex-col shadow-[0_0_60px_rgba(6,182,212,0.2)] overflow-hidden">
        <div className="p-6 border-b border-white/10 flex items-center justify-between">
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-xl bg-[#06b6d4]/10 border border-[#06b6d4]/30 text-[#06b6d4]">
              <Rocket className="w-5 h-5" />
            </div>
            <div>
              <h2 className="font-sans font-bold text-lg text-white">Starting Run</h2>
              <p className="text-xs text-white/50 font-mono">{project.name}</p>
            </div>
          </div>
          <button
            onClick={onCancel}
            className="p-2 rounded-lg text-white/50 hover:text-white hover:bg-white/5 transition-colors cursor-pointer"
          >
            <X className="w-5 h-5" />
          </button>
        </div>

        <div className="p-8 flex flex-col items-center gap-4">
          <div className="text-6xl font-mono font-bold text-[#22d3ee] tabular-nums">
            {secondsLeft}
          </div>
          <p className="text-xs font-mono text-white/60 text-center leading-relaxed">
            Once the run starts, it cannot be safely interrupted mid-scenario — leave your PC
            alone until it finishes. This is your last chance to cancel.
          </p>
        </div>

        <div className="p-6 border-t border-white/10 bg-black/30 flex items-center justify-center">
          <button
            onClick={onCancel}
            className="px-6 py-2.5 rounded-xl glass-pill text-sm font-mono font-bold text-white/80 hover:text-white cursor-pointer"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
};
