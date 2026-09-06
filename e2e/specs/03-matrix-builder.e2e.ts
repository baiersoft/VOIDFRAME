import { $, expect } from "@wdio/globals";
import { createProjectViaUI, deleteProjectsByNamePrefix, expectBodyContains, projectCard } from "../helpers";

const PREFIX = "E2E Builder";

describe("Matrix Builder — scenarios and modules (real IPC, no system mutation)", () => {
  after(async () => {
    await deleteProjectsByNamePrefix(PREFIX);
  });

  it("adds a scenario, toggles it, adds a real catalog module, then removes both", async () => {
    const name = `${PREFIX} ${Date.now()}`;
    await createProjectViaUI(name);
    await (await $(`h3=${name}`)).waitForDisplayed();
    await (await projectCard(name).$("button=Edit Matrix")).click();
    await (await $(`h2=${name}`)).waitForDisplayed();

    // A fresh project has zero scenarios (NewProjectModal saves
    // `scenarios: []`) -- "Add Scenario" names the first one "Scenario 1"
    // deterministically and auto-selects it.
    await $("button=Add Scenario").click();
    await (await $("span=Scenario 1")).waitForDisplayed();

    // XPath ancestor lookups (re-queried fresh, not chained
    // `.parentElement()` handles) throughout this test -- a chained handle
    // can go stale between commands if React re-renders the row in
    // between, which is exactly what broke this test intermittently.
    // The scenario row's own onClick selects it; the checkbox stops that
    // propagation to toggle `enabled` independently.
    const scenarioRow = () =>
      $('//span[text()="Scenario 1"]/ancestor::div[contains(@class,"cursor-pointer") and .//input[@type="checkbox"]][1]');
    const enabledToggle = await scenarioRow().$('input[type="checkbox"]');
    await expect(enabledToggle).toBeSelected();
    await enabledToggle.click();
    await expect(enabledToggle).not.toBeSelected();
    await enabledToggle.click();
    await expect(enabledToggle).toBeSelected();

    // "Configured Modules ({count})" mixes literal text with an
    // interpolated count -- see expectBodyContains's doc comment.
    await expectBodyContains("Configured Modules (0)");
    await $("button=Browse Catalog").click();

    // "Exclude Core 0" (affinity_cpu) -- chosen deliberately over the
    // other simple catalog entries because it already has solid existing
    // test coverage elsewhere and is least likely to be renamed/removed.
    await (await $("h4=Exclude Core 0")).waitForDisplayed();
    const entryRow = () => $('//h4[text()="Exclude Core 0"]/ancestor::div[contains(@class,"glass-card")][1]');
    await (await entryRow().$("button=Add Module")).click();
    await expect(entryRow().$("button=Added")).toBeDisplayed();

    // Modal-only close X (isModal=true here, unlike the library full page)
    // -- it's the lone button in the header row next to the "Add Module →
    // <scenario>" heading.
    await (
      await $('//h2[starts-with(text(),"Add Module")]/ancestor::div[contains(@class,"border-b")][1]//button')
    ).click();

    await expectBodyContains("Configured Modules (1)");
    // Confirmed via direct diagnostic: body text genuinely contains
    // "affinity_cpu" (lowercase, matching the raw JSX value) even though
    // the badge span has `uppercase` CSS applied -- but the single-element
    // `$('text=affinity_cpu')` selector fails to find it regardless of
    // case tried. This embedded driver's per-element `text=` matching is
    // unreliable in ways beyond just multi-text-node splitting (see
    // expectBodyContains's doc comment); standardizing on body-text checks
    // for plain presence assertions throughout this file.
    await expectBodyContains("affinity_cpu");

    await (await $('button[title="Remove Module"]')).click();
    await expectBodyContains("Configured Modules (0)");

    await (await $('button[title="Delete Scenario"]')).click();
    await expectBodyContains("Select or create a scenario to begin configuring the matrix.");

    await $("button=Project Explorer").click();
  });
});
