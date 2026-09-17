import assert from "node:assert/strict";
import { join } from "node:path";

import {
  reply,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";
import {
  assertImageLifecycle,
  assertNoImageMeasurementRevision,
  imageCardInvariant,
} from "./image-lifecycle.mjs";
import { assertMosaicLattice } from "./mosaic-lattice.mjs";
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
  assertCanonicalProbeStable,
  assertUnmoved,
  finishPlacementProbe,
  waitForMosaicReady,
} from "./mosaic-placement-reference.mjs";
import { assertRuntimeToolMediaCards } from "./tool-media-reference.mjs";

export {
  armPlacementProbe,
  assertModeSwitchPreservesDeferredMosaic,
  assertMosaicAnchorPreserved,
  assertMosaicLifecycleAdvanced,
  assertMosaicScrollInputs,
  assertRetainedMosaicIdentity,
  assertPlacementRetained,
  assertPlacementStable,
  assertUnmoved,
  finishPlacementProbe,
  mosaicScrollSnapshot,
  scrollMosaicAwayFromTop,
  settleMosaicActivity,
  waitForMosaicReady,
} from "./mosaic-placement-reference.mjs";
export { assertStagedMosaicHydration } from "./mosaic-history-pagination.mjs";
export { assertMosaicLattice } from "./mosaic-lattice.mjs";

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
  artifacts,
) {
  await waitForMosaicReady(page);
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
  // without one starts in `plan`, which `workable` never selects. Give it
  // acceptance criteria and an unrelated agent could claim this no-op probe.
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
  await waitForPage(page, "a no-op refresh reaches the settled surface", TRANSITION_MS,
    (expected) => {
      const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
      return document.querySelector(".bar-meta")?.textContent === expected
        && plane?.dataset.measurementPhase === "ready"
        && plane.dataset.measurementPending === "0";
    }, `cursor ${refresh.seq.last}`);
  const refreshed = await finishPlacementProbe(plane, refreshProbe);
  assertUnmoved(refreshProbe, refreshed, "a no-op refresh");
  assert.equal(refreshed.commits, refreshProbe.commits, "a no-op refresh commits no plane");

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
      ".lane-mosaic-plane.mosaic-width-probe-tiny > .lane-mosaic-track { width: 12px; }",
      ".lane-mosaic-plane.mosaic-width-probe-a > .lane-mosaic-track { width: 540px; }",
      ".lane-mosaic-plane.mosaic-width-probe-b > .lane-mosaic-track { width: 580px; }",
      ".lane-mosaic-plane.mosaic-width-probe-final > .lane-mosaic-track { width: 620px; }",
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
    assertCanonicalProbeStable(
      pendingProbe,
      pending,
      `a pending image ${outcome} append`,
    );
    assert.ok(pending.retained, `pending image ${outcome} retains untouched card nodes`);
    assert.deepEqual(pending.writes, [], `pending image ${outcome} writes no untouched placement`);
    assert.equal(pending.commits, pendingProbe.commits + 1, `pending image ${outcome} appends once`);
    const cardKey = await card.getAttribute("data-mosaic-card-key");
    const invariant = await imageCardInvariant(card);
    const resolutionProbe = await armPlacementProbe(plane, [cardKey]);
    assert.equal(await plane.getAttribute("data-measurement-phase"), "ready");
    release();
    try {
      await waitForPage(page, `late image ${outcome} resolves in place`, TRANSITION_MS, (expected) => {
        const card = [...document.querySelectorAll(".mosaic-message-card")]
          .find((candidate) => candidate.dataset.mosaicCardKey === expected.key);
        return card?.querySelector("img")?.getAttribute("data-mosaic-image-state") === expected.state;
      }, { key: cardKey, state: outcome === "load" ? "loaded" : "error" });
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
    assert.equal(resolved.commits, pending.commits,
      `late image ${outcome} resolves without a plane commit`);
    assert.equal(await card.locator("img").getAttribute("data-mosaic-image-state"),
      outcome === "load" ? "loaded" : "error");
    await assertImageLifecycle(
      card.locator(".message-image"),
      `resolved remote ${outcome} preview`,
      outcome === "load" ? "loaded" : "error",
    );
    await assertNoImageMeasurementRevision(card, `late remote ${outcome}`);
    assert.deepEqual(await imageCardInvariant(card), invariant,
      `late remote ${outcome} preserves fingerprint-facing identity, commands, and span latch`);
    assert.equal(
      await card.getAttribute("data-mosaic-card-key"),
      cardKey,
      `in-place resolution retains the authored ${outcome} assistant record`,
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
  await waitForPage(page, "already-complete image appends with final geometry", TRANSITION_MS, (expected) => {
    const card = [...document.querySelectorAll(".mosaic-message-card")]
      .find((candidate) => candidate.dataset.mosaicCardKey === expected.key);
    const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return card?.dataset.layoutPhase === "committed"
      && card.querySelector("img")?.dataset.mosaicImageState === "loaded"
      && Number(plane?.dataset.planeCommits) >= expected.commits + 1;
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
  assert.equal(ready.commits, readyProbe.commits + 1,
    "an already-complete image needs only its append commit");

  const embeddedPreview = readyCard.locator("a.message-image");
  await assertImageLifecycle(embeddedPreview, "embedded ordinary preview", "loaded");
  await assertNoImageMeasurementRevision(readyCard, "embedded mixed prose and image");
  await assertImagePreview(embeddedPreview, "embedded ordinary preview", 140);
  await assertFullSizeTarget(
    embeddedPreview,
    "embedded ordinary preview",
    `data:image/png;base64,${PIXEL_PNG}`,
  );
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
      && card.querySelector("img")?.dataset.mosaicImageState === "loaded";
  }, await imageOnlyCard.getAttribute("data-mosaic-card-key"));
  assert.ok(
    await imageOnlyCard.evaluate((card) => card.classList.contains("mosaic-message-card--image-only")),
    "a pure image message receives the large preview tier",
  );
  await assertImagePreview(imageOnlyCard.locator(".message-image"), "image-only preview", 252);
  await assertImageLifecycle(imageOnlyCard.locator(".message-image"), "image-only preview", "loaded");
  await assertNoImageMeasurementRevision(imageOnlyCard, "image-only preview");
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
  await waitForPage(page, "multi-image lifecycle resolves in place", TRANSITION_MS, (key) => {
    const card = [...document.querySelectorAll(".mosaic-message-card")]
      .find((candidate) => candidate.dataset.mosaicCardKey === key);
    return card?.dataset.layoutPhase === "committed"
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
  await assertNoImageMeasurementRevision(multiImageCard, "multi-image preview stack");
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

  const attachment = await activityReplay.injectAgentToolMedia(page, member.lane, [{
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
      && card.querySelector("img")?.dataset.mosaicImageState === "loaded";
  }, attachment.key);
  await assertImagePreview(attachmentPreview, "unlinked attachment preview", 252);
  await assertImageLifecycle(attachmentPreview, "unlinked attachment preview", "loaded");
  await assertNoImageMeasurementRevision(attachmentCard, "unlinked attachment preview");
  assert.equal(await attachmentCard.getAttribute("data-agent-activity"), "tool",
    "the hotlink belongs to the inspected-image tool result");
  assert.equal(await attachmentCard.locator(".message-body").innerText(), "",
    "the durable data URL is not exposed as visible transcript text");
  assert.ok(await attachmentPreview.locator("img").getAttribute("data-media-identity"),
    "the loaded attachment retains its admitted media identity");
  await assertFullSizeTarget(
    attachmentPreview,
    "unlinked attachment preview",
    `data:image/png;base64,${PIXEL_PNG}`,
  );
  await openAndAssertIsolated(
    page,
    attachmentPreview,
    "unlinked attachment preview",
    /^blob:/,
  );
  await multiImageCard.screenshot({
    path: join(artifacts, "chromium-image-previews-wide.png"),
  });

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
  await multiImageCard.screenshot({
    path: join(artifacts, "chromium-image-previews-narrow.png"),
  });
  await page.setViewportSize(originalViewport);
  await waitForMosaicReady(page);
  await assertRuntimeToolMediaCards(
    page,
    console,
    member.lane,
    activityReplay,
    "Chromium runtime tool media",
    { history: true },
  );
  await assertMosaicLattice(page, true);
}
