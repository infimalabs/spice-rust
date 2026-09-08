import assert from "node:assert/strict";
import { EventEmitter, once } from "node:events";
import { createServer, request as requestHttp } from "node:http";
import { performance } from "node:perf_hooks";

import {
  chooseCanvasView,
  describeConnection,
  monitorPage,
} from "./canvas-fixture.mjs";
import { HANDSHAKE_TIMEOUT_MS, RECONNECT_BASE_MS } from "./client-deadlines.mjs";

// The rendered reason quotes the client's deadline back, so this pattern is the
// one assertion here that fails if the app and its own constant disagree.
const renderedHandshakeReason = new RegExp(
  `canvas connection handshake timed out after ${HANDSHAKE_TIMEOUT_MS} ms`,
);

const dispatchToleranceMilliseconds = 250;
const renderedToleranceMilliseconds = 2_000;
// This watchdog only rejects an absent callback; it never advances the fixture.
const eventBoundMilliseconds = 10_000;

/// Prove one stalled bootstrap is aborted before the Canvas retries it.
///
/// The server is the page's origin and proxies every production asset to the
/// resident. Only `/bootstrap` is retained without headers or a body, so the
/// production Fetch itself stalls rather than a Playwright route promise.
export async function assertHandshakeTimeout(browser, residentEndpoint) {
  const events = new EventEmitter();
  const attempts = [];
  const state = { open: 0, maximumOpen: 0, paths: [] };
  const upstream = new URL(residentEndpoint);
  const server = createServer((request, response) => {
    const path = new URL(request.url ?? "/", upstream).pathname;
    state.paths.push(request.url ?? "/");
    if (path !== "/bootstrap") {
      proxyRequest(request, response, upstream);
      return;
    }

    request.resume();
    const attempt = {
      index: attempts.length,
      opened: performance.now(),
      closed: null,
    };
    attempts.push(attempt);
    state.open += 1;
    state.maximumOpen = Math.max(state.maximumOpen, state.open);
    response.once("close", () => {
      attempt.closed = performance.now();
      state.open -= 1;
      events.emit(`close:${attempt.index}`, attempt);
    });
    events.emit(`request:${attempt.index}`, attempt);
  });

  await listen(server);
  const address = server.address();
  assert.ok(address && typeof address === "object", "stall server has a TCP address");
  const endpoint = new URL(upstream.pathname, `http://127.0.0.1:${address.port}`);
  endpoint.search = upstream.search;
  endpoint.hash = upstream.hash;

  let context;
  try {
    const firstRequest = observedEvent(events, "request:0");
    context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
    const page = await context.newPage();
    const pageRequests = [];
    const pageProblems = [];
    page.on("request", (request) => pageRequests.push(request.url()));
    page.on("pageerror", (error) => pageProblems.push(error.stack ?? String(error)));
    page.on("console", (message) => {
      if (message.type() === "error" || message.type() === "warning") {
        pageProblems.push(`${message.type()}: ${message.text()}`);
      }
    });
    const monitor = monitorPage(page);
    monitor.expectNetworkInterruption(true);
    await page.goto(endpoint.toString(), { waitUntil: "domcontentloaded" });
    const first = await firstRequest.catch(async (error) => {
      const connection = await describeConnection(page);
      throw new Error(
        `stall origin saw ${JSON.stringify(state.paths)}; page requested ${JSON.stringify(pageRequests)}; problems=${JSON.stringify(pageProblems)}; connection=${JSON.stringify(connection)}`,
        { cause: error },
      );
    });
    const firstClosed = observedEvent(events, "close:0");
    const secondRequest = observedEvent(events, "request:1");
    const status = page.locator(".status.offline");
    // Two handshakes wide, not one. `Offline` is not a resting state the page
    // settles into here -- `run_surface` sets it, sleeps the backoff, and clears
    // it to `Connecting` -- so this waits on a window exactly as wide as the
    // client's first delay. Everywhere else that window is effectively open,
    // because a retry against a stopped resident is refused instantly and the
    // next `Offline` follows it within a millisecond. Only here does the retry
    // *stall*, holding `Connecting` for a whole handshake deadline and leaving
    // that delay as a narrow island in a much longer cycle.
    //
    // One handshake plus slack covered that island when the delay was flat and
    // wide. Against a ladder starting at `RECONNECT_BASE_MS` it is the shortest
    // window the client will ever render, and a bound this size reaches the next
    // one only if the setup above happened to be slow -- so a fast machine got a
    // single chance at it and a loaded one missed it. Spanning the second island
    // makes the wait independent of how long the setup took.
    //
    // Arm before optional UI navigation so a healthy run observes the first
    // island instead of routinely paying for the second handshake. The wider
    // production-derived bound remains the fallback when rendering starts late.
    const statusOutcome = status
      .waitFor({
        timeout:
          2 * HANDSHAKE_TIMEOUT_MS + RECONNECT_BASE_MS + renderedToleranceMilliseconds,
      })
      .then(async () => {
        const [renderedLabel, described] = await Promise.all([
          status.locator(".status-copy").innerText(),
          describeConnection(page),
        ]);
        return { observation: { renderedLabel, described } };
      })
      .catch((error) => ({ error }));
    await page.getByLabel("Open Spice Canvas menu").waitFor();
    await chooseCanvasView(page, "Flow");
    const [closed, second] = await Promise.all([firstClosed, secondRequest]);
    assert.ok(closed.closed <= second.opened, "the first Fetch closes before its retry begins");
    assert.equal(state.maximumOpen, 1, "retry never accumulates stalled bootstrap requests");
    assert.ok(
      closed.closed - first.opened >=
        HANDSHAKE_TIMEOUT_MS - dispatchToleranceMilliseconds,
      "the abort was not an inverted or prematurely short handshake deadline",
    );
    // The base and not the ceiling: this is the first attempt to end, so the
    // client's ladder is still on its bottom rung. A later retry in the same
    // outage would owe more than this, which is why the bound names the rung it
    // means rather than the delay in general.
    assert.ok(
      second.opened - first.opened >=
        HANDSHAKE_TIMEOUT_MS +
          RECONNECT_BASE_MS -
          dispatchToleranceMilliseconds,
      "the retry includes the production reconnect interval after the deadline",
    );
    const statusResult = await statusOutcome;
    if ("error" in statusResult) throw statusResult.error;
    const { renderedLabel, described } = statusResult.observation;
    assert.match(
      renderedLabel,
      renderedHandshakeReason,
      "rendered Offline state names the bounded handshake",
    );
    assert.equal(described.rendered, true, "describeConnection sees the rendered status");
    assert.match(described.classes, /\boffline\b/, "describeConnection sees Offline");
    assert.match(
      described.label,
      renderedHandshakeReason,
      "describeConnection carries the same timeout reason",
    );

    monitor.assertClean();
  } finally {
    if (context) await context.close();
    await closeServer(server);
  }
}

function proxyRequest(request, response, upstream) {
  const target = new URL(request.url ?? "/", upstream);
  const headers = { ...request.headers, host: target.host };
  // The resident enforces same-origin request headers. The browser still sees
  // one stall-server origin; only this server-to-resident hop is rewritten.
  if (headers.origin) headers.origin = upstream.origin;
  if (headers.referer) headers.referer = target.toString();
  const proxy = requestHttp(target, { method: request.method, headers }, (proxied) => {
    response.writeHead(proxied.statusCode ?? 502, proxied.headers);
    proxied.pipe(response);
  });
  proxy.on("error", (error) => response.destroy(error));
  response.on("close", () => proxy.destroy());
  request.pipe(proxy);
}

function observedEvent(events, name) {
  return once(events, name, { signal: AbortSignal.timeout(eventBoundMilliseconds) })
    .then(([value]) => value)
    .catch((error) => {
      throw new Error(`timed out waiting for stall-server callback ${name}`, { cause: error });
    });
}

function listen(server) {
  return new Promise((resolve, reject) => {
    const failed = (error) => reject(error);
    server.once("error", failed);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", failed);
      resolve();
    });
  });
}

function closeServer(server) {
  return new Promise((resolve, reject) => {
    server.close((error) => {
      if (error) reject(error);
      else resolve();
    });
    server.closeAllConnections();
  });
}
