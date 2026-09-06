import path from "node:path";
import os from "node:os";
import fs from "node:fs";
import { execFileSync } from "node:child_process";

// A fresh directory per `wdio` invocation (not per-worker -- all 3 spec
// files in a run share one, matching a single real user session) so the
// suite never reads or mutates the operator's actual
// %LOCALAPPDATA%\baiersoft\VOIDFRAME (real projects, config, run history).
// Passed to the app via VOIDFRAME_DATA_ROOT (state.rs's AppState::new()).
const DATA_ROOT = fs.mkdtempSync(path.join(os.tmpdir(), "voidframe-e2e-"));

// Seeds the fresh data root with a real, current-schema project + its real
// run results (`fixtures/wcps-v3-study/`, copied from an actual PresentMon
// capture study, reprocessed under the current WCPS v3 scoring pipeline
// specifically for UI testing) -- `list_results`/`get_results_impl` only
// ever read `runs/<run_id>/results.json` off disk (confirmed by reading
// their source; no journal/CSV files needed to just view a result), so
// this is the whole seed. Lets 05-results-visualizer.e2e.ts exercise the
// real Visualizer & Charts view against rich, real data without ever
// running (or waiting out) a real benchmark.
function seedFixtures(dataRoot: string): void {
  const fixtureDir = path.join(__dirname, "fixtures", "wcps-v3-study");
  const project = JSON.parse(fs.readFileSync(path.join(fixtureDir, "project.json"), "utf8"));
  const results = JSON.parse(fs.readFileSync(path.join(fixtureDir, "results.json"), "utf8"));

  const projectsDir = path.join(dataRoot, "projects");
  fs.mkdirSync(projectsDir, { recursive: true });
  fs.copyFileSync(
    path.join(fixtureDir, "project.json"),
    path.join(projectsDir, `${project.id}.json`)
  );

  const runDir = path.join(dataRoot, "runs", results.run_id);
  fs.mkdirSync(runDir, { recursive: true });
  fs.copyFileSync(path.join(fixtureDir, "results.json"), path.join(runDir, "results.json"));
}
seedFixtures(DATA_ROOT);

// The workspace root's own target/ dir, NOT src-tauri/target/ -- this repo
// is a Cargo workspace (resolver 3), so `cargo`/`tauri build` place real
// output here. A stray, stale src-tauri/target/debug/voidframe.exe exists
// on disk from before workspace consolidation and looks like a valid
// binary (same name, launches, has a window) but lacks the
// requireAdministrator manifest.
//
// The binary here MUST be built with the `webdriver-testing` Cargo
// feature (`npx tauri build --debug --no-bundle --features
// webdriver-testing`), or `@wdio/tauri-service`'s embedded driver has
// nothing to attach to -- plain `tauri-driver`/msedgedriver against this
// app's WebView2 hangs and times out ("DevToolsActivePort file doesn't
// exist") because the app runs elevated (`requireAdministrator`), a
// currently-open upstream Tauri/WebView2 issue for elevated apps. The
// embedded provider sidesteps that failure mode entirely by running its
// own in-process WebDriver-compatible server (`tauri-plugin-wdio-webdriver`,
// gated in this app behind that Cargo feature + `WDIO_EMBEDDED_SERVER`,
// which @wdio/tauri-service sets automatically when it spawns the app --
// no extra env config needed here).
const APP_BINARY = path.resolve(__dirname, "..", "target", "debug", "voidframe.exe");

export const config: WebdriverIO.Config = {
  runner: "local",

  // Default run covers every non-mutating UI flow, including the mock-run
  // flow (06 -- real event stream, fake backend, no real mutations). Only
  // the REAL, system-mutating run-flow spec (04) stays opt-in via
  // `--suite full` or `--spec specs/04-full-run-flow.e2e.ts` -- it applies
  // real registry/affinity mutations and launches a real CS2 session, so
  // it must never run just because someone ran the default suite.
  specs: [
    "./specs/01-*.e2e.ts",
    "./specs/02-*.e2e.ts",
    "./specs/03-*.e2e.ts",
    "./specs/05-*.e2e.ts",
    "./specs/06-*.e2e.ts",
    "./specs/07-*.e2e.ts",
  ],
  suites: {
    safe: [
      "./specs/01-*.e2e.ts",
      "./specs/02-*.e2e.ts",
      "./specs/03-*.e2e.ts",
      "./specs/05-*.e2e.ts",
      "./specs/06-*.e2e.ts",
      "./specs/07-*.e2e.ts",
    ],
    full: ["./specs/**/*.e2e.ts"],
  },
  maxInstances: 1,

  services: [["@wdio/tauri-service", { driverProvider: "embedded" }]],

  capabilities: [
    {
      browserName: "tauri",
      "tauri:options": {
        application: APP_BINARY,
      },
      // `env` here is applied to the spawned app process itself in
      // embedded mode (@wdio/tauri-service's own `startEmbeddedDriver`
      // merges it into the child's environment alongside its own
      // WDIO_EMBEDDED_SERVER/TAURI_WEBDRIVER_PORT) -- confirmed by reading
      // its source, not documented.
      "wdio:tauriServiceOptions": {
        // VOIDFRAME_SIMULATE_RUN only does anything if the binary was
        // built with the `mock-run` Cargo feature (`npx tauri build
        // --debug --no-bundle --features webdriver-testing,mock-run`) --
        // harmless to always set otherwise. Swaps the real
        // SystemController/CaptureRunner for `voidframe_engine::mock_harness`'s
        // synthetic pair (src-tauri/src/commands/run.rs's `prepare_run`):
        // no real CS2 launch, no real PresentMon capture, so
        // 06-mock-run-flow.e2e.ts's real Run Matrix -> Countdown -> Live
        // Monitor -> Results flow completes in about a minute instead of
        // the real pipeline's ~20 (each iteration still pays the real,
        // capture-independent 8s settle delay in
        // run/execute/scenario.rs -- that's deliberately left alone rather
        // than special-cased for testing, so keep the test project's own
        // scenario/loop counts low).
        env: { VOIDFRAME_DATA_ROOT: DATA_ROOT, VOIDFRAME_SIMULATE_RUN: "1" },
      },
    } as WebdriverIO.Capabilities,
  ],

  logLevel: "warn",
  bail: 0,
  waitforTimeout: 15_000,

  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: {
    ui: "bdd",
    timeout: 180_000, // real registry/power-plan mutations + a CS2 launch in the run-flow spec are slow
  },

  // @wdio/tauri-service's `beforeCommand` hook runs a window-focus-recovery
  // check (`ensureActiveWindowFocus`) before every `getTitle`/`$`/`$$`/
  // `elementClick` command -- i.e. nearly every command a spec issues. In
  // this app that check's own internal probe never succeeds (its `catch`
  // only logs a warning, so it doesn't fail anything, but it still pays
  // the full 5-second internal poll timeout EVERY SINGLE TIME first) --
  // confirmed by direct testing that neither `withGlobalTauri` nor
  // registering `tauri_plugin_wdio` alongside `tauri_plugin_wdio_webdriver`
  // changes this. A single, real, native `switchToWindow` command makes
  // the service's own `afterCommand` hook mark this session as having
  // handled window focus explicitly (`suppressActiveWindowFocus`),
  // permanently skipping the check for the rest of the session -- this is
  // a single-window app, so there is never a real window to "lose focus"
  // of in the first place. Without this, a 4-test spec file took several
  // minutes; this is the actual fix for the reported slowness, not a
  // workaround for a bug in this app's own code.
  before: async () => {
    const handle = await browser.getWindowHandle();
    await browser.switchToWindow(handle);
  },

  // The embedded provider's own session teardown does not reliably kill
  // the spawned voidframe.exe before the next worker (next spec file)
  // starts -- observed directly: a later spec's very first, brand-new
  // session already had a project selected (Matrix Builder tab visible
  // with zero projects created in that session), meaning it was actually
  // still talking to the PREVIOUS worker's leftover process/window rather
  // than a genuinely fresh one. `AppState::with_data_root`'s single-
  // instance lock (`state/instance.lock`) means a second, truly-new
  // process launched while an old one is still alive would panic on
  // startup rather than silently share state -- so the old process itself
  // must still be alive across the worker boundary. Force-killing here
  // guarantees the next worker's "fresh" launch actually is one.
  onWorkerEnd: () => {
    try {
      execFileSync("taskkill", ["/IM", "voidframe.exe", "/F"], { stdio: "ignore" });
    } catch {
      // No matching process -- already exited cleanly, nothing to do.
    }
  },

  onComplete: () => {
    fs.rmSync(DATA_ROOT, { recursive: true, force: true });
  },
};
