import { $, expect } from "@wdio/globals";
import {
  createProjectViaUI,
  deleteProjectsByNamePrefix,
  expectBodyContains,
  projectCard,
  typeIntoSearchBox,
} from "../helpers";

const PREFIX = "E2E CRUD";

describe("Dashboard — project CRUD (real IPC, no system mutation)", () => {
  after(async () => {
    await deleteProjectsByNamePrefix(PREFIX);
  });

  it("creates a project via the UI and it appears in the explorer immediately", async () => {
    const name = `${PREFIX} ${Date.now()}`;
    await createProjectViaUI(name, "Created by the E2E suite.");

    // Real save_project -> real list_projects round trip through the
    // actual Rust IPC layer -- the exact class of bug that slipped past
    // mocked frontend tests (a stale schema_version literal silently
    // filtered out by list_projects_impl). createProjectViaUI already
    // navigated back to Project Explorer after the real backend round
    // trip; this card only renders if that round trip actually worked.
    const card = await $(`h3=${name}`);
    await card.waitForDisplayed({ timeout: 10_000 });
  });

  it("filters the grid by search term and shows the empty-search state", async () => {
    const name = `${PREFIX} Searchable ${Date.now()}`;
    await createProjectViaUI(name);
    await (await $(`h3=${name}`)).waitForDisplayed();

    const search = await $('input[placeholder="Search matrices or descriptions..."]');
    await typeIntoSearchBox(search, "no project should ever match this string");

    // The empty-search message mixes literal JSX text with an interpolated
    // `{searchTerm}` -- see expectBodyContains's own doc comment for why
    // that needs a body-text check rather than `$('*=...')`.
    await expectBodyContains("No matrices match your search");
    await $("button=Clear search").click();
    await expect(await $(`h3=${name}`)).toBeDisplayed();
  });

  it("Edit Matrix navigates to the Matrix Builder for that project", async () => {
    const name = `${PREFIX} Editable ${Date.now()}`;
    await createProjectViaUI(name);
    await (await $(`h3=${name}`)).waitForDisplayed();

    // Scope to this project's own card so multiple cards on screen don't
    // collide on a shared "Edit Matrix" label.
    await (await projectCard(name).$("button=Edit Matrix")).click();

    await expect($("button=Launch Autonomous Pipeline")).toBeDisplayed();
    await expect($(`h2=${name}`)).toBeDisplayed();
    await $("button=Project Explorer").click();
  });

  it("delete requires inline confirmation and can be cancelled", async () => {
    const name = `${PREFIX} Deletable ${Date.now()}`;
    await createProjectViaUI(name);
    const card = await $(`h3=${name}`);
    await card.waitForDisplayed();
    const cardRoot = projectCard(name);

    await (await cardRoot.$('button[title="Delete Project"]')).click();
    await (await cardRoot.$('button[title="Cancel"]')).click();
    // Cancelled -- the card must still exist.
    await expect(card).toBeDisplayed();

    await (await cardRoot.$('button[title="Delete Project"]')).click();
    await (await cardRoot.$('button[title="Confirm delete project"]')).click();
    await card.waitForDisplayed({ reverse: true, timeout: 10_000 });
  });
});
