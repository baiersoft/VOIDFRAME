import { $, $$, browser } from "@wdio/globals";

/** Shared helpers for real-binary E2E specs.
 *
 * `browser.tauri.execute()` (the officially supported bridge to real
 * `invoke()`, backed by `tauri-plugin-wdio-webdriver`'s embedded server)
 * hits its own unconditional "Tauri core.invoke not available after 5s
 * timeout" in this app, on every call, regardless of the window-focus fix
 * in wdio.conf.ts's `before` hook (a separate code path -- confirmed by
 * direct testing). Rather than block the whole suite on root-causing that
 * too, cleanup and verification below go through the real UI instead --
 * slower per-operation, but exercises the exact thing these specs exist
 * to test anyway, and isn't blocked on a still-unexplained bridge issue.
 */

/** Deletes every real, on-disk project whose name starts with `prefix` via
 * the same delete-icon-then-confirm flow a user would use -- run this in
 * an `after` hook for any spec that creates projects via the UI, so
 * re-running the suite doesn't accumulate
 * `%LOCALAPPDATA%\baiersoft\VOIDFRAME\projects\*.json` cruft. Must be
 * called while on the Project Explorer tab. */
export async function deleteProjectsByNamePrefix(prefix: string): Promise<void> {
  await $("button=Project Explorer").click();
  // Re-queries each loop iteration -- deleting a card changes the DOM, so
  // a snapshot list of stale elements would go invalid after the first delete.
  for (;;) {
    const headings = await $$("h3");
    const texts = await headings.map((h) => h.getText());
    const match = (texts as unknown as string[]).find((t) => t.startsWith(prefix));
    if (!match) break;
    const card = projectCard(match);
    await (await card.$('button[title="Delete Project"]')).click();
    await (await card.$('button[title="Confirm delete project"]')).click();
    await card.waitForDisplayed({ reverse: true, timeout: 10_000 });
  }
}

/** Types into a React-controlled text input via real key events rather
 * than `element.setValue()` -- confirmed by direct testing that `setValue`
 * on this app's dashboard/catalog *search* inputs (unlike NewProjectModal's
 * name/description fields, which work fine with plain `setValue`) doesn't
 * reliably fire the input's real `onChange`, leaving React's filter state
 * unchanged even though the DOM value visibly updates. Click to focus,
 * select-all + delete any existing value, then send real keystrokes. */
export async function typeIntoSearchBox(
  input: { click(): Promise<void> },
  text: string
): Promise<void> {
  await input.click();
  await browser.keys(["Control", "a"]);
  await browser.keys("Delete");
  await browser.keys(text);
}

/** Waits for `substring` to appear anywhere in the page's rendered text.
 * Use this instead of `$('text=...')`/`$('*=...')` for any JSX text that
 * mixes literal text with an interpolated expression (e.g. `Configured
 * Modules ({count})`, `No matrices match your search for "{term}".`) --
 * React splits that into multiple sibling text nodes under one parent,
 * and confirmed by direct diagnostic (dumping `body`'s real text
 * alongside a failing assertion) that this embedded WebDriver provider's
 * `text=`/`*=` strategies don't reliably match across that split, even
 * though the exact expected text is genuinely present on the page. A
 * single-expression element (`<h4>{name}</h4>`, one whole child) is
 * unaffected and `text=`/`*=`/tag selectors on it work fine. */
export async function expectBodyContains(substring: string, timeout = 10_000): Promise<void> {
  await browser.waitUntil(async () => (await (await $("body")).getText()).includes(substring), {
    timeout,
    timeoutMsg: `page text never contained ${JSON.stringify(substring)}`,
  });
}

/** Returns the whole project card element (the `.glass-card` root) for a
 * given project name in ProjectExplorer -- an XPath ancestor lookup rather
 * than a fixed count of `.parentElement()` hops, so it doesn't silently
 * break (by landing on the wrong ancestor level) if the card's internal
 * markup nesting ever changes. Contains everything: Edit Matrix, Run
 * Matrix, the delete icon/confirm buttons, and the metrics pills. */
export function projectCard(name: string) {
  return $(`//h3[text()="${name}"]/ancestor::div[contains(@class,"glass-card")][1]`);
}

/** Drives NewProjectModal end-to-end exactly as a user would -- this is
 * the real-binary regression path for the schema_version bug fixed
 * earlier: a stale literal here would save fine but vanish from the very
 * next `list_projects` refresh, which no mocked-IPC unit test can catch.
 *
 * `handleCreateNewProject` (App.tsx) navigates to the Matrix Builder tab
 * immediately on success -- it does NOT stay on the dashboard. Every
 * caller of this helper expects to land back on Project Explorer (to see
 * the new card in the grid), so this waits for that navigation to
 * actually complete (the builder's own heading, keyed on the new
 * project's name) and then switches back, rather than leaving callers to
 * each work around the real navigation themselves. */
export async function createProjectViaUI(name: string, description = ""): Promise<void> {
  await $("button=New Benchmark Project").click();
  const nameInput = await $(
    'input[placeholder="e.g., BYOB Driver Branch vs. GPU Interrupt Matrix"]'
  );
  await nameInput.waitForDisplayed();
  await nameInput.setValue(name);
  if (description) {
    await $('textarea[placeholder^="e.g., Testing if NVIDIA"]').setValue(description);
  }
  await $("button=Create Matrix").click();
  await (await $(`h2=${name}`)).waitForDisplayed({ timeout: 10_000 });
  await $("button=Project Explorer").click();
}
