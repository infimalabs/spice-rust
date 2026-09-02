import assert from "node:assert/strict";

import {
  chooseCanvasView,
  reply,
  RECONNECT_MS,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";
import {
  assertNoUnavailableToolTerminal,
  assertUnavailableToolTerminal,
} from "./compaction-boundary-reference.mjs";
import {
  assertImageLifecycle,
  assertOwningImageRevision,
  imageCardInvariant,
} from "./image-lifecycle.mjs";

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
        fleet: describe(element.closest(".lineage-fleet")),
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

async function hydrationPlacement(plane) {
  return plane.evaluate((element) => ({
    commits: Number(element.dataset.planeCommits),
    transition: element.dataset.planeTransition,
    cards: [...element.querySelectorAll('.mosaic-message-card[data-layout-phase="committed"]')]
      .map((card) => ({
        key: card.dataset.mosaicCardKey,
        track: card.dataset.track,
        span: card.dataset.span,
        base: card.dataset.base,
        rows: card.dataset.rows,
      })),
  }));
}

async function waitForHydration(page, expectedKeys) {
  await waitForPage(page, "staged Mosaic hydration settles in total chronology", TRANSITION_MS,
    (expected) => {
      const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
      if (plane?.dataset.measurementPhase !== "ready"
        || plane.dataset.measurementPending !== "0") return false;
      const cards = [...plane.querySelectorAll('.mosaic-message-card[data-layout-phase="committed"]')];
      return cards.length === expected.length
        && cards.every((card, index) => card.dataset.mosaicCardKey === expected[index]);
    }, expectedKeys);
}

async function assertHydrationEdge(page, plane, hydration, initial, armed, snapshot) {
  const edge = await finishPlacementProbe(plane, armed);
  assert.ok(edge.retained, "edge hydration retains every committed card node");
  assert.deepEqual(edge.writes, [], "edge hydration writes no retained card placement");
  const stablePlacement = ({ key, track, span, base, row, rows }) => (
    { key, track, span, base, row, rows }
  );
  assert.deepEqual(
    edge.cards.map(stablePlacement),
    armed.cards.map(stablePlacement),
    "edge hydration preserves every retained committed placement",
  );
  assert.equal(snapshot.cards.at(-1)?.key, hydration.newestKey,
    "the newest transport edge appends its DOM node");
  const visual = await plane.evaluate((element, { newestKey, retainedKeys }) => {
    const boxFor = (key) => element
      .querySelector(`[data-mosaic-card-key="${key}"]`)
      ?.getBoundingClientRect();
    const newest = boxFor(newestKey);
    const retained = retainedKeys.map(boxFor).filter(Boolean);
    return {
      newest: newest ? { bottom: newest.bottom, top: newest.top } : null,
      retained: retained.map((box) => ({ bottom: box.bottom, top: box.top })),
    };
  }, { newestKey: hydration.newestKey, retainedKeys: initial.cards.map((card) => card.key) });
  assert.ok(visual.newest && visual.retained.length === initial.cards.length,
    `edge hydration renders every visual witness: ${JSON.stringify(visual)}`);
  assert.ok(
    visual.newest.top <= Math.min(...visual.retained.map((box) => box.top)) + 1,
    `the newest transport edge leads the visual stack: ${JSON.stringify(visual)}`,
  );
  return {
    committedWidth: edge.committedWidth,
    commits: edge.commits,
    retained: edge.retained,
    retainedCards: edge.cards,
    transition: edge.transition,
    visual,
    writes: edge.writes,
  };
}

export async function assertStagedMosaicHydration(page, team, activityReplay, inspectOneShot) {
  assert.equal(typeof inspectOneShot, "function", "hydration supplies an integrated proof sink");
  const plane = team.locator(".lane-mosaic-plane");
  const hydration = activityReplay.stagedHydration();
  await plane.evaluate((element) => {
    window.__stagedHydrationViolations = [];
    window.__stagedHydrationExpected = [];
    const inspect = (expected) => {
      if (element.dataset.measurementPhase !== "ready"
        || element.dataset.measurementPending !== "0") return;
      const cards = [...element.querySelectorAll('.mosaic-message-card[data-layout-phase="committed"]')];
      const keys = cards.map((card) => card.dataset.mosaicCardKey);
      if (expected.length > 0 && JSON.stringify(keys) !== JSON.stringify(expected)) {
        window.__stagedHydrationViolations.push({ expected, keys });
      }
      if (new Set(keys).size !== keys.length) {
        window.__stagedHydrationViolations.push({ duplicateKeys: keys });
      }
    };
    const observer = new MutationObserver((mutations) => {
      if (!mutations.some((mutation) => mutation.target === element
        && mutation.attributeName === "data-plane-commits")) return;
      const expected = [...window.__stagedHydrationExpected];
      inspect(expected);
    });
    observer.observe(element, { attributes: true, attributeFilter: ["data-plane-commits"] });
    element.__stagedHydrationObserver = observer;
  });

  const apply = async (messages, expectedTransition = null) => {
    const keys = hydration.keys(messages);
    await page.evaluate((expected) => { window.__stagedHydrationExpected = expected; }, keys);
    await hydration.send(page, messages);
    await waitForHydration(page, keys);
    const snapshot = await hydrationPlacement(plane);
    if (expectedTransition) assert.equal(snapshot.transition, expectedTransition);
    return snapshot;
  };

  const initial = await apply(hydration.stages[0]);
  const edgeProbe = await armPlacementProbe(plane);
  const edgeSnapshot = await apply(hydration.stages[1], "append");
  const edge = await assertHydrationEdge(
    page, plane, hydration, initial, edgeProbe, edgeSnapshot,
  );
  const staged = await apply(hydration.stages[2], "epoch-replay");
  assert.deepEqual(
    staged.cards.find((card) => card.key === hydration.boundaryKey),
    edgeSnapshot.cards.find((card) => card.key === hydration.boundaryKey),
    "epoch hydration preserves the compaction divider position and extent",
  );
  const commits = staged.commits;
  await hydration.send(page, hydration.stages[2]);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
  assert.equal(Number(await plane.getAttribute("data-plane-commits")), commits,
    "an unchanged complete hydration snapshot commits no plane");

  await page.evaluate(() => { window.__stagedHydrationExpected = []; });
  await hydration.send(page, []);
  await waitForPage(page, "the one-shot hydration reference starts empty", TRANSITION_MS,
    () => document.querySelectorAll(
      '[data-pane="agent-console"] .mosaic-message-card[data-layout-phase="committed"]',
    ).length === 0);
  const oneShot = await apply(hydration.chronological);
  assert.deepEqual(oneShot.cards, staged.cards,
    "staged hydration converges to byte-identical one-shot Mosaic placement");
  assert.deepEqual(await plane.evaluate((element) => {
    element.__stagedHydrationObserver.disconnect();
    delete element.__stagedHydrationObserver;
    return window.__stagedHydrationViolations;
  }), [], "hydration never exposes duplicate or misordered cards after settlement");
  await inspectOneShot({
    edge,
    oneShot,
    staged,
    stageSizes: hydration.stages.map((stage) => stage.length),
  });
  await activityReplay.restoreProseSnapshot(page);
  await waitForMosaicReady(page);
}

export async function assertCompactionBoundaryPersistence(page, seeded, boundaryKeys) {
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Agents");
  const restoredTeam = page.locator('[data-pane="agent-console"]');
  await restoredTeam.getByRole("tab", { name: /^Compose/ }).click();
  const restoredUnresolved = restoredTeam.locator(
    `[data-mosaic-card-key="${boundaryKeys.unresolved}"]`,
  );
  const restoredEmpty = restoredTeam.locator(
    `[data-mosaic-card-key="${boundaryKeys.empty}"]`,
  );
  await Promise.all([
    restoredUnresolved.waitFor({ timeout: RECONNECT_MS }),
    restoredEmpty.waitFor({ timeout: RECONNECT_MS }),
  ]);
  assert.equal(
    await restoredTeam.locator(
      `.mosaic-message-card[data-card-kind="compaction"][data-agent-lane="${seeded.lane}"]`,
    ).count(),
    2,
    "reload restores both compaction boundaries exactly once",
  );
  for (const boundary of [restoredUnresolved, restoredEmpty]) {
    assert.equal(await boundary.getAttribute("data-span"), "12");
    assert.equal(
      (await boundary.locator(".message-key").innerText()).trim(),
      "compacted context",
    );
  }
  await assertUnavailableToolTerminal(
    restoredUnresolved,
    "reloaded unresolved compaction boundary",
    "Bash: cargo test --locked -p spice",
  );
  await assertNoUnavailableToolTerminal(
    restoredEmpty,
    "reloaded empty compaction boundary",
    seeded.agentAttribution,
  );

  const socketCount = await page.evaluate(() => window.__canvasSockets?.length ?? 0);
  assert.ok(socketCount > 0, "the browser fixture captures the active Canvas socket");
  await page.evaluate(() => window.__canvasSockets.at(-1).close(4000, "compaction reconnect"));
  await waitForPage(
    page,
    "compaction boundary survives an in-place reconnect",
    RECONNECT_MS,
    ({ expectedKeys, previousSockets }) => (
      (window.__canvasSockets?.length ?? 0) > previousSockets
      && expectedKeys.every((expectedKey) => (
        document.querySelectorAll(`[data-mosaic-card-key="${expectedKey}"]`).length === 1
        && document.querySelector(`[data-mosaic-card-key="${expectedKey}"]`)
          ?.dataset.layoutPhase === "committed"
      ))
    ),
    { expectedKeys: Object.values(boundaryKeys), previousSockets: socketCount },
  );
  await assertUnavailableToolTerminal(
    restoredUnresolved,
    "reconnected unresolved compaction boundary",
    "Bash: cargo test --locked -p spice",
  );
  await assertNoUnavailableToolTerminal(
    restoredEmpty,
    "reconnected empty compaction boundary",
    seeded.agentAttribution,
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

  await chooseCanvasView(page, "Flow");
  await waitForPage(page, "hidden Mosaic observes zero width", TRANSITION_MS, () => {
    const workspace = document.querySelector(".mosaic-workspace");
    const plane = workspace?.querySelector(".lane-mosaic-plane");
    return workspace?.hidden === true && Number(plane?.dataset.observedWidth) < 24;
  });
  const history = await activityReplay.injectAgentHistory(
    page,
    member.lane,
    "history response received while Agents is hidden",
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

  await chooseCanvasView(page, "Agents");
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
  await chooseCanvasView(page, "Flow");
  await waitForPage(page, "repeated hide returns to zero width", TRANSITION_MS, () => (
    Number(document.querySelector(".mosaic-workspace .lane-mosaic-plane")?.dataset.observedWidth) < 24
  ));
  await chooseCanvasView(page, "Agents");
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
  for (const imageCase of imageCases) await page.unroute(imageCase.url);
}

export async function armPlacementProbe(plane, excluded = []) {
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
      maxRow: Number(element.dataset.maxRow),
      planeRows: Number(element.dataset.planeRows),
      scrollHeight: element.scrollHeight,
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

export async function finishPlacementProbe(plane, armed) {
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
      maxRow: Number(element.dataset.maxRow),
      planeRows: Number(element.dataset.planeRows),
      scrollHeight: element.scrollHeight,
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

export function assertUnmoved(armed, finished, label) {
  assert.ok(finished.retained, `${label} retains every committed card node`);
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
  assert.ok(finished.retained, `${label} retains every committed card node`);
  assert.deepEqual(finished.writes, [], `${label} writes no card placement`);
  assert.deepEqual(
    withoutOrder(finished.cards),
    withoutOrder(armed.cards),
    `${label} preserves committed placement and viewport geometry`,
  );
}

export function assertPlacementRetained(armed, finished, label) {
  const withoutBox = (cards) => cards.map(({ box: _box, ...card }) => card);
  assert.ok(finished.retained, `${label} retains every committed card node`);
  assert.deepEqual(finished.writes, [], `${label} writes no card placement`);
  assert.deepEqual(
    withoutBox(finished.cards),
    withoutBox(armed.cards),
    `${label} preserves DOM order and committed placement`,
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
  assert.ok(Math.abs(observed.image.width - observed.box.width) <= 1,
    `${label} fills its stable letterbox: ${JSON.stringify(observed)}`);
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
    await assertImageLifecycle(card.locator(".message-image"), `pending remote ${outcome} preview`, "pending");
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
    const invariant = await imageCardInvariant(card);
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
    await assertImageLifecycle(
      card.locator(".message-image"),
      `resolved remote ${outcome} preview`,
      outcome === "load" ? "loaded" : "error",
    );
    await assertOwningImageRevision(card, `late remote ${outcome}`);
    assert.deepEqual(await imageCardInvariant(card), invariant,
      `late remote ${outcome} preserves fingerprint-facing identity, commands, and span latch`);
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
  await waitForPage(page, "already-complete image appends then corrects once", TRANSITION_MS, (expected) => {
    const card = [...document.querySelectorAll(".mosaic-message-card")]
      .find((candidate) => candidate.dataset.mosaicCardKey === expected.key);
    const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return card?.dataset.layoutPhase === "committed"
      && card.dataset.mosaicImageRevision === "1"
      && Number(plane?.dataset.planeCommits) >= expected.commits + 2;
  }, {
    key: await readyCard.getAttribute("data-mosaic-card-key"),
    commits: readyProbe.commits,
  });
  await firstCard.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  const ready = await finishPlacementProbe(plane, readyProbe);
  assert.ok(ready.retained, "an already-complete image retains untouched card nodes");
  assert.deepEqual(ready.writes, [], "an already-complete image writes no untouched placement");
  assert.equal(ready.commits, readyProbe.commits + 2,
    "an already-complete image appends pending then corrects once");

  const embeddedPreview = readyCard.locator("a.message-image");
  await assertImageLifecycle(embeddedPreview, "embedded ordinary preview", "loaded");
  await assertOwningImageRevision(readyCard, "embedded mixed prose and image");
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
  await waitForPage(page, "image-only lifecycle resolves once", TRANSITION_MS, (key) => {
    const card = [...document.querySelectorAll(".mosaic-message-card")]
      .find((candidate) => candidate.dataset.mosaicCardKey === key);
    return card?.dataset.layoutPhase === "committed"
      && card.dataset.mosaicImageRevision === "1";
  }, await imageOnlyCard.getAttribute("data-mosaic-card-key"));
  assert.ok(
    await imageOnlyCard.evaluate((card) => card.classList.contains("mosaic-message-card--image-only")),
    "a pure image message receives the large preview tier",
  );
  await assertImagePreview(imageOnlyCard.locator(".message-image"), "image-only preview", 252);
  await assertImageLifecycle(imageOnlyCard.locator(".message-image"), "image-only preview", "loaded");
  await assertOwningImageRevision(imageOnlyCard, "image-only preview");
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
  await waitForPage(page, "multi-image lifecycle resolves in one card revision", TRANSITION_MS, (key) => {
    const card = [...document.querySelectorAll(".mosaic-message-card")]
      .find((candidate) => candidate.dataset.mosaicCardKey === key);
    return card?.dataset.layoutPhase === "committed"
      && card.dataset.mosaicImageRevision === "1"
      && [...card.querySelectorAll("img")].every((image) => image.dataset.mosaicImageState === "loaded");
  }, await multiImageCard.getAttribute("data-mosaic-card-key"));
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
  await assertImageLifecycle(multiPreviews.first(), "multi-image preview", "loaded");
  await assertOwningImageRevision(multiImageCard, "multi-image preview stack");
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
  await assertImageLifecycle(malformedCard.locator("span.message-image"),
    "malformed raster preview", "error");

  const attachmentText = "unlinked attachment preview";
  const attachment = await activityReplay.injectAgentProse(page, member.lane, attachmentText, [{
    name: "attachment.png",
    content_type: "image/png",
    data_url: `data:image/png;base64,${PIXEL_PNG}`,
  }]);
  const attachmentCard = console.locator(`[data-mosaic-card-key="${attachment.key}"]`);
  await attachmentCard.waitFor({ state: "attached", timeout: 10_000 });
  const attachmentPreview = attachmentCard.locator(".message-attachments .message-image");
  await waitForPage(page, "unlinked attachment resolves once", TRANSITION_MS, (key) => {
    const card = [...document.querySelectorAll(".mosaic-message-card")]
      .find((candidate) => candidate.dataset.mosaicCardKey === key);
    return card?.dataset.layoutPhase === "committed"
      && card.dataset.mosaicImageRevision === "1";
  }, attachment.key);
  await assertImagePreview(attachmentPreview, "unlinked attachment preview", 140);
  await assertImageLifecycle(attachmentPreview, "unlinked attachment preview", "loaded");
  await assertOwningImageRevision(attachmentCard, "unlinked attachment preview");
  assert.ok(await attachmentPreview.locator("img").getAttribute("data-media-identity"),
    "the loaded attachment retains its admitted media identity");
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
    const creationIndices = plane.cards.map((card) => card.creationIndex);
    assert.ok(
      creationIndices.every((index, position) => Number.isSafeInteger(index)
        && (position === 0 || index > creationIndices[position - 1])),
      `DOM order follows unique monotonic Rust creation labels: ${JSON.stringify(creationIndices)}`,
    );
    for (const card of plane.cards) {
      assert.ok(card.key, "every Mosaic card exposes its stable identity");
      const measuredBaseSpan = card.candidateSpans[0];
      const wideSpan = Math.min(plane.trackCount, measuredBaseSpan * 2);
      const wideEligible = wideSpan > measuredBaseSpan && card.heights[0] >= plane.tallThreshold;
      const expectedSpans = wideEligible ? [measuredBaseSpan, wideSpan] : [measuredBaseSpan];
      const latchedSpans = card.measuredSpans.slice(expectedSpans.length);
      assert.ok(latchedSpans.length <= 1, "measurement performs at most one frozen-span read");
      assert.ok(
        latchedSpans.every((span) => !expectedSpans.includes(span)),
        "a frozen-span read is distinct from every current policy candidate",
      );
      const expectedMeasuredSpans = [...expectedSpans, ...latchedSpans];
      assert.equal(
        card.reads,
        expectedMeasuredSpans.length,
        `height reads follow the bounded policy, including at most one frozen-span read: ${JSON.stringify({
          card,
          expectedMeasuredSpans,
          expectedSpans,
          latchedSpans,
          plane: {
            baseSpan: plane.baseSpan,
            tallThreshold: plane.tallThreshold,
            width: plane.width,
          },
        })}`,
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
      if (card.tallWitness && expectedSpans.length === 2) {
        assert.equal(card.candidateSpans.length, 2, "the tall text witness exercises both candidates");
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
          && card.candidateSpans.length >= 2
          && card.span === card.candidateSpans[1]
          && card.heights[1] < card.heights[0]
      ) {
        const wideRows = Math.max(
          plane.minCardRows,
          Math.ceil((card.heights[1] + plane.columnGap) / plane.module),
        );
        assert.equal(card.rows, wideRows, "a wide text card commits its measured wide row count");
        assert.ok(
          Math.abs(card.footerInset - card.paddingBottom) <= 1.1,
          `a wide text card leaves only its natural padding below the footer: ${JSON.stringify({
            footerInset: card.footerInset,
            key: card.key,
            paddingBottom: card.paddingBottom,
          })}`,
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
      readPaths.add(expectedSpans.length);
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
  assert.ok(readPaths.has(1), "a short card takes exactly one policy height read");
  if (requireBothReadPaths) {
    assert.ok(readPaths.has(2), "a tall card takes exactly two policy height reads");
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
        && plane.dataset.measurementPending === "0"
        && Math.abs(Number(plane.dataset.committedWidth) - plane.clientWidth) <= 1,
    );
  });
}
