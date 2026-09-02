import assert from "node:assert/strict";

import {
  reply,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";
import {
  assertImageLifecycle,
  assertOwningImageRevision,
  imageCardInvariant,
} from "./image-lifecycle.mjs";
import {
  assertFullSizeTarget,
  assertImagePreview,
  assertOneImageOnlyTier,
  imageOnlyTierGeometry,
  openAndAssertIsolated,
  submitAssistantActivity,
} from "./mosaic-image-reference.mjs";
import {
  armPlacementProbe,
  assertUnmoved,
  finishPlacementProbe,
  settleMosaicActivity,
  waitForMosaicReady,
} from "./mosaic-placement-reference.mjs";
import {
  assertCanonicalPlacementsStable,
  captureCanonicalPlacements,
} from "./mosaic-placement-stability.mjs";

export {
  armPlacementProbe,
  assertCompactionBoundaryPersistence,
  assertModeSwitchPreservesDeferredMosaic,
  assertMosaicAnchorPreserved,
  assertMosaicLifecycleAdvanced,
  assertMosaicScrollInputs,
  assertPlacementRetained,
  assertPlacementStable,
  assertStagedMosaicHydration,
  assertUnmoved,
  finishPlacementProbe,
  mosaicScrollSnapshot,
  scrollMosaicAwayFromTop,
  settleMosaicActivity,
  waitForMosaicReady,
} from "./mosaic-placement-reference.mjs";

const PIXEL_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const PERSISTED_IMAGE_ROUTE = /\/mosaic-(?:load|error)-[0-9a-f-]+\.png$/;

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
  const refreshPlacement = await captureCanonicalPlacements(plane);
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
  assertCanonicalPlacementsStable(
    refreshPlacement,
    await captureCanonicalPlacements(plane),
    "a no-op refresh",
  );
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
  const agePlacement = await captureCanonicalPlacements(plane);
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
  assertCanonicalPlacementsStable(
    agePlacement,
    await captureCanonicalPlacements(plane),
    "a content-only deadline age update",
  );
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
    const pendingPlacement = await captureCanonicalPlacements(plane);
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
    assertCanonicalPlacementsStable(
      pendingPlacement,
      await captureCanonicalPlacements(plane),
      `a pending image ${outcome} append`,
      [cardKey],
    );
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
