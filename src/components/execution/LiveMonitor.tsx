import React, { useEffect, useRef, useState } from "react";
import { Pause, Play, Square, Terminal, Activity, CheckCircle2, XCircle, ShieldAlert, RefreshCw } from "lucide-react";
import { getRunSnapshot, sendControl, setShutdownWhenComplete, subscribeToEngineEvents } from "../../lib/api";
import type { EngineEvent, Phase, Project, RunProgress } from "../../lib/bindings";
import { canOperatorControl, phaseLabel, phaseScenarioId } from "../../lib/phase";
import { countReboots, scenarioRequiresReboot } from "../../lib/reboots";
import type { RunOutcome } from "../../App";

interface LiveMonitorProps {
  project: Project;
  runId: string | null;
  // Run-lifecycle outcome (RunComplete/RunFailed), owned and updated by
  // App.tsx's own app-lifetime engine-event subscription -- not derived
  // locally -- so the terminal banner below still renders correctly after
  // this component unmounts (tab switch) and remounts, even if the terminal
  // event itself arrived while it was unmounted.
  runOutcome: RunOutcome | null;
  onBackToBuilder: () => void;
  onViewResults?: (runId: string) => void;
  /** Fetched by App.tsx (`getRunProgress`) when a run becomes active and on
   * every `PhaseChanged`/`ScenarioComplete` it already handles -- this
   * component does no polling of its own, only renders what it's given. */
  progress?: RunProgress | null;
  /** The value the run was started with (the countdown modal's choice) --
   * seeds the checkbox until the first `progress.json` exists (the engine
   * only writes it after the thermal baseline). */
  initialShutdownWhenComplete?: boolean;
}

/** Matches the engine's own `Post-boot settle: {n}s remaining` log line
 * (emitted every 30s during the settle wait, spec D5) to drive the
 * boot_resume countdown below. */
const SETTLE_LOG_PATTERN = /^Post-boot settle: (\d+)s remaining$/;

/** Human-readable heading text for the two M3 reboot phases -- mirrors the
 * `thermal_baseline` special case just below it: `phaseLabel` itself stays
 * a literal mirror of the engine's `Display` impl (`reboot_pending:apply_next`
 * etc, asserted in `phase.test.ts`), so the human-friendly text lives here
 * instead of replacing that contract. */
function rebootHeading(phase: Phase): string | null {
  if (phase.kind === "reboot_pending") {
    switch (phase.reason) {
      case "apply_next":
        return "Rebooting to apply the next scenario…";
      case "revert_only":
        return "Rebooting to revert…";
      case "revert_and_apply_next":
        return "Rebooting to revert and apply the next scenario…";
      default: {
        const _exhaustive: never = phase.reason;
        return _exhaustive;
      }
    }
  }
  if (phase.kind === "boot_resume") {
    return "Resumed after reboot — settling…";
  }
  return null;
}

interface LogLine {
  id: number;
  text: string;
  /** `Date.now()` at receipt -- for correlating event timing during a live
   * diagnosis (e.g. how long a capture actually ran before failing), not a
   * backend-authoritative timestamp. */
  timestamp: number;
}

let nextLogId = 0;

function formatLogTimestamp(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number, width = 2) => String(n).padStart(width, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}.${pad(d.getMilliseconds(), 3)}`;
}

export const LiveMonitor: React.FC<LiveMonitorProps> = ({
  project,
  runId,
  runOutcome,
  onBackToBuilder,
  onViewResults,
  progress = null,
  initialShutdownWhenComplete = false,
}) => {
  const [hasActiveRun, setHasActiveRun] = useState<boolean | null>(null); // null = still checking
  const [phase, setPhase] = useState<Phase | null>(null);
  const [currentScenario, setCurrentScenario] = useState<string | null>(null);
  const [completedScenarios, setCompletedScenarios] = useState<string[]>([]);
  const [logLines, setLogLines] = useState<LogLine[]>([]);
  const [operatorPrompt, setOperatorPrompt] = useState<string | null>(null);
  // Parsed from the engine's own `Post-boot settle: {n}s remaining` log
  // line (emitted every 30s during `boot_resume`) -- no separate event/poll
  // exists for this, so the log stream is the source of truth.
  const [settleSecondsRemaining, setSettleSecondsRemaining] = useState<number | null>(null);
  const rebootCount = countReboots(
    (project.scenarios ?? []).filter((s) => s.enabled ?? true).map(scenarioRequiresReboot)
  );
  // Local-only optimistic flag: the engine only honours Pause between
  // iterations and never emits a distinct "paused" confirmation event, so
  // this reflects "the user asked to pause", not a backend-confirmed state.
  const [pauseRequested, setPauseRequested] = useState(false);
  // Local-only optimistic flags -- same reasoning as pauseRequested above.
  const [abortRequested, setAbortRequested] = useState(false);
  // The last shutdown-toggle value the user successfully sent this mount;
  // null = nothing sent yet, fall through to progress/initial. Safe to be
  // optimistic: the backend channel is a watch (latest value wins, the send
  // never blocks) -- see set_shutdown_when_complete in commands/run.rs.
  const [shutdownChecked, setShutdownChecked] = useState<boolean | null>(null);
  const [thermalProgress, setThermalProgress] = useState<{
    sample: number;
    total: number;
    cpuTempCelsius: number | null;
  } | null>(null);
  const [controlError, setControlError] = useState<string | null>(null);
  const [isSendingControl, setIsSendingControl] = useState(false);
  // Own loading/error state, independent of isSendingControl/controlError
  // above -- those are scoped to ControlMsg actions (Pause/Resume/Abort/
  // OperatorAcknowledged) specifically, and mixing this in would spuriously
  // disable the Pause/Abort buttons while the checkbox is mid-toggle (or
  // vice versa).
  const [isSendingShutdownToggle, setIsSendingShutdownToggle] = useState(false);
  const [shutdownToggleError, setShutdownToggleError] = useState<string | null>(null);

  const pushLog = (text: string) => {
    setLogLines((prev) => {
      const next = [...prev, { id: nextLogId++, text, timestamp: Date.now() }];
      return next.length > 500 ? next.slice(next.length - 500) : next;
    });
  };

  // Terminal banner state comes entirely from the `runOutcome` prop (App's
  // app-lifetime subscription), not from this component's own event
  // subscription below -- see the RunComplete/RunFailed cases there, which
  // are intentionally no-ops for lifecycle purposes. Scoped to the run this
  // instance is actually showing: a `runId` prop match for a live/just-
  // started run, or -- on the tab-revisit reconnect path where `runId` is
  // null and reconciliation instead goes through `getRunSnapshot()` -- any
  // outcome for the current project, matching that same reconnect path's
  // existing (documented, not-in-scope-to-fix-here) project-only matching.
  const terminal = runOutcome && (runId === null || runOutcome.runId === runId) ? runOutcome : null;

  const terminalEndRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    terminalEndRef.current?.scrollIntoView({ behavior: "auto" });
  }, [logLines]);

  useEffect(() => {
    let cancelled = false;
    let unsubscribe: (() => void) | null = null;
    // `getRunSnapshot()` below is a separate, best-effort read of the
    // backend's persisted RunState -- it can resolve *after* the live
    // subscription (started synchronously just below, when `runId` is
    // already known) has already delivered fresher PhaseChanged events.
    // Without this guard, a slow-resolving snapshot response overwrites
    // whatever phase the live stream already advanced to (e.g. clobbering
    // "thermal_baseline" back down to "preflight") -- the exact race this
    // flags. `getRunSnapshot()` is only actually needed to bootstrap state
    // on the tab-revisit reconnect path (`runId === null`, no live events
    // have happened yet in this effect instance); once a live event has
    // arrived, the snapshot response is redundant and must not win.
    let liveEventReceived = false;
    setHasActiveRun(null);
    setPhase(null);
    setCurrentScenario(null);
    setCompletedScenarios([]);
    setLogLines([]);
    setOperatorPrompt(null);
    setPauseRequested(false);
    setAbortRequested(false);
    setShutdownChecked(null);
    setThermalProgress(null);
    setControlError(null);
    setSettleSecondsRemaining(null);

    // Subscribes to live engine events, idempotently. Called synchronously
    // below when runId is already known to be active, and also from the
    // getRunSnapshot() continuation for the tab-revisit path (runId ===
    // null but a matching snapshot is found) -- whenever hasActiveRun ends
    // up true, we must reach this before the effect is done, or the
    // reconnect path never receives new events (no log lines, no phase
    // advancement).
    const ensureSubscribed = () => {
      if (unsubscribe || cancelled) return;
      unsubscribe = subscribeToEngineEvents((event: EngineEvent) => {
        liveEventReceived = true;
        switch (event.type) {
        case "PhaseChanged": {
          setPhase(event.phase);
          const sc = phaseScenarioId(event.phase);
          if (sc !== null) setCurrentScenario(sc);
          // A stale countdown must not survive into the next phase.
          setThermalProgress(null);
          break;
        }
        case "LogLine": {
          pushLog(event.text);
          const settleMatch = SETTLE_LOG_PATTERN.exec(event.text);
          if (settleMatch) setSettleSecondsRemaining(Number(settleMatch[1]));
          break;
        }
        case "IterationStarted":
          pushLog(`[${event.kind}] iteration ${event.index} started`);
          break;
        case "CapturePending":
          pushLog("Capture pending…");
          break;
        case "CaptureResumed":
          pushLog("Capture resumed");
          break;
        case "RecordingStarted":
          pushLog("Recording started");
          break;
        case "RecordingStopped":
          pushLog("Recording stopped");
          break;
        case "IterationComplete":
          pushLog(
            `Iteration complete — avg ${event.metrics.avg_fps ?? "?"} FPS, P1 ${event.metrics.p1_fps ?? "?"} FPS`
          );
          break;
        case "ScenarioComplete":
          setCompletedScenarios((prev) => [...prev, event.result.scenario_id]);
          pushLog(`Scenario "${event.result.name}" complete`);
          break;
        case "ScenarioScored":
          pushLog(
            `Scenario "${event.result.name}" scored: wcps=${event.result.wcps?.toFixed(2) ?? "?"} verdict=${event.result.verdict}`
          );
          break;
        case "OperatorPrompt":
          setOperatorPrompt(event.text);
          break;
        case "RunComplete":
        case "RunFailed":
          // No-op here: run-lifecycle outcome is owned by App.tsx's own
          // app-lifetime subscription (see App.tsx) and flows back down as
          // the `runOutcome` prop, precisely so it isn't lost when this
          // component is unmounted (tab switch) before the terminal event
          // arrives.
          break;
        case "RollbackProgress":
          pushLog(`Rolling back — ${event.reverted} mutation(s) reverted so far`);
          break;
        case "ThermalProgress":
          setThermalProgress({
            sample: event.sample,
            total: event.total,
            cpuTempCelsius: event.cpu_temp_celsius,
          });
          break;
        default: {
          const _exhaustive: never = event;
          void _exhaustive;
        }
      }
      });
    };

    if (runId) {
      // A run is known to be active from this session -- subscribe
      // immediately rather than waiting on the snapshot fetch below, so no
      // events are missed during that async window.
      ensureSubscribed();
    }

    getRunSnapshot()
      .then((snap) => {
        if (cancelled) return;
        const snapMatches = snap !== null && snap.project_id === project.id;
        // Skip if a live event already arrived while this fetch was in
        // flight -- see this effect's own guard comment above for why an
        // older snapshot must never overwrite fresher live-event state.
        if (snap && snapMatches && !liveEventReceived) {
          setPhase(snap.phase);
          setCurrentScenario(snap.current_scenario);
          setCompletedScenarios(snap.completed_scenarios);
        }
        const active = snapMatches;
        setHasActiveRun(active);
        if (active) ensureSubscribed();
      })
      .catch(() => {
        if (cancelled) return;
        setHasActiveRun(runId !== null); // runId is set -- assume active even if the snapshot read failed
      });

    return () => {
      cancelled = true;
      if (unsubscribe) unsubscribe();
    };
  }, [runId, project.id]);

  const handlePause = async () => {
    if (isSendingControl) return;
    setControlError(null);
    setPauseRequested(true);
    setIsSendingControl(true);
    try {
      await sendControl("Pause");
    } catch (e) {
      setPauseRequested(false);
      setControlError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsSendingControl(false);
    }
  };

  const handleResume = async () => {
    if (isSendingControl) return;
    setControlError(null);
    setIsSendingControl(true);
    try {
      await sendControl("Resume");
      setPauseRequested(false);
    } catch (e) {
      setControlError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsSendingControl(false);
    }
  };

  const handleAbort = async () => {
    if (isSendingControl || abortRequested) return;
    setControlError(null);
    setAbortRequested(true);
    setIsSendingControl(true);
    try {
      await sendControl("Abort");
    } catch (e) {
      setAbortRequested(false);
      setControlError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsSendingControl(false);
    }
  };

  const handleAcknowledgePrompt = async () => {
    if (isSendingControl) return;
    setControlError(null);
    setIsSendingControl(true);
    try {
      await sendControl("OperatorAcknowledged");
      setOperatorPrompt(null);
    } catch (e) {
      setControlError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsSendingControl(false);
    }
  };

  const handleShutdownToggle = async (checked: boolean) => {
    if (isSendingShutdownToggle) return;
    setShutdownToggleError(null);
    setShutdownChecked(checked);
    setIsSendingShutdownToggle(true);
    try {
      await setShutdownWhenComplete(checked);
    } catch (e) {
      setShutdownChecked(!checked);
      setShutdownToggleError(e instanceof Error ? e.message : String(e));
    } finally {
      setIsSendingShutdownToggle(false);
    }
  };

  if (hasActiveRun === null) {
    return <div className="p-8 text-center text-white/50 font-mono text-sm">Checking for an active run…</div>;
  }

  if (!hasActiveRun) {
    return (
      <div className="p-8 max-w-3xl mx-auto text-center space-y-4">
        <Activity className="w-8 h-8 mx-auto text-white/20" />
        <p className="text-sm text-white/50 font-mono">No active run for this project.</p>
        <button
          onClick={onBackToBuilder}
          className="px-4 py-2 rounded-xl glass-pill text-xs font-mono text-white/80 cursor-pointer"
        >
          Back to Builder
        </button>
      </div>
    );
  }

  // Both thermal phases sample HWiNFO the same way (the same ThermalProgress
  // events drive `thermalProgress` below) -- only the heading text differs,
  // so they share one spinner+readout render branch.
  const thermalStatusLabel =
    phase?.kind === "thermal_baseline"
      ? "Collecting thermal baseline…"
      : phase?.kind === "thermal_cooldown"
        ? "Waiting for thermal cooldown…"
        : null;

  return (
    <div className="p-8 max-w-7xl mx-auto space-y-8 w-full">
      <div className="glass-panel p-6 rounded-2xl space-y-4">
        <div className="flex flex-col lg:flex-row lg:items-center justify-between gap-4">
          <div className="space-y-1">
            <div className="flex items-center gap-2">
              <span className={`w-2.5 h-2.5 rounded-full ${terminal ? "bg-white/20" : "bg-emerald-400 animate-pulse"}`} />
              <span
                className={`font-mono text-xs font-bold uppercase tracking-wider ${terminal?.kind === "failed" ? "text-red-400" : "text-emerald-400"}`}
              >
                {terminal?.kind === "complete"
                  ? "Run Complete"
                  : terminal?.kind === "failed"
                    ? "Run Failed"
                    : "Autonomous Pipeline Active"}
              </span>
              <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-white/5 text-white/60 border border-white/10">
                Project: {project.name}
              </span>
              {rebootCount > 0 && (
                <span className="px-2 py-0.5 rounded text-[10px] font-mono bg-amber-500/15 text-amber-300 border border-amber-500/30">
                  {rebootCount} reboot{rebootCount === 1 ? "" : "s"}
                </span>
              )}
            </div>
            <h2 className="font-sans font-bold text-2xl text-white flex items-center gap-2">
              {phase?.kind === "aborting" ? (
                <>
                  <RefreshCw className="w-5 h-5 animate-spin" aria-hidden="true" />
                  <span>Aborting — rolling back…</span>
                </>
              ) : thermalStatusLabel ? (
                <>
                  <RefreshCw className="w-5 h-5 animate-spin" aria-hidden="true" />
                  <span>{thermalStatusLabel}</span>
                  {thermalProgress && (
                    <span className="text-sm font-normal text-white/50">
                      (sample {thermalProgress.sample}/{thermalProgress.total}
                      {thermalProgress.cpuTempCelsius !== null &&
                        `, ${thermalProgress.cpuTempCelsius.toFixed(1)}°C`}
                      )
                    </span>
                  )}
                </>
              ) : phase && rebootHeading(phase) !== null ? (
                <>
                  <RefreshCw className="w-5 h-5 animate-spin" aria-hidden="true" />
                  <span>{rebootHeading(phase)}</span>
                  {phase.kind === "boot_resume" && settleSecondsRemaining !== null && (
                    <span className="text-sm font-normal text-white/50">
                      ({settleSecondsRemaining}s remaining)
                    </span>
                  )}
                </>
              ) : (
                <span>Phase: {phase ? phaseLabel(phase) : "starting…"}</span>
              )}
            </h2>
            <p className="text-xs text-white/50 font-mono">
              {currentScenario ? `Current scenario: ${currentScenario}` : "No scenario active yet"} •{" "}
              {completedScenarios.length} completed
            </p>
          </div>

          {!terminal && canOperatorControl(phase) && (
            <div className="flex flex-col gap-3">
              <div className="flex items-center gap-3">
                {pauseRequested ? (
                  <button
                    onClick={handleResume}
                    disabled={isSendingControl}
                    className="glass-pill px-4 py-2 rounded-xl text-xs font-mono text-white/80 hover:text-white flex items-center gap-2 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                  >
                    <Play className="w-4 h-4" />
                    <span>Resume</span>
                  </button>
                ) : (
                  <button
                    onClick={handlePause}
                    disabled={isSendingControl}
                    className="glass-pill px-4 py-2 rounded-xl text-xs font-mono text-white/80 hover:text-white flex items-center gap-2 cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
                  >
                    <Pause className="w-4 h-4" />
                    <span>Pause</span>
                  </button>
                )}
                <button
                  onClick={handleAbort}
                  disabled={isSendingControl || abortRequested}
                  className="px-4 py-2 rounded-xl bg-red-500/20 hover:bg-red-500/30 text-red-300 border border-red-500/40 text-xs font-mono font-semibold flex items-center gap-2 cursor-pointer transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
                >
                  <Square className="w-3.5 h-3.5 fill-current" />
                  <span>{abortRequested ? "Aborting…" : "Abort & Roll Back"}</span>
                </button>
              </div>
              {pauseRequested && !abortRequested && (
                <p className="text-[10px] font-mono text-white/40">
                  Pause requested — pausing at the next safe point…
                </p>
              )}
              <label
                className={`glass-pill px-4 py-2 rounded-xl text-xs font-mono text-white/80 flex items-center gap-2 w-fit ${isSendingShutdownToggle ? "opacity-70" : "cursor-pointer"}`}
              >
                <input
                  type="checkbox"
                  checked={shutdownChecked ?? progress?.shutdown_when_complete ?? initialShutdownWhenComplete}
                  disabled={isSendingShutdownToggle}
                  onChange={(e) => handleShutdownToggle(e.target.checked)}
                  className="rounded bg-black/50 border-white/20 text-[#06b6d4] focus:ring-0 w-4 h-4 cursor-pointer disabled:cursor-not-allowed"
                />
                <span>Shut down when the run completes</span>
                {isSendingShutdownToggle && <RefreshCw className="w-3 h-3 animate-spin" aria-hidden="true" />}
              </label>
            </div>
          )}
        </div>
        {controlError && (
          <div className="p-3 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 text-xs font-mono">
            {controlError}
          </div>
        )}
        {shutdownToggleError && (
          <div className="p-3 rounded-xl bg-red-500/10 border border-red-500/30 text-red-300 text-xs font-mono">
            {shutdownToggleError}
          </div>
        )}
      </div>

      {operatorPrompt && canOperatorControl(phase) && (
        <div className="glass-panel p-6 rounded-2xl border-amber-500/30 space-y-3">
          <div className="flex items-center gap-2 text-amber-300 font-mono text-xs font-bold uppercase">
            <ShieldAlert className="w-4 h-4" /> Operator Action Required
          </div>
          <p className="text-sm text-white/80">{operatorPrompt}</p>
          <button
            onClick={handleAcknowledgePrompt}
            disabled={isSendingControl}
            className="px-4 py-2 rounded-xl bg-amber-500/20 hover:bg-amber-500/30 text-amber-200 border border-amber-500/40 text-xs font-mono font-semibold cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed"
          >
            Acknowledge
          </button>
        </div>
      )}

      {progress && progress.unstable.length > 0 && (
        <div className="glass-panel p-6 rounded-2xl border-amber-500/30 space-y-3">
          <div className="flex items-center gap-2 text-amber-300 font-mono text-xs font-bold uppercase">
            <ShieldAlert className="w-4 h-4" /> Unstable Scenarios
          </div>
          <ul className="space-y-1.5">
            {progress.unstable.map((u) => (
              <li key={u.scenario_id} className="text-xs font-mono text-white/70">
                <span className="text-white font-semibold">{u.scenario_id}</span>: {u.reason}
              </li>
            ))}
          </ul>
        </div>
      )}

      {terminal && (
        <div
          className={`glass-panel p-6 rounded-2xl space-y-3 ${terminal.kind === "complete" ? "border-emerald-500/30" : "border-red-500/30"}`}
        >
          <div
            className={`flex items-center gap-2 font-mono text-xs font-bold uppercase ${terminal.kind === "complete" ? "text-emerald-400" : "text-red-400"}`}
          >
            {terminal.kind === "complete" ? <CheckCircle2 className="w-4 h-4" /> : <XCircle className="w-4 h-4" />}
            {terminal.kind === "complete" ? "Run Complete" : "Run Failed"}
          </div>
          {terminal.reason && <p className="text-sm text-white/80">{terminal.reason}</p>}
          <div className="flex items-center gap-3">
            <button
              onClick={onBackToBuilder}
              className="px-4 py-2 rounded-xl glass-pill text-xs font-mono text-white/80 cursor-pointer"
            >
              Back to Builder
            </button>
            {terminal.kind === "complete" && runId && onViewResults && (
              <button
                onClick={() => onViewResults(runId)}
                className="px-4 py-2 rounded-xl bg-gradient-to-r from-[#06b6d4] to-[#8b5cf6] text-white font-mono text-xs font-bold cursor-pointer"
              >
                View Results
              </button>
            )}
          </div>
        </div>
      )}

      <div className="glass-panel p-6 rounded-2xl space-y-4">
        <div className="flex items-center gap-2.5">
          <Terminal className="w-4 h-4 text-[#06b6d4]" />
          <h3 className="font-mono text-xs font-bold text-white uppercase tracking-wider">Live Event Stream</h3>
        </div>
        <div className="bg-[#010103] border border-white/10 rounded-xl p-4 font-mono text-xs max-h-96 overflow-y-auto space-y-1.5 shadow-inner">
          {logLines.map((line) => (
            <div key={line.id} className="text-white/80 leading-relaxed">
              <span className="text-white/40">{formatLogTimestamp(line.timestamp)}</span> {line.text}
            </div>
          ))}
          <div ref={terminalEndRef} />
        </div>
      </div>
    </div>
  );
};
