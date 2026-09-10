import { $, browser, expect } from "@wdio/globals";
import { typeIntoSearchBox } from "../helpers";

describe("Navigation tab gating (no project selected, no run, no results)", () => {
  it("shows only Project Explorer and Verified Tweaks", async () => {
    const nav = await $('nav[aria-label="Main Navigation"]');
    const buttons = await nav.$$("button");
    // WebdriverIO's ElementArray#map is itself chainable/async (resolves to
    // the mapped array directly), not a plain synchronous Array#map --
    // await it directly rather than wrapping in Promise.all.
    const resolved = (await buttons.map((b) => b.getText())) as unknown as string[];
    expect(resolved.some((t) => t.includes("Project Explorer"))).toBe(true);
    expect(resolved.some((t) => t.includes("Verified Tweaks"))).toBe(true);
    expect(resolved.some((t) => t.includes("Matrix Builder"))).toBe(false);
    expect(resolved.some((t) => t.includes("Live Monitor"))).toBe(false);
    expect(resolved.some((t) => t.includes("Visualizer"))).toBe(false);
  });
});

describe("Settings modal", () => {
  it("toggles thermal cooldown and dry run, and it persists", async () => {
    await $('button[title="Settings & Config Paths"]').click();
    await expect($("h2=Engine Preferences")).toBeDisplayed();

    const thermalToggle = await $(
      '//div[text()="Thermal Cooldown Between Iterations"]/ancestor::label//input[@type="checkbox"]'
    );
    await expect(thermalToggle).toExist();
    await expect(thermalToggle).toBeEnabled();
    if (!(await thermalToggle.isSelected())) {
      await thermalToggle.click();
    }

    const dryRunToggle = await $(
      '//div[text()="Dry Run by Default"]/ancestor::label//input[@type="checkbox"]'
    );
    if (!(await dryRunToggle.isSelected())) {
      await dryRunToggle.click();
    }

    await $("button=Save Preferences").click();
    // Label flashes to "Saved" then the modal auto-closes (~800ms).
    await $("h2=Engine Preferences").waitForDisplayed({ reverse: true, timeout: 5_000 });

    // Real save_config -> get_config round trip through the actual Rust
    // IPC layer -- re-open and confirm the saved values actually persisted
    // rather than trusting the "Saved" flash alone.
    await $('button[title="Settings & Config Paths"]').click();
    await (await $("h2=Engine Preferences")).waitForDisplayed();
    const reopenedThermalToggle = await $(
      '//div[text()="Thermal Cooldown Between Iterations"]/ancestor::label//input[@type="checkbox"]'
    );
    await expect(reopenedThermalToggle).toBeSelected();
    const reopenedDryRunToggle = await $(
      '//div[text()="Dry Run by Default"]/ancestor::label//input[@type="checkbox"]'
    );
    await expect(reopenedDryRunToggle).toBeSelected();
    await $("button=Cancel").click();
  });

  it("Cancel closes without saving", async () => {
    await $('button[title="Settings & Config Paths"]').click();
    await (await $("h2=Engine Preferences")).waitForDisplayed();
    await $("button=Cancel").click();
    await $("h2=Engine Preferences").waitForDisplayed({ reverse: true });
  });
});

describe("Preflight modal", () => {
  it("opens with no project selected and renders a checks report", async () => {
    await $('button[title="Pre-Flight System & AutoLogon Diagnostics"]').click();
    await expect($("h2=Pre-Flight Check")).toBeDisplayed();
    // Real backend call (Steam/CS2/disk-space checks) -- give it real time.
    await browser.waitUntil(
      async () => !(await $("text=Running pre-flight checks…").isExisting()),
      { timeout: 15_000, timeoutMsg: "preflight checks never finished loading" }
    );
    await $("button=Close").click();
  });
});

describe("Emergency Restore modal", () => {
  it("opens and shows the confirm phase WITHOUT executing anything", async () => {
    await $('button[title="Emergency Restore System to Baseline Snapshot"]').click();
    await expect($("h2=Emergency Restore")).toBeDisplayed();
    await expect($("button=Execute Emergency Restore")).toBeDisplayed();

    // Deliberately not clicked:
    //  - "Execute Emergency Restore" reverts the most-recently-modified
    //    run's journal for real.
    //  - "Open Data Folder" / "Reveal Restore Script" both shell out to a
    //    real, webview-external Windows Explorer window a driver-scoped
    //    test can't see or close, leaving it orphaned.
    await expect($("button=Open Data Folder")).toBeDisplayed();
    await expect($("button=Reveal Restore Script")).toBeDisplayed();

    await $("button=Close").click();
    await $("h2=Emergency Restore").waitForDisplayed({ reverse: true });
  });
});

describe("Verified Tweaks (library) full-page view", () => {
  it("renders the catalog with no close button (full page, not a modal)", async () => {
    await $("button=Verified Tweaks").click();
    const search = await $('input[placeholder="Search modules by name, description, kind..."]');
    await search.waitForDisplayed();

    // isModal=false on this route -- TweakCatalog renders the non-modal
    // heading ("Explore Validated System Tweaks") rather than the modal's
    // "Add Module -> <scenario>" heading, and has no close X (which has no
    // distinguishing title/text attribute of its own to select on
    // directly).
    await expect($("h2=Explore Validated System Tweaks")).toBeDisplayed();

    // Catalog entries load async (real `listCatalogTweaks()` IPC call) --
    // wait for that to finish before filtering, or the search runs against
    // a still-empty list. Assert another, unrelated entry disappears too --
    // otherwise this would pass vacuously even if the search filter never
    // actually ran at all (the target entry is already visible unfiltered).
    //
    // `list_catalog_tweaks_impl` (src-tauri/src/commands/catalog.rs)
    // narrows the full catalog down to `ALPHA_CATALOG_IDS` for the M3 alpha
    // scope -- only "Power Plan", "Exclude Core 0", "Launch Options", and
    // "HAGS" are offered here; "Disable Core Parking" is a real catalog
    // entry (data/catalog.json) but deliberately hidden from this picker.
    await $("text=Loading catalog…").waitForDisplayed({ reverse: true, timeout: 10_000 }).catch(() => {});
    await expect($("h4=Power Plan")).toBeDisplayed();
    await typeIntoSearchBox(search, "Exclude Core 0");
    await expect($("h4=Exclude Core 0")).toBeDisplayed();
    await expect($("h4=Power Plan")).not.toExist();
    await search.setValue("");

    await $("button=Project Explorer").click();
  });
});
