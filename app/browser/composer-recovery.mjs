import assert from "node:assert/strict";

import { pasteImages } from "./composer-image-indicators.mjs";
import {
  assertOfflineObserved,
  assertOnlineObserved,
  chooseCanvasView,
  reply,
  CONNECT_MS,
  RECONNECT_MS,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

async function unsafeState(page, expected, count, bound = TRANSITION_MS) {
  await waitForPage(
    page,
    `composer unsafe=${expected} submissions=${count}`,
    bound,
    ({ expectedUnsafe, expectedCount }) => {
      const shell = document.querySelector(".shell");
      return shell?.dataset.unsafeIntent === String(expectedUnsafe)
        && Number(shell?.dataset.submissionCount) === expectedCount;
    },
    { expectedUnsafe: expected, expectedCount: count },
    {
      observed: async () => ({
        shellUnsafe: await page.locator(".shell").getAttribute("data-unsafe-intent"),
        submissions: await page.locator(".shell").getAttribute("data-submission-count"),
        draftUnsafe: await page.locator(".composer-shard--primary").first()
          .getAttribute("data-draft-unsafe"),
        draft: await page.locator(".composer-shard--primary").first().getByRole("textbox")
          .inputValue(),
      }),
    },
  );
  const event = await page.evaluate(() => {
    const before = new Event("beforeunload", { cancelable: true });
    const dispatched = window.dispatchEvent(before);
    return { defaultPrevented: before.defaultPrevented, dispatched };
  });
  assert.equal(event.defaultPrevented, expected);
  assert.equal(event.dispatched, !expected);
}

async function inbox(fixture, lane) {
  return (await reply(fixture, ["inbox", lane])).body.lanes[0].pending;
}

async function assertPendingPlaceholder(primary, expected, label) {
  const placeholder = await primary.getByRole("textbox").getAttribute("placeholder");
  assert.ok(placeholder, `${label}: the composer has a placeholder`);
  const pendingLine = placeholder.split("\n").find((line) => line.includes(" pending, "));
  assert.ok(pendingLine, `${label}: the composer placeholder carries pending state`);
  assert.equal(pendingLine.split(", ")[0], `${expected} pending`, label);
  assert.equal(
    await primary.locator(".composer-shard-standing").count(),
    0,
    `${label}: pending state does not mint a separate composer banner`,
  );
}

async function waitForInbox(fixture, lane, text) {
  const deadline = Date.now() + CONNECT_MS;
  while (Date.now() < deadline) {
    const pending = await inbox(fixture, lane);
    const found = pending.find((directive) => directive.text.includes(text));
    if (found) return { found, pending };
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`inbox never received ${JSON.stringify(text)}`);
}

async function installPreDurabilityCut(page, marker) {
  await page.evaluate((match) => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    const nativeSend = socket.send.bind(socket);
    let cut = false;
    const route = (data, forward) => {
      let bytes;
      if (typeof data === "string") {
        bytes = new TextEncoder().encode(data);
      } else if (data instanceof ArrayBuffer) {
        bytes = new Uint8Array(data);
      } else {
        bytes = new Uint8Array(data.buffer, data.byteOffset ?? 0, data.byteLength);
      }
      const command = JSON.parse(new TextDecoder().decode(bytes));
      if (!cut && command.kind === "steer::send" && command.body.message.includes(match)) {
        cut = true;
        socket.close(4000, "cut before steering durability");
        return;
      }
      forward();
    };
    socket.send = (data) => {
      if (data instanceof Blob) {
        void data.arrayBuffer().then((buffer) => route(
          new Uint8Array(buffer),
          () => nativeSend(data),
        ));
        return;
      }
      route(data, () => nativeSend(data));
    };
  }, marker);
}

async function suppressIncoming(page) {
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    socket.addEventListener("message", (event) => event.stopImmediatePropagation(), {
      capture: true,
    });
  });
}

async function reconnectInPlace(page, monitor, reason, socketCount) {
  await waitForPage(
    page,
    reason,
    RECONNECT_MS,
    (before) => {
      const socket = window.__canvasSockets.at(-1);
      return window.__canvasSockets.length > before && socket?.readyState === WebSocket.OPEN;
    },
    socketCount,
  );
  monitor.expectNetworkInterruption(false);
}

/// Cut one command before durability and another after durability.
///
/// The first half queues two complete documents on one lane while its first
/// write is cut. Their pending floor rises immediately, the socket replays the
/// same first identity, and only then may the second write drain. The second
/// half lets the resident commit while swallowing its response, proving the
/// reconnect replay is idempotent rather than a second directive.
export async function assertComposerRecovery(page, fixture, seeded, monitor) {
  await chooseCanvasView(page, "Lanes");
  const lane = page.locator(`.organization-agent[data-agent-lane="${seeded.lane}"]`);
  const sibling = page.locator(`.organization-agent[data-agent-lane="${seeded.quoteLane}"]`);
  const activity = page.locator(
    `.mosaic-message-card[data-agent-lane="${seeded.lane}"]`,
  );
  const primary = lane.locator(".composer-shard--primary");
  const textbox = primary.getByRole("textbox");

  await unsafeState(page, false, 0, CONNECT_MS);
  await textbox.fill("unsafe draft guard witness");
  await unsafeState(page, true, 0);
  await textbox.fill("");
  await unsafeState(page, false, 0);

  const before = await inbox(fixture, seeded.lane);
  const first = "composer recovery before durability";
  const second = "composer recovery queued second";
  await activity
    .first()
    .getByRole("button", { name: "Quote this message" })
    .click();
  await lane.locator(".composer-shard--quote").getByRole("textbox").fill("recovered quote reply");
  await textbox.fill(first);
  await pasteImages(primary, ["recovery-before.png"]);
  await primary.getByRole("img", { name: "recovery-before.png" }).waitFor();
  await unsafeState(page, true, 0);
  await installPreDurabilityCut(page, first);
  monitor.expectNetworkInterruption(true);
  await textbox.press("Control+Enter");
  await textbox.fill(second);
  await textbox.press("Control+Enter");
  await unsafeState(page, true, 2);
  await assertPendingPlaceholder(
    primary,
    before.length + 2,
    "queued submissions are an immediate pending floor",
  );
  await chooseCanvasView(page, "Flow");
  await assertOfflineObserved(page, "the first steering write is cut before durability");
  await assertOnlineObserved(page, "the cut steering socket reconnects", RECONNECT_MS);
  monitor.expectNetworkInterruption(false);
  await chooseCanvasView(page, "Lanes");
  await page.getByRole("tab", { name: /^Threads/ }).first().click();

  const secondInbox = await waitForInbox(fixture, seeded.lane, second);
  const firstDirective = secondInbox.pending.find((directive) => directive.text.includes(first));
  assert.ok(firstDirective, "the first same-lane submission remains ahead of the second result");
  assert.ok(firstDirective.text.includes("recovered quote reply"));
  assert.deepEqual(
    firstDirective.attachments.map((attachment) => attachment.name),
    ["recovery-before.png"],
    "replay retains the detached image",
  );
  assert.equal(secondInbox.pending.filter((directive) => directive.text.includes(first)).length, 1);
  assert.equal(secondInbox.pending.filter((directive) => directive.text.includes(second)).length, 1);
  await unsafeState(page, false, 0);
  await assertPendingPlaceholder(
    primary,
    secondInbox.pending.length,
    "settled predictions render exactly the backend pending count",
  );

  for (const directive of [firstDirective, secondInbox.found]) {
    await reply(fixture, ["steer", "cancel", directive.directive]);
  }
  assert.equal(
    await page.locator('[data-card-kind="operator-message"]').count(),
    0,
    "recovered and canceled submissions never render standalone operator cards",
  );
  for (const text of [first, second]) {
    assert.equal(
      await page.locator(".mosaic-message-card").filter({ hasText: text }).count(),
      0,
      "recovered unacknowledged operator prose remains outside the conversation",
    );
  }

  const durable = "composer recovery after durability";
  await suppressIncoming(page);
  await textbox.fill(durable);
  await textbox.press("Control+Enter");
  await unsafeState(page, true, 1);
  const committed = await waitForInbox(fixture, seeded.lane, durable);
  const siblingTextbox = sibling.locator(".composer-shard--primary").getByRole("textbox");
  await siblingTextbox.focus();
  const siblingTarget = await siblingTextbox.getAttribute("data-primary-composer-target-id");
  const socketCount = await page.evaluate(() => window.__canvasSockets.length);
  monitor.expectNetworkInterruption(true);
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    socket.close(4000, "cut after steering durability");
  });
  await reconnectInPlace(
    page,
    monitor,
    "the durable steering response is withheld and replayed in place",
    socketCount,
  );
  const recovered = await waitForInbox(fixture, seeded.lane, durable);
  assert.equal(
    recovered.pending.filter((directive) => directive.text.includes(durable)).length,
    1,
    "replaying the command identity returns its result without duplicating direction",
  );
  assert.equal(recovered.found.directive, committed.found.directive);
  await unsafeState(page, false, 0);
  await assertPendingPlaceholder(
    primary,
    recovered.pending.length,
    "idempotent replay leaves one exact backend-derived pending count",
  );
  assert.equal(
    await page.evaluate(() => document.activeElement?.dataset.primaryComposerTargetId),
    siblingTarget,
    "late completion never steals focus from the composer's new target",
  );
  await reply(fixture, ["steer", "cancel", recovered.found.directive]);
}
