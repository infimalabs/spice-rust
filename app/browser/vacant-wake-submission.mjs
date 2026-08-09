import assert from "node:assert/strict";

import {
  chooseCanvasView,
  reply,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

const WAKE_MS = 15_000;

/// Drive the exact bound-vacant composer path through a fresh resident witness.
///
/// The lane is drained outside the browser so the test begins with the state an
/// operator encounters after an agent exits. The only browser gesture after
/// that is the discoverable team action: its resulting directive card proves
/// durable steering, while a new agent-authored card proves the wake launched a
/// replacement occupant rather than sending against the departed one.
export async function assertVacantWakeSubmission(page, fixture, seeded) {
  await chooseCanvasView(page, "Agents");
  const console_ = page.locator('[data-pane="agent-console"]');
  await console_.getByRole("tab", { name: /^Compose/ }).click();
  const lane = console_.locator(`.team-agent[data-agent-lane="${seeded.lane}"]`);
  const form = lane.locator("xpath=ancestor::form[1]");
  const textbox = lane.locator(".composer-shard--primary").getByRole("textbox");
  const agentCards = console_.locator(
    `.mosaic-message-card[data-card-kind="agent-message"]`
      + `[data-agent-lane="${seeded.lane}"]`,
  );
  const priorCardKeys = await agentCards.evaluateAll((cards) =>
    cards.map((card) => card.dataset.mosaicCardKey));
  const before = await reply(fixture, ["lane"]);
  const departed = before.body.lanes.find((candidate) => candidate.lane === seeded.lane);
  assert.ok(departed?.occupant, "the wake scenario starts from an occupied bound lane");
  assert.equal(departed.bound, true, "the lane retains resumable wake authority");

  await reply(fixture, ["lane", "drain", seeded.lane]);
  await waitForPage(
    page,
    "drained bound lane exposes wake-and-send",
    WAKE_MS,
    (laneId) => {
      const member = document.querySelector(`.team-agent[data-agent-lane="${laneId}"]`);
      return member?.dataset.wakeAndSend === "true"
        && member.querySelector(".composer-shard-standing")?.textContent?.includes("vacant");
    },
    seeded.lane,
  );

  const exactDraft = `wake and steer exact draft ${crypto.randomUUID()}`;
  await textbox.fill(exactDraft);
  const wakeAndSteer = form.getByRole("button", { name: "Wake & Steer", exact: true });
  assert.equal(await wakeAndSteer.isEnabled(), true, "a nonempty vacant draft enables wake");
  assert.match(
    await wakeAndSteer.getAttribute("title"),
    /confirm fresh residency/i,
    "the action explains that steering follows a fresh residency check",
  );
  await wakeAndSteer.click();

  const directiveCard = console_
    .locator(
      `.mosaic-message-card[data-card-kind="directive"]`
        + `[data-agent-lane="${seeded.lane}"]`,
    )
    .filter({ hasText: exactDraft });
  await directiveCard.waitFor({ timeout: WAKE_MS });
  const directive = await directiveCard.getAttribute("data-directive");
  assert.ok(directive, "the wake path renders the durable directive identity");

  await waitForPage(
    page,
    "woken occupant contributes a new mosaic message card",
    WAKE_MS,
    ({ laneId, prior }) => [...document.querySelectorAll(
      `.mosaic-message-card[data-card-kind="agent-message"]`
        + `[data-agent-lane="${laneId}"]`,
    )].some((card) => !prior.includes(card.dataset.mosaicCardKey)),
    { laneId: seeded.lane, prior: priorCardKeys },
  );

  const after = await reply(fixture, ["lane"]);
  const resident = after.body.lanes.find((candidate) => candidate.lane === seeded.lane);
  assert.ok(resident?.occupant, "the lane has an occupant before the directive is observed");
  assert.notEqual(
    resident.occupant,
    departed.occupant,
    "wake establishes a replacement residency instead of reusing the stale occupant",
  );
  const inbox = await reply(fixture, ["inbox", seeded.lane]);
  const durable = inbox.body.lanes[0].pending.find((candidate) => candidate.directive === directive);
  assert.equal(durable?.text, exactDraft, "wake-and-steer preserves the exact authored draft");

  await reply(fixture, ["steer", "cancel", directive]);
  await directiveCard.waitFor({ state: "detached", timeout: TRANSITION_MS });
}
