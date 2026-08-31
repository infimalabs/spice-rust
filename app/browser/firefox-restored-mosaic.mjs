import assert from "node:assert/strict";
import { join } from "node:path";

import { assertMosaicLattice } from "./agent-conversation.mjs";
import { installPersistedMosaicImageRoutes } from "./agent-conversation-mosaic.mjs";
import { chooseCanvasView, CONNECT_MS, monitorPage } from "./canvas-fixture.mjs";
import { assertAgentPageBottomSpacing } from "./canvas-readiness.mjs";
import { LIVENESS_PROBE_MS } from "./client-deadlines.mjs";
import {
  assertCompactionDivider,
  resizeCompactionViewport,
  waitForCompactionReady,
} from "./compaction-boundary-reference.mjs";
import { assertComposerPointerSelection } from "./composer-shards.mjs";
import { launchBrowser } from "./headless-launch.mjs";
import { assertNaturalMessageFooterInsets } from "./message-card-reference.mjs";

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
  await page.evaluate((pixel) => {
    const fixture = document.createElement("section");
    fixture.dataset.firefoxImageFixture = "true";
    const card = (imageOnly, count) => {
      const article = document.createElement("article");
      article.className = "mosaic-message-card";
      if (imageOnly) article.classList.add("mosaic-message-card--image-only");
      const body = document.createElement("div");
      body.className = "message-body markdown-body";
      const stack = document.createElement("p");
      stack.className = "message-image-stack";
      for (let index = 0; index < count; index += 1) {
        const anchor = document.createElement("a");
        anchor.className = "message-image message-image--openable";
        anchor.href = "about:blank";
        anchor.target = "_blank";
        anchor.rel = "noopener";
        anchor.ariaLabel = `Open Firefox fixture ${index + 1} full size`;
        anchor.title = anchor.ariaLabel;
        const image = document.createElement("img");
        image.src = `data:image/png;base64,${pixel}`;
        image.alt = `Firefox fixture ${index + 1}`;
        anchor.append(image);
        stack.append(anchor);
      }
      body.append(stack);
      article.append(body);
      return article;
    };
    fixture.append(card(false, 1), card(true, 3));
    document.body.append(fixture);
  }, PIXEL_PNG);
  return page.locator('[data-firefox-image-fixture="true"]');
}

async function assertFirefoxImageFixture(fixture, label) {
  const observed = await fixture.evaluate((host) => [...host.querySelectorAll("article")].map((card) => ({
    cardWidth: card.getBoundingClientRect().width,
    stackWidth: card.querySelector(".message-image-stack").getBoundingClientRect().width,
    previews: [...card.querySelectorAll(".message-image")].map((preview) => {
      const image = preview.querySelector("img");
      const box = preview.getBoundingClientRect();
      const imageBox = image.getBoundingClientRect();
      return {
        top: box.top,
        right: box.right,
        height: imageBox.height,
        objectFit: getComputedStyle(image).objectFit,
        target: preview.target,
        rel: preview.rel,
      };
    }),
  })));
  assert.ok(Math.abs(observed[0].previews[0].height - 140) <= 1,
    `${label} bounds an ordinary preview: ${JSON.stringify(observed)}`);
  assert.ok(observed[1].previews.every((preview) => Math.abs(preview.height - 252) <= 1),
    `${label} bounds image-only previews: ${JSON.stringify(observed)}`);
  assert.ok(observed[1].previews.every((preview) => preview.objectFit === "contain"),
    `${label} preserves image aspect ratios`);
  assert.ok(observed[1].previews.every((preview) => preview.target === "_blank" && preview.rel === "noopener"),
    `${label} preserves isolated full-size target semantics`);
  assert.ok(observed[1].previews.every((preview) => Math.abs(preview.top - observed[1].previews[0].top) <= 1),
    `${label} keeps multiple images in one bounded row`);
  assert.ok(observed.every((card) => card.stackWidth <= card.cardWidth),
    `${label} keeps image stacks inside their cards: ${JSON.stringify(observed)}`);
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
    if (populated) {
      await page.locator(
        '.mosaic-message-card[data-layout-phase="committed"]',
      ).first().waitFor({ timeout: CONNECT_MS });
    }
    await assertAgentPageBottomSpacing(page, state);
    await page.screenshot({ path: join(artifacts, file) });
    monitor.assertClean();
  } finally {
    await context.close();
    await firefox.close();
  }
}

export async function assertFirefoxRestoredMosaic(endpoint, artifacts, expectedAttribution) {
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
    const before = await settledMosaicCards(page, "before reload");
    assert.ok(before.length > 0, "Firefox receives restored Mosaic cards");
    await assertNaturalMessageFooterInsets(console, "Firefox wide restored Mosaic");
    const imageFixture = await installFirefoxImageFixture(page);
    await assertFirefoxImageFixture(imageFixture, "Firefox wide image fixture");
    await imageFixture.screenshot({ path: join(artifacts, "firefox-image-previews-wide.png") });
    const boundary = console.locator('[data-card-kind="compaction"]');
    assert.equal(await boundary.count(), 1, "Firefox restores the real compaction exactly once");
    await waitForCompactionReady(page, boundary, "Firefox wide compaction divider");
    await assertCompactionDivider(boundary, "Firefox wide compaction divider", expectedAttribution);
    await resizeCompactionViewport(
      page,
      boundary,
      { width: 390, height: 720 },
      "Firefox narrow compaction divider",
    );
    await assertCompactionDivider(boundary, "Firefox narrow compaction divider", expectedAttribution);
    await assertNaturalMessageFooterInsets(console, "Firefox narrow restored Mosaic");
    await assertFirefoxImageFixture(imageFixture, "Firefox narrow image fixture");
    await imageFixture.screenshot({ path: join(artifacts, "firefox-image-previews-narrow.png") });
    await resizeCompactionViewport(
      page,
      boundary,
      { width: 1717, height: 1470 },
      "Firefox restored compaction divider",
    );
    await assertCompactionDivider(boundary, "Firefox restored compaction divider", expectedAttribution);
    await resizeCompactionViewport(
      page,
      boundary,
      { width: 480, height: 468 },
      "Firefox reported Agents viewport",
    );
    await assertAgentPageBottomSpacing(page, "Firefox reported Agents viewport");
    await page.screenshot({ path: join(artifacts, "firefox-agents-reported-480x468.png") });
    await resizeCompactionViewport(
      page,
      boundary,
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
    assert.equal(await boundary.count(), 1, "Firefox reload keeps one compaction divider");
    await waitForCompactionReady(page, boundary, "Firefox replayed compaction divider");
    await assertCompactionDivider(boundary, "Firefox replayed compaction divider", expectedAttribution);

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
    await boundary.screenshot({ path: join(artifacts, "firefox-compaction-boundary.png") });
    monitor.assertClean();
  } finally {
    await context.close();
    await firefox.close();
  }
}
