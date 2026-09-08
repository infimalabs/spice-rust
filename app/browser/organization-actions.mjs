import assert from "node:assert/strict";

import { TRANSITION_MS } from "./canvas-fixture.mjs";

const labels = [
  "Close team",
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

async function assertSquareActionLayout(actionMenu, label, expectedFirstRow) {
  const geometry = await actionMenu.evaluate((node) => {
    const bounds = (element) => {
      const box = element.getBoundingClientRect();
      return {
        bottom: box.bottom,
        height: box.height,
        left: box.left,
        right: box.right,
        top: box.top,
        width: box.width,
      };
    };
    return {
      menu: {
        ...bounds(node),
        clientWidth: node.clientWidth,
        scrollWidth: node.scrollWidth,
      },
      items: [...node.querySelectorAll(":scope > .organization-action")].map(bounds),
    };
  });
  const rows = [];
  for (const item of geometry.items) {
    const row = rows.find((candidate) => Math.abs(candidate[0].top - item.top) <= 1);
    if (row) row.push(item);
    else rows.push([item]);
  }
  assert.equal(geometry.menu.scrollWidth <= geometry.menu.clientWidth + 1, true, `${label} stays contained`);
  assert.equal(rows[0]?.length, expectedFirstRow, `${label} fills available square slots before wrapping`);
  assert.ok(geometry.items.every((item, index, items) => (
    item.width >= 136
      && Math.abs(item.width - item.height) <= 1
      && item.left >= geometry.menu.left - 1
      && item.right <= geometry.menu.right + 1
      && (index === 0 || item.left >= items[index - 1].right - 1
        || item.top >= items[index - 1].bottom - 1)
  )), `${label} keeps square, source-ordered controls without overlap: ${JSON.stringify(geometry)}`);
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
  await assertSquareActionLayout(primaryMenu, "the roomy organization menu", 4);
  await assertSquareActionLayout(secondaryMenu, "the floor-width organization menu", 2);
  const primaryImport = primaryMenu.getByRole("menuitem", { name: /Import agent/ });
  await primaryImport.focus();
  assert.equal(
    await primaryImport.evaluate((node) => node === document.activeElement),
    true,
    "a square organization action remains keyboard focusable",
  );

  await primaryMenu.getByRole("menuitem", { name: /Close team/ }).click();
  await primary.waitFor({ state: "detached", timeout: TRANSITION_MS });
  assert.equal(await secondary.isVisible(), true, "Close team leaves the sibling surface open");
  assert.equal(await secondaryMenu.isVisible(), true, "the sibling's independent menu stays open");

  await page.getByLabel("Open Spice Canvas menu").click();
  const globalOrg = page.locator(
    `.canvas-menu-panel .canvas-organization[data-organization-lineage="${primaryLineage}"]`,
  );
  const reopen = globalOrg.getByRole("button", { name: "Open team", exact: true });
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
