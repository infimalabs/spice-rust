// Installed Canvas compatibility behavior at the pre-socket boundary.
import assert from "node:assert/strict";

import {
  CONNECT_MS,
  waitForPageFunction,
} from "./canvas-fixture.mjs";

/// Make bootstrap advertise the next Canvas protocol once.
///
/// A version-aware page that outlives an installed resident must replace its
/// document before it opens a socket. Otherwise the strict decoder would meet
/// a DTO generation it cannot read and the ordinary reconnect loop would
/// repeat the same failure forever.
export async function assertCanvasProtocolRefresh(browser, endpoint) {
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  const page = await context.newPage();
  let bootstraps = 0;
  let sockets = 0;
  page.on("websocket", () => { sockets += 1; });
  await page.route("**/bootstrap", async (route) => {
    const response = await route.fetch();
    const bootstrap = await response.json();
    bootstraps += 1;
    if (bootstraps === 1) bootstrap.canvas += 1;
    await route.fulfill({
      response,
      contentType: "application/json; charset=utf-8",
      body: JSON.stringify(bootstrap),
    });
  });

  await page.goto(endpoint, { waitUntil: "networkidle" });
  await waitForPageFunction(
    page,
    "a stale Canvas application reloads the current root and connects",
    CONNECT_MS,
    () => /cursor \d+/.test(document.querySelector(".bar-meta")?.textContent ?? ""),
  );

  assert.equal(bootstraps, 2, "one foreign bootstrap causes exactly one current-root reload");
  assert.equal(sockets, 1, "the foreign Canvas protocol opens no socket before the reload");
  assert.equal(new URL(page.url()).pathname, "/", "the replacement loads the current app root");
  await context.close();
}
