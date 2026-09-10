// Thin, ergonomic wrapper over `./bindings` (the generated Tauri Specta
// bindings) and the 4 hand-written window-chrome commands that have no
// generated bindings (see `src-tauri/src/lib.rs`'s own comment on why they
// are excluded from the specta registry).
//
// Every later frontend-integration plan should import from this module instead of
// touching `@tauri-apps/api`/`./bindings` directly. This file's own job:
//   - normalize `bindings.ts`'s `{ status: "ok" | "error" }` tagged-union
//     return shape into plain throw-on-error `Promise<T>`s, so call sites
//     use `try`/`catch` or `.catch()` rather than manual tag checking.
//   - give every command a clean, camelCase async function (already true of
//     `bindings.ts`'s own `commands` object, but re-exported here as free
//     functions so this module is the single, stable import path).
//   - wrap the 4 window-chrome commands, which have no generated bindings,
//     over raw `invoke()` calls by string name.
//   - wrap the hand-emitted `vf:event` channel (not specta-managed, so
//     `bindings.ts` does not cover it) via `@tauri-apps/api/event`'s `listen`.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getVersion } from "@tauri-apps/api/app";
import { commands } from "./bindings";
import type {
  AffinityCpuPayload,
  AffinityMode,
  Baseline,
  CatalogEntry,
  Check,
  CheckStatus,
  Config,
  ControlMsg,
  EngineEvent,
  Hive,
  MetricDelta,
  Metrics,
  Module,
  PowerPlan,
  PowerPlanPayload,
  PreflightReport,
  Project,
  RegistryPayload,
  RegistryValueView,
  RegType,
  RevertReport,
  RunProgress,
  RunResults,
  RunState,
  RunSummary,
  Scenario,
  ScenarioResult,
  Settings,
  Verdict,
} from "./bindings";

// Re-exports every type reachable from the commands/types this module
// already depends on (transitively, via EngineEvent/Project/Module/etc.) —
// so a later component never has to import from `./bindings` directly to
// get a type `api.ts` itself already relies on.
export type {
  AffinityCpuPayload,
  AffinityMode,
  Baseline,
  CatalogEntry,
  Check,
  CheckStatus,
  Config,
  ControlMsg,
  EngineEvent,
  Hive,
  MetricDelta,
  Metrics,
  Module,
  PowerPlan,
  PowerPlanPayload,
  PreflightReport,
  Project,
  RegistryPayload,
  RegistryValueView,
  RegType,
  RevertReport,
  RunProgress,
  RunResults,
  RunState,
  RunSummary,
  Scenario,
  ScenarioResult,
  Settings,
  Verdict,
};

/**
 * The tagged-union shape every `bindings.ts` command call resolves to
 * (`typedError`'s own return type). `error` is always `string` for this
 * app's commands (every `#[tauri::command]` here returns `Result<T, String>`).
 */
type CommandResult<T> = { status: "ok"; data: T } | { status: "error"; error: string };

/** Unwraps a `bindings.ts` command call into throw-on-error semantics. */
async function unwrap<T>(promise: Promise<CommandResult<T>>): Promise<T> {
  const result = await promise;
  if (result.status === "error") {
    throw new Error(result.error);
  }
  return result.data;
}

// --- Project commands ------------------------------------------------------

export async function listProjects(): Promise<Project[]> {
  return unwrap(commands.listProjects());
}

export async function getProject(id: string): Promise<Project> {
  return unwrap(commands.getProject(id));
}

export async function saveProject(project: Project): Promise<void> {
  await unwrap(commands.saveProject(project));
}

export async function deleteProject(id: string): Promise<void> {
  await unwrap(commands.deleteProject(id));
}

export async function validateScenario(project: Project): Promise<string[]> {
  return unwrap(commands.validateScenario(project));
}

// --- Preflight / config / catalog ------------------------------------------

export async function preflight(projectId: string | null): Promise<PreflightReport> {
  return unwrap(commands.preflight(projectId));
}

export async function getConfig(): Promise<Config> {
  return unwrap(commands.getConfig());
}

export async function saveConfig(config: Config): Promise<void> {
  await unwrap(commands.saveConfig(config));
}

export async function listCatalogTweaks(): Promise<CatalogEntry[]> {
  return unwrap(commands.listCatalogTweaks());
}

export async function listPowerPlans(): Promise<PowerPlan[]> {
  return unwrap(commands.listPowerPlans());
}

/** The real CPU model string, read once from the registry. */
export async function getCpuModel(): Promise<string> {
  return unwrap(commands.cpuModel());
}

/** The real GPU model string, read once via DXGI. */
export async function getGpuModel(): Promise<string> {
  return unwrap(commands.gpuModel());
}

/** The user's real current CS2 launch options, with VOIDFRAME's own
 * required tokens already stripped out -- always just the human part. */
export async function readCs2LaunchOptions(): Promise<string> {
  return unwrap(commands.readCs2LaunchOptions());
}

/** The user's real current cs2_video.txt settings, for pre-filling the
 * cs2_config builder form on "Add Module" -- mirrors readCs2LaunchOptions'
 * own live-value-on-add pattern for launch_args. */
export async function readCs2VideoConfig(): Promise<Record<string, string>> {
  return unwrap(commands.readCs2VideoConfig());
}

/** A live registry read, e.g. for the builder's "already your baseline" hint. */
export async function readRegistryValue(
  hive: Hive,
  subkey: string,
  valueName: string,
): Promise<RegistryValueView> {
  return unwrap(commands.readRegistryValue(hive, subkey, valueName));
}

// --- Custom scripts / cs2_config ---------------------------------------

export async function importCustomScript(
  projectId: string,
  sourcePath: string,
): Promise<string> {
  return unwrap(commands.importCustomScript(projectId, sourcePath));
}

// --- Run lifecycle -----------------------------------------------------

export async function startRun(
  projectId: string,
  dryRun: boolean,
  shutdownWhenComplete: boolean,
): Promise<string> {
  return unwrap(commands.startRun(projectId, dryRun, shutdownWhenComplete));
}

export async function resumeRun(): Promise<string> {
  return unwrap(commands.resumeRun());
}

export async function sendControl(msg: ControlMsg): Promise<void> {
  await unwrap(commands.sendControl(msg));
}

export async function setShutdownWhenComplete(value: boolean): Promise<void> {
  await unwrap(commands.setShutdownWhenComplete(value));
}

export async function getResults(runId: string): Promise<RunResults> {
  return unwrap(commands.getResults(runId));
}

export async function listResults(projectId: string): Promise<RunSummary[]> {
  return unwrap(commands.listResults(projectId));
}

export async function getRunSnapshot(): Promise<RunState | null> {
  return unwrap(commands.getRunSnapshot());
}

/** Whether THIS process launch was a `voidframe.exe --resume` invocation --
 * a synchronous, race-free fact known at process start (not tied to whether
 * the backend's own auto-resume has actually finished yet). */
export async function isResumeLaunch(): Promise<boolean> {
  return unwrap(commands.isResumeLaunch());
}

export async function getRunProgress(runId: string): Promise<RunProgress | null> {
  return unwrap(commands.getRunProgress(runId));
}

export async function cancelShutdown(): Promise<void> {
  await unwrap(commands.cancelShutdown());
}

export async function rollbackNow(): Promise<RevertReport> {
  return unwrap(commands.rollbackNow());
}

export async function emergencyRollback(): Promise<RevertReport[]> {
  return unwrap(commands.emergencyRollback());
}

export async function openDataDir(): Promise<void> {
  await unwrap(commands.openDataDir());
}

export async function revealRestoreBat(): Promise<void> {
  await unwrap(commands.revealRestoreBat());
}

// --- Engine events -----------------------------------------------------

/**
 * Subscribes to the hand-emitted `vf:event` channel (not specta-managed —
 * `bindings.ts` does not wrap this). Returns an unsubscribe function that is
 * safe to call even before `listen`'s own promise has resolved.
 */
export function subscribeToEngineEvents(
  handler: (event: EngineEvent) => void
): () => void {
  let unlisten: (() => void) | undefined;
  let cancelled = false;
  listen<EngineEvent>("vf:event", (e) => handler(e.payload))
    .then((fn) => {
      if (cancelled) {
        fn();
      } else {
        unlisten = fn;
      }
    })
    .catch((err) => {
      console.error("Failed to subscribe to vf:event:", err);
    });
  return () => {
    cancelled = true;
    unlisten?.();
  };
}

// --- Window chrome (no generated bindings) ------------------------------

export async function minimizeWindow(): Promise<void> {
  return invoke("minimize_window");
}

export async function toggleMaximizeWindow(): Promise<boolean> {
  return invoke("toggle_maximize_window");
}

export async function closeWindow(): Promise<void> {
  return invoke("close_window");
}

export async function isWindowMaximized(): Promise<boolean> {
  return invoke("is_window_maximized");
}

// --- App metadata (built-in Tauri core API, no custom command) ---------

/** The app version Tauri reads from `tauri.conf.json` (itself pointed at
 * `package.json`) -- the single source of truth for what the title bar
 * displays, so it can never again drift the way the old hardcoded
 * `v0.1.0-RC` string did. */
export async function getAppVersion(): Promise<string> {
  return getVersion();
}
