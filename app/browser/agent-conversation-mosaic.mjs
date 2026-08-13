import assert from "node:assert/strict";

import {
  reply,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

const PIXEL_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

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
    };
  }, armed);
}

async function cancelDirection(fixture, lane, text) {
  const inbox = await reply(fixture, ["inbox", lane]);
  const directive = inbox.body.lanes[0].pending.find((candidate) => candidate.text === text);
  assert.ok(directive, `the persistent-Mosaic probe reached ${lane}`);
  await reply(fixture, ["steer", "cancel", directive.directive]);
}

async function submitDirection(console, lane, text) {
  const composer = console.locator(`.team-agent[data-agent-lane="${lane}"]`);
  await composer.getByRole("textbox").fill(text);
  await console.getByRole("button", { name: "Steer", exact: true }).click();
  return console.locator(`[data-card-kind="directive"][data-agent-lane="${lane}"]`)
    .filter({ hasText: text.split("\n", 1)[0] });
}

export async function assertPersistentMosaic(page, fixture, console, plane, member) {
  const firstCard = console.locator('.mosaic-message-card[data-layout-phase="committed"]').first();
  const life = console.locator('.team-lifetime input[type="range"]');
  const presentationProbe = await armPlacementProbe(plane);
  await life.fill("1");
  await console.getByRole("button", { name: "Drive", exact: true }).waitFor();
  const presentation = await finishPlacementProbe(plane, presentationProbe);
  assert.ok(presentation.retained, "a presentation-only update retains every card node");
  assert.deepEqual(presentation.writes, [], "a presentation-only update writes no card placement");
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
  assert.ok(refreshed.retained, "a no-op refresh retains every committed card node");
  assert.deepEqual(refreshed.writes, [], "a no-op refresh writes no card placement");
  assert.equal(refreshed.commits, refreshProbe.commits, "a no-op refresh commits no plane");

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
    const card = await submitDirection(console, member.lane, text);
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
    await cancelDirection(fixture, member.lane, text);
    await card.waitFor({ state: "detached", timeout: 10_000 });
    await waitForPage(page, `removing late image ${outcome} replays once`, TRANSITION_MS,
      (commits) => Number(document.querySelector(
        '[data-pane="agent-console"] .lane-mosaic-plane',
      )?.dataset.planeCommits) > commits,
      resolved.commits);
    await page.unroute(url);
  }

  const readyText = `already complete Mosaic image\n\n![ready](data:image/png;base64,${PIXEL_PNG})`;
  const readyProbe = await armPlacementProbe(plane);
  const readyCard = await submitDirection(console, member.lane, readyText);
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
  await cancelDirection(fixture, member.lane, readyText);
  await readyCard.waitFor({ state: "detached", timeout: 10_000 });
  await waitForPage(page, "removing the complete image settles", TRANSITION_MS,
    (commits) => Number(document.querySelector(
      '[data-pane="agent-console"] .lane-mosaic-plane',
    )?.dataset.planeCommits) > commits,
    ready.commits);
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
            heights: card.dataset.measuredHeights.split(",").filter(Boolean).map(Number),
            candidateSpans: card.dataset.candidateSpans.split(",").filter(Boolean).map(Number),
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
  for (const plane of snapshot.planes) {
    assert.equal(plane.trackCount, 12, "the browser renders the twelve-track lattice");
    assert.equal(plane.columnGap, 12, "the horizontal lattice keeps the predecessor gap");
    assert.equal(plane.rowGap, plane.columnGap, "the vertical lattice shares that grid rhythm");
    assert.equal(plane.minLane, 272, "the browser exposes the predecessor minimum lane width");
    assert.equal(plane.minCardRows, 3, "compact cards reserve three vertical modules");
    assert.deepEqual(
      plane.cards.map((card) => card.creationIndex),
      plane.cards.map((_, index) => index),
      "DOM order is the stable creation order submitted to Rust",
    );
    for (const card of plane.cards) {
      assert.ok(card.key, "every Mosaic card exposes its stable identity");
      const wideSpan = Math.min(plane.trackCount, plane.baseSpan * 2);
      const wideEligible = wideSpan > plane.baseSpan && card.heights[0] >= plane.tallThreshold;
      const expectedSpans = wideEligible ? [plane.baseSpan, wideSpan] : [plane.baseSpan];
      assert.equal(card.reads, wideEligible ? 2 : 1, "height reads follow the bounded policy");
      assert.equal(card.heights.length, card.reads, "every instrumented read records one height");
      assert.deepEqual(card.candidateSpans, expectedSpans, "height alone admits the wide tier");
      assert.ok(card.track >= 0 && card.track + card.span <= plane.trackCount);
      assert.ok(card.rows >= plane.minCardRows);
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
    assert.ok(kinds.has("directive"), "pending directives use the measured policy");
    assert.ok(readPaths.has(2), "a tall card takes exactly two DOM height reads");
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
