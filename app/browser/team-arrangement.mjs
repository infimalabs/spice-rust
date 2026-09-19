/// Prove team lanes are arranged by dragging one team onto another.
///
/// Two gestures share one durable command, and the fixture keeps them apart
/// the way the operator does: where inside the target the team is dropped.
/// A drop in the middle takes the target's place, a drop in either outer
/// fifth gives the target its agents. Both are read back from the resident
/// rather than the DOM, because the predecessor reordered teams with a bare
/// `insertBefore` that no reload survived.
///
/// The third assertion is the one that keeps the gesture honest. A member is
/// dragged by its primary composer too, and that gesture commits `lane::arrange`
/// against the very same lane table. Moving a member inside its own team must
/// therefore leave the teams exactly where this fixture just put them: the two
/// arrangement laws share a table, and neither may stand in for the other.

import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { reply, waitForPageFunction, TRANSITION_MS } from "./canvas-fixture.mjs";

/// The private media a dragged team carries, and the only one a team accepts.
const TEAM_MEDIA = "application/x-spice-team";

/// The one draggable composer a member exposes, by the lane it belongs to.
function primaryComposer(agents, lane) {
  return agents.locator(`.composer-shard--primary[data-shard-target-id="${lane}"]`);
}

/// The members the canvas drew inside one team, in the order it drew them.
async function waitForMemberOrder(page, label, team, order) {
  await waitForPageFunction(page, label, TRANSITION_MS, ({ team, expected }) => {
    const drawn = [...document.querySelectorAll(
      `.team-surface[data-team="${team}"] .team-agent`,
    )].map((member) => member.dataset.agentLane);
    return drawn.length === expected.length
      && drawn.every((lane, at) => lane === expected[at]);
  }, { team, expected: order });
}

/// The teams the resident calls durable, in the order it states them.
function durableTeams(lanes) {
  const teams = [];
  for (const lane of lanes) {
    const team = teams.find((row) => row.team === lane.team);
    if (team) {
      team.lanes.push(lane.lane);
    } else {
      teams.push({ team: lane.team, lanes: [lane.lane] });
    }
  }
  return teams;
}

/// The teams the canvas drew, in the order it drew them.
function renderedTeams(agents) {
  return agents.locator(".team-surface").evaluateAll((surfaces) => surfaces.map((surface) => ({
    team: surface.dataset.team,
    lanes: [...surface.querySelectorAll(".team-agent")].map((agent) => agent.dataset.agentLane),
  })));
}

/// Wait for the canvas to redraw the team order the resident now states.
async function waitForTeamOrder(page, label, order) {
  await waitForPageFunction(page, label, TRANSITION_MS, (expected) => {
    const drawn = [...document.querySelectorAll(".team-surface")]
      .map((surface) => surface.dataset.team);
    return drawn.length === expected.length
      && drawn.every((team, at) => team === expected[at]);
  }, order);
}

/// Drag one team by its pip stack and drop it across the target at `fraction`.
///
/// The pip stack's real `dragstart` fills the transfer, so a handle that carried
/// nothing would fail here rather than be papered over by a transfer the
/// fixture wrote itself.
function dragTeam(page, gesture) {
  return page.evaluate(({ dragged, target, fraction }) => {
    const pips = document.querySelector(
      `.team-surface[data-team="${dragged}"] .team-lights[data-lane-drag-handle]`,
    );
    const surface = document.querySelector(`.team-surface[data-team="${target}"]`);
    if (!pips || !surface) throw new Error("arranging teams needs pips and both teams rendered");
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
      carried: transfer.getData("application/x-spice-team"),
      types: [...transfer.types],
      offered: over.defaultPrevented,
      taken: drop.defaultPrevented,
    };
  }, gesture);
}

function teamPips(agents, team) {
  return agents.locator(
    `.team-surface[data-team="${team}"] .team-lights[data-lane-drag-handle]`,
  );
}

function arrangementMenu(agents, team) {
  return agents.locator(
    `.team-surface[data-team="${team}"] .team-arrangement-menu`,
  );
}

async function assertKeyboardArrangement(page, fixture, agents, primary, secondary) {
  const primaryPips = teamPips(agents, primary.team);
  await primaryPips.focus();
  assert.equal(await primaryPips.evaluate((node) => document.activeElement === node), true,
    "the team pips accept keyboard focus");
  await primaryPips.press("Enter");
  const primaryMenu = arrangementMenu(agents, primary.team);
  await primaryMenu.waitFor();
  assert.equal(await primaryPips.getAttribute("aria-expanded"), "true");
  assert.equal(
    await primaryMenu.locator(`[data-team-arrangement="after"][data-team-arrangement-target="${secondary.team}"]`).count(),
    1,
    "the first team offers its non-noop move-after action",
  );
  assert.equal(
    await primaryMenu.locator(`[data-team-arrangement="join"][data-team-arrangement-target="${secondary.team}"]`).count(),
    1,
    "the keyboard menu offers a join action",
  );
  await agents.locator(
    `.team-surface[data-team="${primary.team}"] .team-mode-rail`,
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

  const secondaryPips = teamPips(agents, secondary.team);
  await secondaryPips.focus();
  await secondaryPips.press("Space");
  const secondaryMenu = arrangementMenu(agents, secondary.team);
  const moveBefore = secondaryMenu.locator(
    `[data-team-arrangement="before"][data-team-arrangement-target="${primary.team}"]`,
  );
  await moveBefore.waitFor();
  await moveBefore.focus();
  await page.keyboard.press("Enter");
  await secondaryMenu.waitFor({ state: "detached" });
  const reordered = [secondary, primary];
  await waitForTeamOrder(page, "keyboard arrangement moves the selected team", [
    secondary.team,
    primary.team,
  ]);
  assert.deepEqual(
    durableTeams((await reply(fixture, ["lane"])).body.lanes),
    reordered,
    "keyboard arrangement is durable resident state",
  );
  return reordered;
}

async function touchJoinTeam(page, fixture, dragged, target) {
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
    const pips = teamPips(agents, dragged);
    await pips.tap();
    assert.equal(await pips.getAttribute("aria-expanded"), "true",
      "a touch tap opens the pip arrangement menu");
    await arrangementMenu(agents, dragged).locator(
      `[data-team-arrangement="join"][data-team-arrangement-target="${target}"]`,
    ).tap();
    await waitForTeamOrder(touch, "touch arrangement joins the selected team", [target]);
    const teams = durableTeams((await reply(fixture, ["lane"])).body.lanes);
    assert.equal(teams.length, 1, "touch arrangement persists one joined team");
    assert.equal(teams[0].team, target, "touch join preserves its target team identity");
    const inertPips = agents.locator(
      `.team-surface[data-team="${target}"] .team-lights`,
    );
    await inertPips.tap();
    assert.equal(await agents.locator(".team-arrangement-menu").count(), 0,
      "touch activation cannot open an unavailable arrangement menu");
  } finally {
    await context.close();
  }
}

/// Dragging a team reorders teams; dropping on an edge joins them.
export async function assertTeamArrangement(page, fixture, worktrees, artifacts) {
  const agents = page.locator('[data-pane="agent-console"]');
  await agents.getByRole("tab", { name: /^Chat/ }).first().click();
  const seated = durableTeams((await reply(fixture, ["lane"])).body.lanes);
  assert.equal(seated.length, 2, "arranging teams needs two durable teams");
  const [primary, secondary] = seated;
  assert.deepEqual(await renderedTeams(agents), seated, "the canvas draws the durable order");
  assert.equal(
    await agents.locator(".team-lights[data-lane-drag-handle]").count(),
    2,
    "every arrangeable team is dragged by its own pip stack",
  );
  assert.equal(
    await agents.locator(".team-drag-handle").count(),
    0,
    "the pip stack replaces the standalone drag symbol",
  );

  const reordered = await assertKeyboardArrangement(
    page, fixture, agents, primary, secondary,
  );

  const reorder = await dragTeam(page, {
    dragged: primary.team,
    target: secondary.team,
    fraction: 0.5,
  });
  assert.deepEqual(reorder.types, [TEAM_MEDIA], "a dragged team carries only its own media");
  assert.equal(reorder.carried, primary.team, "the pips carry the team they belong to");
  assert.deepEqual(
    [reorder.offered, reorder.taken],
    [true, true],
    "a team surface offers itself as a drop target and takes the drop",
  );
  await waitForTeamOrder(page, "mouse drag moves the team back before its target",
    seated.map((team) => team.team));
  assert.deepEqual(
    durableTeams((await reply(fixture, ["lane"])).body.lanes),
    seated,
    "the drag changes the durable order and moves no agent between teams",
  );
  assert.equal(await agents.locator(".team-arrangement-menu").count(), 0,
    "mouse dragging does not accidentally activate the pip menu");
  await dragTeam(page, {
    dragged: secondary.team,
    target: primary.team,
    fraction: 0.5,
  });
  await waitForTeamOrder(page, "mouse drag restores the keyboard arrangement",
    reordered.map((team) => team.team));

  const crowded = reordered.find((team) => team.lanes.length >= 2);
  assert.ok(crowded, "proving the two laws apart needs one team holding two members");
  const [leading, following] = crowded.lanes;
  const moved = [following, leading, ...crowded.lanes.slice(2)];
  await primaryComposer(agents, following).locator(".composer-shard-prose")
    .dragTo(primaryComposer(agents, leading), {
    sourcePosition: { x: 10, y: 10 },
    targetPosition: { x: 10, y: 10 },
  });
  await waitForMemberOrder(page, "the dragged member takes its target's place",
    crowded.team, moved);
  const afterMember = durableTeams((await reply(fixture, ["lane"])).body.lanes);
  assert.deepEqual(
    afterMember.map((team) => team.team),
    reordered.map((team) => team.team),
    "arranging a member never rearranges the teams",
  );
  assert.deepEqual(
    afterMember.map((team) => [...team.lanes].sort()),
    reordered.map((team) => [...team.lanes].sort()),
    "a member dragged inside its own team joins no other team",
  );
  assert.deepEqual(
    afterMember.find((team) => team.team === crowded.team).lanes,
    moved,
    "the member drag is durable, so the two laws wrote the one lane table in turn",
  );
  crowded.lanes = moved;

  const laneRows = (await reply(fixture, ["lane"])).body.lanes;
  const primaryNarrator = laneRows.find((lane) => lane.lane === primary.lanes[0]);
  const secondaryNarrator = laneRows.find((lane) => lane.lane === secondary.lanes[0]);
  assert.ok(primaryNarrator && secondaryNarrator, "both teams expose a live narrator worktree");
  await agents.locator(`.team-surface[data-team="${primary.team}"]`)
    .locator('.team-speech input[type="range"]').fill("2");
  await agents.locator(`.team-surface[data-team="${secondary.team}"]`)
    .locator('.team-speech input[type="range"]').fill("2");
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
  await waitForPageFunction(page, "the surviving team owns narration", TRANSITION_MS,
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
    "both team tails wait behind the committed head",
  );

  const primarySurface = agents.locator(
    `.team-surface[data-team="${primary.team}"]`,
  );
  const secondarySurface = agents.locator(
    `.team-surface[data-team="${secondary.team}"]`,
  );
  const selectedMode = (surface) => surface.locator(".team-pane").getAttribute("data-team-mode");
  await primarySurface.getByRole("tab", { name: /^Work/ }).click();
  await secondarySurface.getByRole("tab", { name: /^Info/ }).click();
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(secondarySurface)],
    ["Work", "Info"],
    "the teams hold distinct views immediately before one team retires",
  );

  const gathered = await dragTeam(page, {
    dragged: secondary.team,
    target: primary.team,
    fraction: 0.1,
  });
  assert.deepEqual(
    [gathered.offered, gathered.taken],
    [true, true],
    "the joining edge takes the drop",
  );
  await waitForTeamOrder(page, "the joined teams draw as one", [primary.team]);
  const joined = await reply(fixture, ["lane"]);
  assert.deepEqual(
    durableTeams(joined.body.lanes),
    [{ team: primary.team, lanes: [...primary.lanes, ...secondary.lanes] }],
    "the dropped team's agents join the target behind its own primary composer",
  );
  assert.deepEqual(
    joined.body.subscriptions.map((subscription) => subscription.team),
    [primary.team],
    "the emptied team keeps no task policy of its own",
  );
  assert.equal(
    await selectedMode(primarySurface),
    "Work",
    "retiring a sibling preserves the surviving team's selected view",
  );
  await primarySurface.getByRole("tab", { name: /^Chat/ }).click();
  assert.deepEqual(
    await renderedTeams(agents),
    [{ team: primary.team, lanes: [...primary.lanes, ...secondary.lanes] }],
    "the canvas draws one team whose first composer never moved",
  );
  await primarySurface.getByRole("tab", { name: /^Work/ }).click();

  await reply(fixture, [
    "lane", "add", worktrees[5],
    "--team", secondary.team,
    "--life", "Drive",
    "--vendor", "claude",
  ]);
  const recreatedSurface = agents.locator(
    `.team-surface[data-team="${secondary.team}"]`,
  );
  await recreatedSurface.waitFor({ timeout: TRANSITION_MS });
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(recreatedSurface)],
    ["Work", "Chat"],
    "a recreated team starts clean instead of reviving its retired view state",
  );
  await page.evaluate(() => {
    const utterance = window.__speechUtterances.at(-1);
    if (!utterance?.onend) throw new Error("the narration head has no completion callback");
    utterance.onend(new Event("end"));
  });
  await waitForPageFunction(page, "only the surviving team's queued narration advances",
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
  await touchJoinTeam(page, fixture, secondary.team, primary.team);
  await waitForTeamOrder(page, "the primary page observes the touch join", [primary.team]);
  const inertPips = agents.locator(
    `.team-surface[data-team="${primary.team}"] .team-lights`,
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
  }, "a team with no eligible peer exposes inert status pips");
  await page.screenshot({
    path: join(artifacts, "canvas-light-team-arrangement.png"),
    fullPage: true,
  });
}
