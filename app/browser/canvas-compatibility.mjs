// Installed Canvas compatibility behavior at the pre-socket boundary.
import assert from "node:assert/strict";

import {
  CONNECT_MS,
  assertOfflineObserved,
  chooseCanvasView,
  waitForPageFunction,
} from "./canvas-fixture.mjs";
import { monitorPage } from "./canvas-monitor.mjs";

const CANVAS_VERSION_HEADER = "x-spice-canvas-version";

async function assertPriorProtocolReplacement(browser, endpoint) {
  const auth = "canvas-prior-token";
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  const page = await context.newPage();
  const monitor = monitorPage(page);
  let bootstraps = 0;
  let sockets = 0;
  const bootstrapAuth = [];
  let admitTopologyFrame;
  const topologyFrame = new Promise((resolve) => { admitTopologyFrame = resolve; });
  page.on("websocket", (socket) => {
    sockets += 1;
    socket.on("framereceived", ({ payload }) => {
      let frame;
      try {
        frame = JSON.parse(typeof payload === "string" ? payload : payload.toString("utf8"));
      } catch {
        return;
      }
      const body = frame?.outcome?.kind === "reply" ? frame.outcome.body : undefined;
      if (Number.isSafeInteger(body?.topology_revision) && body.topology_revision >= 0) {
        admitTopologyFrame(body.topology_revision);
      }
    });
  });
  await page.route(/\/bootstrap(?:\?.*)?$/, async (route) => {
    const response = await route.fetch();
    const headers = response.headers();
    const body = await response.body();
    let fulfilledBody = body;
    const bootstrap = JSON.parse(body.toString());
    assert.equal(
      Object.hasOwn(bootstrap, "canvas"),
      false,
      "the version header does not leave a second clock in the strict body",
    );
    bootstrapAuth.push(new URL(route.request().url()).searchParams.get("auth"));
    bootstraps += 1;
    if (bootstraps === 1) {
      fulfilledBody = JSON.stringify({ future: "bootstrap-shape" });
      assert.equal(
        headers[CANVAS_VERSION_HEADER],
        "4",
        "the current application advertises the requirement-aware Canvas protocol",
      );
      headers[CANVAS_VERSION_HEADER] = "3";
    }
    await route.fulfill({
      response,
      headers,
      body: fulfilledBody,
    });
  });

  await page.goto(`${endpoint}#${auth}`, { waitUntil: "networkidle" });
  await waitForPageFunction(
    page,
    "a Canvas application seeing a prior version reloads and connects",
    CONNECT_MS,
    () => /cursor \d+/.test(document.querySelector(".bar-meta")?.textContent ?? ""),
  );
  let topologyDeadline;
  const observedTopologyRevision = await Promise.race([
    topologyFrame,
    new Promise((_, reject) => {
      topologyDeadline = setTimeout(
        () => reject(new Error("the replacement document received no topology-bearing lane frame")),
        CONNECT_MS,
      );
    }),
  ]).finally(() => clearTimeout(topologyDeadline));
  await waitForPageFunction(
    page,
    `the replacement document admits topology revision ${observedTopologyRevision}`,
    CONNECT_MS,
    (revision) => document.querySelector(".shell")?.dataset.topologyRevision === String(revision),
    observedTopologyRevision,
  );

  assert.equal(bootstraps, 2, "one prior header causes exactly one current-root reload");
  assert.equal(sockets, 1, "the prior header opens no socket before the reload");
  assert.ok(
    Number.isSafeInteger(observedTopologyRevision),
    "the replacement document admits the current topology-bearing lane answer",
  );
  assert.equal(new URL(page.url()).pathname, "/", "the replacement loads the current app root");
  assert.equal(new URL(page.url()).hash, `#${auth}`, "the replacement retains Canvas authentication");
  assert.deepEqual(
    bootstrapAuth,
    [auth, auth],
    "both the stale and reloaded applications authenticate bootstrap discovery",
  );
  monitor.assertClean();
  await context.close();
}

async function assertMalformedProtocolStopsBeforeSocket(browser, endpoint) {
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  const page = await context.newPage();
  const monitor = monitorPage(page);
  let sockets = 0;
  let navigations = 0;
  page.on("websocket", () => { sockets += 1; });
  page.on("framenavigated", (frame) => {
    if (frame === page.mainFrame()) navigations += 1;
  });
  await page.route(/\/bootstrap(?:\?.*)?$/, async (route) => {
    const response = await route.fetch();
    const headers = response.headers();
    headers[CANVAS_VERSION_HEADER] = "not-a-version";
    await route.fulfill({ response, headers, body: "{not-json" });
  });

  await page.goto(endpoint, { waitUntil: "domcontentloaded" });
  await waitForPageFunction(
    page,
    "the app mounts before malformed Canvas version metadata is inspected",
    CONNECT_MS,
    () => document.querySelector(".canvas-menu") !== null,
  );
  await chooseCanvasView(page, "Flow");
  await assertOfflineObserved(page, "malformed Canvas version metadata");
  assert.match(
    await page.locator(".status-copy").innerText(),
    /bootstrap Canvas version header was invalid/,
  );
  assert.equal(sockets, 0, "malformed version metadata opens no socket");
  assert.equal(navigations, 1, "malformed version metadata does not enter the reload path");
  monitor.assertClean();
  await context.close();
}

/// Exercise the only pre-socket Canvas clock before strict body decoding.
///
/// A page seeing a prior clock replaces its document without opening a socket.
/// Malformed metadata is not a historical generation and is therefore reported
/// instead of being silently mapped onto the reload path. The Rust transport
/// boundary covers the absent and other non-current clock values that take the
/// same replacement branch.
export async function assertCanvasProtocolRefresh(browser, endpoint) {
  await assertPriorProtocolReplacement(browser, endpoint);
  await assertMalformedProtocolStopsBeforeSocket(browser, endpoint);
}
