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
  const agents = page.locator('.mosaic-workspace[data-active="true"]');
  await agents.waitFor();
  const menu = page.locator(".canvas-menu");
  assert.equal(await menu.getAttribute("open"), null, "the update proof starts with the menu closed");

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
  assert.equal(await agents.isVisible(), true, "discovering a rebuilt Canvas preserves Agents");
  const trigger = menu.locator(":scope > summary");
  assert.equal(await menu.getAttribute("open"), null, "the pending update leaves the menu closed");
  await trigger.click();
  const update = menu.getByRole("button", { name: `Reload Canvas v${currentVersion}` });
  await update.waitFor();
  await page.keyboard.press("Escape");
  await update.waitFor({ state: "hidden" });
  await trigger.focus();
  await page.keyboard.press("Enter");
  await update.waitFor();
  assert.equal(await update.getAttribute("data-canvas-update-generation"), alternateGeneration);
  assert.match(await update.innerText(), new RegExp(alternateGeneration.slice(0, 8)));
  assert.match(await trigger.getAttribute("title"), new RegExp(`Canvas v${currentVersion} update available`));
  const themes = menu.getByRole("group", { name: "Canvas theme" });
  const systemTheme = themes.getByRole("button", { name: "Use System theme" });
  const lightTheme = themes.getByRole("button", { name: "Use Light theme" });
  const darkTheme = themes.getByRole("button", { name: "Use Dark theme" });
  assert.equal(await themes.getByRole("button").count(), 3);
  assert.deepEqual(await themes.getByRole("button").allTextContents(), ["", "", ""]);
  assert.deepEqual(
    await themes.getByRole("button").evaluateAll((buttons) => buttons.map((button) => button.title)),
    ["Use Light theme", "Use System theme", "Use Dark theme"],
  );
  assert.equal(await systemTheme.getAttribute("aria-pressed"), "true");
  await darkTheme.focus();
  assert.equal(await darkTheme.evaluate((button) => button === document.activeElement), true);
  await page.keyboard.press("Enter");
  assert.equal(await page.locator("html").getAttribute("data-canvas-effective-theme"), "dark");
  assert.equal(await darkTheme.getAttribute("aria-pressed"), "true");
  assert.equal(await page.evaluate(() => localStorage.getItem("spice:canvas-theme")), "dark");
  const selectedPresentation = await darkTheme.evaluate((button) => {
    const selected = getComputedStyle(button);
    const unselected = getComputedStyle(button.parentElement.querySelector('[data-canvas-theme="light"]'));
    const luminance = (color) => {
      const values = color.match(/[\d.]+/g).slice(0, 3).map(Number);
      const scale = Math.max(...values) > 1 ? 255 : 1;
      return values.map((value) => value / scale).map(
        (value) => (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4),
      ).reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index], 0);
    };
    const foreground = luminance(selected.color);
    const background = luminance(selected.backgroundColor);
    return {
      backgroundDiffers: selected.backgroundColor !== unselected.backgroundColor,
      borderDiffers: selected.borderColor !== unselected.borderColor,
      colorDiffers: selected.color !== unselected.color,
      markerLarger: button.firstElementChild.getBoundingClientRect().width
        > button.parentElement.querySelector('[data-canvas-theme="light"]')
          .firstElementChild.getBoundingClientRect().width,
      contrast: (Math.max(foreground, background) + 0.05)
        / (Math.min(foreground, background) + 0.05),
      outlineStyle: selected.outlineStyle,
      outlineWidth: selected.outlineWidth,
    };
  });
  const { contrast, ...presentation } = selectedPresentation;
  assert.deepEqual(presentation, {
    backgroundDiffers: true,
    borderDiffers: true,
    colorDiffers: true,
    markerLarger: true,
    outlineStyle: "solid",
    outlineWidth: "2px",
  });
  assert.ok(contrast >= 4.5, `selected dark theme contrast is ${contrast}`);
  await page.emulateMedia({ colorScheme: "light" });
  assert.equal(await page.locator("html").getAttribute("data-canvas-effective-theme"), "dark");
  await systemTheme.click();
  assert.equal(await systemTheme.getAttribute("aria-pressed"), "true");
  assert.equal(await page.evaluate(() => localStorage.getItem("spice:canvas-theme")), null);
  assert.equal(await page.locator("html").getAttribute("data-canvas-effective-theme"), "light");
  await page.emulateMedia({ colorScheme: "dark" });
  await waitForPageFunction(
    page,
    "System theme follows a live dark OS preference",
    CONNECT_MS,
    () => document.documentElement.dataset.canvasEffectiveTheme === "dark",
  );
  await darkTheme.click();

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
  const narrow = await menu.evaluate((menuNode) => {
    const footer = menuNode.querySelector(".canvas-menu-footer");
    const button = footer.querySelector(".canvas-menu-update").getBoundingClientRect();
    const global = menuNode.querySelector(".canvas-menu-global-controls");
    const globalBounds = global.getBoundingClientRect();
    const fast = global.querySelector(".canvas-fast-setting").getBoundingClientRect();
    const themeNode = global.querySelector(".canvas-theme-selector");
    const themes = themeNode.getBoundingClientRect();
    const options = [...themeNode.querySelectorAll(".canvas-theme-option")]
      .map((option) => option.getBoundingClientRect());
    const marker = themeNode.querySelector('[aria-pressed="true"] .canvas-theme-position')
      .getBoundingClientRect();
    const cursorNode = footer.querySelector(".canvas-menu-cursor");
    const cursor = cursorNode.getBoundingClientRect();
    const bounds = footer.getBoundingClientRect();
    return {
      contained: button.left >= bounds.left - 0.5 && button.right <= bounds.right + 0.5,
      height: button.height,
      ordered: button.right <= cursor.left + 0.5,
      noFooterTheme: footer.querySelector(".canvas-theme-selector") === null,
      globalContained: fast.left >= globalBounds.left - 0.5
        && themes.right <= globalBounds.right + 0.5,
      globalOrdered: fast.right <= themes.left + 0.5,
      circles: options.every((option) => Math.abs(option.width - option.height) <= 0.5
        && option.width >= 28),
      markerCircular: Math.abs(marker.width - marker.height) <= 0.5,
      themeText: themeNode.innerText,
      themeWidth: themes.width,
      cursorClipped: cursorNode.scrollWidth > cursorNode.clientWidth,
    };
  });
  assert.equal(narrow.contained, true, JSON.stringify(narrow));
  assert.equal(narrow.height, 23, JSON.stringify(narrow));
  assert.equal(narrow.ordered, true, JSON.stringify(narrow));
  assert.equal(narrow.noFooterTheme, true, JSON.stringify(narrow));
  assert.equal(narrow.globalContained, true, JSON.stringify(narrow));
  assert.equal(narrow.globalOrdered, true, JSON.stringify(narrow));
  assert.equal(narrow.circles, true, JSON.stringify(narrow));
  assert.equal(narrow.markerCircular, true, JSON.stringify(narrow));
  assert.equal(narrow.themeText, "", JSON.stringify(narrow));
  assert.ok(narrow.themeWidth <= 96, JSON.stringify(narrow));
  assert.equal(narrow.cursorClipped, false, JSON.stringify(narrow));
  await trigger.click();
  await update.waitFor({ state: "hidden" });

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
  assert.equal(sockets, socketsBeforeRefusal, "a protocol mismatch stops before the socket");
  assert.equal(navigations, 1, "a protocol mismatch does not navigate automatically");
  assert.equal(await agents.isVisible(), true, "a protocol mismatch preserves Agents");
  assert.equal(await menu.getAttribute("open"), null, "the refusal preserves closed menu state");
  await trigger.click();
  await menu.getByRole("button", {
    name: new RegExp(`Reload Canvas v${Number(currentVersion) - 1}`),
  }).waitFor();

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
  assert.equal(await page.locator("html").getAttribute("data-canvas-effective-theme"), "dark");
  await page.getByLabel("Open Spice Canvas menu").click();
  assert.equal(
    await page.getByRole("button", { name: "Use Dark theme" }).getAttribute("aria-pressed"),
    "true",
    "the explicit theme persists through the Canvas update reload",
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

/// Exercise explicit app updates and protocol refusal before strict body decoding.
export async function assertCanvasExplicitUpdate(browser, endpoint) {
  await assertExplicitCanvasUpdate(browser, endpoint);
  await assertMalformedProtocolStopsBeforeSocket(browser, endpoint);
}
