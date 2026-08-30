import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

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
/// operator encounters after an agent exits. The browser then crosses its
/// visible refresh boundary before using the discoverable team action. Pending
/// directions are cleared first so lifecycle auto-wake cannot consume the
/// vacancy under test. The resulting operator card proves durable steering,
/// while a new agent-authored card proves the wake launched a replacement
/// occupant rather than sending against the departed one.
export async function assertVacantWakeSubmission(page, fixture, seeded) {
  const targetLane = seeded.lane;
  await chooseCanvasView(page, "Agents");
  const console_ = page.locator('[data-pane="agent-console"]');
  await console_.getByRole("tab", { name: /^Compose/ }).click();
  const lane = console_.locator(`.team-agent[data-agent-lane="${targetLane}"]`);
  const form = lane.locator("xpath=ancestor::form[1]");
  const textbox = lane.locator(".composer-shard--primary").getByRole("textbox");
  const agentCards = console_.locator(
    `.mosaic-message-card[data-card-kind="agent-message"]`
      + `[data-agent-lane="${targetLane}"]`,
  );
  const chronologyBeforeDrain = await lane.locator(".composer-shard-age").evaluate((age) => ({
    href: age.getAttribute("href"),
    authored: age.querySelector("[data-age-authored]")?.dataset.ageAuthored,
  }));
  const before = await reply(fixture, ["lane"]);
  const departed = before.body.lanes.find((candidate) => candidate.lane === targetLane);
  assert.ok(departed?.occupant, "the wake scenario starts from an occupied bound lane");
  assert.equal(departed.bound, true, "the lane retains resumable wake authority");

  const pendingBeforeDrain = await reply(fixture, ["inbox", targetLane]);
  for (const directive of pendingBeforeDrain.body.lanes[0].pending) {
    await reply(fixture, ["steer", "cancel", directive.directive]);
  }
  const quietInbox = await reply(fixture, ["inbox", targetLane]);
  assert.deepEqual(
    quietInbox.body.lanes[0].pending,
    [],
    "pending direction cannot automatically consume the vacancy under test",
  );
  await reply(fixture, ["lane", "drain", targetLane]);
  await chooseCanvasView(page, "Chronology");
  await page.getByRole("button", { name: "Refresh", exact: true }).click();
  await chooseCanvasView(page, "Agents");
  await waitForPage(
    page,
    "drained bound lane exposes wake-and-send",
    WAKE_MS,
    (laneId) => {
      const member = document.querySelector(`.team-agent[data-agent-lane="${laneId}"]`);
      return member?.dataset.wakeAndSend === "true"
        && member.querySelector(".composer-shard-standing")?.textContent?.includes("vacant");
    },
    targetLane,
    {
      observed: () => page.evaluate(() =>
        [...document.querySelectorAll(".team-agent")].map((member) => ({
          lane: member.dataset.agentLane,
          wake: member.dataset.wakeAndSend,
          classes: member.className,
          standing: member.querySelector(".composer-shard-standing")?.textContent,
        }))),
    },
  );

  const vacantChronology = await lane.locator(".composer-shard-age").evaluate((age) => {
    const href = age.getAttribute("href");
    const card = href?.startsWith("#") ? document.getElementById(href.slice(1)) : null;
    const headingAge = age.querySelector(".composer-shard-age-label");
    return {
      href,
      status: age.getAttribute("data-lane-status"),
      headerAuthored: headingAge?.dataset.ageAuthored,
      cardAuthored: card?.querySelector("[data-age-authored]")?.dataset.ageAuthored,
      cardKind: card?.dataset.cardKind,
      clock: headingAge?.dataset.ageClock,
    };
  });
  assert.equal(vacantChronology.status, "bound", "the retained chronology is read while vacant");
  assert.deepEqual(
    { href: vacantChronology.href, authored: vacantChronology.headerAuthored },
    chronologyBeforeDrain,
    "draining the lane cannot replace the selected message or timestamp",
  );
  assert.equal(
    vacantChronology.cardKind,
    "agent-message",
    `the vacant composer still links to its latest prose card: ${
      JSON.stringify(vacantChronology)
    }`,
  );
  assert.equal(
    vacantChronology.headerAuthored,
    vacantChronology.cardAuthored,
    `vacancy does not replace the linked message timestamp: ${JSON.stringify(vacantChronology)}`,
  );
  assert.ok(
    vacantChronology.headerAuthored === undefined || vacantChronology.clock === "shared",
    `a source-authored vacant heading keeps the shared ticking clock: ${
      JSON.stringify(vacantChronology)
    }`,
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
      `.mosaic-message-card[data-card-kind="operator-message"]`
        + `[data-agent-lane="${targetLane}"]`,
    )
    .filter({ hasText: exactDraft });
  try {
    await directiveCard.waitFor({ timeout: WAKE_MS });
  } catch (error) {
    const operatorCards = console_.locator('[data-card-kind="operator-message"]');
    const observed = {
      lane: await reply(fixture, ["lane"]),
      inbox: await reply(fixture, ["inbox", targetLane]),
      member: await lane.evaluate((node) => ({
        classes: node.className,
        text: node.textContent,
        wake: node.dataset.wakeAndSend,
      })),
      operatorCardCount: await operatorCards.count(),
      recentOperatorCards: await operatorCards.evaluateAll((cards) =>
        cards.slice(-8).map((card) => ({
          directive: card.dataset.directive,
          id: card.id,
          lane: card.dataset.agentLane,
          phase: card.dataset.layoutPhase,
          state: card.dataset.state,
          visible: card.getClientRects().length > 0,
        })),
      ),
    };
    throw new Error(`wake direction did not render: ${JSON.stringify(observed)}`, {
      cause: error,
    });
  }
  assert.equal(await directiveCard.count(), 1, "the exact wake draft renders once");
  const directive = await directiveCard.getAttribute("data-directive");
  assert.ok(directive, "the wake path renders the durable directive identity");
  assert.equal(
    await directiveCard.getAttribute("id"),
    `message-operator-${directive}`,
    "the Operator card identity derives from the durable directive",
  );

  const after = await reply(fixture, ["lane"]);
  const resident = after.body.lanes.find((candidate) => candidate.lane === targetLane);
  assert.ok(resident?.occupant, "the lane has an occupant after the directive is observed");
  assert.notEqual(
    resident.occupant,
    departed.occupant,
    "wake establishes a replacement residency instead of reusing the stale occupant",
  );
  const postDirectiveCardKeys = await agentCards.evaluateAll((cards) =>
    cards.map((card) => card.dataset.mosaicCardKey));
  await writeFile(join(resident.worktree, ".spice-prose-trigger"), "emit\n");
  await waitForPage(
    page,
    "woken occupant contributes post-directive prose to the mosaic",
    WAKE_MS,
    ({ laneId, prior }) => [...document.querySelectorAll(
      `.mosaic-message-card[data-card-kind="agent-message"]`
        + `[data-agent-lane="${laneId}"]`,
    )].some((card) => !prior.includes(card.dataset.mosaicCardKey)
      && card.textContent?.includes("newest lane prose witness")),
    { laneId: targetLane, prior: postDirectiveCardKeys },
  );

  const inbox = await reply(fixture, ["inbox", targetLane]);
  const durable = inbox.body.lanes[0].pending.find((candidate) => candidate.directive === directive);
  assert.equal(durable?.text, exactDraft, "wake-and-steer preserves the exact authored draft");

  await reply(fixture, ["steer", "cancel", directive]);
  await directiveCard.waitFor({ timeout: TRANSITION_MS });
  const reloaded = await page.context().newPage();
  try {
    await reloaded.goto(page.url(), { waitUntil: "networkidle" });
    await reloaded.reload({ waitUntil: "networkidle" });
    await chooseCanvasView(reloaded, "Agents");
    const reloadedCard = reloaded
      .locator('[data-pane="agent-console"]')
      .locator(
        `.mosaic-message-card[data-card-kind="operator-message"]`
          + `[data-agent-lane="${targetLane}"]`
          + `[data-directive="${directive}"]`,
      )
      .filter({ hasText: exactDraft });
    await reloadedCard.waitFor({ timeout: WAKE_MS });
    assert.equal(await reloadedCard.count(), 1, "reload restores the exact wake draft once");
    assert.equal(
      await reloadedCard.getAttribute("id"),
      `message-operator-${directive}`,
      "reload preserves the Operator card's directive identity",
    );
  } finally {
    await reloaded.close();
  }
}
