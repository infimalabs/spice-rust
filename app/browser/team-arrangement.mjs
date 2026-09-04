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
async function waitForMemberOrder(page, label, lineage, order) {
  await waitForPageFunction(page, label, TRANSITION_MS, ({ lineage, expected }) => {
    const drawn = [...document.querySelectorAll(
      `.team-surface[data-lineage-id="${lineage}"] .team-agent`,
    )].map((member) => member.dataset.agentLane);
    return drawn.length === expected.length
      && drawn.every((lane, at) => lane === expected[at]);
  }, { lineage, expected: order });
}

/// The teams the resident calls durable, in the order it states them.
function durableTeams(lanes) {
  const teams = [];
  for (const lane of lanes) {
    const team = teams.find((row) => row.lineage === lane.lineage_id);
    if (team) {
      team.lanes.push(lane.lane);
    } else {
      teams.push({ lineage: lane.lineage_id, lanes: [lane.lane] });
    }
  }
  return teams;
}

/// The teams the canvas drew, in the order it drew them.
function renderedTeams(agents) {
  return agents.locator(".team-surface").evaluateAll((surfaces) => surfaces.map((surface) => ({
    lineage: surface.dataset.lineageId,
    lanes: [...surface.querySelectorAll(".team-agent")].map((agent) => agent.dataset.agentLane),
  })));
}

/// Wait for the canvas to redraw the team order the resident now states.
async function waitForTeamOrder(page, label, order) {
  await waitForPageFunction(page, label, TRANSITION_MS, (expected) => {
    const drawn = [...document.querySelectorAll(".team-surface")]
      .map((surface) => surface.dataset.lineageId);
    return drawn.length === expected.length
      && drawn.every((lineage, at) => lineage === expected[at]);
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
      `.team-surface[data-lineage-id="${dragged}"] .team-lights[data-lane-drag-handle]`,
    );
    const surface = document.querySelector(`.team-surface[data-lineage-id="${target}"]`);
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

/// Dragging a team reorders teams; dropping on an edge joins them.
export async function assertTeamArrangement(page, fixture, worktrees, artifacts) {
  const agents = page.locator('[data-pane="agent-console"]');
  await agents.getByRole("tab", { name: /^Compose/ }).first().click();
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

  const reorder = await dragTeam(page, {
    dragged: secondary.lineage,
    target: primary.lineage,
    fraction: 0.5,
  });
  assert.deepEqual(reorder.types, [TEAM_MEDIA], "a dragged team carries only its own media");
  assert.equal(reorder.carried, secondary.lineage, "the pips carry the team they belong to");
  assert.deepEqual(
    [reorder.offered, reorder.taken],
    [true, true],
    "a team surface offers itself as a drop target and takes the drop",
  );
  const reordered = [secondary, primary];
  await waitForTeamOrder(page, "the dragged team takes its target's place",
    reordered.map((team) => team.lineage));
  assert.deepEqual(
    durableTeams((await reply(fixture, ["lane"])).body.lanes),
    reordered,
    "the drag changes the durable order and moves no agent between teams",
  );

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
    crowded.lineage, moved);
  const afterMember = durableTeams((await reply(fixture, ["lane"])).body.lanes);
  assert.deepEqual(
    afterMember.map((team) => team.lineage),
    reordered.map((team) => team.lineage),
    "arranging a member never rearranges the teams",
  );
  assert.deepEqual(
    afterMember.map((team) => [...team.lanes].sort()),
    reordered.map((team) => [...team.lanes].sort()),
    "a member dragged inside its own team joins no other team",
  );
  assert.deepEqual(
    afterMember.find((team) => team.lineage === crowded.lineage).lanes,
    moved,
    "the member drag is durable, so the two laws wrote the one lane table in turn",
  );
  crowded.lanes = moved;

  const laneRows = (await reply(fixture, ["lane"])).body.lanes;
  const primaryNarrator = laneRows.find((lane) => lane.lane === primary.lanes[0]);
  const secondaryNarrator = laneRows.find((lane) => lane.lane === secondary.lanes[0]);
  assert.ok(primaryNarrator && secondaryNarrator, "both teams expose a live narrator worktree");
  await agents.locator(`.team-surface[data-lineage-id="${primary.lineage}"]`)
    .locator('.team-speech input[type="range"]').fill("2");
  await agents.locator(`.team-surface[data-lineage-id="${secondary.lineage}"]`)
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
    "both lineage tails wait behind the committed head",
  );

  const primarySurface = agents.locator(
    `.team-surface[data-lineage-id="${primary.lineage}"]`,
  );
  const secondarySurface = agents.locator(
    `.team-surface[data-lineage-id="${secondary.lineage}"]`,
  );
  const selectedMode = (surface) => surface.locator(".team-pane").getAttribute("data-team-mode");
  await primarySurface.getByRole("tab", { name: /^Tasks/ }).click();
  await secondarySurface.getByRole("tab", { name: /^Metrics/ }).click();
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(secondarySurface)],
    ["Tasks", "Metrics"],
    "the teams hold distinct views immediately before one lineage retires",
  );

  const gathered = await dragTeam(page, {
    dragged: secondary.lineage,
    target: primary.lineage,
    fraction: 0.1,
  });
  assert.deepEqual(
    [gathered.offered, gathered.taken],
    [true, true],
    "the joining edge takes the drop",
  );
  await waitForTeamOrder(page, "the joined teams draw as one", [primary.lineage]);
  const joined = await reply(fixture, ["lane"]);
  assert.deepEqual(
    durableTeams(joined.body.lanes),
    [{ lineage: primary.lineage, lanes: [...primary.lanes, ...secondary.lanes] }],
    "the dropped team's agents join the target behind its own primary composer",
  );
  assert.deepEqual(
    joined.body.subscriptions.map((subscription) => subscription.lineage_id),
    [primary.lineage],
    "the emptied team keeps no task policy of its own",
  );
  assert.equal(
    await selectedMode(primarySurface),
    "Tasks",
    "retiring a sibling preserves the surviving team's selected view",
  );
  await primarySurface.getByRole("tab", { name: /^Compose/ }).click();
  assert.deepEqual(
    await renderedTeams(agents),
    [{ lineage: primary.lineage, lanes: [...primary.lanes, ...secondary.lanes] }],
    "the canvas draws one team whose first composer never moved",
  );
  await primarySurface.getByRole("tab", { name: /^Tasks/ }).click();

  await reply(fixture, [
    "lane", "add", worktrees[5],
    "--lineage", secondary.lineage,
    "--life", "Drive",
    "--vendor", "claude",
  ]);
  const recreatedSurface = agents.locator(
    `.team-surface[data-lineage-id="${secondary.lineage}"]`,
  );
  await recreatedSurface.waitFor({ timeout: TRANSITION_MS });
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(recreatedSurface)],
    ["Tasks", "Compose"],
    "a recreated lineage starts clean instead of reviving its retired view state",
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
  await page.screenshot({
    path: join(artifacts, "canvas-light-team-arrangement.png"),
    fullPage: true,
  });
}
