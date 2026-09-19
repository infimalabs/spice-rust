// The acknowledgement half of the external operator replay proof: a spoken
// ACK renders as awaiting retirement, the journal retirement credits it, and
// the Agents pip agrees with the card in the same render at both moments.
import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import {
  chooseCanvasView,
  reply,
  replyAsLane,
  streamFixtureAssistantMessage,
  CONNECT_MS,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

export async function assertOperatorProseHidden(console, text, label) {
  assert.equal(
    await console.locator('[data-card-kind="operator-message"]').count(),
    0,
    `${label}: Canvas renders no standalone operator cards`,
  );
  assert.equal(
    await console.locator('.mosaic-message-card').filter({ hasText: text }).count(),
    0,
    `${label}: unacknowledged operator prose is absent from the conversation`,
  );
}

// One DOM read of the assistant card and the lane pip, so the two figures the
// operator compares come from the same render rather than from two moments.
function readAcknowledgementRender(page, lane, replyText) {
  return page.evaluate(({ lane, replyText }) => {
    const card = [...document.querySelectorAll(
      `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
    )].find((candidate) => candidate.textContent?.includes(replyText));
    const pip = document.querySelector(`.team-light[data-lane-light-target-id="${lane}"]`);
    return {
      acknowledged: card?.dataset.messageAckCount ?? null,
      pending: card?.dataset.messagePendingCount ?? null,
      status: pip?.dataset.laneStatus ?? null,
    };
  }, { lane, replyText });
}

// The badge's own box and where the quoted words start beside it, so the
// pending and settled renders can be compared as geometry rather than prose.
function readAcknowledgementMetric(card) {
  return card.locator(".message-ack-pill").evaluate((pill) => {
    const round = (value) => Math.round(value * 100) / 100;
    const box = pill.getBoundingClientRect();
    const range = document.createRange();
    range.selectNodeContents(pill.parentElement);
    range.setStartAfter(pill);
    const prose = range.getBoundingClientRect();
    return {
      state: pill.dataset.acknowledgement,
      label: pill.textContent,
      width: round(box.width),
      height: round(box.height),
      proseLeft: round(prose.left - box.left),
      proseTop: round(prose.top - box.top),
    };
  });
}

export async function assertExternalOperatorReplay(page, fixture, console, lane, worktree) {
  const quotedText = `external operator premise ${crypto.randomUUID()}`;
  const replyText = `approved ${crypto.randomUUID()}`;
  const text = `> ${quotedText}\n\n${replyText}`;
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    window.__externalOperatorFacts = [];
    socket.addEventListener("message", async (event) => {
      const text = typeof event.data === "string"
        ? event.data
        : event.data instanceof Blob
          ? await event.data.text()
          : new TextDecoder().decode(event.data);
      const frame = JSON.parse(text);
      if (frame.stream === "fact" && frame.fact?.fact?.kind === "steer::sent") {
        window.__externalOperatorFacts.push(frame.fact);
      }
    });
  });
  const sent = await reply(fixture, ["steer", lane, text]);
  await waitForPage(page, "external steer fact reaches Canvas", TRANSITION_MS, (directive) => (
    window.__externalOperatorFacts.some((fact) => fact.fact.directive === directive)
  ), sent.body.directive, {
    observed: () => page.evaluate(() => window.__externalOperatorFacts),
  });
  const streamed = await page.evaluate((directive) => (
    window.__externalOperatorFacts.find((fact) => fact.fact.directive === directive)
  ), sent.body.directive);
  assert.equal(streamed.actor, "operator", `external steer retains operator authorship: ${JSON.stringify(streamed)}`);
  assert.equal(streamed.fact.lane, lane, "external steer fact retains its target lane");
  await assertOperatorProseHidden(console, replyText, "external live steering");

  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Lanes");
  await console.getByRole("tab", { name: /^Agents/ }).click();
  await assertOperatorProseHidden(console, replyText, "pre-acknowledgement replay");

  await writeFile(join(worktree, ".spice-ack-trigger"), `${sent.body.key}\n`);
  const acknowledgedCard = console.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
  ).filter({ hasText: replyText });
  await waitForPage(page, "spoken acknowledgement reaches its assistant card as pending", CONNECT_MS,
    ({ lane, replyText }) => [...document.querySelectorAll(
      `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
    )].some((card) => (
      card.textContent?.includes(replyText)
        && card.dataset.messageRole === "agent"
        && card.dataset.messageAckCount === "0"
        && card.dataset.messagePendingCount === "1"
        && card.dataset.messageTasteCount === "0"
        && [...card.querySelectorAll("blockquote > p > .message-ack-pill")]
          .map((badge) => `${badge.dataset.acknowledgement}:${badge.textContent}`)
          .join(",") === "pending:ACK"
    )), { lane, replyText });
  assert.equal(await acknowledgedCard.count(), 1, "a spoken acknowledgement renders one assistant card");
  assert.deepEqual(
    await acknowledgedCard.locator(".message-ack-pill").allTextContents(),
    ["ACK"],
    "a key the journal still holds live renders one badge and no credit",
  );
  const pendingMetric = await readAcknowledgementMetric(acknowledgedCard);
  assert.equal(pendingMetric.state, "pending", "the held key reads as pending");
  assert.doesNotMatch(
    await acknowledgedCard.locator(".message-body").innerText(),
    /\[ACK PENDING\]/u,
    "the pending marker is presentation-only",
  );
  const pendingInbox = await reply(fixture, ["inbox", lane]);
  assert.ok(
    pendingInbox.body.lanes[0].pending.some((row) => row.directive === sent.body.directive),
    "the journal still holds the spoken key live",
  );
  assert.deepEqual(
    await readAcknowledgementRender(page, lane, replyText),
    { acknowledged: "0", pending: "1", status: "queued" },
    "the Agents pip reads queued in the same render that shows the card awaiting retirement",
  );
  await waitForPage(page, "pending acknowledgement Mosaic settles", TRANSITION_MS, () => {
    const element = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return element?.dataset.measurementPhase === "ready"
      && element.dataset.measurementPending === "0";
  });
  const acknowledged = await replyAsLane(worktree, lane, ["ack", sent.body.key]);
  await waitForPage(page, "external acknowledgement reaches Canvas", TRANSITION_MS, (cursor) => (
    document.querySelector(".bar-meta")?.textContent === cursor
  ), `cursor ${acknowledged.seq.last}`);
  await waitForPage(page, "journal retirement flips the card to acknowledged", TRANSITION_MS,
    ({ lane, replyText }) => [...document.querySelectorAll(
      `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
    )].some((card) => (
      card.textContent?.includes(replyText)
        && card.dataset.messageRole === "agent"
        && card.dataset.messageAckCount === "1"
        && card.dataset.messagePendingCount === "0"
        && card.dataset.messageTasteCount === "0"
        && [...card.querySelectorAll("blockquote > p > .message-ack-pill")]
          .map((badge) => badge.textContent).join(",") === "ACK"
    )), { lane, replyText });
  const retiredInbox = await reply(fixture, ["inbox", lane]);
  assert.deepEqual(
    retiredInbox.body.lanes[0].pending.map((row) => row.directive),
    [],
    "steer::acked retires the only live key on the lane",
  );
  assert.equal(await acknowledgedCard.count(), 1, "acknowledgement renders one assistant card");
  assert.equal(await acknowledgedCard.getAttribute("data-message-role"), "agent");
  assert.equal(await acknowledgedCard.getAttribute("data-message-ack-count"), "1");
  assert.equal(await acknowledgedCard.getAttribute("data-message-pending-count"), "0");
  assert.notEqual(
    (await readAcknowledgementRender(page, lane, replyText)).status,
    "queued",
    "the Agents pip leaves queued in the same render that credits the card",
  );
  assert.equal(await acknowledgedCard.getAttribute("data-message-taste-count"), "0");
  assert.equal(await acknowledgedCard.getAttribute("data-message-review-count"), "0");
  assert.deepEqual(
    await acknowledgedCard.locator(".message-ack-pill").allTextContents(),
    ["ACK"],
    "authenticated acknowledgement evidence renders one uncounted inline ACK pill",
  );
  const settledMetric = await readAcknowledgementMetric(acknowledgedCard);
  assert.equal(settledMetric.state, "acked", "retirement credits the same badge");
  assert.deepEqual(
    { ...settledMetric, state: "pending" },
    pendingMetric,
    "settling a spoken acknowledgement moves neither the badge nor the words beside it",
  );
  assert.equal(
    await acknowledgedCard.locator(".message-classifiers, .message-ack-pill .message-badge-count").count(),
    0,
    "the ACK pill carries no detached header and no count",
  );
  const outerQuote = acknowledgedCard.locator("blockquote").first();
  assert.equal(
    await acknowledgedCard.locator(".message-ack-pill").evaluate((element) => {
      let depth = 0;
      for (let node = element.parentElement; node; node = node.parentElement) {
        if (node.tagName === "BLOCKQUOTE") depth += 1;
      }
      return depth;
    }),
    1,
    "the ACK pill nests inside the acknowledged context quote like TASTE, REVIEW, and BROADCAST",
  );
  assert.equal(
    await outerQuote.evaluate((quote) => (
      quote.firstElementChild?.matches("p:has(> .message-ack-pill:first-child)")
    )),
    true,
    "the ACK pill heads the acknowledged quote above the nested operator quote",
  );
  assert.doesNotMatch(
    await acknowledgedCard.locator(".message-body").innerText(),
    /\[ACK\]/u,
    "the authenticated ACK marker is presentation-only",
  );
  assert.equal(
    (await outerQuote.locator("blockquote").innerText()).trim(),
    quotedText,
    "the acknowledged operator quote remains nested",
  );
  assert.match(
    (await outerQuote.innerText()).trim(),
    new RegExp(`${replyText.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`),
    "the operator reply follows its nested quote",
  );
  assert.doesNotMatch(
    (await acknowledgedCard.locator(".message-body").innerText()).trim(),
    new RegExp(`(?:ACK|NACK)\\s+${sent.body.key}`),
    "recognized protocol framing is absent from visible prose",
  );
  assert.match(
    (await acknowledgedCard.locator(".message-body").innerText()).trim(),
    /\nApplied$/,
    "the assistant response follows its acknowledgement quote",
  );
  assert.equal(await console.locator('[data-card-kind="operator-message"]').count(), 0);
  await waitForPage(page, "external acknowledgement Mosaic settles", TRANSITION_MS, () => {
    const element = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return element?.dataset.measurementPhase === "ready"
      && element.dataset.measurementPending === "0";
  });
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Lanes");
  await console.getByRole("tab", { name: /^Agents/ }).click();
  await acknowledgedCard.waitFor({ timeout: CONNECT_MS });
  assert.equal(await acknowledgedCard.count(), 1, "bootstrap restores one acknowledged assistant card");
  assert.deepEqual(
    await acknowledgedCard.locator("blockquote > p > .message-ack-pill").allTextContents(),
    ["ACK"],
    "durable replay restores the inline ACK pill inside its quote",
  );
  assert.equal(
    await acknowledgedCard.locator('.message-ack-pill[data-acknowledgement="pending"]').count(),
    0,
    "durable replay reads the retired key from the journal and never re-pends it",
  );
  assert.equal(
    (await acknowledgedCard.locator("blockquote blockquote").innerText()).trim(),
    quotedText,
    "durable replay restores the nested operator quote",
  );
  assert.doesNotMatch(
    (await acknowledgedCard.locator(".message-body").innerText()).trim(),
    new RegExp(`(?:ACK|NACK)\\s+${sent.body.key}`),
    "durable replay keeps protocol framing hidden",
  );
  assert.equal(await console.locator('[data-card-kind="operator-message"]').count(), 0);
  const repeatedText = `Repeated response ${crypto.randomUUID()}`;
  await streamFixtureAssistantMessage(worktree, {
    type: "message",
    role: "assistant",
    content: [{ type: "text", text: `ACK ${sent.body.key}: ${repeatedText}` }],
  });
  const repeatedCard = console.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
  ).filter({ hasText: repeatedText });
  await repeatedCard.waitFor({ timeout: CONNECT_MS });
  const assertRepeatedHighlight = async (label) => {
    assert.deepEqual(
      await repeatedCard.locator("blockquote > p > .message-ack-pill").allTextContents(),
      ["ACK"],
      `${label}: a response to a retired key retains the context highlight`,
    );
    assert.equal(
      (await repeatedCard.locator("blockquote blockquote").innerText()).trim(),
      quotedText,
      `${label}: the repeated response quotes the same operator directive`,
    );
    assert.doesNotMatch(
      await repeatedCard.locator(".message-body").innerText(),
      new RegExp(`ACK\\s+${sent.body.key}`),
      `${label}: the retired key does not leak raw protocol framing`,
    );
  };
  await assertRepeatedHighlight("live");
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Lanes");
  await console.getByRole("tab", { name: /^Agents/ }).click();
  await repeatedCard.waitFor({ timeout: CONNECT_MS });
  await assertRepeatedHighlight("history");
  return repeatedText;
}
