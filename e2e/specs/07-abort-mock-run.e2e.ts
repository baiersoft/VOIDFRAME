import { $, browser, expect } from "@wdio/globals";
import { createProjectViaUI, deleteProjectsByNamePrefix, expectBodyContains, projectCard } from "../helpers";

const PREFIX = "E2E Abort Mock Run";

// Exercises the real Abort control end to end against the synthetic
// mock-run backend (same VOIDFRAME_SIMULATE_RUN setup as
// 06-mock-run-flow.e2e.ts) -- real IPC (`send_control`), real engine
// control-flow (the `tokio::sync::watch` abort signal `spawn_run`'s tee
// task sets, raced via `abortable()` against every CS2-readiness/capture/
// thermal wait in run/execute/{scenario,mod}.rs), real `RunFailed` event.
// The regression this guards: before that control-flow work, `ControlMsg::
// Abort` was only ever observed while already paused or during an operator
// prompt -- during normal execution (map-load wait, capture, thermal
// sampling) it was silently swallowed, so clicking Abort mid-run did
// nothing until the run happened to finish on its own.
describe("Abort — mock run flow (synthetic backend, no CS2)", () => {
  after(async () => {
    await deleteProjectsByNamePrefix(PREFIX);
  });

  it("stops the run promptly and reports it as aborted, not left to finish naturally", async function () {
    this.timeout(60_000);

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

    await (await $("button=Abort & Roll Back")).click();

    // The mock run's own baseline+scenario would otherwise take ~80s (see
    // 06-mock-run-flow.e2e.ts's own timing note) -- a short timeout here is
    // exactly the regression check: if Abort were still being swallowed
    // (the pre-fix behavior), this would time out rather than reaching
    // "Run Failed" quickly.
    await browser.waitUntil(
      async () => (await (await $("body")).getText()).includes("Run Failed"),
      { timeout: 15_000, interval: 500, timeoutMsg: "Abort did not stop the run promptly" }
    );

    const reason = await (await $(".text-white\\/80")).getText();
    expect(reason.toLowerCase()).toContain("aborted");

    await $("button=Project Explorer").click();
  });
});
