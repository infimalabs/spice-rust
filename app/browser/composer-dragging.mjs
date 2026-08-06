import assert from "node:assert/strict";

import {
  reply,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

export async function assertPrimaryComposerDragging(
  page,
  fixture,
  console,
  primaryLineage,
  secondaryLineage,
  artifactPath,
) {
  const laneSnapshot = await reply(fixture, ["lane"]);
  const primaryRows = laneSnapshot.body.lanes.filter(
    (lane) => lane.lineage_id === primaryLineage,
  );
  const secondaryRows = laneSnapshot.body.lanes.filter(
    (lane) => lane.lineage_id === secondaryLineage,
  );
  assert.ok(primaryRows.length >= 3, "same-team drag has three durable members to distinguish");
  assert.ok(secondaryRows.length >= 1, "cross-team drag has one durable target member");
  const source = primaryRows[2];
  const sameTeamTarget = primaryRows[0];
  const crossTeamTarget = secondaryRows[0];
  const primarySurface = console.locator(
    `.team-surface[data-lineage-id="${primaryLineage}"]`,
  );
  const secondarySurface = console.locator(
    `.team-surface[data-lineage-id="${secondaryLineage}"]`,
  );
  const primaryComposer = (lane) => console.locator(
    `.composer-shard--primary[data-shard-target-id="${lane}"]`,
  );
  const sourceComposer = primaryComposer(source.lane);
  assert.equal(await sourceComposer.getAttribute("draggable"), "true");
  assert.equal(await sourceComposer.getAttribute("data-primary-composer-draggable"), "true");
  assert.equal(
    await sourceComposer.evaluate((composer) => getComputedStyle(composer).cursor),
    "grab",
    "the primary composer alone exposes the member drag affordance",
  );

  const sameTeamOrder = primaryRows.map((lane) => lane.lane);
  sameTeamOrder.splice(sameTeamOrder.indexOf(source.lane), 1);
  sameTeamOrder.splice(sameTeamOrder.indexOf(sameTeamTarget.lane), 0, source.lane);
  await sourceComposer.dragTo(primaryComposer(sameTeamTarget.lane));
  await waitForPage(
    page,
    "same-team primary composer drag reorders members",
    TRANSITION_MS,
    ({ lineage, expected }) => JSON.stringify([...document.querySelectorAll(
      `.team-surface[data-lineage-id="${lineage}"] .team-agent`,
    )].map((member) => member.dataset.agentLane)) === JSON.stringify(expected),
    { lineage: primaryLineage, expected: sameTeamOrder },
  );
  const sameTeamDurable = await reply(fixture, ["lane"]);
  assert.deepEqual(
    sameTeamDurable.body.lanes
      .filter((lane) => lane.lineage_id === primaryLineage)
      .map((lane) => lane.lane),
    sameTeamOrder,
    "same-team drag changes the resident's durable member order",
  );

  const stableIdentity = {
    lane: source.lane,
    occupant: source.occupant,
    session: source.session,
    worktree: source.worktree,
  };
  const retainedDraft = "draft follows the primary composer across teams";
  await primaryComposer(source.lane).getByRole("textbox").fill(retainedDraft);
  await primaryComposer(source.lane).dragTo(primaryComposer(crossTeamTarget.lane));
  const destinationOrder = [source.lane, ...secondaryRows.map((lane) => lane.lane)];
  await waitForPage(
    page,
    "cross-team primary composer drag transfers the member",
    TRANSITION_MS,
    ({ lineage, sourceLane, expected }) => {
      const destination = document.querySelector(
        `.team-surface[data-lineage-id="${lineage}"]`,
      );
      return destination
        && JSON.stringify([...destination.querySelectorAll(".team-agent")]
          .map((member) => member.dataset.agentLane)) === JSON.stringify(expected)
        && destination.querySelector(
          `.composer-shard--primary[data-shard-target-id="${sourceLane}"]`,
        );
    },
    { lineage: secondaryLineage, sourceLane: source.lane, expected: destinationOrder },
  );
  const crossTeamDurable = await reply(fixture, ["lane"]);
  const transferred = crossTeamDurable.body.lanes.find((lane) => lane.lane === source.lane);
  assert.ok(transferred, "the transferred member keeps its stable lane identity");
  assert.deepEqual(
    {
      lane: transferred.lane,
      occupant: transferred.occupant,
      session: transferred.session,
      worktree: transferred.worktree,
    },
    stableIdentity,
    "cross-team transfer changes lineage and order without replacing the primary composer target",
  );
  assert.equal(transferred.lineage_id, secondaryLineage);
  assert.deepEqual(
    crossTeamDurable.body.lanes
      .filter((lane) => lane.lineage_id === secondaryLineage)
      .map((lane) => lane.lane),
    destinationOrder,
  );
  assert.equal(
    await secondarySurface
      .locator(`.composer-shard--primary[data-shard-target-id="${source.lane}"]`)
      .getByRole("textbox")
      .inputValue(),
    retainedDraft,
    "the lane-keyed primary draft moves with the existing composer",
  );
  assert.equal(
    await primarySurface
      .locator(`.composer-shard--primary[data-shard-target-id="${source.lane}"]`)
      .count(),
    0,
    "the source team no longer renders a duplicate primary composer",
  );

  const cards = console.locator(".mosaic-message-card");
  assert.ok(await cards.count(), "the fixed-card proof exercises rendered messages");
  assert.deepEqual(
    new Set(await cards.evaluateAll((nodes) => nodes.map((node) => node.getAttribute("draggable")))),
    new Set([null]),
    "message cards expose no native drag affordance",
  );
  assert.ok(
    (await cards.evaluateAll((nodes) => nodes.map((node) => getComputedStyle(node).cursor)))
      .every((cursor) => cursor !== "grab" && cursor !== "grabbing"),
    "message-card styling never suggests that cards can be reordered",
  );
  const beforeCardDrag = crossTeamDurable.body.lanes.map((lane) => ({
    lane: lane.lane,
    lineage: lane.lineage_id,
  }));
  await cards.first().dragTo(primaryComposer(crossTeamTarget.lane));
  const afterCardDrag = await reply(fixture, ["lane"]);
  assert.deepEqual(
    afterCardDrag.body.lanes.map((lane) => ({ lane: lane.lane, lineage: lane.lineage_id })),
    beforeCardDrag,
    "dragging a fixed message card cannot reorder or transfer members",
  );
  await page.screenshot({ path: artifactPath, fullPage: true });
}
