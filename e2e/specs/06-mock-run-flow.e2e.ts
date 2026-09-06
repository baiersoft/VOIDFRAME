import { $, browser, expect } from "@wdio/globals";
import { createProjectViaUI, deleteProjectsByNamePrefix, expectBodyContains, projectCard } from "../helpers";

const PREFIX = "E2E Mock Run";

// Exercises the REAL Run Matrix -> Countdown -> Live Monitor -> Results
// flow -- real event stream, real UI state transitions -- against the
// synthetic backend `VOIDFRAME_SIMULATE_RUN` activates (wdio.conf.ts's
// capability env, only meaningful because the E2E binary is built with
// `--features webdriver-testing,mock-run`). No real CS2/Steam required, no
// real registry/power-plan/affinity mutation happens (voidframe_engine::
// mock_harness's MockController/MockCaptureRunner). Still real time, not
// instant: each iteration pays the real, capture-independent 8s settle
// delay in run/execute/scenario.rs (deliberately left alone rather than
// special-cased for testing -- see wdio.conf.ts's own note) -- with this
// project's default 2 warmup + 3 measure loops across baseline + 1
// scenario, that's (1+1)*(2+3) = 10 iterations, ~80s, not the real
// pipeline's ~20 minutes.
describe("Mock run flow — Matrix Builder → Live Monitor → Results (synthetic backend, no CS2)", () => {
  after(async () => {
    await deleteProjectsByNamePrefix(PREFIX);
  });

  it("runs a simulated single-scenario benchmark end to end", async function () {
    this.timeout(5 * 60 * 1000);

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

    // --- Launch, countdown, and hand-off to Live Monitor. No confirm
    // button exists by design (RunCountdownModal.tsx) -- it auto-starts
    // after a real 5-second countdown in a non-test build. ---
    await $("button=Launch Autonomous Pipeline").click();
    await (await $("h2=Starting Run")).waitForDisplayed();
    await $("h2=Starting Run").waitForDisplayed({ reverse: true, timeout: 15_000 });
    await expectBodyContains("Autonomous Pipeline Active");

    await browser.waitUntil(
      async () => {
        const text = await (await $("body")).getText();
        return text.includes("Run Complete") || text.includes("Run Failed");
      },
      { timeout: 4 * 60 * 1000, interval: 2_000, timeoutMsg: "mock run never reached a terminal state" }
    );

    const failedBanner = await $("text=Run Failed");
    if (await failedBanner.isExisting()) {
      const reason = await (await $(".text-white\\/80")).getText().catch(() => "(no reason text found)");
      throw new Error(`Mock run ended in RunFailed -- reason: ${reason}`);
    }

    await $("button=View Results").click();
    await (await $("button=Percentiles (FPS)")).waitForDisplayed({ timeout: 10_000 });

    // Baseline and "Scenario 1" (this test's own, auto-named scenario --
    // see MatrixBuilder's handleAddScenario) get identical synthetic
    // metrics from MockCaptureRunner (a single queued Metrics value,
    // reused for every call) -- deterministically "no measurable
    // difference" between them.
    await expectBodyContains("Scenario 1");
    await expectBodyContains("CONFIRMED SAME");

    for (const tab of ["Frame-Time Timeline", "Pacing & Stability", "WCPS v3 Ranking", "Percentiles (FPS)"]) {
      await $(`button=${tab}`).click();
      await expect($(`button=${tab}`)).toBeDisplayed();
    }

    await $("button=Project Explorer").click();
  });
});
