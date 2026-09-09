// Explicit Canvas updates at the pre-socket boundary.
import assert from "node:assert/strict";

import {
  CONNECT_MS,
  assertOfflineObserved,
  chooseCanvasView,
  waitForPageFunction,
} from "./canvas-fixture.mjs";
import { monitorPage } from "./canvas-monitor.mjs";
import { installSocketControl } from "./canvas-speech.mjs";

const CANVAS_GENERATION_HEADER = "x-spice-canvas-generation";
const CANVAS_VERSION_HEADER = "x-spice-canvas-version";

async function closeCurrentSocket(page, reason) {
  await page.evaluate(async (message) => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("Canvas update proof has no socket to close");
    await socket.__fixtureSubscriptionReady;
    socket.close(4000, message);
  }, reason);
}

async function assertExplicitCanvasUpdate(browser, endpoint) {
  const auth = "canvas-prior-token";
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installSocketControl(context);
  const page = await context.newPage();
  const monitor = monitorPage(page);
  let bootstrapMode = "current";
  let bootstraps = 0;
  let sockets = 0;
  let navigations = 0;
  let currentGeneration;
  let currentVersion;
  let alternateGeneration;
  page.on("websocket", () => { sockets += 1; });
  page.on("framenavigated", (frame) => {
    if (frame === page.mainFrame()) navigations += 1;
  });
  await page.route(/\/bootstrap(?:\?.*)?$/, async (route) => {
    const response = await route.fetch();
    const headers = response.headers();
    const body = await response.body();
    currentGeneration ??= headers[CANVAS_GENERATION_HEADER];
    currentVersion ??= headers[CANVAS_VERSION_HEADER];
    alternateGeneration ??= currentGeneration === "1".repeat(64) ? "2".repeat(64) : "1".repeat(64);
    let fulfilledBody = body;
    assert.equal(
      Object.hasOwn(JSON.parse(body.toString()), "canvas"),
      false,
      "the update headers leave the strict bootstrap body unchanged",
    );
    bootstraps += 1;
    if (bootstrapMode === "generation") {
      headers[CANVAS_GENERATION_HEADER] = alternateGeneration;
    } else if (bootstrapMode === "protocol") {
      headers[CANVAS_VERSION_HEADER] = String(Number(currentVersion) - 1);
      fulfilledBody = JSON.stringify({ future: "bootstrap-shape" });
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
    "the current Canvas application connects before an update is offered",
    CONNECT_MS,
    () => window.__canvasSockets.length === 1
      && window.__canvasSockets[0].readyState === WebSocket.OPEN
      && /cursor \d+/.test(document.querySelector(".canvas-menu-cursor")?.textContent ?? ""),
  );
  assert.equal(
    await page.locator("#app").getAttribute("data-canvas-generation"),
    currentGeneration,
    "the loaded shell names the generation its bootstrap serves",
  );
  await chooseCanvasView(page, "Flow");
  const menu = page.locator(".canvas-menu");
  await menu.locator(":scope > summary").click();
  assert.notEqual(await menu.getAttribute("open"), null, "the update proof starts with the menu open");

  bootstrapMode = "generation";
  await closeCurrentSocket(page, "observe rebuilt Canvas generation");
  await waitForPageFunction(
    page,
    "a same-protocol rebuild reconnects and publishes its update",
    CONNECT_MS,
    () => window.__canvasSockets.length === 2
      && window.__canvasSockets.at(-1).readyState === WebSocket.OPEN
      && document.querySelector(".canvas-menu-trigger--update") !== null,
  );
  assert.equal(navigations, 1, "discovering a rebuilt Canvas does not navigate");
  assert.equal((await page.locator(".page-heading h1").innerText()).trim(), "Tasks");
  assert.notEqual(await menu.getAttribute("open"), null, "the pending update preserves menu state");
  const trigger = menu.locator(":scope > summary");
  const update = menu.getByRole("button", { name: `Reload Canvas v${currentVersion}` });
  assert.equal(await update.getAttribute("data-canvas-update-generation"), alternateGeneration);
  assert.match(await update.innerText(), new RegExp(alternateGeneration.slice(0, 8)));
  assert.match(await trigger.getAttribute("title"), new RegExp(`Canvas v${currentVersion} update available`));

  const fastMode = menu.getByRole("menuitemcheckbox", { name: /Fast mode/ });
  const initiallyFast = await fastMode.getAttribute("aria-checked") === "true";
  if (!initiallyFast) await fastMode.click();
  await waitForPageFunction(
    page,
    "Fast mode and update state coexist on the trigger",
    CONNECT_MS,
    () => document.querySelector(".canvas-menu-trigger--fast.canvas-menu-trigger--update") !== null,
  );
  const precedence = await trigger.evaluate((node) => ({
    accent: getComputedStyle(node).getPropertyValue("--canvas-menu-accent").trim(),
    focus: getComputedStyle(document.documentElement).getPropertyValue("--focus").trim(),
  }));
  assert.ok(precedence.focus.length > 0, "the palette exposes its blue focus token");
  assert.equal(precedence.accent, precedence.focus, "update blue takes precedence over Fast gold");
  if (!initiallyFast) await fastMode.click();

  await page.setViewportSize({ width: 332, height: 720 });
  const narrow = await menu.locator(".canvas-menu-footer").evaluate((footer) => {
    const button = footer.querySelector(".canvas-menu-update").getBoundingClientRect();
    const bounds = footer.getBoundingClientRect();
    return {
      contained: button.left >= bounds.left - 0.5 && button.right <= bounds.right + 0.5,
      height: button.height,
    };
  });
  assert.deepEqual(narrow, { contained: true, height: 23 });

  bootstrapMode = "protocol";
  const socketsBeforeRefusal = sockets;
  const bootstrapsBeforeRefusal = bootstraps;
  await closeCurrentSocket(page, "observe unsupported Canvas protocol");
  await waitForPageFunction(
    page,
    "a protocol mismatch is named without opening another socket",
    CONNECT_MS,
    (version) => {
      const button = document.querySelector(".canvas-menu-update");
      return button?.dataset.canvasUpdateVersion === version;
    },
    String(Number(currentVersion) - 1),
  );
  assert.ok(bootstraps > bootstrapsBeforeRefusal, "the refused reconnect reached bootstrap");
  await assertOfflineObserved(page, "explicit Canvas update required");
  assert.equal(sockets, socketsBeforeRefusal, "a protocol mismatch stops before the socket");
  assert.equal(navigations, 1, "a protocol mismatch does not navigate automatically");
  assert.equal((await page.locator(".page-heading h1").innerText()).trim(), "Tasks");
  assert.notEqual(await menu.getAttribute("open"), null, "the refusal preserves menu state");

  bootstrapMode = "current";
  const navigation = page.waitForEvent("framenavigated", (frame) => frame === page.mainFrame());
  await menu.getByRole("button", { name: new RegExp(`Reload Canvas v${Number(currentVersion) - 1}`) }).click();
  await navigation;
  await page.waitForLoadState("networkidle");
  await waitForPageFunction(
    page,
    "the explicit reload enters the current Canvas once",
    CONNECT_MS,
    () => /cursor \d+/.test(document.querySelector(".canvas-menu-cursor")?.textContent ?? "")
      && document.querySelector(".canvas-menu-trigger--update") === null,
  );
  assert.equal(navigations, 2, "one update click causes exactly one document reload");
  assert.equal(new URL(page.url()).pathname, "/", "the explicit update loads the current app root");
  assert.equal(new URL(page.url()).hash, `#${auth}`, "the explicit update retains Canvas authentication");
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

/// Exercise explicit app updates and protocol refusal before strict body decoding.
export async function assertCanvasExplicitUpdate(browser, endpoint) {
  await assertExplicitCanvasUpdate(browser, endpoint);
  await assertMalformedProtocolStopsBeforeSocket(browser, endpoint);
}
