import { $, browser, expect } from "@wdio/globals";
import { createProjectViaUI, deleteProjectsByNamePrefix, expectBodyContains, projectCard } from "../helpers";

const PREFIX = "E2E Run";

// REAL, SYSTEM-MUTATING TEST. Applies real registry/affinity mutations
// (via the "Exclude Core 0" module) and launches a real CS2 session for a
// real measured benchmark, then auto-rolls the baseline back at the end --
// this is the app's actual core function, exercised end-to-end exactly as
// a user would trigger it. Requires:
//   - UAC disabled machine-wide (the app's `requireAdministrator` manifest
//     must not block a non-interactive elevation prompt), and
//   - CS2 + Steam actually installed, since Preflight/the real run
//     pipeline depend on them.
// This is deliberately its own spec file so it can be run in isolation
// (`npx wdio run wdio.conf.ts --spec specs/04-full-run-flow.e2e.ts`)
// rather than always bundled with the fast, non-mutating specs.
describe("Full run flow — Matrix Builder → Run Countdown → Live Monitor → Results (REAL mutation)", () => {
  after(async () => {
    await deleteProjectsByNamePrefix(PREFIX);
  });

  it("runs a real single-scenario benchmark end to end", async function () {
    // MatrixBuilder's own estimate for 1 enabled scenario at the default
    // 2 warmup / 3 measure loops, 105s capture, is ~20 minutes
    // ((1+1)*2 warmup + (1+1)*3 measure runs, each capture+15s) --
    // give it real headroom rather than the suite's default timeout.
    this.timeout(45 * 60 * 1000);

    const name = `${PREFIX} ${Date.now()}`;
    await createProjectViaUI(name);
    await (await $(`h3=${name}`)).waitForDisplayed();
    await (await projectCard(name).$("button=Edit Matrix")).click();
    await (await $(`h2=${name}`)).waitForDisplayed();

    await $("button=Add Scenario").click();
    await (await $("span=Scenario 1")).waitForDisplayed();
    await $("button=Browse Catalog").click();
    await (await $("h4=Exclude Core 0")).waitForDisplayed();
    // XPath ancestor lookups, re-queried fresh rather than chained
    // `.parentElement()` handles -- see spec 03's identical note on why.
    const entryRow = () => $('//h4[text()="Exclude Core 0"]/ancestor::div[contains(@class,"glass-card")][1]');
    await (await entryRow().$("button=Add Module")).click();
    await (
      await $('//h2[starts-with(text(),"Add Module")]/ancestor::div[contains(@class,"border-b")][1]//button')
    ).click();
    // "Configured Modules ({count})" mixes literal text with an
    // interpolated count -- see expectBodyContains's doc comment for why
    // this needs a body-text check rather than `$('text=...')`.
    await expectBodyContains("Configured Modules (1)");

    // --- Launch, countdown, and hand-off to Live Monitor ---
    await $("button=Launch Autonomous Pipeline").click();
    await (await $("h2=Starting Run")).waitForDisplayed();

    // No confirm button exists by design (see RunCountdownModal.tsx) --
    // it auto-starts after a REAL 5-second countdown in a non-test build.
    // Wait it out rather than looking for something to click.
    await $("h2=Starting Run").waitForDisplayed({ reverse: true, timeout: 15_000 });

    await (await $("text=Autonomous Pipeline Active")).waitForDisplayed({ timeout: 30_000 });

    // --- Poll for the terminal state, defensively acknowledging any
    // operator prompt along the way (its exact trigger conditions --
    // e.g. a Steam/AutoLogon precondition -- aren't something this test
    // controls, so treat "Acknowledge" appearing as always safe to click
    // rather than trying to predict when it happens). ---
    let sawOperatorPrompt = false;
    await browser.waitUntil(
      async () => {
        const ack = await $("button=Acknowledge");
        if (await ack.isExisting()) {
          sawOperatorPrompt = true;
          await ack.click();
        }
        const complete = await $("text=Run Complete");
        const failed = await $("text=Run Failed");
        return (await complete.isExisting()) || (await failed.isExisting());
      },
      { timeout: 40 * 60 * 1000, interval: 5_000, timeoutMsg: "run never reached a terminal state" }
    );
    void sawOperatorPrompt;

    const failedBanner = await $("text=Run Failed");
    if (await failedBanner.isExisting()) {
      const reason = await (await $(".text-white\\/80")).getText().catch(() => "(no reason text found)");
      throw new Error(`Real run ended in RunFailed -- reason: ${reason}`);
    }

    await expect($("text=Run Complete")).toBeDisplayed();

    // --- Click through to the real UI results view. "View Results" only
    // renders when `terminal.kind==="complete" && runId && onViewResults`
    // all held (LiveMonitor.tsx) and its handler does a real
    // `getResults(runId)` IPC round trip before ResultsVisualizer ever
    // renders -- reaching the chart tabs below already proves the real
    // backend produced real results data for this run. ---
    await $("button=View Results").click();
    await (await $("button=Percentiles (FPS)")).waitForDisplayed();
    for (const tab of ["Frame-Time Timeline", "Pacing & Stability", "WCPS v3 Ranking", "Percentiles (FPS)"]) {
      await $(`button=${tab}`).click();
    }

    await $("button=Project Explorer").click();
  });
});
