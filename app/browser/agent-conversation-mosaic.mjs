import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import {
  chooseCanvasView,
  reply,
  RECONNECT_MS,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";
import {
  assertCompactionDivider,
  resizeCompactionViewport,
} from "./compaction-boundary-reference.mjs";

const PIXEL_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const PERSISTED_IMAGE_ROUTE = /\/mosaic-(?:load|error)-[0-9a-f-]+\.png$/;
// Span the fixture's one-second delivery cadence before arming exact lattice probes.
const ACTIVITY_QUIET_MS = 1_500;
const ACTIVITY_DEADLINE_MS = 15_000;

/// Keep test-owned image probe URLs resolvable when durable cards reach a new client.
export async function installPersistedMosaicImageRoutes(context) {
  await context.route(PERSISTED_IMAGE_ROUTE, async (route) => {
    const loads = route.request().url().includes("/mosaic-load-");
    await route.fulfill({
      contentType: "image/png",
      body: loads ? Buffer.from(PIXEL_PNG, "base64") : "not an image",
    });
  });
}

async function committedPlacementSnapshot(team) {
  return team.locator('.mosaic-message-card[data-layout-phase="committed"]').evaluateAll(
    (cards) => cards.map((card) => ({
      key: card.dataset.mosaicCardKey,
      track: card.dataset.track,
      span: card.dataset.span,
      base: card.dataset.base,
      rows: card.dataset.rows,
    })),
  );
}

function placementGeometry(placement) {
  return {
    track: placement.track,
    span: placement.span,
    base: placement.base,
    rows: placement.rows,
  };
}

export async function mosaicScrollSnapshot(plane) {
  const snapshot = await plane.evaluate((element) => {
    const viewport = element.getBoundingClientRect();
    const cards = [...element.querySelectorAll(
      '.mosaic-message-card[data-layout-phase="committed"]',
    )].map((card) => {
      const box = card.getBoundingClientRect();
      return {
        key: card.dataset.mosaicCardKey,
        offset: box.top - viewport.top,
        visible: box.bottom > viewport.top && box.top < viewport.bottom,
        lifecycle: card.classList.contains("frozen") ? "frozen" : "liquid",
      };
    });
    const visible = cards
      .filter((card) => card.visible)
      .sort((left, right) => Math.abs(left.offset) - Math.abs(right.offset));
    return {
      anchor: visible[0] ?? null,
      cards,
      scrollTop: element.scrollTop,
      scrollHeight: element.scrollHeight,
      clientHeight: element.clientHeight,
      committedWidth: Number(element.dataset.committedWidth),
      commits: Number(element.dataset.planeCommits),
      maxRow: Number(element.dataset.maxRow),
      module: Number(element.dataset.module),
    };
  });
  assert.ok(snapshot.anchor, `the scrolled Mosaic has a visible anchor: ${JSON.stringify(snapshot)}`);
  return snapshot;
}

export async function scrollMosaicAwayFromTop(page, plane) {
  await plane.evaluate((element) => {
    const maximum = element.scrollHeight - element.clientHeight;
    element.scrollTo(0, Math.min(maximum, Math.max(100, Math.round(maximum * 0.6))));
  });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  const snapshot = await mosaicScrollSnapshot(plane);
  assert.ok(snapshot.scrollTop > 80, `the Mosaic probe is beyond the top threshold: ${JSON.stringify(snapshot)}`);
  return snapshot;
}

export async function assertMosaicScrollInputs(page, plane) {
  await plane.evaluate((element) => element.scrollTo(0, 0));
  await plane.focus();
  await plane.evaluate((element) => {
    window.__mosaicKeyboardProbe = null;
    element.addEventListener("keydown", (event) => {
      if (event.key !== "PageDown") return;
      event.preventDefault();
      element.scrollBy(0, element.clientHeight);
      window.__mosaicKeyboardProbe = {
        defaultPrevented: event.defaultPrevented,
        key: event.key,
      };
    }, { once: true });
  });
  await page.keyboard.press("PageDown");
  await waitForPage(page, "PageDown reaches the Mosaic", TRANSITION_MS,
    () => window.__mosaicKeyboardProbe !== null);
  assert.deepEqual(
    await page.evaluate(() => window.__mosaicKeyboardProbe),
    { defaultPrevented: true, key: "PageDown" },
    "the PageDown packet targets and scrolls the Mosaic exactly once",
  );
  await waitForPage(page, "keyboard scrolling crosses the Mosaic top threshold", TRANSITION_MS,
    () => document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane')?.scrollTop > 80);

  await plane.evaluate((element) => element.scrollTo(0, 0));
  await plane.hover();
  await plane.evaluate((element) => {
    window.__mosaicWheelProbe = null;
    element.addEventListener("wheel", (event) => {
      event.preventDefault();
      element.scrollBy(0, event.deltaY);
      window.__mosaicWheelProbe = {
        deltaY: event.deltaY,
        defaultPrevented: event.defaultPrevented,
        targetsMosaic: event.target.closest(".lane-mosaic-plane") === element,
      };
    }, { once: true, passive: false });
  });
  await page.mouse.wheel(0, 240);
  await waitForPage(page, "wheel input reaches the Mosaic", TRANSITION_MS,
    () => window.__mosaicWheelProbe !== null);
  const wheel = await page.evaluate(() => window.__mosaicWheelProbe);
  assert.deepEqual(
    wheel,
    { deltaY: 240, defaultPrevented: true, targetsMosaic: true },
    `the wheel packet targets and scrolls the Mosaic exactly once: ${JSON.stringify(wheel)}`,
  );
  await waitForPage(page, "wheel and trackpad scrolling cross the Mosaic top threshold", TRANSITION_MS,
    () => document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane')?.scrollTop > 80);

  await plane.evaluate((element) => element.scrollTo(0, 0));
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  await waitForPage(page, "programmatic scrolling returns to the Mosaic top threshold", TRANSITION_MS,
    () => document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane')?.scrollTop <= 80);
}

export function assertMosaicAnchorPreserved(before, after, label) {
  assert.equal(after.anchor.key, before.anchor.key, `${label} retains the visible anchor identity`);
  assert.ok(
    Math.abs(after.anchor.offset - before.anchor.offset) <= 1,
    `${label} retains the anchor viewport offset: ${JSON.stringify({ before, after })}`,
  );
}

export function assertMosaicLifecycleAdvanced(before, after, label) {
  const earlier = new Map(before.cards.map((card) => [card.key, card.lifecycle]));
  const advanced = after.cards.filter(
    (card) => earlier.get(card.key) === "liquid" && card.lifecycle === "frozen",
  );
  assert.ok(
    advanced.length > 0,
    `${label} freezes at least one existing liquid card: ${JSON.stringify({ before, after })}`,
  );
}

export async function assertCompactionBoundary(
  page,
  fixture,
  team,
  seeded,
  artifacts,
  activityReplay,
) {
  const plane = team.locator(".lane-mosaic-plane");
  const viewport = page.viewportSize();
  const compactViewport = viewport && {
    width: Math.min(viewport.width, 390),
    height: Math.min(viewport.height, 360),
  };
  if (compactViewport
    && (compactViewport.width !== viewport.width || compactViewport.height !== viewport.height)) {
    await page.setViewportSize(compactViewport);
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
      () => requestAnimationFrame(resolve),
    )));
    await settleMosaicActivity(page, plane);
  }
  const before = await committedPlacementSnapshot(team);
  const scrolled = await scrollMosaicAwayFromTop(page, plane);
  await writeFile(join(seeded.teamWorktrees[0], ".spice-compaction-trigger"), "");
  const divider = team.locator(
    `.mosaic-message-card[data-card-kind="compaction"][data-agent-lane="${seeded.lane}"]`,
  );
  await divider.waitFor({ timeout: 10_000 });
  await waitForPage(
    page,
    "compaction commits as one visible full-width boundary",
    TRANSITION_MS,
    (lane) => {
      const card = document.querySelector(
        `.mosaic-message-card[data-card-kind="compaction"][data-agent-lane="${lane}"]`,
      );
      return card?.dataset.layoutPhase === "committed"
        && card.dataset.track === "0"
        && card.dataset.span === "12"
        && card.querySelectorAll(".mosaic-compaction-rule").length === 2
        && card.querySelector(".message-key")?.textContent?.trim() === "compacted context"
        && card.querySelector(".mosaic-compaction-agent")?.textContent?.trim().length > 0
        && card.querySelectorAll(".message-actions button").length === 0;
    },
    seeded.lane,
  );

  const laneAccent = await team
    .locator(`.team-agent[data-agent-lane="${seeded.lane}"]`)
    .getAttribute("data-accent");
  assert.equal(await divider.getAttribute("data-accent"), laneAccent);
  assert.equal(
    await team.locator('[data-card-kind="compaction"]').count(),
    1,
    "the real transcript compaction renders exactly once",
  );

  const afterBoundary = new Map(
    (await committedPlacementSnapshot(team)).map((placement) => [placement.key, placement]),
  );
  for (const placement of before) {
    const retained = afterBoundary.get(placement.key);
    assert.ok(retained, `the compaction boundary retains ${placement.key}`);
    assert.deepEqual(
      placementGeometry(retained),
      placementGeometry(placement),
      `the compaction boundary preserves committed geometry for ${placement.key}`,
    );
  }
  assertMosaicAnchorPreserved(
    scrolled,
    await mosaicScrollSnapshot(plane),
    "a compaction epoch while scrolled",
  );
  const boundaryKey = await divider.getAttribute("data-mosaic-card-key");
  assert.ok(boundaryKey, "the compaction boundary has one stable source identity");

  const beforeKeys = await team.locator('[data-card-kind="agent-message"]').evaluateAll(
    (cards) => cards.map((card) => card.dataset.mosaicCardKey),
  );
  const nextProse = activityReplay.nextAgentProse(seeded.lane);
  await writeFile(join(seeded.teamWorktrees[0], ".spice-prose-trigger"), "emit\n");
  const afterMessage = await nextProse;
  const afterKey = [
    "message-agent",
    afterMessage.lane,
    afterMessage.locus.member,
    afterMessage.locus.offset,
    afterMessage.locus.length,
    afterMessage.locus.event,
  ].join("-");
  const afterCard = team.locator(`[data-mosaic-card-key="${afterKey}"]`);
  await afterCard.waitFor({ timeout: TRANSITION_MS });
  assert.ok(!beforeKeys.includes(afterKey), "post-compaction activity is a new assistant record");
  await waitForPage(
    page,
    "post-compaction activity commits beyond the boundary floor",
    TRANSITION_MS,
    ({ boundaryKey, cardKey }) => {
      const boundary = document.querySelector(`[data-mosaic-card-key="${boundaryKey}"]`);
      const card = document.querySelector(`[data-mosaic-card-key="${cardKey}"]`);
      if (boundary?.dataset.layoutPhase !== "committed"
        || card?.dataset.layoutPhase !== "committed") return false;
      return Number(card.dataset.base) + Number(card.dataset.rows)
        <= Number(boundary.dataset.base);
    },
    {
      boundaryKey,
      cardKey: afterKey,
    },
  );
  const boundaryBox = await divider.boundingBox();
  const afterBox = await afterCard.boundingBox();
  assert.ok(boundaryBox && afterBox, "the boundary and post-compaction card have visible geometry");
  assert.ok(
    afterBox.y >= boundaryBox.y + boundaryBox.height - 1,
    `post-compaction activity stays below its boundary: ${JSON.stringify({ boundaryBox, afterBox })}`,
  );
  await assertCompactionDivider(
    divider,
    "Chromium narrow compaction divider",
    seeded.agentAttribution,
  );
  assertMosaicAnchorPreserved(
    scrolled,
    await mosaicScrollSnapshot(plane),
    "post-compaction activity while scrolled",
  );
  await divider.scrollIntoViewIfNeeded();
  if (viewport && compactViewport
    && (compactViewport.width !== viewport.width || compactViewport.height !== viewport.height)) {
    await resizeCompactionViewport(page, divider, viewport, "Chromium restored compaction divider");
    await assertCompactionDivider(
      divider,
      "Chromium wide compaction divider",
      seeded.agentAttribution,
    );
  }
  await page.screenshot({
    path: join(artifacts, "canvas-compaction-boundary.png"),
    fullPage: true,
  });
  return boundaryKey;
}

export async function assertCompactionBoundaryPersistence(page, seeded, boundaryKey) {
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Agents");
  const restoredTeam = page.locator('[data-pane="agent-console"]');
  await restoredTeam.getByRole("tab", { name: /^Compose/ }).click();
  const restored = restoredTeam.locator(`[data-mosaic-card-key="${boundaryKey}"]`);
  await restored.waitFor({ timeout: RECONNECT_MS });
  assert.equal(
    await restoredTeam.locator(
      `.mosaic-message-card[data-card-kind="compaction"][data-agent-lane="${seeded.lane}"]`,
    ).count(),
    1,
    "reload restores the compaction boundary exactly once",
  );
  assert.equal(await restored.getAttribute("data-span"), "12");
  assert.equal((await restored.locator(".message-key").innerText()).trim(), "compacted context");

  const socketCount = await page.evaluate(() => window.__canvasSockets?.length ?? 0);
  assert.ok(socketCount > 0, "the browser fixture captures the active Canvas socket");
  await page.evaluate(() => window.__canvasSockets.at(-1).close(4000, "compaction reconnect"));
  await waitForPage(
    page,
    "compaction boundary survives an in-place reconnect",
    RECONNECT_MS,
    ({ expectedKey, previousSockets }) => (
      (window.__canvasSockets?.length ?? 0) > previousSockets
      && document.querySelectorAll(`[data-mosaic-card-key="${expectedKey}"]`).length === 1
      && document.querySelector(`[data-mosaic-card-key="${expectedKey}"]`)
        ?.dataset.layoutPhase === "committed"
    ),
    { expectedKey: boundaryKey, previousSockets: socketCount },
  );
}

export async function assertInitiallyDeferredMosaicIsReadable(page, console) {
  await console.getByRole("tab", { name: /^Info/ }).click();
  const rule = await page.evaluate(() => {
    const sheet = [...document.styleSheets]
      .find((candidate) => candidate.href?.endsWith("mosaic.css"));
    if (!sheet) throw new Error("the Mosaic stylesheet is unavailable");
    const index = sheet.cssRules.length;
    sheet.insertRule(".lane-mosaic-plane { width: 12px !important; }", index);
    return index;
  });
  await console.getByRole("tab", { name: /^Compose/ }).click();
  const plane = console.locator(".lane-mosaic-plane");
  await plane.waitFor({ state: "attached" });
  await waitForPage(page, "fresh Mosaic defers below its measurable width", TRANSITION_MS, () => {
    const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return plane?.dataset.measurementPhase === "base"
      && Number(plane.dataset.observedWidth) < 24;
  });
  const cards = plane.locator(".mosaic-message-card");
  assert.ok(await cards.count(), "the initially deferred plane retains transcript cards");
  const fallback = await cards.evaluateAll((nodes) => nodes.map((node) => {
    const box = node.getBoundingClientRect();
    return {
      visibility: getComputedStyle(node).visibility,
      top: box.top,
      bottom: box.bottom,
      height: box.height,
    };
  }));
  assert.ok(
    fallback.every((card) => card.visibility === "visible" && card.height > 0),
    "an initially unmeasurable plane keeps every transcript card readable",
  );
  assert.ok(
    fallback.slice(1).every((card, index) => card.top >= fallback[index].bottom),
    "deferred transcript cards occupy non-overlapping flow rows",
  );
  await page.evaluate((index) => {
    const sheet = [...document.styleSheets]
      .find((candidate) => candidate.href?.endsWith("mosaic.css"));
    if (!sheet) throw new Error("the Mosaic stylesheet is unavailable");
    sheet.deleteRule(index);
  }, rule);
  await waitForPage(page, "fresh Mosaic commits after becoming measurable", TRANSITION_MS, () => {
    const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return plane?.dataset.measurementPhase === "ready"
      && [...plane.querySelectorAll(".mosaic-message-card")]
        .every((card) => card.dataset.layoutPhase === "committed");
  });
}

async function armPlacementProbe(plane, excluded = []) {
  return plane.evaluate((element, excludedKeys) => {
    const token = crypto.randomUUID();
    const attributes = ["data-track", "data-span", "data-base", "data-row", "data-rows", "style"];
    const cards = [...element.querySelectorAll('.mosaic-message-card[data-layout-phase="committed"]')]
      .filter((card) => !excludedKeys.includes(card.dataset.mosaicCardKey));
    const keys = cards.map((card) => card.dataset.mosaicCardKey);
    // Every assertion this probe feeds is a negative one: no placement writes,
    // and `retained` folded over these keys. An empty key set answers both with
    // a pass it did not earn, so arming on nothing is the one outcome that must
    // be loud. It also fails the way a broken selector deserves — at the arm,
    // naming the selector, not later as a green nobody questions.
    if (keys.length === 0) {
      throw new Error("the Mosaic placement probe armed on no committed card");
    }
    for (const card of cards) card.__mosaicPlacementProbe = token;
    const writes = [];
    const record = (mutation) => {
      if (mutation.target.__mosaicPlacementProbe !== token) return;
      writes.push({
        key: mutation.target.dataset.mosaicCardKey,
        attribute: mutation.attributeName,
        before: mutation.oldValue,
        after: mutation.target.getAttribute(mutation.attributeName),
      });
    };
    const observer = new MutationObserver((mutations) => mutations.forEach(record));
    observer.observe(element, {
      subtree: true,
      attributes: true,
      attributeOldValue: true,
      attributeFilter: attributes,
    });
    element.__mosaicPlacementObserver = { token, keys, writes, observer, record };
    return {
      token,
      commits: Number(element.dataset.planeCommits),
      committedWidth: Number(element.dataset.committedWidth),
      cards: cards.map((card) => {
        const box = card.getBoundingClientRect();
        return {
          key: card.dataset.mosaicCardKey,
          order: [...element.querySelectorAll(".mosaic-message-card")].indexOf(card),
          track: card.dataset.track,
          span: card.dataset.span,
          base: card.dataset.base,
          row: card.dataset.row,
          rows: card.dataset.rows,
          box: { x: box.x, y: box.y, width: box.width, height: box.height },
        };
      }),
    };
  }, excluded);
}

async function finishPlacementProbe(plane, armed) {
  return plane.evaluate((element, expected) => {
    const probe = element.__mosaicPlacementObserver;
    if (!probe || probe.token !== expected.token) throw new Error("Mosaic placement probe changed");
    probe.observer.takeRecords().forEach(probe.record);
    probe.observer.disconnect();
    const cards = [...element.querySelectorAll(".mosaic-message-card")];
    const retained = probe.keys.every((key) => cards.some(
      (card) => card.dataset.mosaicCardKey === key && card.__mosaicPlacementProbe === probe.token,
    ));
    delete element.__mosaicPlacementObserver;
    return {
      retained,
      writes: probe.writes,
      commits: Number(element.dataset.planeCommits),
      transition: element.dataset.planeTransition,
      observedWidth: Number(element.dataset.observedWidth),
      committedWidth: Number(element.dataset.committedWidth),
      cards: cards
        .filter((card) => probe.keys.includes(card.dataset.mosaicCardKey))
        .map((card) => {
          const box = card.getBoundingClientRect();
          return {
            key: card.dataset.mosaicCardKey,
            order: cards.indexOf(card),
            track: card.dataset.track,
            span: card.dataset.span,
            base: card.dataset.base,
            row: card.dataset.row,
            rows: card.dataset.rows,
            box: { x: box.x, y: box.y, width: box.width, height: box.height },
          };
        }),
    };
  }, armed);
}

function assertUnmoved(armed, finished, label) {
  assert.ok(finished.retained, `${label} retains every committed card node`);
  assert.deepEqual(finished.writes, [], `${label} writes no card placement`);
  assert.deepEqual(
    finished.cards,
    armed.cards,
    `${label} preserves DOM order, committed placement, and bounding geometry`,
  );
}

async function submitAssistantActivity(page, console, lane, text, activityReplay) {
  const injected = await activityReplay.injectAgentProse(page, lane, text);
  return console.locator(`[data-mosaic-card-key="${injected.key}"]`);
}

async function assertImagePreview(preview, label, expectedHeight) {
  const observed = await preview.evaluate((element) => {
    const image = element.querySelector("img");
    const box = element.getBoundingClientRect();
    const imageBox = image.getBoundingClientRect();
    const cardBox = element.closest(".mosaic-message-card").getBoundingClientRect();
    const style = getComputedStyle(image);
    return {
      box: { left: box.left, right: box.right, width: box.width },
      card: { left: cardBox.left, right: cardBox.right, width: cardBox.width },
      image: { width: imageBox.width, height: imageBox.height },
      objectFit: style.objectFit,
      maxHeight: style.maxHeight,
    };
  });
  assert.ok(
    Math.abs(observed.image.height - expectedHeight) <= 1,
    `${label} uses the ${expectedHeight}px bounded slot: ${JSON.stringify(observed)}`,
  );
  assert.ok(observed.box.width <= observed.card.width, `${label} stays within its card`);
  assert.ok(
    observed.box.left >= observed.card.left - 1 && observed.box.right <= observed.card.right + 1,
    `${label} does not overflow its card: ${JSON.stringify(observed)}`,
  );
  assert.equal(observed.objectFit, "contain", `${label} preserves aspect ratio within its slot`);
  assert.equal(observed.maxHeight, `${expectedHeight}px`, `${label} exposes a stable computed cap`);
}

async function imageOnlyTierGeometry(card) {
  return card.evaluate((element) => {
    const body = element.querySelector(".message-body");
    const stacks = [...body.querySelectorAll(":scope > p.message-image-stack")];
    const cardBox = element.getBoundingClientRect();
    const bodyBox = body.getBoundingClientRect();
    const stackBox = stacks[0]?.getBoundingClientRect();
    return {
      stacks: stacks.length,
      cardHeight: cardBox.height,
      cardScrollHeight: element.scrollHeight,
      bodyHeight: bodyBox.height,
      stackHeight: stackBox?.height,
      contained: Boolean(stackBox)
        && stackBox.top >= bodyBox.top - 1
        && stackBox.bottom <= bodyBox.bottom + 1
        && bodyBox.top >= cardBox.top - 1
        && bodyBox.bottom <= cardBox.bottom + 1,
    };
  });
}

function assertOneImageOnlyTier(reference, observed, label) {
  assert.equal(observed.stacks, 1, `${label} renders exactly one card-wide image stack`);
  assert.ok(observed.contained, `${label} keeps the image tier inside its body and card`);
  assert.ok(Math.abs(observed.stackHeight - 252) <= 1,
    `${label} has one 252px stack: ${JSON.stringify(observed)}`);
  assert.ok(observed.bodyHeight <= reference.bodyHeight + 1,
    `${label} body does not exceed one image tier: ${JSON.stringify({ reference, observed })}`);
  assert.ok(observed.cardScrollHeight <= reference.cardScrollHeight + 1,
    `${label} card content does not sum image tiers: ${JSON.stringify({ reference, observed })}`);
}

async function assertFullSizeTarget(preview, label, expectedHref) {
  assert.equal(await preview.getAttribute("target"), "_blank", `${label} opens in a new tab`);
  assert.equal(await preview.getAttribute("rel"), "noopener", `${label} isolates its opener`);
  assert.match(await preview.getAttribute("aria-label"), /^Open .+ full size$/,
    `${label} names its full-size action`);
  assert.match(await preview.getAttribute("title"), /^Open .+ full size$/,
    `${label} exposes a discoverable tooltip`);
  if (expectedHref) assert.equal(await preview.getAttribute("href"), expectedHref, `${label} targets its source`);
}

async function openAndAssertIsolated(page, preview, label, expectedUrl) {
  const opened = page.waitForEvent("popup");
  await preview.click();
  const popup = await opened;
  await popup.waitForLoadState("domcontentloaded");
  if (expectedUrl instanceof RegExp) assert.match(popup.url(), expectedUrl, `${label} opens full size`);
  else assert.equal(popup.url(), expectedUrl, `${label} opens full size`);
  assert.equal(await popup.evaluate(() => window.opener), null, `${label} has no opener`);
  await popup.close();
}

async function settleMosaicActivity(page, plane) {
  await plane.evaluate((element, config) => new Promise((resolve, reject) => {
    let quiet;
    const snapshot = () => ({
      cards: element.querySelectorAll(".mosaic-message-card").length,
      commits: Number(element.dataset.planeCommits),
      pending: Number(element.dataset.measurementPending),
    });
    const cleanup = () => {
      clearTimeout(quiet);
      clearTimeout(deadline);
      observer.disconnect();
    };
    const arm = () => {
      clearTimeout(quiet);
      quiet = setTimeout(() => {
        if (Number(element.dataset.measurementPending) !== 0) return arm();
        cleanup();
        resolve();
      }, config.quietMs);
    };
    const observer = new MutationObserver((mutations) => {
      const activityChanged = mutations.some((mutation) => mutation.type === "attributes"
        || [...mutation.addedNodes, ...mutation.removedNodes].some((node) => (
          node instanceof Element
            && (node.matches(".mosaic-message-card")
              || node.querySelector(".mosaic-message-card"))
        )));
      if (activityChanged) arm();
    });
    observer.observe(element, {
      attributes: true,
      attributeFilter: ["data-plane-commits", "data-measurement-pending"],
      childList: true,
      subtree: true,
    });
    const deadline = setTimeout(() => {
      const observed = snapshot();
      cleanup();
      reject(new Error(`Mosaic activity did not settle: ${JSON.stringify(observed)}`));
    }, config.deadlineMs);
    arm();
  }), { quietMs: ACTIVITY_QUIET_MS, deadlineMs: ACTIVITY_DEADLINE_MS });
}

export async function assertPersistentMosaic(
  page,
  fixture,
  console,
  plane,
  member,
  activityReplay,
) {
  await settleMosaicActivity(page, plane);
  const firstCard = console.locator('.mosaic-message-card[data-layout-phase="committed"]').first();
  const life = console.locator('.team-lifetime input[type="range"]');
  const presentationProbe = await armPlacementProbe(plane);
  await life.fill("1");
  await console.getByRole("button", { name: "Drive", exact: true }).waitFor();
  const presentation = await finishPlacementProbe(plane, presentationProbe);
  assertUnmoved(presentationProbe, presentation, "a presentation-only update");
  assert.equal(presentation.commits, presentationProbe.commits, "chrome never commits the lattice");
  await life.fill("0");
  await console.getByRole("button", { name: "Steer", exact: true }).waitFor();

  // Clause 2's no-op refresh. A task is neither an agent message nor a pending
  // directive, so the server republishes the whole surface and every card
  // re-renders from freshly cloned props while nothing a card measures moved.
  // The command-bar cursor is the positive control: without it a refresh that
  // never arrived would answer this probe with exactly the silence a correct
  // one does, and the assertions below would pass having tested nothing. Bind
  // that control to this command's committed range so an unrelated cursor
  // advance cannot satisfy it.
  //
  // The missing `--acceptance` is load-bearing, not an omission: a task added
  // without one starts in `plan`, which `workable` never selects, so this task
  // can never be held and cannot disturb `assertHeldTaskCount`. Give it
  // acceptance criteria and a distant assertion about badges goes red.
  const refreshProbe = await armPlacementProbe(plane);
  const refresh = await reply(fixture, [
    "task",
    "add",
    `mosaic no-op refresh ${crypto.randomUUID()}`,
    "--project",
    "spice.browser",
  ]);
  assert.ok(
    Number.isSafeInteger(refresh.seq?.last)
      && typeof refresh.body?.task === "string"
      && refresh.body.task.length > 0,
    "the no-op refresh command commits an exact journal range",
  );
  await waitForPage(page, "a no-op refresh reaches the surface", TRANSITION_MS, (expected) => (
    document.querySelector(".bar-meta")?.textContent === expected
  ), `cursor ${refresh.seq.last}`);
  const refreshed = await finishPlacementProbe(plane, refreshProbe);
  assertUnmoved(refreshProbe, refreshed, "a no-op refresh");
  assert.equal(refreshed.commits, refreshProbe.commits, "a no-op refresh commits no plane");
  await settleMosaicActivity(page, plane);

  const tickingAge = console.locator(
    '.mosaic-message-card[data-layout-phase="committed"] .message-age[data-age-clock="deadline"]',
  )
    .filter({ hasText: /^(?:now|\d+s)$/ })
    .last();
  await tickingAge.waitFor();
  const ageTarget = await tickingAge.evaluate((element) => {
    const card = element.closest(".mosaic-message-card");
    const token = crypto.randomUUID();
    element.__messageFooterAgeProbe = token;
    return {
      authored: element.dataset.ageAuthored,
      cardKey: card?.dataset.mosaicCardKey,
      token,
    };
  });
  assert.ok(
    ageTarget.authored && ageTarget.cardKey,
    `the selected message footer names its authored instant and card: ${JSON.stringify(ageTarget)}`,
  );
  assert.ok(
    await console.locator('.composer-shard-age-label[data-age-clock="deadline"]')
      .evaluateAll((ages, authored) => ages.some(
        (age) => age.dataset.ageAuthored === authored,
      ), ageTarget.authored),
    "the composer header shares the selected message footer's authored instant",
  );
  const ageProbe = await armPlacementProbe(plane);
  const ageCardBefore = ageProbe.cards.find((card) => card.key === ageTarget.cardKey);
  assert.ok(ageCardBefore, `the placement probe includes message card ${ageTarget.cardKey}`);
  const ageBefore = await tickingAge.innerText();
  await tickingAge.evaluate((element, expected) => new Promise((resolve, reject) => {
    const cleanup = () => {
      clearTimeout(deadline);
      observer.disconnect();
    };
    const changed = () => {
      if (element.textContent === expected.before) return;
      cleanup();
      resolve();
    };
    if (element.__messageFooterAgeProbe !== expected.token
      || element.dataset.ageAuthored !== expected.authored
      || element.closest(".mosaic-message-card")?.dataset.mosaicCardKey !== expected.cardKey) {
      reject(new Error(`the selected message footer moved before its tick: ${JSON.stringify(expected)}`));
      return;
    }
    const observer = new MutationObserver(changed);
    observer.observe(element, { childList: true, characterData: true, subtree: true });
    const deadline = setTimeout(() => {
      cleanup();
      reject(new Error(`the selected message footer age did not advance: ${JSON.stringify(expected)}`));
    }, expected.deadlineMs);
    changed();
  }), {
    ...ageTarget,
    before: ageBefore,
    deadlineMs: TRANSITION_MS,
  });
  const aged = await finishPlacementProbe(plane, ageProbe);
  assert.deepEqual(
    aged.cards.find((card) => card.key === ageTarget.cardKey),
    ageCardBefore,
    "the selected message card preserves its Mosaic placement and bounding geometry",
  );
  assertUnmoved(ageProbe, aged, "a deadline-driven age tick");
  assert.equal(
    aged.commits,
    ageProbe.commits,
    `an age tick commits no plane: ${JSON.stringify({
      before: ageProbe.commits,
      after: aged.commits,
      transition: aged.transition,
      writes: aged.writes,
    })}`,
  );

  const originalWidth = Number(await plane.getAttribute("data-committed-width"));
  const widthRules = await plane.evaluate((element) => {
    const sheet = [...document.styleSheets].find((candidate) => candidate.href?.endsWith("mosaic.css"));
    if (!sheet) throw new Error("the Mosaic stylesheet is absent");
    const rules = [
      ".lane-mosaic-plane.mosaic-width-probe-tiny { width: 12px; }",
      ".lane-mosaic-plane.mosaic-width-probe-a { width: 540px; }",
      ".lane-mosaic-plane.mosaic-width-probe-b { width: 580px; }",
      ".lane-mosaic-plane.mosaic-width-probe-final { width: 620px; }",
    ];
    return rules.map((rule) => sheet.insertRule(rule, sheet.cssRules.length));
  });
  const deferredProbe = await armPlacementProbe(plane);
  await plane.evaluate((element) => element.classList.add("mosaic-width-probe-tiny"));
  await waitForPage(page, "Mosaic width defers below the measurable floor", TRANSITION_MS, () => {
    const element = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return Number(element?.dataset.observedWidth) < 24;
  });
  const deferred = await finishPlacementProbe(plane, deferredProbe);
  assert.ok(deferred.retained, "deferral retains the committed card nodes");
  assert.deepEqual(deferred.writes, [], "deferral writes no committed card placement");
  assert.equal(deferred.commits, deferredProbe.commits, "deferral commits no guessed plane");
  assert.equal(deferred.committedWidth, originalWidth, "deferral retains the last real geometry");

  await plane.evaluate((element) => element.classList.remove("mosaic-width-probe-tiny"));
  await waitForPage(page, "Mosaic reveal corrects immediately", TRANSITION_MS, (expected) => {
    const element = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return Math.abs(Number(element?.dataset.observedWidth) - expected) <= 1;
  }, originalWidth);
  const resizeProbe = await armPlacementProbe(plane);
  await plane.evaluate((element) => new Promise((resolve) => {
    element.classList.add("mosaic-width-probe-a");
    requestAnimationFrame(() => {
      element.classList.replace("mosaic-width-probe-a", "mosaic-width-probe-b");
      requestAnimationFrame(() => {
        element.classList.replace("mosaic-width-probe-b", "mosaic-width-probe-final");
        resolve();
      });
    });
  }));
  await waitForPage(page, "Mosaic resize burst commits the final width", TRANSITION_MS, () => {
    const element = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return Number(element?.dataset.committedWidth) === 620;
  });
  const resized = await finishPlacementProbe(plane, resizeProbe);
  assert.ok(resized.retained, "a width replay retains keyed DOM nodes");
  assert.equal(resized.commits, resizeProbe.commits + 1, "a resize burst commits once");
  await plane.evaluate((element, indices) => {
    element.classList.remove("mosaic-width-probe-final");
    const sheet = [...document.styleSheets].find((candidate) => candidate.href?.endsWith("mosaic.css"));
    for (const index of [...indices].sort((left, right) => right - left)) sheet.deleteRule(index);
  }, widthRules);
  await waitForPage(page, "Mosaic restores its host width", TRANSITION_MS, (expected) => {
    const element = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return Math.abs(Number(element?.dataset.committedWidth) - expected) <= 1;
  }, originalWidth);

  const origin = new URL(page.url()).origin;
  for (const outcome of ["load", "error"]) {
    const url = `${origin}/mosaic-${outcome}-${crypto.randomUUID()}.png`;
    let release;
    let arrived;
    const released = new Promise((resolve) => { release = resolve; });
    const requested = new Promise((resolve) => { arrived = resolve; });
    await page.route(url, async (route) => {
      arrived();
      await released;
      if (outcome === "load") {
        await route.fulfill({ contentType: "image/png", body: Buffer.from(PIXEL_PNG, "base64") });
      } else {
        await route.fulfill({ contentType: "image/png", body: "not an image" });
      }
    });
    const text = `late Mosaic image ${outcome}\n\n![${outcome}](${url})`;
    const pendingProbe = await armPlacementProbe(plane);
    const card = await submitAssistantActivity(page, console, member.lane, text, activityReplay);
    await card.waitFor({ state: "attached", timeout: 10_000 });
    await requested;
    await card.locator('img[data-mosaic-image-state="pending"]').waitFor({ state: "attached" });
    await waitForPage(page, `pending image ${outcome} reserves a committed card`, TRANSITION_MS,
      (key) => [...document.querySelectorAll(".mosaic-message-card")]
        .find((candidate) => candidate.dataset.mosaicCardKey === key)
        ?.dataset.layoutPhase === "committed",
      await card.getAttribute("data-mosaic-card-key"));
    const pending = await finishPlacementProbe(plane, pendingProbe);
    assert.ok(pending.retained, `pending image ${outcome} retains untouched card nodes`);
    assert.deepEqual(pending.writes, [], `pending image ${outcome} writes no untouched placement`);
    assert.equal(pending.commits, pendingProbe.commits + 1, `pending image ${outcome} appends once`);
    const cardKey = await card.getAttribute("data-mosaic-card-key");
    const resolutionProbe = await armPlacementProbe(plane, [cardKey]);
    assert.equal(await plane.getAttribute("data-measurement-phase"), "ready");
    release();
    try {
      await waitForPage(page, `late image ${outcome} corrects once`, TRANSITION_MS, (expected) => {
        const card = [...document.querySelectorAll(".mosaic-message-card")]
          .find((candidate) => candidate.dataset.mosaicCardKey === expected.key);
        const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
        return card?.querySelector("img")?.getAttribute("data-mosaic-image-state") === expected.state
          && Number(plane?.dataset.planeCommits) >= expected.commits + 1;
      }, { key: cardKey, state: outcome === "load" ? "loaded" : "error", commits: pending.commits });
    } catch (error) {
      const diagnosis = await plane.evaluate((element, key) => {
        const image = [...element.querySelectorAll(".mosaic-message-card")]
          .find((candidate) => candidate.dataset.mosaicCardKey === key)?.querySelector("img");
        return {
          plane: { ...element.dataset },
          image: image ? { ...image.dataset, complete: image.complete, naturalWidth: image.naturalWidth } : null,
        };
      }, cardKey);
      throw new Error(`late image ${outcome} diagnosis: ${JSON.stringify(diagnosis)}`, { cause: error });
    }
    const resolved = await finishPlacementProbe(plane, resolutionProbe);
    assert.ok(resolved.retained, `late image ${outcome} retains untouched card nodes`);
    assert.deepEqual(resolved.writes, [], `late image ${outcome} writes no untouched placement`);
    assert.equal(resolved.commits, pending.commits + 1, `late image ${outcome} corrects once`);
    assert.equal(await card.locator("img").getAttribute("data-mosaic-image-state"),
      outcome === "load" ? "loaded" : "error");
    assert.equal(
      await card.getAttribute("data-mosaic-card-key"),
      cardKey,
      `reconciliation retains the authored ${outcome} assistant record`,
    );
    const remotePreview = card.locator("a.message-image");
    await assertImagePreview(remotePreview, `remote ${outcome} preview`, 140);
    await assertFullSizeTarget(remotePreview, `remote ${outcome} preview`, url);
    if (outcome === "load") {
      await openAndAssertIsolated(page, remotePreview, "remote image preview", url);
    }
    await page.unroute(url);
  }

  const readyText = `already complete Mosaic image\n\n![ready](data:image/png;base64,${PIXEL_PNG})`;
  const readyProbe = await armPlacementProbe(plane);
  const readyCard = await submitAssistantActivity(
    page,
    console,
    member.lane,
    readyText,
    activityReplay,
  );
  await readyCard.waitFor({ state: "attached", timeout: 10_000 });
  await waitForPage(page, "already-complete image commits", TRANSITION_MS, (key) => {
    const card = [...document.querySelectorAll(".mosaic-message-card")]
      .find((candidate) => candidate.dataset.mosaicCardKey === key);
    return card?.dataset.layoutPhase === "committed";
  }, await readyCard.getAttribute("data-mosaic-card-key"));
  await firstCard.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  const ready = await finishPlacementProbe(plane, readyProbe);
  assert.ok(ready.retained, "an already-complete image retains untouched card nodes");
  assert.deepEqual(ready.writes, [], "an already-complete image writes no untouched placement");
  assert.equal(ready.commits, readyProbe.commits + 1, "an already-complete image corrects once");

  const embeddedPreview = readyCard.locator("a.message-image");
  await assertImagePreview(embeddedPreview, "embedded ordinary preview", 140);
  await assertFullSizeTarget(embeddedPreview, "embedded ordinary preview", "about:blank");
  assert.equal(await embeddedPreview.getAttribute("data-full-size-data-image"), "true");
  await openAndAssertIsolated(page, embeddedPreview, "embedded image preview", /^blob:/);

  const imageOnlyCard = await submitAssistantActivity(
    page,
    console,
    member.lane,
    `![large embedded preview](data:image/png;base64,${PIXEL_PNG})`,
    activityReplay,
  );
  await imageOnlyCard.waitFor({ state: "attached", timeout: 10_000 });
  assert.ok(
    await imageOnlyCard.evaluate((card) => card.classList.contains("mosaic-message-card--image-only")),
    "a pure image message receives the large preview tier",
  );
  await assertImagePreview(imageOnlyCard.locator(".message-image"), "image-only preview", 252);
  const imageOnlyGeometry = await imageOnlyTierGeometry(imageOnlyCard);

  const multiImageCard = await submitAssistantActivity(
    page,
    console,
    member.lane,
    ["one", "two", "three"].map((name) => (
      `![${name}](data:image/png;base64,${PIXEL_PNG})`
    )).join("\n\n"),
    activityReplay,
  );
  await multiImageCard.waitFor({ state: "attached", timeout: 10_000 });
  const multiPreviews = multiImageCard.locator("p.message-image-stack > .message-image");
  assert.equal(await multiPreviews.count(), 3,
    "blank-line-separated images retain every preview in one intentional stack");
  const multiGeometry = await multiPreviews.evaluateAll((previews) => previews.map((preview) => {
    const box = preview.getBoundingClientRect();
    return { top: box.top, left: box.left, right: box.right };
  }));
  assert.ok(multiGeometry.every((box) => Math.abs(box.top - multiGeometry[0].top) <= 1),
    `multi-image previews share one row: ${JSON.stringify(multiGeometry)}`);
  await assertImagePreview(multiPreviews.first(), "multi-image preview", 252);
  assertOneImageOnlyTier(
    imageOnlyGeometry,
    await imageOnlyTierGeometry(multiImageCard),
    "blank-line-separated desktop previews",
  );

  const unsupportedCard = await submitAssistantActivity(
    page,
    console,
    member.lane,
    "![bounded unsupported data image](data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIxIiBoZWlnaHQ9IjEiPjwvc3ZnPg==)",
    activityReplay,
  );
  await unsupportedCard.waitFor({ state: "attached", timeout: 10_000 });
  assert.equal(await unsupportedCard.locator("a.message-image").count(), 0,
    "a non-raster data image has no top-level opening action");
  await assertImagePreview(unsupportedCard.locator("span.message-image"),
    "unsupported data-image preview", 252);

  const malformedCard = await submitAssistantActivity(
    page,
    console,
    member.lane,
    "![malformed raster data](data:image/png;base64,%%%)",
    activityReplay,
  );
  await malformedCard.waitFor({ state: "attached", timeout: 10_000 });
  assert.equal(await malformedCard.locator("a.message-image").count(), 0,
    "malformed raster base64 has no fallback opening path");
  await assertImagePreview(malformedCard.locator("span.message-image"),
    "malformed raster preview", 252);

  const attachmentText = "unlinked attachment preview";
  const attachment = await activityReplay.injectAgentProse(page, member.lane, attachmentText, [{
    name: "attachment.png",
    content_type: "image/png",
    data_url: `data:image/png;base64,${PIXEL_PNG}`,
  }]);
  const attachmentCard = console.locator(`[data-mosaic-card-key="${attachment.key}"]`);
  await attachmentCard.waitFor({ state: "attached", timeout: 10_000 });
  const attachmentPreview = attachmentCard.locator(".message-attachments .message-image");
  await assertImagePreview(attachmentPreview, "unlinked attachment preview", 140);
  await assertFullSizeTarget(attachmentPreview, "unlinked attachment preview", "about:blank");

  const originalViewport = page.viewportSize();
  await page.setViewportSize({ width: 390, height: 844 });
  await waitForMosaicReady(page);
  await assertImagePreview(multiPreviews.first(), "narrow multi-image preview", 252);
  assertOneImageOnlyTier(
    await imageOnlyTierGeometry(imageOnlyCard),
    await imageOnlyTierGeometry(multiImageCard),
    "blank-line-separated narrow previews",
  );
  const narrowStack = await multiImageCard.locator("p.message-image-stack").evaluate((stack) => ({
    clientWidth: stack.clientWidth,
    scrollWidth: stack.scrollWidth,
    cardWidth: stack.closest(".mosaic-message-card").clientWidth,
  }));
  assert.ok(narrowStack.clientWidth <= narrowStack.cardWidth,
    `the narrow image stack stays inside its card: ${JSON.stringify(narrowStack)}`);
  assert.ok(narrowStack.scrollWidth >= narrowStack.clientWidth,
    "the narrow multi-image arrangement retains bounded horizontal access");
  await page.setViewportSize(originalViewport);
  await waitForMosaicReady(page);
  await assertMosaicLattice(page, true);
}

export async function assertMosaicLattice(page, requireBothReadPaths) {
  await waitForMosaicReady(page);
  const snapshot = await page.locator('[data-pane="agent-console"]').evaluate((console) => ({
    activities: [...console.querySelectorAll(".team-activity-grid")].map((activity) => ({
      columnCount: getComputedStyle(activity).columnCount,
      columnWidth: getComputedStyle(activity).columnWidth,
    })),
    planes: [...console.querySelectorAll(".lane-mosaic-plane")].map((plane) => {
      const planeStyle = getComputedStyle(plane);
      return {
        trackCount: Number(plane.dataset.trackCount),
        baseSpan: Number(plane.dataset.baseSpan),
        minLane: Number(plane.dataset.minLane),
        minCardRows: Number(plane.dataset.minCardRows),
        maxRow: Number(plane.dataset.maxRow),
        module: Number(plane.dataset.module),
        columnGap: Number.parseFloat(planeStyle.columnGap),
        rowGap: Number.parseFloat(planeStyle.rowGap),
        width: plane.clientWidth,
        tallThreshold: Number(plane.dataset.tallThreshold),
        cards: [...plane.querySelectorAll(".mosaic-message-card")].map((card) => {
          const box = card.getBoundingClientRect();
          const style = getComputedStyle(card);
          const footer = card.querySelector(".message-footer")?.getBoundingClientRect();
          return {
            key: card.dataset.mosaicCardKey,
            kind: card.dataset.cardKind,
            lane: card.dataset.agentLane,
            creationIndex: Number(card.dataset.creationIndex),
            track: Number(card.dataset.track),
            span: Number(card.dataset.span),
            base: Number(card.dataset.base),
            row: Number(card.dataset.row),
            rows: Number(card.dataset.rows),
            reads: Number(card.dataset.measurementReads),
            measuredSpans: card.dataset.measuredSpans.split(",").filter(Boolean).map(Number),
            measuredWidths: card.dataset.measuredWidths.split(",").filter(Boolean).map(Number),
            heights: card.dataset.measuredHeights.split(",").filter(Boolean).map(Number),
            candidateSpans: card.dataset.candidateSpans.split(",").filter(Boolean).map(Number),
            measureSpan: Number(card.dataset.measureSpan),
            measureProperty: style.getPropertyValue("--mosaic-measure-span"),
            measurementActive: card.hasAttribute("data-mosaic-measurement-active"),
            tallWitness: card.querySelector("h2")?.textContent?.trim() === "Measured lattice witness",
            textOnly: card.querySelector(".message-image") === null,
            columnStart: Number(style.gridColumnStart),
            columnEnd: style.gridColumnEnd,
            rowStart: Number(style.gridRowStart),
            rowEnd: style.gridRowEnd,
            top: Number.parseFloat(style.top),
            height: Number.parseFloat(style.height),
            box: {
              left: box.left,
              right: box.right,
              top: box.top,
              bottom: box.bottom,
            },
            clientHeight: card.clientHeight,
            scrollHeight: card.scrollHeight,
            footerInset: footer ? box.bottom - footer.bottom : null,
            paddingBottom: Number.parseFloat(style.paddingBottom),
          };
        }),
      };
    }),
  }));

  assert.ok(snapshot.activities.length > 0, "the live activity region is mounted");
  assert.ok(
    snapshot.activities.every(
      (activity) => activity.columnCount === "auto" && activity.columnWidth === "auto",
    ),
    "the live activity region uses no CSS multi-column flow",
  );
  const readPaths = new Set();
  const kinds = new Set();
  const placements = [];
  let wideTextCommits = 0;
  for (const plane of snapshot.planes) {
    assert.equal(plane.trackCount, 12, "the browser renders the twelve-track lattice");
    assert.equal(plane.columnGap, 12, "the horizontal lattice keeps the predecessor gap");
    assert.equal(plane.rowGap, plane.columnGap, "the vertical lattice shares that grid rhythm");
    assert.equal(plane.minLane, 272, "the browser exposes the predecessor minimum lane width");
    assert.equal(plane.minCardRows, 3, "ordinary cards reserve three vertical modules");
    assert.deepEqual(
      plane.cards.map((card) => card.creationIndex),
      plane.cards.map((_, index) => index),
      "DOM order is the stable creation order submitted to Rust",
    );
    for (const card of plane.cards) {
      assert.ok(card.key, "every Mosaic card exposes its stable identity");
      const measuredBaseSpan = card.candidateSpans[0];
      const wideSpan = Math.min(plane.trackCount, measuredBaseSpan * 2);
      const wideEligible = wideSpan > measuredBaseSpan && card.heights[0] >= plane.tallThreshold;
      const expectedSpans = wideEligible ? [measuredBaseSpan, wideSpan] : [measuredBaseSpan];
      const latchedRead = expectedSpans.includes(card.span) ? 0 : 1;
      const expectedMeasuredSpans = latchedRead
        ? [...expectedSpans, card.span]
        : expectedSpans;
      assert.equal(
        card.reads,
        expectedSpans.length + latchedRead,
        "height reads follow the bounded policy, including at most one frozen-span read",
      );
      assert.deepEqual(
        card.measuredSpans,
        expectedMeasuredSpans,
        "every DOM read reports the span applied through the measurement layout channel",
      );
      assert.equal(card.measuredWidths.length, card.reads, "every DOM read records its real width");
      assert.equal(card.heights.length, card.reads, "every instrumented read records one height");
      assert.deepEqual(card.candidateSpans, expectedSpans, "height alone admits the wide tier");
      assert.equal(card.measureSpan, measuredBaseSpan, "measurement restores the base diagnostic span");
      assert.equal(
        Number(card.measureProperty),
        card.measureSpan,
        "the restored diagnostic and CSP-safe measurement property agree",
      );
      assert.equal(card.measurementActive, false, "measurement mode exits before committed paint");
      const trackWidth = (
        plane.width - plane.columnGap * (plane.trackCount - 1)
      ) / plane.trackCount;
      for (const [index, span] of card.measuredSpans.entries()) {
        const expectedWidth = trackWidth * span + plane.columnGap * (span - 1);
        assert.ok(
          Math.abs(card.measuredWidths[index] - expectedWidth) <= 1,
          `measurement read ${index} uses span ${span}'s rendered width: ${JSON.stringify({
            expectedWidth,
            plane,
            card,
          })}`,
        );
      }
      if (card.tallWitness) {
        assert.equal(card.measuredSpans.length, 2, "the tall text witness exercises both candidates");
        assert.ok(
          card.measuredWidths[1] > card.measuredWidths[0],
          "the tall text witness measures a genuinely wider second candidate",
        );
        assert.ok(
          card.heights[1] < card.heights[0],
          "the tall text witness becomes naturally shorter at double width",
        );
        const rowsFor = (height) => Math.max(
          plane.minCardRows,
          Math.ceil((height + plane.columnGap) / plane.module),
        );
        const baseRows = rowsFor(card.heights[0]);
        const wideRows = rowsFor(card.heights[1]);
        assert.ok(wideRows < baseRows, "the wide text witness has a smaller natural row count");
      }
      if (
        card.textOnly
          && card.measuredSpans.length >= 2
          && card.span === card.measuredSpans[1]
          && card.heights[1] < card.heights[0]
      ) {
        const wideRows = Math.max(
          plane.minCardRows,
          Math.ceil((card.heights[1] + plane.columnGap) / plane.module),
        );
        assert.equal(card.rows, wideRows, "a wide text card commits its measured wide row count");
        assert.ok(
          Math.abs(card.footerInset - card.paddingBottom) <= 1,
          "a wide text card leaves only its natural padding below the footer",
        );
        wideTextCommits += 1;
      }
      assert.ok(card.track >= 0 && card.track + card.span <= plane.trackCount);
      if (card.kind === "compaction") {
        assert.ok(card.rows >= 1, "a compaction divider keeps a nonzero row reservation");
      } else {
        assert.ok(card.rows >= plane.minCardRows);
      }
      assert.ok(
        card.box.right - card.box.left + 0.5 >= Math.min(plane.minLane, plane.width),
        `Mosaic card keeps the responsive minimum lane width: ${JSON.stringify(card)}`,
      );
      assert.equal(card.columnStart, card.track + 1);
      assert.equal(card.columnEnd, `span ${card.span}`);
      assert.equal(card.row, card.base, "the stable row instrument is the committed base");
      assert.ok(
        Math.abs(card.top - (plane.maxRow - card.base - card.rows) * plane.module) <= 0.5,
        `the plane frontier positions the stable card: ${JSON.stringify({ plane, card })}`,
      );
      assert.ok(
        Math.abs(card.height - (card.rows * plane.module - plane.columnGap)) <= 0.5,
        `the committed reservation fixes card height: ${JSON.stringify({ plane, card })}`,
      );
      assert.ok(
        card.clientHeight + 1 >= card.scrollHeight,
        `Mosaic card contains its own content: ${JSON.stringify(card)}`,
      );
      readPaths.add(card.reads);
      kinds.add(card.kind);
      placements.push(card);
    }
    for (let left = 0; left < plane.cards.length; left += 1) {
      for (let right = left + 1; right < plane.cards.length; right += 1) {
        const one = plane.cards[left].box;
        const two = plane.cards[right].box;
        const separated = one.right <= two.left + 0.5 || two.right <= one.left + 0.5
          || one.bottom <= two.top + 0.5 || two.bottom <= one.top + 0.5;
        assert.ok(separated, `Mosaic cards ${left} and ${right} do not overlap`);
      }
    }
  }
  assert.ok(kinds.has("agent-message"), "agent-authored cards use the measured policy");
  assert.ok(readPaths.has(1), "a short card takes exactly one DOM height read");
  if (requireBothReadPaths) {
    assert.ok(readPaths.has(2), "a tall card takes exactly two DOM height reads");
    assert.ok(wideTextCommits > 0, "a text card commits its shorter wide measurement");
  }
  return placements;
}

export async function waitForMosaicReady(page) {
  await waitForPage(page, "mosaic measurements settle", TRANSITION_MS, () => {
    const planes = [...document.querySelectorAll(
      '[data-pane="agent-console"] .lane-mosaic-plane',
    )];
    return planes.length > 0 && planes.every(
      (plane) => plane.dataset.measurementPhase === "ready"
        && plane.dataset.measurementPending === "0",
    );
  });
}
