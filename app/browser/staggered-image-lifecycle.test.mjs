import assert from "node:assert/strict";
import { mkdtemp, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { installActivityReplay } from "./activity-replay.mjs";
import {
  assertImageLifecycle,
  assertNoImageMeasurementRevision,
  imageCardInvariant,
} from "./image-lifecycle.mjs";
import {
  buildResident,
  chooseCanvasView,
  configureFixture,
  reply,
  seedAuthority,
  startResident,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";
import { ownFixture } from "./canvas-lifecycle.mjs";
import { launchBrowser } from "./headless-launch.mjs";
import { armSteadyStateProbe, finishSteadyStateProbe } from "./steady-state.mjs";

const PIXEL_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

async function armPlacementProbe(plane) {
  return plane.evaluate((element) => {
    const token = crypto.randomUUID();
    const attributes = ["data-track", "data-span", "data-base", "data-row", "data-rows", "style"];
    const cards = [...element.querySelectorAll('.mosaic-message-card[data-layout-phase="committed"]')];
    if (cards.length === 0) throw new Error("the staggered image probe requires one committed card");
    for (const card of cards) card.__staggeredImageProbe = token;
    const writes = [];
    const record = (mutations) => {
      for (const mutation of mutations) {
        if (mutation.target.__staggeredImageProbe !== token) continue;
        writes.push({ key: mutation.target.dataset.mosaicCardKey, attribute: mutation.attributeName });
      }
    };
    const observer = new MutationObserver(record);
    observer.observe(element, { subtree: true, attributes: true, attributeFilter: attributes });
    element.__staggeredImageObserver = { observer, record, token, writes };
    return {
      commits: Number(element.dataset.planeCommits),
      token,
      keys: cards.map((card) => card.dataset.mosaicCardKey),
    };
  });
}

async function finishNeighborProbe(plane, armed) {
  return plane.evaluate((element, expected) => {
    const probe = element.__staggeredImageObserver;
    probe.record(probe.observer.takeRecords());
    probe.observer.disconnect();
    const cards = [...element.querySelectorAll(".mosaic-message-card")];
    return {
      commits: Number(element.dataset.planeCommits),
      retained: expected.keys.every((key) => cards.some((card) => (
        card.dataset.mosaicCardKey === key && card.__staggeredImageProbe === expected.token
      ))),
      writes: probe.writes,
    };
  }, armed);
}

async function imageGeometry(card) {
  return card.evaluate((element) => {
    const box = (node) => {
      const bounds = node.getBoundingClientRect();
      return [bounds.x, bounds.y, bounds.width, bounds.height].map((value) => Math.round(value));
    };
    return {
      card: box(element),
      body: box(element.querySelector(".message-body")),
      stack: box(element.querySelector(".message-image-stack")),
      previews: [...element.querySelectorAll(".message-image")].map((preview) => ({
        preview: box(preview),
        image: box(preview.querySelector("img")),
      })),
    };
  });
}

test("staggered image outcomes preserve the committed plane", { timeout: 90_000 }, async () => {
  if (process.env.SPICE_CANVAS_PREBUILT !== "1") await buildResident();
  const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-staggered-images-")));
  const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-staggered-image-lanes-")));
  const owner = ownFixture({ fixture, linkedRoot });
  let browser;
  try {
    await configureFixture(fixture, linkedRoot, 1, { laneLease: "5m" });
    const resident = owner.ownResident(await startResident(fixture));
    await seedAuthority(fixture);
    browser = await launchBrowser();
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    const activityReplay = await installActivityReplay(context);
    const page = await context.newPage();
    await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
    const seated = await reply(fixture, [
      "lane", "add", fixture, "--name", "Image", "--life", "Steer", "--vendor", "claude",
    ]);
    const lane = seated.body.lane;
    await reply(fixture, ["lane", "wake", lane]);
    await chooseCanvasView(page, "Lanes");
    const console = page.locator('[data-pane="agent-console"]');
    await console.getByRole("tab", { name: /^Agents/ }).click();
    await console.locator('[data-card-kind="agent-message"]').first().waitFor();

    const origin = new URL(page.url()).origin;
    const images = [];
    for (const [name, outcome] of [["one", "load"], ["two", "error"], ["three", "load"]]) {
      const url = `${origin}/mosaic-${outcome}-${crypto.randomUUID()}.png`;
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
      images.push({ name, outcome, url, release, requested });
    }
    const injected = await activityReplay.injectAgentProse(
      page,
      lane,
      images.map(({ name, url }) => `![${name}](${url})`).join("\n\n"),
    );
    const card = console.locator(`[data-mosaic-card-key="${injected.key}"]`);
    await card.waitFor();
    await Promise.all(images.map(({ requested }) => requested));
    await waitForPage(page, "staggered card reserves pending media", TRANSITION_MS, (key) => {
      const card = document.querySelector(`[data-mosaic-card-key="${key}"]`);
      return card?.dataset.layoutPhase === "committed"
        && !card.dataset.mosaicImageRevision
        && [...card.querySelectorAll("img")].every((image) => image.dataset.mosaicImageState === "pending");
    }, injected.key);

    const invariant = await imageCardInvariant(card);
    const geometry = await imageGeometry(card);
    const plane = console.locator(".lane-mosaic-plane");
    const probe = await armPlacementProbe(plane);
    const steady = await armSteadyStateProbe(page, "staggered image resolution");
    for (const [index, image] of images.entries()) {
      image.release();
      const state = image.outcome === "load" ? "loaded" : "error";
      await waitForPage(page, `image ${index + 1} reaches ${state}`, TRANSITION_MS,
        (expected) => document.querySelector(`[data-mosaic-card-key="${expected.key}"]`)
          ?.querySelectorAll("img")[expected.index]?.dataset.mosaicImageState === expected.state,
        { key: injected.key, index, state });
      await assertImageLifecycle(card.locator(".message-image").nth(index), `image ${index + 1}`, state);
      if (index + 1 < images.length) {
        await assertImageLifecycle(card.locator(".message-image").nth(index + 1),
          "unresolved sibling image", "pending");
        await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
          () => requestAnimationFrame(resolve),
        )));
        assert.equal(await card.getAttribute("data-mosaic-image-revision"), null);
        assert.equal(Number(await plane.getAttribute("data-plane-commits")), probe.commits);
      }
    }
    await waitForPage(page, "staggered card resolves in place", TRANSITION_MS, (expected) => {
      const card = document.querySelector(`[data-mosaic-card-key="${expected.key}"]`);
      return Boolean(card) && [...card.querySelectorAll("img")].every((image) => (
          image.dataset.mosaicImageState === "loaded" || image.dataset.mosaicImageState === "error"
        ));
    }, { key: injected.key });
    const resolved = await finishNeighborProbe(plane, probe);
    await finishSteadyStateProbe(page, steady, "staggered image resolution");
    assert.equal(resolved.commits, probe.commits,
      "staggered outcomes add no decode-driven plane commit");
    assert.ok(resolved.retained, "in-place resolution retains every committed card node");
    assert.deepEqual(resolved.writes, [], "in-place resolution writes no placement attribute");
    await assertNoImageMeasurementRevision(card, "staggered image card");
    assert.deepEqual(await imageCardInvariant(card), invariant,
      "in-place resolution preserves identity, text, attribution, span, and commands");
    assert.deepEqual(await imageGeometry(card), geometry,
      "staggered load and error outcomes preserve the card and every preview box");
    await context.close();
  } finally {
    if (browser) await browser.close();
    await owner.cleanup();
  }
});
