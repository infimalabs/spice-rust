import assert from "node:assert/strict";

import { chooseCanvasView, reply, TRANSITION_MS } from "./canvas-fixture.mjs";

async function openSpiceMenu(page) {
  const menu = page.locator(".canvas-menu");
  if ((await menu.getAttribute("open")) === null) {
    await menu.locator(":scope > summary").click();
  }
  const panel = menu.locator(".canvas-menu-panel");
  await panel.waitFor({ timeout: TRANSITION_MS });
  return panel;
}

async function assertAxisMenu(page, view, copy) {
  await chooseCanvasView(page, view);
  const panel = await openSpiceMenu(page);
  const menuPage = panel.locator(".canvas-menu-page");
  assert.equal(
    await menuPage.getAttribute("data-menu-page"),
    view.toLowerCase(),
    `${view} opens its own Spice panel rather than the Agents panel`,
  );
  assert.equal(await panel.locator(".canvas-team").count(), 0, `${view} renders no team manager`);
  await panel.getByRole("heading", { name: view, exact: true }).waitFor();
  assert.match(await menuPage.innerText(), copy, `${view} describes its own page context`);
}

async function waitForMemberIn(scope, lane) {
  const member = scope.locator(`.canvas-team-member[data-team-member="${lane}"]`);
  await member.waitFor({ timeout: TRANSITION_MS });
  return member;
}

async function assertResidentLineage(fixture, lane, lineage, reason) {
  const answer = await reply(fixture, ["lane"]);
  const member = answer.body.lanes.find((row) => row.lane === lane);
  assert.ok(member, `${reason}: moved member remains in the authoritative lane answer`);
  assert.equal(member.lineage_id, lineage, `${reason}: resident committed the target lineage`);
}

/** Exercise the Agents-page Spice panel, shared navigation, and durable team moves. */
export async function assertTeamMenu(page, fixture) {
  await chooseCanvasView(page, "Agents");
  const shortcuts = page.locator(".canvas-shortcuts");
  assert.deepEqual(
    await shortcuts.locator(".pill-label").allTextContents(),
    ["Agents", "Topology", "Chronology"],
    "Agents, Topology, and Chronology share one rail left of the primary Spice menu",
  );
  const chromeOrder = await page.locator(".commandbar--mosaic").evaluate((bar) => {
    const topology = bar.querySelector('.canvas-shortcuts button:nth-child(2)').getBoundingClientRect();
    const chronology = bar.querySelector('.canvas-shortcuts button:last-child').getBoundingClientRect();
    const spice = bar.querySelector('.canvas-menu > summary').getBoundingClientRect();
    return [topology.right <= chronology.left + 1, chronology.right <= spice.left + 1];
  });
  assert.deepEqual(chromeOrder, [true, true], "context navigation extends left from Spice");
  await shortcuts.getByRole("button", { name: /^Topology/ }).click();
  await page.getByRole("heading", { name: "Topology", exact: true }).waitFor();
  assert.equal(
    await page.getByRole("navigation", { name: "Canvas views" })
      .getByRole("button", { name: /^Agents/ }).count(),
    1,
    "the analytics pages keep the same route back to live Agents",
  );
  await assertAxisMenu(page, "Topology", /resident teams, seats, capabilities/i);
  await assertAxisMenu(page, "Chronology", /facts, command outcomes, and their causal thread/i);
  await chooseCanvasView(page, "Agents");

  let panel = await openSpiceMenu(page);
  assert.equal(await panel.locator(".canvas-menu-page").getAttribute("data-menu-page"), "agents");
  await panel.getByRole("heading", { name: "Global", exact: true }).waitFor();
  assert.match(await panel.locator(".canvas-menu-global").innerText(), /Fast mode\s+off/);
  assert.equal(await panel.locator(".canvas-team").count(), 1, "the four agents begin together");
  assert.equal(await panel.locator(".canvas-team-member").count(), 4);
  assert.equal(
    await panel.locator('[data-team-drop="new"]').innerText(),
    "DROP AGENT HERE",
  );
  assert.match(
    await panel.locator(".canvas-unteamed").innerText(),
    /Agents Without Team[\s\S]*No unteamed agents/i,
  );

  const initial = await reply(fixture, ["lane"]);
  const host = initial.body.lanes[0].lineage_id;
  const moved = initial.body.lanes.at(-1).lane;
  let originalTeam = panel.locator(`.canvas-team[data-team-lineage="${host}"]`);
  let member = await waitForMemberIn(originalTeam, moved);
  await member.dragTo(panel.locator('[data-team-drop="unteamed"]'));

  panel = await openSpiceMenu(page);
  member = await waitForMemberIn(panel.locator(".canvas-unteamed"), moved);
  assert.equal(await panel.locator(".canvas-team").count(), 1);
  assert.equal(await panel.locator(".canvas-team").locator(".canvas-team-member").count(), 3);
  await assertResidentLineage(fixture, moved, moved, "removing an agent from its team");

  originalTeam = panel.locator(`.canvas-team[data-team-lineage="${host}"]`);
  await member.dragTo(originalTeam);
  panel = await openSpiceMenu(page);
  originalTeam = panel.locator(`.canvas-team[data-team-lineage="${host}"]`);
  member = await waitForMemberIn(originalTeam, moved);
  assert.equal(await originalTeam.locator(".canvas-team-member").count(), 4);
  await assertResidentLineage(fixture, moved, host, "returning an agent to its team");

  await member.dragTo(panel.locator('[data-team-drop="new"]'));
  panel = await openSpiceMenu(page);
  const createdTeam = panel.locator(
    `.canvas-team:not([data-team-lineage="${host}"])`
      + `:has(.canvas-team-member[data-team-member="${moved}"])`,
  );
  await createdTeam.waitFor({ timeout: TRANSITION_MS });
  const created = await createdTeam.getAttribute("data-team-lineage");
  assert.notEqual(created, host, "the New Team target mints a distinct lineage identity");
  assert.notEqual(created, moved, "a new one-agent team stays distinct from an unteamed agent");
  assert.equal(await panel.locator(".canvas-team").count(), 2);
  await assertResidentLineage(fixture, moved, created, "creating a team by drop");

  member = await waitForMemberIn(createdTeam, moved);
  originalTeam = panel.locator(`.canvas-team[data-team-lineage="${host}"]`);
  await member.dragTo(originalTeam);
  panel = await openSpiceMenu(page);
  originalTeam = panel.locator(`.canvas-team[data-team-lineage="${host}"]`);
  await waitForMemberIn(originalTeam, moved);
  assert.equal(await panel.locator(".canvas-team").count(), 1, "the empty created team disappears");
  assert.equal(await originalTeam.locator(".canvas-team-member").count(), 4);
  await assertResidentLineage(fixture, moved, host, "restoring the original four-agent team");
}
