import assert from "node:assert/strict";

import {
  chooseCanvasView,
  monitorPage,
  reply,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

function decode(message) {
  return JSON.parse(typeof message === "string" ? message : message.toString("utf8"));
}

async function waitForObservation(label, predicate) {
  const deadline = Date.now() + TRANSITION_MS;
  while (Date.now() < deadline) {
    if (predicate()) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw new Error(`timed out waiting for ${label}`);
}

/// Prove a two-fact commit refreshes one established causal pane once.
export async function assertLiveThreadCoalescing(browser, endpoint, fixture, parentTask) {
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  const page = await context.newPage();
  const monitor = monitorPage(page);
  const liveFacts = [];
  const threadRequests = [];
  await page.routeWebSocket(/\/socket\?/, (socket) => {
    const server = socket.connectToServer();
    socket.onMessage((message) => {
      const request = decode(message);
      if (request.stream === "thread") threadRequests.push(request);
      server.send(message);
    });
    server.onMessage((message) => {
      const frame = decode(message);
      if (frame.stream === "fact") liveFacts.push(frame.fact);
      socket.send(message);
    });
  });

  await page.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(page, "Topology");
  await page.locator(".status.online").waitFor({ timeout: TRANSITION_MS });
  const factFloor = liveFacts.length;
  const requestFloor = threadRequests.length;

  await reply(fixture, [
    "task",
    "oops",
    "Live thread coalescing browser witness",
    "--origin",
    `task:${parentTask}`,
  ]);
  await waitForObservation(
    "both live facts from the one commit",
    () => liveFacts.length >= factFloor + 2,
  );
  const burst = liveFacts.slice(factFloor);
  assert.deepEqual(
    burst.slice(0, 2).map((fact) => fact.fact.kind),
    ["task::added", "task::noted"],
    "the browser observes the causal two-fact commit",
  );
  const newest = Math.max(...burst.map((fact) => Number(fact.seq)));
  await waitForObservation(
    "one newest causal-thread request",
    () => threadRequests.slice(requestFloor).some((request) => Number(request.cursor.seq) === newest),
  );
  // Let a hypothetical per-fact timer clear before counting. The production
  // coalescing bound is 75 ms, so this covers more than two complete windows.
  await new Promise((resolve) => setTimeout(resolve, 200));
  const refreshes = threadRequests.slice(requestFloor);
  assert.equal(refreshes.length, 1, "one fact burst emits one thread request");
  assert.equal(Number(refreshes[0].cursor.seq), newest, "the request names the newest live fact");

  const thread = page.locator('[data-pane="lineage"]');
  await waitForPage(page, "the live causal pane is repopulated", TRANSITION_MS, (expected) => {
    const pane = document.querySelector('[data-pane="lineage"]');
    const first = pane?.querySelector("tbody tr td");
    const terminal = pane?.querySelector(".section-count");
    return first?.textContent === String(expected) && terminal?.textContent === "root";
  }, newest, {
    observed: async () => ({
      terminal: await thread.locator(".section-count").innerText(),
      rows: await thread.locator("tbody tr").allTextContents(),
    }),
  });
  monitor.assertClean();
  await context.close();
}
