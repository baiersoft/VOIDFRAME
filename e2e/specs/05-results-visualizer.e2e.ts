import { $, $$, expect } from "@wdio/globals";
import { expectBodyContains, projectCard } from "../helpers";

// Uses the seeded "WCPS v3 Matrix Study" fixture (wdio.conf.ts's
// seedFixtures, from e2e/fixtures/wcps-v3-study/) -- a real project with a
// real, current-schema run result already on disk when the app launches.
// This is the only way to exercise the real Visualizer & Charts view with
// rich, multi-scenario, multi-verdict data without ever running (or
// waiting out) an actual benchmark: `dry_run` in this codebase only
// no-ops registry/power-plan/affinity writes -- it does NOT skip the real
// CS2 launch or shorten the real warmup/capture loops, so it would not
// make a real run any faster.
const PROJECT_NAME = "WCPS v3 Matrix Study";

describe("Results Visualizer — real seeded run data, no benchmark required", () => {
  it("opens straight to results from the dashboard's run-count pill", async () => {
    const card = projectCard(PROJECT_NAME);
    await (await card.$("h3=" + PROJECT_NAME)).waitForDisplayed({ timeout: 10_000 });

    // Only rendered when `resultSummaries[project.id]?.length > 0` --
    // clicking it is itself a real list_results -> get_results round trip.
    await (await card.$("button*=run")).click();

    await (await $("button=Percentiles (FPS)")).waitForDisplayed({ timeout: 10_000 });
  });

  it("shows every scenario with its real verdict and WCPS score", async () => {
    // From the fixture's real data (e2e/fixtures/wcps-v3-study/results.json):
    // a mix of all three verdicts actually present in VERDICT_STYLES.
    await expectBodyContains("Control 1 (No-Op)");
    await expectBodyContains("Power Saver #1");
    await expectBodyContains("Exclude Core 0 #1");
    await expectBodyContains("-noreflex #1");
    await expectBodyContains("-vulkan #1");

    await expectBodyContains("BETTER");
    await expectBodyContains("WORSE");
    await expectBodyContains("CONFIRMED SAME");
  });

  it("switches between all four chart tabs without error", async () => {
    for (const tab of [
      "Frame-Time Timeline",
      "Pacing & Stability",
      "WCPS v3 Ranking",
      "Percentiles (FPS)",
    ]) {
      await $(`button=${tab}`).click();
      await expect($(`button=${tab}`)).toBeDisplayed();
    }
  });

  it("expands a scenario row to show its telemetry drawer", async () => {
    const inspectButtons = await $$("button=Inspect");
    await expect(inspectButtons[0]).toBeDisplayed();
    await inspectButtons[0].click();
    await expect($("button=Hide")).toBeDisplayed();
    await $("button=Hide").click();
  });

  it("copies the markdown table", async () => {
    await $("button=Markdown Table").click();
    await expectBodyContains("Copied Markdown");
  });
});
