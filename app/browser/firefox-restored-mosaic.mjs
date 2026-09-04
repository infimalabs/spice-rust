import assert from "node:assert/strict";
import { join } from "node:path";

import { assertMosaicLattice } from "./agent-conversation.mjs";
import { installPersistedMosaicImageRoutes } from "./agent-conversation-mosaic.mjs";
import { chooseCanvasView, CONNECT_MS, monitorPage } from "./canvas-fixture.mjs";
import { assertAgentWorkspaceGeometry } from "./canvas-readiness.mjs";
import { LIVENESS_PROBE_MS } from "./client-deadlines.mjs";
import {
  assertCompactionDivider,
  assertNoUnavailableToolTerminal,
  resizeCompactionViewport,
  waitForCompactionReady,
} from "./compaction-boundary-reference.mjs";
import { assertComposerPointerSelection } from "./composer-shards.mjs";
import { launchBrowser } from "./headless-launch.mjs";
import { assertLaneHeaderPipMatrix } from "./lane-header-pips.mjs";
import { assertNaturalMessageFooterInsets } from "./message-card-reference.mjs";
import { assertCandidateMeasurementPaintAtomic } from "./mosaic-candidate-paint.mjs";
import {
  assertQuoteBlockDensity,
  captureQuoteBlockDensity,
} from "./quote-block-density.mjs";

/// A ready lattice settles layout, not delivery: under verifier load the
/// initial card stream can still be arriving when measurements first report
/// ready, so a raw read undercounts and the reload equality below would
/// compare a partial paint against the durable truth. Hold until the card
/// count survives a quiet window, and refuse past the deadline naming the
/// count still in motion. The probe only counts cards; the full lattice
/// invariant battery runs once, on the settled state, exactly as often as
/// the unsettled reader ran it.
const ARRIVAL_QUIET_MS = 500;
const ARRIVAL_DEADLINE_MS = 15_000;
const ARRIVAL_PROBE_MS = 125;
const PIXEL_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const COMPACTED_TOOL_PREVIEW = "Bash: cargo test --locked -p spice";
const COVER_IMAGE_FIXTURES = Object.freeze([
  Object.freeze({ name: "portrait", width: 120, height: 240, color: "#d1495b" }),
  Object.freeze({ name: "landscape", width: 240, height: 120, color: "#00798c" }),
  Object.freeze({ name: "square", width: 160, height: 160, color: "#edae49" }),
  Object.freeze({ name: "ultra-wide", width: 400, height: 40, color: "#30638e" }),
  Object.freeze({ name: "ultra-tall", width: 40, height: 400, color: "#6a994e" }),
]);

function mosaicCardCount(page) {
  return page.locator('[data-pane="agent-console"] .mosaic-message-card').count();
}

async function settledMosaicCards(page, label) {
  const startedAt = Date.now();
  let count = await mosaicCardCount(page);
  let quietSince = Date.now();
  while (Date.now() - quietSince < ARRIVAL_QUIET_MS) {
    if (Date.now() - startedAt > ARRIVAL_DEADLINE_MS) {
      throw new Error(
        `${label}: Mosaic cards kept arriving past the settle deadline at ${count}`,
      );
    }
    await page.waitForTimeout(ARRIVAL_PROBE_MS);
    const current = await mosaicCardCount(page);
    if (current !== count) quietSince = Date.now();
    count = current;
  }
  return assertMosaicLattice(page, false);
}

async function installFirefoxImageFixture(page) {
  await page.evaluate(({ fixtures, pixel }) => {
    const fixture = document.createElement("section");
    fixture.className = "team-surface";
    fixture.dataset.firefoxImageFixture = "true";
    fixture.style.cssText = [
      "display: grid",
      "flex: none",
      "gap: 12px",
      "width: min(56rem, calc(100vw - 24px))",
      "min-height: max-content",
      "overflow: visible",
      "padding: 12px",
    ].join(";");
    const fixtureSource = ({ width, height, color }) => {
      const canvas = document.createElement("canvas");
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext("2d");
      context.fillStyle = color;
      context.fillRect(0, 0, width, height);
      context.fillStyle = "#f7f7f2";
      context.fillRect(width * 0.375, height * 0.375, width * 0.25, height * 0.25);
      return canvas.toDataURL("image/png");
    };
    const card = (imageOnly, images) => {
      const article = document.createElement("article");
      article.className = "mosaic-message-card";
      if (imageOnly) article.classList.add("mosaic-message-card--image-only");
      const body = document.createElement("div");
      body.className = "message-body markdown-body";
      const stack = document.createElement("p");
      stack.className = "message-image-stack";
      for (const [index, fixtureImage] of images.entries()) {
        const anchor = document.createElement("a");
        anchor.className = "message-image message-image--openable";
        anchor.href = "about:blank";
        anchor.target = "_blank";
        anchor.rel = "noopener";
        anchor.ariaLabel = `Open Firefox fixture ${index + 1} full size`;
        anchor.title = anchor.ariaLabel;
        const image = document.createElement("img");
        image.src = fixtureImage ? fixtureSource(fixtureImage) : `data:image/png;base64,${pixel}`;
        image.alt = fixtureImage?.name ?? `Firefox fixture ${index + 1}`;
        image.dataset.mosaicImageState = "loaded";
        if (fixtureImage) image.dataset.coverFixture = fixtureImage.name;
        anchor.append(image);
        stack.append(anchor);
      }
      body.append(stack);
      article.append(body);
      return article;
    };
    fixture.append(card(false, [null]), card(true, fixtures));
    document.body.append(fixture);
  }, { fixtures: COVER_IMAGE_FIXTURES, pixel: PIXEL_PNG });
  const fixture = page.locator('[data-firefox-image-fixture="true"]');
  await fixture.locator("img").evaluateAll((images) => Promise.all(images.map((image) => {
    if (image.complete && image.naturalWidth > 0) return undefined;
    return new Promise((resolve, reject) => {
      image.addEventListener("load", resolve, { once: true });
      image.addEventListener("error", () => reject(new Error(`fixture image failed: ${image.alt}`)), {
        once: true,
      });
    });
  })));
  return fixture;
}

// Chromium's rich quote is deliberately client-injected and not replayable.
// Give Firefox equivalent production markup without perturbing Mosaic placement.
async function installFirefoxQuoteFixture(page) {
  await page.evaluate(() => {
    const element = (tag, text) => {
      const node = document.createElement(tag);
      if (text) node.textContent = text;
      return node;
    };
    const fixture = element("section");
    fixture.className = "team-surface";
    fixture.dataset.firefoxQuoteFixture = "true";
    const article = element("article");
    article.className = "mosaic-message-card";
    const body = element("div");
    body.className = "message-body markdown-body";
    const outer = element("blockquote");
    const nested = element("blockquote");
    const deep = element("blockquote");
    deep.append(element("p", "Deep context advances by one compact rail."));
    nested.append(deep, element("p", "The first nested paragraph stays close to its successor."));
    const rich = element("p");
    rich.append("The second keeps ", element("strong", "inline emphasis"), " and ");
    rich.append(element("code", "code"), " inside the quote.");
    nested.append(rich);
    const list = element("ul");
    list.append(
      element("li", "Nested lists retain compact spacing."),
      element(
        "li",
        "A deliberately long list item demonstrates that useful line width remains available instead of disappearing into progressively wider indentation.",
      ),
    );
    outer.append(nested, list);
    body.append(outer, element("p", "The authored conclusion remains outside the quote stack."));
    article.append(body);
    fixture.append(article);
    document.body.append(fixture);
  });
  return page.locator('[data-firefox-quote-fixture="true"] .mosaic-message-card');
}

async function assertFirefoxImageFixture(fixture, label) {
  const observed = await fixture.evaluate((host) => [...host.querySelectorAll("article")].map((card) => ({
    cardWidth: card.getBoundingClientRect().width,
    stackWidth: card.querySelector(".message-image-stack").getBoundingClientRect().width,
    previews: [...card.querySelectorAll(".message-image")].map((preview) => {
      const image = preview.querySelector("img");
      const box = preview.getBoundingClientRect();
      const imageBox = image.getBoundingClientRect();
      const style = getComputedStyle(image);
      const contentWidth = imageBox.width
        - Number.parseFloat(style.borderLeftWidth)
        - Number.parseFloat(style.borderRightWidth);
      const contentHeight = imageBox.height
        - Number.parseFloat(style.borderTopWidth)
        - Number.parseFloat(style.borderBottomWidth);
      const scale = Math.max(contentWidth / image.naturalWidth, contentHeight / image.naturalHeight);
      return {
        fixture: image.dataset.coverFixture ?? null,
        top: box.top,
        right: box.right,
        width: imageBox.width,
        height: imageBox.height,
        naturalWidth: image.naturalWidth,
        naturalHeight: image.naturalHeight,
        contentWidth,
        contentHeight,
        scaledWidth: image.naturalWidth * scale,
        scaledHeight: image.naturalHeight * scale,
        objectFit: style.objectFit,
        objectPosition: style.objectPosition,
        target: preview.target,
        rel: preview.rel,
      };
    }),
  })));
  assert.ok(Math.abs(observed[0].previews[0].height - 140) <= 1,
    `${label} bounds an ordinary preview: ${JSON.stringify(observed)}`);
  assert.ok(observed[1].previews.every((preview) => Math.abs(preview.height - 252) <= 1),
    `${label} bounds image-only previews: ${JSON.stringify(observed)}`);
  assert.ok(observed.flatMap(({ previews }) => previews).every((preview) => (
    preview.width > 2 && preview.contentWidth > 0 && preview.contentHeight > 0
  )), `${label} gives every preview positive visible area: ${JSON.stringify(observed)}`);
  assert.deepEqual(
    observed[1].previews.map(({ fixture: name, naturalWidth: width, naturalHeight: height }) => ({
      name,
      width,
      height,
    })),
    COVER_IMAGE_FIXTURES.map(({ name, width, height }) => ({ name, width, height })),
    `${label} loads every intrinsic aspect-ratio fixture`,
  );
  assert.ok(observed.flatMap(({ previews }) => previews).every(
    (preview) => preview.objectFit === "cover" && preview.objectPosition === "50% 50%",
  ), `${label} applies a centered cover crop to every preview`);
  assert.ok(observed[1].previews.every((preview) => (
    preview.scaledWidth >= preview.contentWidth - 0.5
      && preview.scaledHeight >= preview.contentHeight - 0.5
  )), `${label} leaves no empty band inside any preview: ${JSON.stringify(observed[1])}`);
  assert.ok(observed[1].previews.every((preview) => Math.abs(
    preview.scaledWidth / preview.scaledHeight
      - preview.naturalWidth / preview.naturalHeight,
  ) <= 0.000_001), `${label} preserves every intrinsic aspect ratio while scaling`);
  assert.ok(observed[1].previews.every((preview) => Math.min(
    preview.scaledWidth - preview.contentWidth,
    preview.scaledHeight - preview.contentHeight,
  ) <= 0.5), `${label} uses the minimum cover scale instead of excess zoom`);
  assert.ok(observed[1].previews.every((preview) => preview.target === "_blank" && preview.rel === "noopener"),
    `${label} preserves isolated full-size target semantics`);
  assert.ok(observed[1].previews.every((preview) => Math.abs(preview.top - observed[1].previews[0].top) <= 1),
    `${label} keeps multiple images in one bounded row`);
  assert.ok(observed.every((card) => card.stackWidth <= card.cardWidth),
    `${label} keeps image stacks inside their cards: ${JSON.stringify(observed)}`);
}

async function assertFirefoxCompactionBoundaries(
  console,
  label,
  expectedAttribution,
) {
  const boundaries = console.locator('[data-card-kind="compaction"]');
  const newest = boundaries.first();
  const earlier = boundaries.last();
  assert.equal(await boundaries.count(), 2, `${label} has both real compaction boundaries`);
  await Promise.all([
    waitForCompactionReady(console.page(), newest, `${label} newest divider`),
    waitForCompactionReady(console.page(), earlier, `${label} earlier divider`),
  ]);
  await assertCompactionDivider(
    newest,
    `${label} newest divider`,
    expectedAttribution,
  );
  await assertNoUnavailableToolTerminal(
    newest,
    `${label} newest divider`,
    expectedAttribution,
    COMPACTED_TOOL_PREVIEW,
  );
  await assertCompactionDivider(
    earlier,
    `${label} earlier divider`,
    expectedAttribution,
  );
  await assertNoUnavailableToolTerminal(
    earlier,
    `${label} earlier divider`,
    expectedAttribution,
    COMPACTED_TOOL_PREVIEW,
  );
  return { earlier, newest };
}

export async function captureFirefoxAgentPageSpacing(
  endpoint,
  artifacts,
  { file, populated, state },
) {
  const firefox = await launchBrowser("firefox");
  const context = await firefox.newContext({
    colorScheme: "dark",
    viewport: { width: 480, height: 468 },
  });
  const page = await context.newPage();
  const monitor = monitorPage(page);
  try {
    await page.goto(endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    const committed = page.locator(
      '.mosaic-message-card[data-layout-phase="committed"]',
    );
    if (populated) {
      await committed.first().waitFor({ timeout: CONNECT_MS });
      assert.ok(await committed.count() > 0, `${state} has committed Mosaic cards`);
    } else {
      assert.equal(await committed.count(), 0, `${state} has zero committed Mosaic cards`);
    }
    await assertAgentWorkspaceGeometry(page, state);
    await page.screenshot({ path: join(artifacts, file) });
    monitor.assertClean();
  } finally {
    await context.close();
    await firefox.close();
  }
}

export async function assertFirefoxRestoredMosaic(
  endpoint,
  artifacts,
  expectedAttribution,
  acknowledgementReply,
) {
  const firefox = await launchBrowser("firefox");
  const context = await firefox.newContext({
    colorScheme: "dark",
    viewport: { width: 1717, height: 1470 },
  });
  await installPersistedMosaicImageRoutes(context);
  const page = await context.newPage();
  await page.clock.install();
  const monitor = monitorPage(page);
  try {
    await page.goto(endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    const console = page.locator('[data-pane="agent-console"]');
    await console.getByRole("tab", { name: /^Compose/ }).click();
    await assertLaneHeaderPipMatrix(page, console, artifacts, "firefox", "dark");
    const before = await settledMosaicCards(page, "before reload");
    assert.ok(before.length > 0, "Firefox receives restored Mosaic cards");
    const userQuoteFixture = await installFirefoxQuoteFixture(page);
    const quoteCards = {
      ack: console.locator('[data-card-kind="agent-message"]')
        .filter({ hasText: acknowledgementReply }),
      user: userQuoteFixture,
    };
    await assertQuoteBlockDensity(quoteCards, "Firefox wide Mosaic");
    await captureQuoteBlockDensity(page, quoteCards, artifacts, "firefox-quotes-wide");
    await assertCandidateMeasurementPaintAtomic(page, "firefox");
    await assertNaturalMessageFooterInsets(console, "Firefox wide restored Mosaic");
    const imageFixture = await installFirefoxImageFixture(page);
    await assertFirefoxImageFixture(imageFixture, "Firefox wide image fixture");
    await imageFixture.screenshot({ path: join(artifacts, "firefox-image-previews-wide.png") });
    const boundaries = await assertFirefoxCompactionBoundaries(
      console,
      "Firefox wide",
      expectedAttribution,
    );
    await resizeCompactionViewport(
      page,
      boundaries.newest,
      { width: 390, height: 720 },
      "Firefox narrow compaction divider",
    );
    await assertFirefoxCompactionBoundaries(
      console,
      "Firefox narrow",
      expectedAttribution,
    );
    await assertNaturalMessageFooterInsets(console, "Firefox narrow restored Mosaic");
    await assertQuoteBlockDensity(quoteCards, "Firefox narrow Mosaic");
    await captureQuoteBlockDensity(page, quoteCards, artifacts, "firefox-quotes-narrow");
    await assertFirefoxImageFixture(imageFixture, "Firefox narrow image fixture");
    await imageFixture.screenshot({ path: join(artifacts, "firefox-image-previews-narrow.png") });
    await resizeCompactionViewport(
      page,
      boundaries.newest,
      { width: 1717, height: 1470 },
      "Firefox restored compaction divider",
    );
    await assertFirefoxCompactionBoundaries(console, "Firefox restored", expectedAttribution);
    await resizeCompactionViewport(
      page,
      boundaries.newest,
      { width: 480, height: 468 },
      "Firefox reported Agents viewport",
    );
    await assertAgentWorkspaceGeometry(page, "Firefox reported Agents viewport");
    await page.screenshot({ path: join(artifacts, "firefox-agents-reported-480x468.png") });
    await resizeCompactionViewport(
      page,
      boundaries.newest,
      { width: 1717, height: 1470 },
      "Firefox restored Agents viewport",
    );
    const lane = console.locator(".team-agent").first();
    await assertComposerPointerSelection(
      page,
      lane,
      lane.locator(".composer-shard--primary"),
    );
    await imageFixture.evaluate((fixture) => fixture.remove());

    await page.reload({ waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    await console.getByRole("tab", { name: /^Compose/ }).click();
    const restored = await settledMosaicCards(page, "after reload");
    assert.equal(restored.length, before.length, "Firefox reload restores every Mosaic card");
    await assertNaturalMessageFooterInsets(console, "Firefox reloaded Mosaic");
    const reloadedBoundaries = await assertFirefoxCompactionBoundaries(
      console,
      "Firefox reloaded",
      expectedAttribution,
    );

    await page.clock.fastForward(LIVENESS_PROBE_MS + 500);
    const settled = await assertMosaicLattice(page, false);
    const settledKeys = new Set(settled.map((card) => card.key));
    assert.ok(
      restored.every((card) => settledKeys.has(card.key)),
      "Firefox retains every restored card after the liveness heartbeat",
    );
    assert.ok(
      settled.length >= restored.length,
      "the liveness heartbeat never removes a restored Firefox card",
    );
    await page.screenshot({ path: join(artifacts, "firefox-restored-mosaic.png"), fullPage: true });
    await reloadedBoundaries.earlier.screenshot({
      path: join(artifacts, "firefox-compaction-boundary.png"),
    });
    await reloadedBoundaries.newest.screenshot({
      path: join(artifacts, "firefox-empty-compaction-boundary.png"),
    });
    monitor.assertClean();
  } finally {
    await context.close();
    await firefox.close();
  }
}
