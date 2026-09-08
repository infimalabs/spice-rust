import assert from "node:assert/strict";

import { TRANSITION_MS } from "./canvas-fixture.mjs";

const labels = [
  "Close organization",
  "Import agent",
  "Split into individuals",
  "Restore previous organization",
];

async function menu(surface) {
  const actions = surface.getByRole("menu", { name: /^Actions for organization / });
  await actions.waitFor({ timeout: TRANSITION_MS });
  return actions;
}

async function closeSpiceMenu(page) {
  const details = page.locator(".canvas-menu");
  if ((await details.getAttribute("open")) !== null) {
    await details.locator(":scope > summary").click();
  }
}

/** Prove each whole-organization hamburger owns one isolated, reversible action menu. */
export async function assertOrgActions(page, primary, secondary, primaryLineage) {
  const primaryToggle = primary.getByRole("button", { name: /^Organization actions for / });
  const secondaryToggle = secondary.getByRole("button", { name: /^Organization actions for / });
  const selectedMode = (surface) => surface.locator(".organization-pane").getAttribute("data-organization-mode");

  await primaryToggle.click();
  const primaryMenu = await menu(primary);
  assert.deepEqual(
    await primaryMenu.getByRole("menuitem").evaluateAll((items) =>
      items.map((item) => item.querySelector(".organization-action-label")?.textContent)),
    labels,
    "the organization hamburger opens the predecessor's four actions in order",
  );
  assert.deepEqual(
    await primaryMenu.locator(".organization-action-detail").allTextContents(),
    ["4 agents", "choose agent", "one organization per agent", "no saved subgroup"],
    "the four-agent menu carries the reference presentation details",
  );
  assert.equal(
    await primaryMenu.getByRole("menuitem", { name: /Restore previous organization/ }).isDisabled(),
    true,
    "restore stays unavailable until this organization has a saved subgroup",
  );
  assert.deepEqual(
    [await selectedMode(primary), await selectedMode(secondary)],
    ["Lanes", "Lanes"],
    "opening an organization menu does not switch either organization to Org",
  );

  await secondaryToggle.click();
  const secondaryMenu = await menu(secondary);
  assert.equal(await primaryMenu.isVisible(), true, "opening a sibling menu leaves this menu open");
  assert.deepEqual(
    await secondaryMenu.locator(".organization-action-detail").allTextContents(),
    ["1 agent", "choose agent", "already individual", "no saved subgroup"],
    "the sibling menu derives its actions from its own one-agent organization",
  );
  assert.equal(
    await secondaryMenu.getByRole("menuitem", { name: /Split into individuals/ }).isDisabled(),
    true,
    "an individual organization cannot be split",
  );

  await primaryMenu.getByRole("menuitem", { name: /Close organization/ }).click();
  await primary.waitFor({ state: "detached", timeout: TRANSITION_MS });
  assert.equal(await secondary.isVisible(), true, "Close organization leaves the sibling surface open");
  assert.equal(await secondaryMenu.isVisible(), true, "the sibling's independent menu stays open");

  await page.getByLabel("Open Spice Canvas menu").click();
  const globalOrg = page.locator(
    `.canvas-menu-panel .canvas-organization[data-organization-lineage="${primaryLineage}"]`,
  );
  const reopen = globalOrg.getByRole("button", { name: "Open organization", exact: true });
  await reopen.waitFor({ timeout: TRANSITION_MS });
  await reopen.click();
  await primary.waitFor({ timeout: TRANSITION_MS });
  assert.equal(await secondary.isVisible(), true, "reopening one organization leaves its sibling untouched");
  await closeSpiceMenu(page);

  await secondaryToggle.click();
  assert.equal(await secondaryMenu.count(), 0, "the sibling hamburger closes only its own menu");
  await secondaryToggle.click();
  await secondary
    .getByRole("menu", { name: /^Actions for organization / })
    .getByRole("menuitem", { name: /Import agent/ })
    .click();
  const importer = secondary.getByRole("menu", { name: /^Import agent into organization / });
  await importer.waitFor({ timeout: TRANSITION_MS });
  assert.equal(
    await secondaryToggle.getAttribute("aria-expanded"),
    "true",
    "the originating hamburger remains expanded while its import panel is open",
  );
  assert.equal(
    await importer.getByRole("menuitem").count(),
    4,
    "Import agent offers the four members outside only the originating organization",
  );
  await importer.getByRole("button", { name: "Cancel", exact: true }).click();
  assert.equal(await importer.count(), 0, "Cancel closes only the originating import menu");
  assert.equal(await secondaryToggle.getAttribute("aria-expanded"), "false");
}
