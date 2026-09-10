import { $, browser, expect } from "@wdio/globals";
import { createProjectViaUI, deleteProjectsByNamePrefix, expectBodyContains, projectCard } from "../helpers";

const PREFIX = "E2E Abort Mock Run";

// Exercises the real Abort control end to end against the synthetic
// mock-run backend (same VOIDFRAME_SIMULATE_RUN setup as
// 06-mock-run-flow.e2e.ts) -- real IPC (`send_control`), real engine
// control-flow (the `tokio::sync::watch` abort signal `spawn_run`'s tee
// task sets, which `run/execute/mod.rs` races the *whole* run body against
// in one `select!` -- the body future is dropped wherever it happens to be
// and cleanup runs from persisted state), real `RunFailed` event.
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

  it("acknowledges the shutdown toggle instantly and aborts instantly during the thermal baseline", async function () {
    this.timeout(90_000);

    const name = `${PREFIX} Baseline ${Date.now()}`;
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

    // The pinned mock window (VOIDFRAME_MOCK_THERMAL=250,40) holds the run
    // here for ~10s -- the exact window the original bug made unresponsive.
    await expectBodyContains("Collecting thermal baseline");
    await expectBodyContains("sample ", 10_000);

    // Regression: the checkbox used to stay unchecked until after the
    // baseline (progress.json doesn't exist yet). It must reflect the click
    // immediately now.
    const checkbox = await $('input[type="checkbox"]');
    await checkbox.click();
    await browser.waitUntil(async () => checkbox.isSelected(), {
      timeout: 2_000,
      timeoutMsg: "shutdown checkbox did not reflect the click immediately",
    });

    // Regression: Abort used to queue for the whole 30s baseline. It must
    // now cut the wait within one poll and show the Aborting state at once.
    await (await $("button=Abort & Roll Back")).click();
    await expectBodyContains("Aborting", 3_000);
    // Controls are gone during Aborting (phase gating).
    await (await $("button=Pause")).waitForExist({ reverse: true, timeout: 5_000 });

    await browser.waitUntil(
      async () => (await (await $("body")).getText()).includes("Run Failed"),
      { timeout: 20_000, interval: 500, timeoutMsg: "abort during the baseline did not end the run promptly" }
    );
  });
});
