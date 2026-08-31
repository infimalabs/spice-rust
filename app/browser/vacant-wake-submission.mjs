import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import {
  chooseCanvasView,
  reply,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

const WAKE_MS = 15_000;

/// Drive the exact bound-vacant composer path through a fresh resident witness.
///
/// The lane is drained outside the browser so the test begins with the state an
/// operator encounters after an agent exits. The browser then crosses its
/// visible refresh boundary before using the discoverable team action. Pending
/// directions are cleared first so lifecycle auto-wake cannot consume the
/// vacancy under test. The durable inbox proves steering without exposing the
/// operator-authored direction as a card, while a new agent-authored card proves
/// the wake launched a replacement occupant rather than sending against the
/// departed one.
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
        && member.querySelector("textarea")?.placeholder.includes("vacant")
        && !member.querySelector(".composer-shard-standing");
    },
    targetLane,
    {
      observed: () => page.evaluate(() =>
        [...document.querySelectorAll(".team-agent")].map((member) => ({
          lane: member.dataset.agentLane,
          wake: member.dataset.wakeAndSend,
          classes: member.className,
          placeholder: member.querySelector("textarea")?.placeholder,
          standingBanners: member.querySelectorAll(".composer-shard-standing").length,
        }))),
    },
  );

  const assertVacantLayout = async (label) => {
    const geometry = await form.evaluate((node, laneId) => {
      const rect = (element) => {
        const box = element.getBoundingClientRect();
        return {
          top: box.top,
          bottom: box.bottom,
          width: box.width,
          height: box.height,
        };
      };
      const agent = node.querySelector(`.team-agent[data-agent-lane="${laneId}"]`);
      const stack = agent.querySelector(".agent-composer-stack");
      const primary = stack.querySelector(".composer-shard--primary");
      const controls = node.querySelector(".team-composer-controls");
      return {
        bodyWidth: document.body.scrollWidth,
        viewportWidth: innerWidth,
        form: rect(node),
        agent: rect(agent),
        stack: rect(stack),
        primary: rect(primary),
        controls: rect(controls),
        placeholder: primary.querySelector("textarea")?.placeholder,
        standingBanners: agent.querySelectorAll(".composer-shard-standing").length,
      };
    }, targetLane);
    assert.equal(
      geometry.standingBanners,
      0,
      `${label}: vacancy renders no separate state banner`,
    );
    assert.ok(
      geometry.placeholder.split("\n").includes("0 pending, vacant"),
      `${label}: the placeholder communicates pending and vacant state: ${JSON.stringify(geometry)}`,
    );
    assert.ok(
      Math.abs(geometry.agent.top - geometry.controls.top) < 1 &&
        Math.abs(geometry.agent.bottom - geometry.controls.bottom) < 1 &&
        Math.abs(geometry.agent.height - geometry.controls.height) < 1,
      `${label}: composer and controls close on the same frame: ${JSON.stringify(geometry)}`,
    );
    assert.ok(
      Math.abs(geometry.stack.height - geometry.agent.height) < 1 &&
        Math.abs(geometry.primary.height - geometry.agent.height) < 1 &&
        geometry.primary.top >= geometry.stack.top - 1 &&
        geometry.primary.bottom <= geometry.stack.bottom + 1 &&
        geometry.controls.bottom <= geometry.form.bottom + 1,
      `${label}: the composer stack stays contained without shift or overlap: ${JSON.stringify(geometry)}`,
    );
    assert.ok(
      geometry.bodyWidth <= geometry.viewportWidth,
      `${label}: composer geometry does not widen the document: ${JSON.stringify(geometry)}`,
    );
  };

  await assertVacantLayout("desktop vacant composer");
  const viewport = page.viewportSize();
  assert.ok(viewport, "the vacant composer fixture has a viewport to restore");
  try {
    await page.setViewportSize({ width: 520, height: viewport.height });
    await lane.scrollIntoViewIfNeeded();
    await assertVacantLayout("narrow vacant composer");
  } finally {
    await page.setViewportSize(viewport);
  }

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
    vacantChronology.headerAuthored === undefined || vacantChronology.clock === "deadline",
    `a source-authored vacant heading keeps the shared ticking clock: ${
      JSON.stringify(vacantChronology)
    }`,
  );

  const exactDraft = `wake and steer exact draft ${crypto.randomUUID()}`;
  await textbox.fill(exactDraft);
  const steer = form.getByRole("button", { name: "Steer", exact: true });
  assert.equal(await steer.isEnabled(), true, "a nonempty vacant draft enables Steer");
  assert.equal(
    await steer.getAttribute("title"),
    "Send with Steer: Use operator-selected work only",
    "the vacant action keeps the established Steer title",
  );
  const composerActions = await form.locator(".team-send").evaluateAll((buttons) =>
    buttons.map((button) => ({
      text: button.textContent.trim(),
      ariaLabel: button.getAttribute("aria-label"),
      title: button.getAttribute("title"),
    })));
  for (const action of composerActions) {
    assert.ok(
      ["Steer", "Drive", "Drain"].includes(action.text),
      `composer action text uses an established label: ${JSON.stringify(action)}`,
    );
    assert.ok(
      action.ariaLabel === null || action.ariaLabel === action.text,
      `composer action accessible names use the same label: ${JSON.stringify(action)}`,
    );
    assert.match(
      action.title,
      new RegExp(`^Send with ${action.text}: `),
      `composer action titles begin with the same established label: ${JSON.stringify(action)}`,
    );
  }
  await page.evaluate(({ laneId, text }) => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    window.__wakeDirectionFact = new Promise((resolve) => {
      socket.addEventListener("message", async (event) => {
        const bytes = typeof event.data === "string"
          ? event.data
          : event.data instanceof Blob
            ? await event.data.text()
            : new TextDecoder().decode(event.data);
        const frame = JSON.parse(bytes);
        if (frame.stream === "fact"
          && frame.fact?.fact?.kind === "steer::sent"
          && frame.fact.fact.lane === laneId
          && frame.fact.fact.text === text) {
          resolve(frame.fact.fact);
        }
      });
    });
  }, { laneId: targetLane, text: exactDraft });
  await steer.click();

  const sent = await page.evaluate((timeout) => Promise.race([
    window.__wakeDirectionFact,
    new Promise((_, reject) => setTimeout(
      () => reject(new Error("wake direction fact did not arrive")),
      timeout,
    )),
  ]), WAKE_MS);
  const durableInbox = await reply(fixture, ["inbox", targetLane]);
  const durable = durableInbox.body.lanes[0].pending.find(
    (candidate) => candidate.directive === sent.directive,
  );
  assert.ok(durable, "vacant Steer durably preserves the exact authored draft");
  assert.equal(durable.text, exactDraft, "the durable direction retains the submitted text");
  const directive = durable.directive;
  assert.equal(
    await console_.locator('[data-card-kind="operator-message"]').count(),
    0,
    "the live client renders no standalone operator card",
  );
  assert.equal(
    await console_.locator(".mosaic-message-card").filter({ hasText: exactDraft }).count(),
    0,
    "the unacknowledged wake direction remains outside the conversation",
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

  await reply(fixture, ["steer", "cancel", directive]);
  assert.equal(await console_.locator('[data-card-kind="operator-message"]').count(), 0);
  assert.equal(
    await console_.locator(".mosaic-message-card").filter({ hasText: exactDraft }).count(),
    0,
    "cancellation does not publish operator prose",
  );
  const reloaded = await page.context().newPage();
  try {
    await reloaded.goto(page.url(), { waitUntil: "networkidle" });
    await reloaded.reload({ waitUntil: "networkidle" });
    await chooseCanvasView(reloaded, "Agents");
    const reloadedConsole = reloaded.locator('[data-pane="agent-console"]');
    await reloadedConsole.getByRole("tab", { name: /^Compose/ }).click();
    await reloadedConsole.locator(`.team-agent[data-agent-lane="${targetLane}"]`).waitFor({
      timeout: WAKE_MS,
    });
    assert.equal(
      await reloadedConsole.locator('[data-card-kind="operator-message"]').count(),
      0,
      "a fresh client restores no standalone operator card",
    );
    assert.equal(
      await reloadedConsole.locator(".mosaic-message-card").filter({ hasText: exactDraft }).count(),
      0,
      "reload keeps the canceled unacknowledged direction invisible",
    );
  } finally {
    await reloaded.close();
  }
}
