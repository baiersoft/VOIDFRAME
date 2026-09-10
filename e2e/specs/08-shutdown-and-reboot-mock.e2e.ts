import { $, browser, expect } from "@wdio/globals";
import { createProjectViaUI, deleteProjectsByNamePrefix, expectBodyContains, projectCard } from "../helpers";

// Proves the two "dangerous-looking" features are fully E2E-testable against
// the mock backend with zero real side effects:
//   - a shutdown-when-complete run finishes with `MockController::shutdown`
//     being a counted no-op (verified by its own
//     `reboot_and_shutdown_are_counted_and_never_executed` test in
//     system/mock.rs -- the machine never actually shuts down);
//   - a reboot-requiring scenario parks the UI in `reboot_pending` with
//     controls hidden (mock `reboot` is a no-op too, so no real reboot
//     fires and this process -- and the suite -- stays alive).
//
// `--resume`/`boot_resume` investigation (Task 10, step 3): read
// `e2e/node_modules/@wdio/tauri-service`'s embedded-driver source
// (dist/cjs/index.js). `wdio:tauriServiceOptions.appArgs` (and the
// capability-level `tauri:options.args`, which the service only validates/
// logs -- `appArgs` is what actually reaches `spawnTauriApp`) DOES let you
// pass launch args to the app, but only at initial session creation: the
// capabilities (and therefore appArgs) are fixed in `wdio.conf.ts` for the
// whole worker's session, and the embedded provider has no supported way to
// relaunch the same session mid-suite with different args. Forcing this
// (a manual `child_process` spawn of a second app instance, or
// `browser.reloadSession()` with mutated capabilities) would (a) be exactly
// the fragile relaunch harness the plan says not to build, and (b) collide
// with `AppState::with_data_root`'s single-instance lock
// (`state/instance.lock`) against the very instance this spec's second test
// deliberately leaves parked in `RebootPending`. So there is no third test
// here for `--resume`/`boot_resume` -- coverage for that path is the
// engine's own `run/execute/tests/reboot.rs` resume tests plus
// `LiveMonitor.test.tsx`'s existing "renders a settling heading for
// boot_resume and a live countdown from the Post-boot settle log line" test
// (already covers `PhaseChanged { kind: "boot_resume" }` and the
// `Post-boot settle: {n}s remaining` LogLine rendering -- Task 8 already
// wrote this, so no new component test is added here either).

const PREFIX = "E2E Shutdown Mock";

describe("Shutdown-when-complete and reboot_pending — mock backend, zero real side effects", () => {
  after(async () => {
    await deleteProjectsByNamePrefix(PREFIX);
  });

  it("completes a shutdown-when-complete run without any real shutdown firing", async function () {
    this.timeout(180_000);

    const name = `${PREFIX} ${Date.now()}`;
    await createProjectViaUI(name);
    await (await $(`h3=${name}`)).waitForDisplayed();
    await (await projectCard(name).$("button=Edit Matrix")).click();
    await (await $(`h2=${name}`)).waitForDisplayed();

    await $("button=Add Scenario").click();
    await (await $("span=Scenario 1")).waitForDisplayed();
    await $("button=Browse Catalog").click();
    await (await $("h4=Exclude Core 0")).waitForDisplayed();
    const entryRow = () => $('//h4[text()="Exclude Core 0"]/ancestor::div[contains(@class,"glass-card")][1]');
    await (await entryRow().$("button=Add Module")).click();
    await (
      await $('//h2[starts-with(text(),"Add Module")]/ancestor::div[contains(@class,"border-b")][1]//button')
    ).click();
    await expectBodyContains("Configured Modules (1)");

    await $("button=Launch Autonomous Pipeline").click();
    await (await $("h2=Starting Run")).waitForDisplayed();
    await $("h2=Starting Run").waitForDisplayed({ reverse: true, timeout: 15_000 });
    await expectBodyContains("Autonomous Pipeline Active");

    // Enable shutdown-on-complete DURING the baseline (the pinned
    // VOIDFRAME_MOCK_THERMAL window) -- doubles as a second regression
    // check on the optimistic checkbox.
    await expectBodyContains("Collecting thermal baseline");
    const checkbox = await $('input[type="checkbox"]');
    await checkbox.click();
    await browser.waitUntil(async () => checkbox.isSelected(), { timeout: 2_000 });

    // The mock run then completes; MockController::shutdown is a counted
    // no-op, so "Run Complete" appearing (and this suite still running at
    // all) IS the no-real-shutdown assertion.
    await browser.waitUntil(
      async () => (await (await $("body")).getText()).includes("Run Complete"),
      { timeout: 150_000, interval: 1_000, timeoutMsg: "mock run did not complete" }
    );

    // The Results view's "Cancel shutdown" button goes through
    // `state.sys` -- the REAL Windows controller, even in mock mode (a real
    // `shutdown /a`; harmless with nothing pending, but never to be relied
    // on or exercised here). Navigating to View Results is plain, read-only
    // navigation (getResults over IPC) -- assert the button's *presence*
    // only, never click it.
    await $("button=View Results").click();
    await expectBodyContains("Cancel shutdown");
    await expect($("button=Cancel shutdown")).toBeDisplayed();

    await $("button=Project Explorer").click();
  });

  it("shows reboot_pending with controls hidden when a reboot scenario applies (mock: no real reboot)", async function () {
    this.timeout(120_000);

    const name = `${PREFIX} Reboot ${Date.now()}`;
    await createProjectViaUI(name);
    await (await $(`h3=${name}`)).waitForDisplayed();
    await (await projectCard(name).$("button=Edit Matrix")).click();
    await (await $(`h2=${name}`)).waitForDisplayed();

    await $("button=Add Scenario").click();
    await (await $("span=Scenario 1")).waitForDisplayed();
    await $("button=Browse Catalog").click();
    await (await $("h4=Hardware-Accelerated GPU Scheduling (HAGS)")).waitForDisplayed();
    const entryRow = () =>
      $(
        '//h4[text()="Hardware-Accelerated GPU Scheduling (HAGS)"]/ancestor::div[contains(@class,"glass-card")][1]'
      );
    await (await entryRow().$("button=Add Module")).click();
    await (
      await $('//h2[starts-with(text(),"Add Module")]/ancestor::div[contains(@class,"border-b")][1]//button')
    ).click();
    await expectBodyContains("Configured Modules (1)");

    await $("button=Launch Autonomous Pipeline").click();
    await (await $("h2=Starting Run")).waitForDisplayed();
    await $("h2=Starting Run").waitForDisplayed({ reverse: true, timeout: 15_000 });
    await expectBodyContains("Autonomous Pipeline Active");

    // The engine reaches the HAGS scenario's apply, calls the mock reboot
    // (a no-op), and the run parks in reboot_pending. Depending on scenario
    // ordering the run may pass the baseline scenario first; the generous
    // timeout covers it. `rebootHeading` renders "Rebooting to apply the
    // next scenario…" / "Rebooting to revert…" -- "Rebooting to" matches
    // all variants.
    await expectBodyContains("Rebooting to", 150_000);

    // Task 8's gating: no controls once past the point of no return.
    await (await $("button=Abort & Roll Back")).waitForExist({ reverse: true, timeout: 5_000 });
    await (await $("button=Pause")).waitForExist({ reverse: true, timeout: 5_000 });

    // Deliberately no cleanup here: this leaves the store's RunState parked
    // at RebootPending forever (nothing reboots, nothing resumes), which
    // blocks any later start_run against this shared DATA_ROOT with "a run
    // is waiting to resume". This is why this test is LAST in this file and
    // this file is LAST in the suite -- onComplete deletes the whole
    // DATA_ROOT afterward. If a later task ever adds specs after this one,
    // they must first clear the state (Emergency Restore through the UI, or
    // a fresh data root).
  });
});
