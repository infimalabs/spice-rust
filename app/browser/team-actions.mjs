import assert from "node:assert/strict";

import { TRANSITION_MS } from "./canvas-fixture.mjs";

const labels = [
  "Close team",
  "Import agent",
  "Split into individuals",
  "Restore previous team",
];

async function menu(surface) {
  const actions = surface.getByRole("menu", { name: /^Actions for team / });
  await actions.waitFor({ timeout: TRANSITION_MS });
  return actions;
}

async function closeSpiceMenu(page) {
  const details = page.locator(".canvas-menu");
  if ((await details.getAttribute("open")) !== null) {
    await details.locator(":scope > summary").click();
  }
}

/** Prove each whole-team hamburger owns one isolated, reversible action menu. */
export async function assertTeamActions(page, primary, secondary, primaryLineage) {
  const primaryToggle = primary.getByRole("button", { name: /^Team actions for / });
  const secondaryToggle = secondary.getByRole("button", { name: /^Team actions for / });
  const selectedMode = (surface) => surface.locator(".team-pane").getAttribute("data-team-mode");

  await primaryToggle.click();
  const primaryMenu = await menu(primary);
  assert.deepEqual(
    await primaryMenu.getByRole("menuitem").evaluateAll((items) =>
      items.map((item) => item.querySelector(".team-action-label")?.textContent)),
    labels,
    "the team hamburger opens the predecessor's four actions in order",
  );
  assert.deepEqual(
    await primaryMenu.locator(".team-action-detail").allTextContents(),
    ["4 agents", "choose agent", "one team per agent", "no saved subgroup"],
    "the four-agent menu carries the reference presentation details",
  );
  assert.equal(
    await primaryMenu.getByRole("menuitem", { name: /Restore previous team/ }).isDisabled(),
    true,
    "restore stays unavailable until this team has a saved subgroup",
  );
  assert.deepEqual(
    [await selectedMode(primary), await selectedMode(secondary)],
    ["Lanes", "Lanes"],
    "opening a team menu does not switch either team to Team",
  );

  await secondaryToggle.click();
  const secondaryMenu = await menu(secondary);
  assert.equal(await primaryMenu.isVisible(), true, "opening a sibling menu leaves this menu open");
  assert.deepEqual(
    await secondaryMenu.locator(".team-action-detail").allTextContents(),
    ["1 agent", "choose agent", "already individual", "no saved subgroup"],
    "the sibling menu derives its actions from its own one-agent team",
  );
  assert.equal(
    await secondaryMenu.getByRole("menuitem", { name: /Split into individuals/ }).isDisabled(),
    true,
    "an individual team cannot be split",
  );

  await primaryMenu.getByRole("menuitem", { name: /Close team/ }).click();
  await primary.waitFor({ state: "detached", timeout: TRANSITION_MS });
  assert.equal(await secondary.isVisible(), true, "Close team leaves the sibling surface open");
  assert.equal(await secondaryMenu.isVisible(), true, "the sibling's independent menu stays open");

  await page.getByLabel("Open Spice Canvas menu").click();
  const globalTeam = page.locator(
    `.canvas-menu-panel .canvas-team[data-team-lineage="${primaryLineage}"]`,
  );
  const reopen = globalTeam.getByRole("button", { name: "Open team", exact: true });
  await reopen.waitFor({ timeout: TRANSITION_MS });
  await reopen.click();
  await primary.waitFor({ timeout: TRANSITION_MS });
  assert.equal(await secondary.isVisible(), true, "reopening one team leaves its sibling untouched");
  await closeSpiceMenu(page);

  await secondaryToggle.click();
  assert.equal(await secondaryMenu.count(), 0, "the sibling hamburger closes only its own menu");
  await secondaryToggle.click();
  await secondary
    .getByRole("menu", { name: /^Actions for team / })
    .getByRole("menuitem", { name: /Import agent/ })
    .click();
  const importer = secondary.getByRole("menu", { name: /^Import agent into team / });
  await importer.waitFor({ timeout: TRANSITION_MS });
  assert.equal(
    await secondaryToggle.getAttribute("aria-expanded"),
    "true",
    "the originating hamburger remains expanded while its import panel is open",
  );
  assert.equal(
    await importer.getByRole("menuitem").count(),
    4,
    "Import agent offers the four members outside only the originating team",
  );
  await importer.getByRole("button", { name: "Cancel", exact: true }).click();
  assert.equal(await importer.count(), 0, "Cancel closes only the originating import menu");
  assert.equal(await secondaryToggle.getAttribute("aria-expanded"), "false");
}
