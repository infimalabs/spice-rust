import assert from "node:assert/strict";

import {
  chooseCanvasView,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";
import {
  assertCanonicalPlacementsStable,
  captureCanonicalPlacements,
} from "./mosaic-placement-stability.mjs";

const PIXEL_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
// Span the fixture's one-second delivery cadence before arming exact lattice probes.
const ACTIVITY_QUIET_MS = 1_500;
const ACTIVITY_DEADLINE_MS = 15_000;
export async function mosaicScrollSnapshot(plane) {
  const snapshot = await plane.evaluate((element) => {
    const viewport = element.getBoundingClientRect();
    const cards = [...element.querySelectorAll(
      '.mosaic-message-card[data-layout-phase="committed"]',
    )].map((card) => {
      const box = card.getBoundingClientRect();
      return {
        key: card.dataset.mosaicCardKey,
        activity: card.dataset.agentActivity,
        height: box.height,
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
  const wheelTarget = await plane.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const left = Math.max(0, Math.ceil(bounds.left));
    const right = Math.min(innerWidth, Math.floor(bounds.right));
    const top = Math.max(0, Math.ceil(bounds.top));
    const bottom = Math.min(innerHeight, Math.floor(bounds.bottom));
    for (let y = top + 1; y < bottom; y += 8) {
      for (let x = left + 1; x < right; x += 8) {
        if (document.elementFromPoint(x, y)?.closest(".lane-mosaic-plane") === element) {
          return { point: { x, y } };
        }
      }
    }
    const describe = (node) => {
      const box = node?.getBoundingClientRect();
      const style = node ? getComputedStyle(node) : null;
      return box && style ? {
        bottom: box.bottom,
        display: style.display,
        flex: style.flex,
        height: box.height,
        overflow: style.overflow,
        top: box.top,
      } : null;
    };
    return {
      diagnostics: {
        activity: describe(element.closest(".team-activity-grid")),
        compose: describe(element.closest(".team-compose")),
        content: describe(element.closest(".content")),
        fleet: describe(element.closest(".team-fleet")),
        mode: describe(element.closest(".team-mode-pane")),
        pane: describe(element.closest(".team-pane")),
        plane: describe(element),
        shell: describe(element.closest(".shell")),
        surface: describe(element.closest(".team-surface")),
        viewport: { height: innerHeight, width: innerWidth },
        workspace: describe(element.closest(".workspace")),
      },
      point: null,
    };
  });
  assert.ok(
    wheelTarget.point,
    `the Mosaic exposes a pixel for physical wheel input: ${JSON.stringify(wheelTarget.diagnostics)}`,
  );
  await page.mouse.move(wheelTarget.point.x, wheelTarget.point.y);
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

// The plane anchors on the committed card nearest its top edge and, after a
// change, scrolls so that same card keeps its viewport offset. The card is
// looked up by key here because a reflow may stack a neighbour above it that
// now straddles the top edge; the promise is about the card that was anchored,
// not about which card is nearest afterwards.
export function assertMosaicAnchorPreserved(before, after, label) {
  const anchored = after.cards.find((card) => card.key === before.anchor.key);
  assert.ok(
    anchored,
    `${label} retains the anchored card: ${JSON.stringify({ before, after })}`,
  );
  assert.ok(
    Math.abs(anchored.offset - before.anchor.offset) <= 1,
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

export async function assertModeSwitchPreservesDeferredMosaic(
  page,
  console,
  member,
  activityReplay,
) {
  const plane = console.locator(".lane-mosaic-plane");
  await settleMosaicActivity(page, plane);
  const before = await plane.evaluate((element) => {
    element.dataset.modeSwitchIdentity = crypto.randomUUID();
    return {
      identity: element.dataset.modeSwitchIdentity,
      commits: Number(element.dataset.planeCommits),
      committedWidth: Number(element.dataset.committedWidth),
      keys: [...element.querySelectorAll(".mosaic-message-card")]
        .map((card) => card.dataset.mosaicCardKey),
    };
  });
  const placement = await armPlacementProbe(plane);
  const origin = new URL(page.url()).origin;
  const imageCases = [];
  for (const outcome of ["load", "error"]) {
    const url = `${origin}/mosaic-hidden-${outcome}-${crypto.randomUUID()}.png`;
    let release;
    let arrived;
    const released = new Promise((resolve) => { release = resolve; });
    const requested = new Promise((resolve) => { arrived = resolve; });
    await page.route(url, async (route) => {
      arrived();
      await released;
      await route.fulfill({
        contentType: "image/png",
        body: outcome === "load" ? Buffer.from(PIXEL_PNG, "base64") : "not an image",
      });
    });
    imageCases.push({ outcome, url, release, requested });
  }

  await chooseCanvasView(page, "Work");
  await waitForPage(page, "hidden Mosaic observes zero width", TRANSITION_MS, () => {
    const workspace = document.querySelector(".mosaic-workspace");
    const plane = workspace?.querySelector(".lane-mosaic-plane");
    return workspace?.hidden === true && Number(plane?.dataset.observedWidth) < 24;
  });
  const history = await activityReplay.injectAgentHistory(
    page,
    member.lane,
    "history response received while Lanes is hidden",
  );
  const boundary = await activityReplay.injectCompaction(page, member.lane);
  const images = [];
  for (const imageCase of imageCases) {
    const image = await activityReplay.injectAgentProse(
      page,
      member.lane,
      `hidden image ${imageCase.outcome}\n\n![hidden ${imageCase.outcome}](${imageCase.url})`,
    );
    await imageCase.requested;
    images.push({ ...imageCase, key: image.key });
  }
  for (const image of images) {
    image.release();
    await waitForPage(page, `hidden image ${image.outcome} is reduced immediately`, TRANSITION_MS,
      (expected) => document.querySelector(`[data-mosaic-card-key="${expected.key}"] img`)
        ?.getAttribute("data-mosaic-image-state") === expected.state,
      { key: image.key, state: image.outcome === "load" ? "loaded" : "error" });
  }
  const received = [history.key, boundary.key, ...images.map((image) => image.key)];
  const hidden = await plane.evaluate((element, expected) => {
    const probe = element.__mosaicPlacementObserver;
    probe.observer.takeRecords().forEach(probe.record);
    const keys = [...element.querySelectorAll(".mosaic-message-card")]
      .map((card) => card.dataset.mosaicCardKey);
    return {
      identity: element.dataset.modeSwitchIdentity,
      commits: Number(element.dataset.planeCommits),
      committedWidth: Number(element.dataset.committedWidth),
      observedWidth: Number(element.dataset.observedWidth),
      pending: Number(element.dataset.measurementPending),
      retained: expected.before.every((key) => keys.includes(key)),
      received: expected.received.every((key) => keys.includes(key)),
      writes: [...probe.writes],
    };
  }, { before: before.keys, received });
  assert.equal(hidden.identity, before.identity, "the hidden Mosaic plane is never remounted");
  assert.equal(hidden.commits, before.commits, "hidden arrivals commit no guessed geometry");
  assert.equal(hidden.committedWidth, before.committedWidth,
    "the zero-width host retains its last measurable geometry");
  assert.ok(hidden.observedWidth < 24, "the hidden host remains unmeasurable");
  assert.ok(hidden.pending >= received.length, "hidden semantic arrivals await only measurement");
  assert.ok(hidden.retained && hidden.received,
    "hidden reduction retains prior cards and materializes every background arrival");
  assert.deepEqual(hidden.writes, [], "the zero-width host performs no card geometry writes");

  await chooseCanvasView(page, "Chat");
  await waitForPage(page, "reveal measures one complete hidden batch", TRANSITION_MS,
    (expected) => {
      const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
      const cards = [...(plane?.querySelectorAll(".mosaic-message-card") ?? [])];
      return Number(plane?.dataset.observedWidth) >= 24
        && plane?.dataset.measurementPending === "0"
        && expected.every((key) => cards.some((card) => (
          card.dataset.mosaicCardKey === key && card.dataset.layoutPhase === "committed"
        )));
    }, received);
  const revealed = await finishPlacementProbe(plane, placement);
  assertPlacementRetained(placement, revealed, "a hidden background batch reveal");
  assert.equal(revealed.commits, before.commits + 1,
    "the complete hidden batch produces one snap correction");
  assert.equal(await plane.getAttribute("data-mode-switch-identity"), before.identity,
    "reveal retains the original Mosaic host node");

  const stable = await armPlacementProbe(plane);
  await chooseCanvasView(page, "Work");
  await waitForPage(page, "repeated hide returns to zero width", TRANSITION_MS, () => (
    Number(document.querySelector(".mosaic-workspace .lane-mosaic-plane")?.dataset.observedWidth) < 24
  ));
  await chooseCanvasView(page, "Chat");
  await waitForPage(page, "repeated reveal restores the committed width", TRANSITION_MS,
    (expected) => Number(
      document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane')
        ?.dataset.committedWidth,
    ) === expected,
    before.committedWidth);
  const repeated = await finishPlacementProbe(plane, stable);
  assertUnmoved(stable, repeated, "a no-op global hide and reveal");
  assert.equal(repeated.commits, revealed.commits, "a no-op reveal commits no plane");
  assert.equal(await plane.locator(`[data-mosaic-card-key="${boundary.key}"]`).count(), 1,
    "the hidden compaction divider retains one stable identity");
  const continuity = await plane.evaluate((element, expectedKeys) => {
    const token = crypto.randomUUID();
    const cards = expectedKeys.map((key) => {
      const card = element.querySelector(`[data-mosaic-card-key="${key}"]`);
      if (!card || card.dataset.layoutPhase !== "committed") {
        throw new Error(`post-hidden continuity card is absent: ${key}`);
      }
      card.__mosaicPostHiddenIdentity = token;
      return { key, creationIndex: card.dataset.creationIndex };
    });
    return { token, cards };
  }, before.keys);
  for (const imageCase of imageCases) await page.unroute(imageCase.url);
  return continuity;
}

export async function assertRetainedMosaicIdentity(plane, armed, label) {
  const current = await plane.evaluate((element, expected) => expected.cards.map((prior) => {
    const card = element.querySelector(`[data-mosaic-card-key="${prior.key}"]`);
    return {
      key: prior.key,
      creationIndex: card?.dataset.creationIndex ?? null,
      retained: card?.__mosaicPostHiddenIdentity === expected.token,
    };
  }), armed);
  assert.deepEqual(
    current,
    armed.cards.map((card) => ({ ...card, retained: true })),
    `${label} retains post-hidden card nodes and semantic creation indexes`,
  );
}

export async function armPlacementProbe(plane, excluded = []) {
  const armed = await plane.evaluate((element, excludedKeys) => {
    const token = crypto.randomUUID();
    const attributes = [
      "data-creation-index",
      "data-track",
      "data-span",
      "data-base",
      "data-row",
      "data-rows",
      "style",
    ];
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
      maxRow: Number(element.dataset.maxRow),
      planeRows: Number(element.dataset.planeRows),
      scrollHeight: element.scrollHeight,
      cards: cards.map((card) => {
        const box = card.getBoundingClientRect();
        return {
          key: card.dataset.mosaicCardKey,
          creationIndex: card.dataset.creationIndex,
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
  return {
    ...armed,
    canonical: await captureCanonicalPlacements(
      plane,
      armed.cards.map((card) => card.key),
    ),
  };
}

export async function finishPlacementProbe(plane, armed) {
  const canonical = await captureCanonicalPlacements(
    plane,
    armed.canonical.map((card) => card.key),
  );
  const finished = await plane.evaluate((element, expected) => {
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
      maxRow: Number(element.dataset.maxRow),
      planeRows: Number(element.dataset.planeRows),
      scrollHeight: element.scrollHeight,
      cards: cards
        .filter((card) => probe.keys.includes(card.dataset.mosaicCardKey))
        .map((card) => {
          const box = card.getBoundingClientRect();
          return {
            key: card.dataset.mosaicCardKey,
            creationIndex: card.dataset.creationIndex,
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
  return { ...finished, canonical };
}

function assertCreationIndexesStable(armed, finished, label) {
  const earlier = new Map(armed.cards.map((card) => [card.key, card.creationIndex]));
  for (const card of finished.cards) {
    assert.equal(
      card.creationIndex,
      earlier.get(card.key),
      `${label} preserves creation index for ${JSON.stringify(card.key)}`,
    );
  }
}

export function assertUnmoved(armed, finished, label) {
  assertCanonicalProbeStable(armed, finished, label);
  assert.ok(finished.retained, `${label} retains every committed card node`);
  assertCreationIndexesStable(armed, finished, label);
  assert.deepEqual(finished.writes, [], `${label} writes no card placement`);
  assert.deepEqual(
    finished.cards,
    armed.cards,
    `${label} preserves DOM order, committed placement, and bounding geometry`,
  );
}

export function assertPlacementStable(armed, finished, label) {
  const withoutOrder = (cards) => cards
    .map(({ order: _order, ...card }) => card)
    .sort((left, right) => left.key.localeCompare(right.key));
  assertCanonicalProbeStable(armed, finished, label);
  assert.ok(finished.retained, `${label} retains every committed card node`);
  assertCreationIndexesStable(armed, finished, label);
  assert.deepEqual(finished.writes, [], `${label} writes no card placement`);
  assert.deepEqual(
    withoutOrder(finished.cards),
    withoutOrder(armed.cards),
    `${label} preserves committed placement and viewport geometry`,
  );
}

export function assertPlacementRetained(armed, finished, label) {
  const withoutBox = (cards) => cards.map(({ box: _box, ...card }) => card);
  assertCanonicalProbeStable(armed, finished, label);
  assert.ok(finished.retained, `${label} retains every committed card node`);
  assertCreationIndexesStable(armed, finished, label);
  assert.deepEqual(finished.writes, [], `${label} writes no card placement`);
  assert.deepEqual(
    withoutBox(finished.cards),
    withoutBox(armed.cards),
    `${label} preserves DOM order and committed placement`,
  );
}

export function assertCanonicalProbeStable(armed, finished, label) {
  assertCanonicalPlacementsStable(armed.canonical, finished.canonical, label);
}

export async function settleMosaicActivity(page, plane) {
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

export async function waitForMosaicReady(page) {
  await waitForPage(page, "mosaic measurements settle", TRANSITION_MS, () => {
    const planes = [...document.querySelectorAll(
      '[data-pane="agent-console"] .lane-mosaic-plane',
    )];
    return planes.length > 0 && planes.every(
      (plane) => plane.dataset.measurementPhase === "ready"
        && plane.dataset.measurementPending === "0"
        && Math.abs(Number(plane.dataset.committedWidth)
          - plane.querySelector(":scope > .lane-mosaic-track").clientWidth) <= 1,
    );
  });
}
