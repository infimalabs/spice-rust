/// Prove team lanes are arranged by dragging one team onto another.
///
/// Two gestures share one durable command, and the fixture keeps them apart
/// the way the operator does: where inside the target the team is dropped.
/// A drop in the middle takes the target's place, a drop in either outer
/// fifth gives the target its agents. Both are read back from the resident
/// rather than the DOM, because the predecessor reordered teams with a bare
/// `insertBefore` that no reload survived.
///
/// The third assertion is the one that keeps the gesture honest. Message
/// cards are draggable already and carry their prose as `text/plain`, so the
/// same drop that steers a composer must leave the arrangement alone. It is
/// dragged here through the card's own handler, never a fabricated transfer.

import assert from "node:assert/strict";
import { join } from "node:path";

import { reply, waitForPageFunction, TRANSITION_MS } from "./canvas-fixture.mjs";

/// The private media a dragged team carries, and the only one a team accepts.
const TEAM_MEDIA = "application/x-spice-team";

/// The steering cards one member has been given.
function directiveCards(lane) {
  return `[data-card-kind="directive"][data-agent-lane="${lane}"]`;
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

/// Drag one team by its own grip and drop it across the target at `fraction`.
///
/// The grip's real `dragstart` fills the transfer, so a handle that carried
/// nothing would fail here rather than be papered over by a transfer the
/// fixture wrote itself.
function dragTeam(page, gesture) {
  return page.evaluate(({ dragged, target, fraction }) => {
    const grip = document.querySelector(
      `.team-surface[data-lineage-id="${dragged}"] .team-drag-handle`,
    );
    const surface = document.querySelector(`.team-surface[data-lineage-id="${target}"]`);
    if (!grip || !surface) throw new Error("arranging teams needs both teams rendered");
    const transfer = new DataTransfer();
    grip.dispatchEvent(new DragEvent("dragstart", {
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

/// Drag one message card onto one member composer, the way quoting works.
function dragCard(page, lane) {
  return page.evaluate((target) => {
    const card = document.querySelector(".mosaic-message-card[draggable=\"true\"]");
    const member = document.querySelector(`.team-agent[data-agent-lane="${target}"]`);
    if (!card || !member) throw new Error("quoting a card needs a card and a member");
    const transfer = new DataTransfer();
    card.dispatchEvent(new DragEvent("dragstart", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
    }));
    const rect = member.getBoundingClientRect();
    const at = { clientX: Math.round(rect.left + 1), clientY: Math.round(rect.top + 1) };
    const over = new DragEvent("dragover", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
      ...at,
    });
    member.dispatchEvent(over);
    member.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
      ...at,
    }));
    return {
      types: [...transfer.types],
      text: transfer.getData("text/plain"),
      offered: over.defaultPrevented,
    };
  }, lane);
}

/// Dragging a team reorders teams; dropping on an edge joins them.
export async function assertTeamArrangement(page, fixture, artifacts) {
  const agents = page.locator('[data-pane="agent-console"]');
  await agents.getByRole("tab", { name: /^Compose/ }).first().click();
  const seated = durableTeams((await reply(fixture, ["lane"])).body.lanes);
  assert.equal(seated.length, 2, "arranging teams needs two durable teams");
  const [primary, secondary] = seated;
  assert.deepEqual(await renderedTeams(agents), seated, "the canvas draws the durable order");
  assert.equal(
    await agents.locator(".team-drag-handle").count(),
    2,
    "every arrangeable team is dragged by its own grip",
  );

  const reorder = await dragTeam(page, {
    dragged: secondary.lineage,
    target: primary.lineage,
    fraction: 0.5,
  });
  assert.deepEqual(reorder.types, [TEAM_MEDIA], "a dragged team carries only its own media");
  assert.equal(reorder.carried, secondary.lineage, "the grip carries the team it belongs to");
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

  const host = primary.lanes[0];
  const before = await agents.locator(directiveCards(host)).count();
  const quoted = await dragCard(page, host);
  assert.equal(quoted.types.includes(TEAM_MEDIA), false, "a message card carries no team");
  assert.equal(quoted.offered, true, "a member still offers itself to a dragged card");
  const [firstLine] = quoted.text.split("\n");
  assert.ok(firstLine, "the dragged card carries its own prose");
  await waitForPageFunction(page, "the quoted card reaches the member", TRANSITION_MS,
    ({ selector, seen }) => document.querySelectorAll(selector).length > seen,
    { selector: directiveCards(host), seen: before });
  const steered = await reply(fixture, ["inbox", host]);
  assert.ok(
    steered.body.lanes[0].pending.some((directive) => directive.text.includes(`> ${firstLine}`)),
    "dropping a card on a member still steers it while the teams stay put",
  );
  assert.deepEqual(
    durableTeams((await reply(fixture, ["lane"])).body.lanes),
    reordered,
    "composer dragging never rearranges teams",
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
  assert.deepEqual(
    await renderedTeams(agents),
    [{ lineage: primary.lineage, lanes: [...primary.lanes, ...secondary.lanes] }],
    "the canvas draws one team whose first composer never moved",
  );
  await page.screenshot({
    path: join(artifacts, "canvas-light-team-arrangement.png"),
    fullPage: true,
  });
}
