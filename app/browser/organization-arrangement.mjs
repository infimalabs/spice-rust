/// Prove organization lanes are arranged by dragging one organization onto another.
///
/// Two gestures share one durable command, and the fixture keeps them apart
/// the way the operator does: where inside the target the organization is dropped.
/// A drop in the middle takes the target's place, a drop in either outer
/// fifth gives the target its agents. Both are read back from the resident
/// rather than the DOM, because the predecessor reordered organizations with a bare
/// `insertBefore` that no reload survived.
///
/// The third assertion is the one that keeps the gesture honest. A member is
/// dragged by its primary composer too, and that gesture commits `lane::arrange`
/// against the very same lane table. Moving a member inside its own organization must
/// therefore leave the organizations exactly where this fixture just put them: the two
/// arrangement laws share a table, and neither may stand in for the other.

import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { reply, waitForPageFunction, TRANSITION_MS } from "./canvas-fixture.mjs";

/// The private media a dragged organization carries, and the only one an organization accepts.
const ORG_MEDIA = "application/x-spice-organization";

/// The one draggable composer a member exposes, by the lane it belongs to.
function primaryComposer(agents, lane) {
  return agents.locator(`.composer-shard--primary[data-shard-target-id="${lane}"]`);
}

/// The members the canvas drew inside one organization, in the order it drew them.
async function waitForMemberOrder(page, label, lineage, order) {
  await waitForPageFunction(page, label, TRANSITION_MS, ({ lineage, expected }) => {
    const drawn = [...document.querySelectorAll(
      `.organization-surface[data-lineage-id="${lineage}"] .organization-agent`,
    )].map((member) => member.dataset.agentLane);
    return drawn.length === expected.length
      && drawn.every((lane, at) => lane === expected[at]);
  }, { lineage, expected: order });
}

/// The organizations the resident calls durable, in the order it states them.
function durableOrgs(lanes) {
  const organizations = [];
  for (const lane of lanes) {
    const organization = organizations.find((row) => row.lineage === lane.lineage_id);
    if (organization) {
      organization.lanes.push(lane.lane);
    } else {
      organizations.push({ lineage: lane.lineage_id, lanes: [lane.lane] });
    }
  }
  return organizations;
}

/// The organizations the canvas drew, in the order it drew them.
function renderedOrgs(agents) {
  return agents.locator(".organization-surface").evaluateAll((surfaces) => surfaces.map((surface) => ({
    lineage: surface.dataset.lineageId,
    lanes: [...surface.querySelectorAll(".organization-agent")].map((agent) => agent.dataset.agentLane),
  })));
}

/// Wait for the canvas to redraw the organization order the resident now states.
async function waitForOrgOrder(page, label, order) {
  await waitForPageFunction(page, label, TRANSITION_MS, (expected) => {
    const drawn = [...document.querySelectorAll(".organization-surface")]
      .map((surface) => surface.dataset.lineageId);
    return drawn.length === expected.length
      && drawn.every((lineage, at) => lineage === expected[at]);
  }, order);
}

/// Drag one organization by its pip stack and drop it across the target at `fraction`.
///
/// The pip stack's real `dragstart` fills the transfer, so a handle that carried
/// nothing would fail here rather than be papered over by a transfer the
/// fixture wrote itself.
function dragOrg(page, gesture) {
  return page.evaluate(({ dragged, target, fraction }) => {
    const pips = document.querySelector(
      `.organization-surface[data-lineage-id="${dragged}"] .organization-lights[data-lane-drag-handle]`,
    );
    const surface = document.querySelector(`.organization-surface[data-lineage-id="${target}"]`);
    if (!pips || !surface) throw new Error("arranging organizations needs pips and both organizations rendered");
    const transfer = new DataTransfer();
    pips.dispatchEvent(new DragEvent("dragstart", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
    }));
    const rect = surface.getBoundingClientRect();
    const at = {
      clientX: Math.round(rect.left + rect.width * fraction),
      clientY: Math.round(rect.top + rect.height / 2),
    };
    const over = new DragEvent("dragover", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
      ...at,
    });
    surface.dispatchEvent(over);
    const drop = new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
      ...at,
    });
    surface.dispatchEvent(drop);
    return {
      carried: transfer.getData("application/x-spice-organization"),
      types: [...transfer.types],
      offered: over.defaultPrevented,
      taken: drop.defaultPrevented,
    };
  }, gesture);
}

function organizationPips(agents, lineage) {
  return agents.locator(
    `.organization-surface[data-lineage-id="${lineage}"] .organization-lights[data-lane-drag-handle]`,
  );
}

function arrangementMenu(agents, lineage) {
  return agents.locator(
    `.organization-surface[data-lineage-id="${lineage}"] .organization-arrangement-menu`,
  );
}

async function assertKeyboardArrangement(page, fixture, agents, primary, secondary) {
  const primaryPips = organizationPips(agents, primary.lineage);
  await primaryPips.focus();
  assert.equal(await primaryPips.evaluate((node) => document.activeElement === node), true,
    "the organization pips accept keyboard focus");
  await primaryPips.press("Enter");
  const primaryMenu = arrangementMenu(agents, primary.lineage);
  await primaryMenu.waitFor();
  assert.equal(await primaryPips.getAttribute("aria-expanded"), "true");
  assert.equal(
    await primaryMenu.locator(`[data-lineage-arrangement="after"][data-lineage-arrangement-target="${secondary.lineage}"]`).count(),
    1,
    "the first organization offers its non-noop move-after action",
  );
  assert.equal(
    await primaryMenu.locator(`[data-lineage-arrangement="join"][data-lineage-arrangement-target="${secondary.lineage}"]`).count(),
    1,
    "the keyboard menu offers a join action",
  );
  await agents.locator(
    `.organization-surface[data-lineage-id="${primary.lineage}"] .organization-mode-rail`,
  ).click({ position: { x: 4, y: 4 } });
  await primaryMenu.waitFor({ state: "detached" });
  assert.equal(await primaryPips.getAttribute("aria-expanded"), "false",
    "activation outside the controls closes the arrangement menu");
  await primaryPips.press("Enter");
  await primaryMenu.waitFor();
  await page.keyboard.press("Escape");
  await primaryMenu.waitFor({ state: "detached" });
  assert.equal(await primaryPips.evaluate((node) => document.activeElement === node), true,
    "Escape closes the arrangement menu and returns focus to its pips");

  const secondaryPips = organizationPips(agents, secondary.lineage);
  await secondaryPips.focus();
  await secondaryPips.press("Space");
  const secondaryMenu = arrangementMenu(agents, secondary.lineage);
  const moveBefore = secondaryMenu.locator(
    `[data-lineage-arrangement="before"][data-lineage-arrangement-target="${primary.lineage}"]`,
  );
  await moveBefore.waitFor();
  await moveBefore.focus();
  await page.keyboard.press("Enter");
  await secondaryMenu.waitFor({ state: "detached" });
  const reordered = [secondary, primary];
  await waitForOrgOrder(page, "keyboard arrangement moves the selected organization", [
    secondary.lineage,
    primary.lineage,
  ]);
  assert.deepEqual(
    durableOrgs((await reply(fixture, ["lane"])).body.lanes),
    reordered,
    "keyboard arrangement is durable resident state",
  );
  return reordered;
}

async function touchJoinOrg(page, fixture, dragged, target) {
  const browser = page.context().browser();
  assert.ok(browser, "touch arrangement requires the live browser");
  const context = await browser.newContext({
    colorScheme: "light",
    hasTouch: true,
    viewport: { width: 390, height: 844 },
  });
  const touch = await context.newPage();
  try {
    await touch.goto(page.url(), { waitUntil: "networkidle" });
    const agents = touch.locator('[data-pane="agent-console"]');
    const pips = organizationPips(agents, dragged);
    await pips.tap();
    assert.equal(await pips.getAttribute("aria-expanded"), "true",
      "a touch tap opens the pip arrangement menu");
    await arrangementMenu(agents, dragged).locator(
      `[data-lineage-arrangement="join"][data-lineage-arrangement-target="${target}"]`,
    ).tap();
    await waitForOrgOrder(touch, "touch arrangement joins the selected organization", [target]);
    const organizations = durableOrgs((await reply(fixture, ["lane"])).body.lanes);
    assert.equal(organizations.length, 1, "touch arrangement persists one joined organization");
    assert.equal(organizations[0].lineage, target, "touch join preserves its target organization identity");
    const inertPips = agents.locator(
      `.organization-surface[data-lineage-id="${target}"] .organization-lights`,
    );
    await inertPips.tap();
    assert.equal(await agents.locator(".organization-arrangement-menu").count(), 0,
      "touch activation cannot open an unavailable arrangement menu");
  } finally {
    await context.close();
  }
}

/// Dragging an organization reorders organizations; dropping on an edge joins them.
export async function assertOrgArrangement(page, fixture, worktrees, artifacts) {
  const agents = page.locator('[data-pane="agent-console"]');
  await agents.getByRole("tab", { name: /^Threads/ }).first().click();
  const seated = durableOrgs((await reply(fixture, ["lane"])).body.lanes);
  assert.equal(seated.length, 2, "arranging organizations needs two durable organizations");
  const [primary, secondary] = seated;
  assert.deepEqual(await renderedOrgs(agents), seated, "the canvas draws the durable order");
  assert.equal(
    await agents.locator(".organization-lights[data-lane-drag-handle]").count(),
    2,
    "every arrangeable organization is dragged by its own pip stack",
  );
  assert.equal(
    await agents.locator(".organization-drag-handle").count(),
    0,
    "the pip stack replaces the standalone drag symbol",
  );

  const reordered = await assertKeyboardArrangement(
    page, fixture, agents, primary, secondary,
  );

  const reorder = await dragOrg(page, {
    dragged: primary.lineage,
    target: secondary.lineage,
    fraction: 0.5,
  });
  assert.deepEqual(reorder.types, [ORG_MEDIA], "a dragged organization carries only its own media");
  assert.equal(reorder.carried, primary.lineage, "the pips carry the organization they belong to");
  assert.deepEqual(
    [reorder.offered, reorder.taken],
    [true, true],
    "an organization surface offers itself as a drop target and takes the drop",
  );
  await waitForOrgOrder(page, "mouse drag moves the organization back before its target",
    seated.map((organization) => organization.lineage));
  assert.deepEqual(
    durableOrgs((await reply(fixture, ["lane"])).body.lanes),
    seated,
    "the drag changes the durable order and moves no agent between organizations",
  );
  assert.equal(await agents.locator(".organization-arrangement-menu").count(), 0,
    "mouse dragging does not accidentally activate the pip menu");
  await dragOrg(page, {
    dragged: secondary.lineage,
    target: primary.lineage,
    fraction: 0.5,
  });
  await waitForOrgOrder(page, "mouse drag restores the keyboard arrangement",
    reordered.map((organization) => organization.lineage));

  const crowded = reordered.find((organization) => organization.lanes.length >= 2);
  assert.ok(crowded, "proving the two laws apart needs one organization holding two members");
  const [leading, following] = crowded.lanes;
  const moved = [following, leading, ...crowded.lanes.slice(2)];
  await primaryComposer(agents, following).locator(".composer-shard-prose")
    .dragTo(primaryComposer(agents, leading), {
    sourcePosition: { x: 10, y: 10 },
    targetPosition: { x: 10, y: 10 },
  });
  await waitForMemberOrder(page, "the dragged member takes its target's place",
    crowded.lineage, moved);
  const afterMember = durableOrgs((await reply(fixture, ["lane"])).body.lanes);
  assert.deepEqual(
    afterMember.map((organization) => organization.lineage),
    reordered.map((organization) => organization.lineage),
    "arranging a member never rearranges the organizations",
  );
  assert.deepEqual(
    afterMember.map((organization) => [...organization.lanes].sort()),
    reordered.map((organization) => [...organization.lanes].sort()),
    "a member dragged inside its own organization joins no other organization",
  );
  assert.deepEqual(
    afterMember.find((organization) => organization.lineage === crowded.lineage).lanes,
    moved,
    "the member drag is durable, so the two laws wrote the one lane table in turn",
  );
  crowded.lanes = moved;

  const laneRows = (await reply(fixture, ["lane"])).body.lanes;
  const primaryNarrator = laneRows.find((lane) => lane.lane === primary.lanes[0]);
  const secondaryNarrator = laneRows.find((lane) => lane.lane === secondary.lanes[0]);
  assert.ok(primaryNarrator && secondaryNarrator, "both organizations expose a live narrator worktree");
  await agents.locator(`.organization-surface[data-lineage-id="${primary.lineage}"]`)
    .locator('.organization-speech input[type="range"]').fill("2");
  await agents.locator(`.organization-surface[data-lineage-id="${secondary.lineage}"]`)
    .locator('.organization-speech input[type="range"]').fill("2");
  await page.evaluate(() => {
    window.__speechLog = [];
    window.__speechUtterances = [];
    window.__speechActive = 0;
    window.__speechMaxActive = 0;
  });
  const primaryCards = agents.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${primaryNarrator.lane}"]`,
  );
  const secondaryCards = agents.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${secondaryNarrator.lane}"]`,
  );
  let before = await primaryCards.count();
  await writeFile(join(primaryNarrator.worktree, ".spice-prose-trigger"), "emit\n");
  await primaryCards.nth(before).waitFor({ timeout: TRANSITION_MS * 4 });
  await waitForPageFunction(page, "the surviving organization owns narration", TRANSITION_MS,
    () => window.__speechLog.filter((row) => row.action === "speak").length === 1);
  before = await secondaryCards.count();
  await writeFile(join(secondaryNarrator.worktree, ".spice-prose-trigger"), "emit\n");
  await secondaryCards.nth(before).waitFor({ timeout: TRANSITION_MS * 4 });
  before = await primaryCards.count();
  await writeFile(join(primaryNarrator.worktree, ".spice-prose-trigger"), "emit\n");
  await primaryCards.nth(before).waitFor({ timeout: TRANSITION_MS * 4 });
  assert.equal(
    await page.evaluate(() => window.__speechLog.filter((row) => row.action === "speak").length),
    1,
    "both lineage tails wait behind the committed head",
  );

  const primarySurface = agents.locator(
    `.organization-surface[data-lineage-id="${primary.lineage}"]`,
  );
  const secondarySurface = agents.locator(
    `.organization-surface[data-lineage-id="${secondary.lineage}"]`,
  );
  const selectedMode = (surface) => surface.locator(".organization-pane").getAttribute("data-organization-mode");
  await primarySurface.getByRole("tab", { name: /^Tasks/ }).click();
  await secondarySurface.getByRole("tab", { name: /^Team/ }).click();
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(secondarySurface)],
    ["Tasks", "Org"],
    "the organizations hold distinct views immediately before one lineage retires",
  );

  const gathered = await dragOrg(page, {
    dragged: secondary.lineage,
    target: primary.lineage,
    fraction: 0.1,
  });
  assert.deepEqual(
    [gathered.offered, gathered.taken],
    [true, true],
    "the joining edge takes the drop",
  );
  await waitForOrgOrder(page, "the joined organizations draw as one", [primary.lineage]);
  const joined = await reply(fixture, ["lane"]);
  assert.deepEqual(
    durableOrgs(joined.body.lanes),
    [{ lineage: primary.lineage, lanes: [...primary.lanes, ...secondary.lanes] }],
    "the dropped organization's agents join the target behind its own primary composer",
  );
  assert.deepEqual(
    joined.body.subscriptions.map((subscription) => subscription.lineage_id),
    [primary.lineage],
    "the emptied organization keeps no task policy of its own",
  );
  assert.equal(
    await selectedMode(primarySurface),
    "Tasks",
    "retiring a sibling preserves the surviving organization's selected view",
  );
  await primarySurface.getByRole("tab", { name: /^Threads/ }).click();
  assert.deepEqual(
    await renderedOrgs(agents),
    [{ lineage: primary.lineage, lanes: [...primary.lanes, ...secondary.lanes] }],
    "the canvas draws one organization whose first composer never moved",
  );
  await primarySurface.getByRole("tab", { name: /^Tasks/ }).click();

  await reply(fixture, [
    "lane", "add", worktrees[5],
    "--lineage", secondary.lineage,
    "--life", "Drive",
    "--vendor", "claude",
  ]);
  const recreatedSurface = agents.locator(
    `.organization-surface[data-lineage-id="${secondary.lineage}"]`,
  );
  await recreatedSurface.waitFor({ timeout: TRANSITION_MS });
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(recreatedSurface)],
    ["Tasks", "Lanes"],
    "a recreated lineage starts clean instead of reviving its retired view state",
  );
  await page.evaluate(() => {
    const utterance = window.__speechUtterances.at(-1);
    if (!utterance?.onend) throw new Error("the narration head has no completion callback");
    utterance.onend(new Event("end"));
  });
  await waitForPageFunction(page, "only the surviving organization's queued narration advances",
    TRANSITION_MS, () =>
      window.__speechLog.filter((row) => row.action === "speak").length === 2);
  assert.equal(
    await page.evaluate(() => window.__speechMaxActive),
    1,
    "removing one lane never overlaps its abandoned speech with the survivor",
  );
  await page.evaluate(() => {
    const utterance = window.__speechUtterances.at(-1);
    if (!utterance?.onend) throw new Error("the surviving narration has no completion callback");
    utterance.onend(new Event("end"));
  });
  await waitForPageFunction(page, "the surviving narration drains without the removed tail",
    TRANSITION_MS, () => window.__speechActive === 0);
  assert.equal(
    await page.evaluate(() => window.__speechLog.filter((row) => row.action === "speak").length),
    2,
    "lane removal abandons only that lane's queued entry",
  );
  await touchJoinOrg(page, fixture, secondary.lineage, primary.lineage);
  await waitForOrgOrder(page, "the primary page observes the touch join", [primary.lineage]);
  const inertPips = agents.locator(
    `.organization-surface[data-lineage-id="${primary.lineage}"] .organization-lights`,
  );
  assert.deepEqual(await inertPips.evaluate((node) => ({
    dragHandle: node.hasAttribute("data-lane-drag-handle"),
    hasPopup: node.hasAttribute("aria-haspopup"),
    role: node.getAttribute("role"),
    tabIndex: node.tabIndex,
  })), {
    dragHandle: false,
    hasPopup: false,
    role: "group",
    tabIndex: -1,
  }, "an organization with no eligible peer exposes inert status pips");
  await page.screenshot({
    path: join(artifacts, "canvas-light-organization-arrangement.png"),
    fullPage: true,
  });
}
