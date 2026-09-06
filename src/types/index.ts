/**
 * MIGRATION NOTE (Phase 4b-1): these types were built as an aspirational UI
 * mockup covering both M1 and later-milestone (M3) features. The real
 * backend (src-tauri) only implements M1's subset -- see spec §13 for what's
 * explicitly out of scope. As each later Phase 4b plan wires a component to
 * real data, prefer importing the real generated types from `src/lib/api`
 * instead of the ones below wherever a real equivalent exists.
 *
 * NAMING COLLISION WARNING: this file's own `ScenarioResult` shares its name
 * with the real generated `ScenarioResult` in `src/lib/bindings.ts` /
 * `src/lib/api.ts` but is a structurally different shape (see below). Any
 * component migrating away from this file's `ScenarioResult` MUST import the
 * real one under an explicit alias (e.g. `import type { ScenarioResult as
 * RealScenarioResult } from "../lib/api"`) rather than assume the two are
 * interchangeable just because the name matches.
 *
 * Superseded by real generated types (import from `src/lib/api` instead once
 * a component migrates):
 * - BenchmarkProject -> Project (id/name/description/createdAt ->
 *   created_at); BenchmarkProject.baseline -> Baseline (shape matches as-is).
 * - BenchmarkScenario -> Scenario (tweaks -> modules; id/name/description/
 *   enabled are all real fields, just camelCase/snake_case-shifted).
 * - TweakItem's registry/powercfg/affinity_cpu/launch_args-shaped fields ->
 *   Module's real tagged-union variants: keyPath -> subkey, valueName ->
 *   value_name, valueType -> value_type, valueData -> value (RegistryPayload);
 *   powerGuid -> PowerPlanPayload.plan_guid on the real `power_plan` variant
 *   (note: NOT `powercfg` -- the real `powercfg` variant is sub/setting/value
 *   instead, a different concept than this file's powerGuid field);
 *   affinityMask -> AffinityCpuPayload.mask_hex, though the real payload also
 *   requires a `mode: AffinityMode` field this file never had; launchFlags
 *   (a single string) -> Module's launch_args.args (also a single string, a
 *   free-text field the user edits directly -- no validation, no allowlist).
 *   TweakItem's id/name/category/description loosely
 *   correspond to CatalogEntry's identically-named fields (CatalogEntry.kind
 *   is the closer analog of TweakItem.type; CatalogEntry.available_in already
 *   carries this project's own milestone-gating string, the same shape
 *   `<MilestoneGate available>` consumes).
 * - ProjectSettings.warmupLoops/measureLoops/targetMapId/
 *   captureDurationSeconds/watchdogTimeoutSeconds/netconPort ->
 *   Settings.warmup_loops/measure_loops/map_id/capture_seconds/
 *   watchdog_seconds/netcon_port.
 * - BenchmarkRunMetrics.avgFps/medianFps/p1Fps/p01Fps/gpuBusyMs/
 *   bottleneckRatio -> Metrics's identically-purposed fields
 *   (frameTimeMs -> frame_time_mean_ms, frameJitterMs -> frame_time_stddev_ms
 *   / frame_time_cv -- renamed, not identical).
 * - ScenarioResult(old).scenarioId/scenarioName/isBaseline/runs -> the real
 *   ScenarioResult.scenario_id/name/is_baseline/per_iteration (see naming
 *   collision warning above).
 * - ProjectResults -> RunResults (projectId -> project_id, baselineResult ->
 *   baseline, scenarioResults -> scenarios).
 *
 * M3-only, no real backend in M1 (stays local/unmapped -- do not try to wire
 * these to real commands, there is nothing on the other end):
 * - TweakCategory's driver_gpu/npi_profile/custom_script variants, TweakType's
 *   affinity_gpu/driver_install/npi_profile/cs2_config/custom_script variants
 *   (the real Module enum only has registry/powercfg/power_plan/affinity_cpu/
 *   launch_args, plus a catch-all `Unsupported`).
 * - TweakItem's videoSettings; scriptPath/scriptArgs/runElevated/
 *   revertScriptPath; driverPackagePath/cleanInstall/dduDeepClean/
 *   nipProfilePath. TweakItem.requiresReboot/msdnUrl/details also have no
 *   equivalent field on the real CatalogEntry at all.
 * - TweakPack entirely (schema_version/author/prerequisites/containerFormat --
 *   no generated equivalent; the real model's "container" concept is just
 *   Project + Scenario + Module, not a separately-authored pack file).
 * - BenchmarkScenario.requiresShaderWarmup/dduDeepCleanEnabled.
 * - ProjectSettings.enableThermalGuard/maxIdleTempCelsius/
 *   cooldownTimeoutSeconds.
 * - BenchmarkRunMetrics.cpuTempCelsius/gpuTempCelsius; FrameDataPoint
 *   entirely (no per-frame stream exists in the real EngineEvent surface,
 *   only aggregate Metrics delivered via IterationComplete).
 * - ProcessInterferenceSpike, DiagnosticResult entirely (background
 *   interference detection is spec §13 out-of-scope entirely).
 *
 * Partially superseded (some fields map to real data, others don't):
 * - BenchmarkType -- no real enum equivalent at all; the real
 *   Settings.map_id is a plain unconstrained string, not a closed set of
 *   known map ids.
 * - TweakCategory's gpu/cpu/kernel/cs2_video/cs2_launch values loosely
 *   correspond to the real CatalogEntry.category, but that field is an
 *   unconstrained string server-side too, not this enum.
 * - BenchmarkProject.lastRunAt / .resultsSummary -- no field on the real
 *   Project; per-run results live in RunResults, fetched separately via
 *   get_results, not inlined on the project object.
 * - BenchmarkProject.status -- no field on Project; run status instead lives
 *   in RunState.phase (a plain string, not this closed union) and the
 *   EngineEvent stream, not on the project object itself.
 * - ProjectSettings.benchmarkType / .autoShutdownOnComplete -- no real
 *   Settings field.
 * - BenchmarkRunMetrics.runIndex/isWarmup/dataPoints -- no real per-iteration
 *   index/warmup-flag/frame-array field; the real per_iteration: Metrics[]
 *   array's position is the only "index" available, and there is no
 *   per-frame data at all.
 * - ScenarioResult(old).deltaVsBaseline -- the real comparisons:
 *   MetricComparison[] covers the same intent (delta_pct/ci_low_pct/
 *   ci_high_pct/verdict per metric) but as an array of per-metric objects
 *   with confidence intervals, not a flat object of named percent fields;
 *   wcpsDeltaPercent has no guaranteed corresponding MetricComparison entry.
 * - ProjectResults.projectName -- no field on RunResults (only project_id).
 * REMOVED (Phase 4b-3, Task 3): this file used to define local `ExecutionPhase`,
 * `LiveExecutionState`, and `LogEntry` types here, plus migration notes for
 * them (ExecutionPhase's closed-union phase names vs. the real RunState.phase
 * / EngineEvent's PhaseChanged.phase, both plain `string`; LogEntry.message
 * vs. EngineEvent's LogLine.text with no structured level/timestamp/id;
 * LiveExecutionState's fields vs. state reconstructed client-side from the
 * EngineEvent stream (PhaseChanged/IterationStarted/CapturePending/
 * CaptureResumed/IterationComplete/ScenarioComplete/OperatorPrompt/
 * RunComplete/RunFailed/RollbackProgress) plus RunState). Task 3 wired
 * LiveMonitor directly to those real types/events, so all three local types
 * and this commentary about them were deleted -- there is nothing left in
 * this file to migrate away from for these three.
 */
export type BenchmarkType = "workshop_dust2" | "aveyo_cfg" | "custom_demo";

export type TweakCategory =
  | "gpu"
  | "cpu"
  | "kernel"
  | "driver_gpu"
  | "npi_profile"
  | "cs2_video"
  | "cs2_launch"
  | "custom_script";

export type TweakType =
  | "registry"
  | "powercfg"
  | "affinity_gpu"
  | "affinity_cpu"
  | "driver_install"
  | "npi_profile"
  | "cs2_config"
  | "launch_args"
  | "custom_script";

export interface TweakItem {
  id: string;
  name: string;
  category: TweakCategory;
  type: TweakType;
  description: string;
  requiresReboot: boolean;
  msdnUrl?: string;
  details?: string;
  // Specific payload fields
  keyPath?: string;
  valueName?: string;
  valueType?: "REG_DWORD" | "REG_SZ" | "REG_QWORD" | "REG_BINARY";
  valueData?: string | number;
  powerGuid?: string;
  affinityMask?: string;
  videoSettings?: Record<string, string>;
  launchFlags?: string;
  // Custom Script (Expert Mode)
  scriptPath?: string;
  scriptArgs?: string[];
  runElevated?: boolean;
  revertScriptPath?: string;
  // Driver & NPI specific
  driverPackagePath?: string;
  cleanInstall?: boolean;
  dduDeepClean?: boolean;
  nipProfilePath?: string;
}

export interface TweakPack {
  schema_version: string;
  id: string;
  name: string;
  author: string;
  description: string;
  prerequisites?: {
    os_build_min?: number;
    gpu_vendor?: "NVIDIA" | "AMD" | "Intel" | "ANY";
  };
  modules: TweakItem[];
  containerFormat?: "vfp" | "json";
}

export interface BenchmarkScenario {
  id: string;
  name: string;
  description: string;
  tweaks: TweakItem[];
  enabled: boolean;
  rebootRequired: boolean;
  requiresShaderWarmup?: boolean;
  dduDeepCleanEnabled?: boolean;
}

export interface ProjectSettings {
  warmupLoops: number;
  measureLoops: number;
  benchmarkType: BenchmarkType;
  targetMapId: string;
  captureDurationSeconds: number;
  watchdogTimeoutSeconds: number;
  netconPort: number;
  autoShutdownOnComplete: boolean;
  // Thermal Guard Settings
  enableThermalGuard: boolean;
  maxIdleTempCelsius: number;
  cooldownTimeoutSeconds: number;
}

export interface BenchmarkProject {
  id: string;
  name: string;
  description: string;
  createdAt: string;
  lastRunAt?: string;
  settings: ProjectSettings;
  baseline: {
    name: string;
    description: string;
  };
  scenarios: BenchmarkScenario[];
  status: "idle" | "running" | "completed" | "error" | "reboot_pending";
  resultsSummary?: {
    totalRuns: number;
    bestScenarioName: string;
    maxP1GainPercent: number;
    bestWcpsScore: number;
  };
}

export interface FrameDataPoint {
  timeMs: number;
  frameTimeMs: number;
  gpuBusyMs: number;
}

export interface BenchmarkRunMetrics {
  runIndex: number;
  isWarmup: boolean;
  avgFps: number;
  medianFps: number;
  p1Fps: number;
  p01Fps: number;
  gpuBusyMs: number;
  frameTimeMs: number;
  frameJitterMs: number;
  bottleneckRatio: number; // 0..1, higher = GPU bound, lower = CPU bound
  wcpsScore: number; // Weighted CS2 Performance Score
  cpuTempCelsius: number;
  gpuTempCelsius: number;
  dataPoints: FrameDataPoint[];
}

export interface ScenarioResult {
  scenarioId: string;
  scenarioName: string;
  isBaseline: boolean;
  aggregated: {
    avgFps: number;
    medianFps: number;
    p1Fps: number;
    p01Fps: number;
    gpuBusyMs: number;
    frameTimeMs: number;
    frameJitterMs: number;
    bottleneckRatio: number;
    wcpsScore: number;
    avgCpuTemp: number;
    avgGpuTemp: number;
  };
  deltaVsBaseline?: {
    avgFpsPercent: number;
    p1FpsPercent: number;
    p01FpsPercent: number;
    jitterPercent: number;
    gpuBusyDeltaMs: number;
    wcpsDeltaPercent: number;
  };
  runs: BenchmarkRunMetrics[];
}

export interface ProjectResults {
  projectId: string;
  projectName: string;
  completedAt: string;
  baselineResult: ScenarioResult;
  scenarioResults: ScenarioResult[];
}

// Background App Interference Diagnostics
export interface ProcessInterferenceSpike {
  processName: string;
  pid: number;
  cpuUsagePercent: number;
  gpuUsagePercent: number;
  timestampSec: number;
  frameTimeSpikeMs: number;
  severity: "LOW" | "MEDIUM" | "HIGH";
  recommendation: string;
}

export interface DiagnosticResult {
  runAt: string;
  avgFps: number;
  p1Fps: number;
  maxFrameSpikeMs: number;
  offendingProcesses: ProcessInterferenceSpike[];
}
