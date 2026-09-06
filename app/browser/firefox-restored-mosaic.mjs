import assert from "node:assert/strict";
import { join } from "node:path";

import { installActivityReplay } from "./activity-replay.mjs";
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
import { assertHeaderRelatimeMatrix } from "./composer-parity.mjs";
import { assertComposerPointerSelection } from "./composer-shards.mjs";
import { launchBrowser } from "./headless-launch.mjs";
import { assertLaneHeaderPipMatrix } from "./lane-header-pips.mjs";
import { assertNaturalMessageFooterInsets } from "./message-card-reference.mjs";
import { captureMessageStreamBoundaries } from "./message-stream-boundaries.mjs";
import { assertCandidateMeasurementPaintAtomic } from "./mosaic-candidate-paint.mjs";
import { assertCursorRelocation } from "./team-menu.mjs";
import {
  assertQuoteBlockDensity,
  captureQuoteBlockDensity,
} from "./quote-block-density.mjs";
import { assertRuntimeToolMediaCards } from "./tool-media-reference.mjs";

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
const COMPACTED_TOOL_PREVIEW = "Bash: cargo test --locked -p spice";

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
    await chooseCanvasView(page, "Lanes");
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
  const activityReplay = await installActivityReplay(context, {
    retainInjectedSnapshots: true,
    withholdBootstrap: false,
  });
  const page = await context.newPage();
  await page.clock.install();
  const monitor = monitorPage(page);
  try {
    await page.goto(endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Lanes");
    await assertCursorRelocation(page, artifacts, "firefox");
    const console = page.locator('[data-pane="agent-console"]');
    await console.getByRole("tab", { name: /^Lanes/ }).click();
    await assertLaneHeaderPipMatrix(page, console, artifacts, "firefox", "dark");
    const linkedComposer = console.locator(".agent-composer-stack").filter({
      has: page.locator(
        '.composer-shard--primary .composer-shard-link[href^="#message-agent-"]',
      ),
    }).first();
    await linkedComposer.locator(".composer-shard-age-label").waitFor({ timeout: CONNECT_MS });
    await assertHeaderRelatimeMatrix(page, linkedComposer, artifacts, {
      artifactPrefix: "firefox",
      restoreScheme: "dark",
      variants: ["primary"],
    });
    const initial = await settledMosaicCards(page, "before tool-media delivery");
    assert.ok(initial.length > 0, "Firefox receives restored Mosaic cards");
    const toolLane = await console.locator(".team-agent").first().getAttribute("data-agent-lane");
    assert.ok(toolLane, "Firefox tool-media fixture resolves one runtime agent lane");
    const toolCards = await assertRuntimeToolMediaCards(
      page,
      console,
      toolLane,
      activityReplay,
      "Firefox runtime tool media",
      { boundary: false, reconnect: true },
    );
    const before = await settledMosaicCards(page, "after tool-media reconnect");
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
    await captureMessageStreamBoundaries(page, console, artifacts, "firefox");
    await console.locator(`[data-mosaic-card-key="${toolCards[2].key}"]`).screenshot({
      path: join(artifacts, "firefox-image-previews-wide.png"),
    });
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
    await console.locator(`[data-mosaic-card-key="${toolCards[2].key}"]`).screenshot({
      path: join(artifacts, "firefox-image-previews-narrow.png"),
    });
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
      "Firefox reported Lanes viewport",
    );
    await assertAgentWorkspaceGeometry(page, "Firefox reported Lanes viewport");
    await page.screenshot({ path: join(artifacts, "firefox-agents-reported-480x468.png") });
    await resizeCompactionViewport(
      page,
      boundaries.newest,
      { width: 1717, height: 1470 },
      "Firefox restored Lanes viewport",
    );
    await page.locator('[data-firefox-quote-fixture="true"]')
      .evaluate((fixture) => fixture.remove());
    const lane = console.locator(".team-agent").first();
    await assertComposerPointerSelection(
      page,
      lane,
      lane.locator(".composer-shard--primary"),
    );

    await page.reload({ waitUntil: "networkidle" });
    await chooseCanvasView(page, "Lanes");
    await console.getByRole("tab", { name: /^Lanes/ }).click();
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
