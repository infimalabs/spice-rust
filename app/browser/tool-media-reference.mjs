import assert from "node:assert/strict";

import {
  chooseCanvasView,
  TRANSITION_MS,
  waitForPageFunction,
} from "./canvas-fixture.mjs";
import { assertImageLifecycle } from "./image-lifecycle.mjs";
import {
  assertFullSizeTarget,
  assertImagePreview,
  openAndAssertIsolated,
} from "./mosaic-image-reference.mjs";

const PIXEL_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const PIXEL_GIF = "R0lGODlhAQABAIAAAAAAAP///ywAAAAAAQABAAACAUwAOw==";
const BROKEN_PNG = "iVBORw0KGgo=";

const media = (name, contentType, base64) => ({
  name,
  content_type: contentType,
  data_url: `data:${contentType};base64,${base64}`,
});

const png = (name) => media(name, "image/png", PIXEL_PNG);
const gif = (name) => media(name, "image/gif", PIXEL_GIF);

async function armPendingObservation(page) {
  await page.evaluate(() => {
    window.__toolMediaPendingNodes = [];
    window.__toolMediaSetAttribute = Element.prototype.setAttribute;
    window.__toolMediaSetAttributeNs = Element.prototype.setAttributeNS;
    const capture = (element, name, value) => {
      if (element instanceof HTMLImageElement
        && name === "data-mosaic-image-state"
        && value === "pending") {
        window.__toolMediaPendingNodes.push(element);
      }
    };
    Element.prototype.setAttribute = function setAttribute(name, value) {
      capture(this, name, value);
      return window.__toolMediaSetAttribute.call(this, name, value);
    };
    Element.prototype.setAttributeNS = function setAttributeNS(namespace, name, value) {
      capture(this, name, value);
      return window.__toolMediaSetAttributeNs.call(this, namespace, name, value);
    };
  });
}

async function waitForImages(page, key, state) {
  await waitForPageFunction(
    page,
    `tool-media card ${key} reaches ${state}`,
    TRANSITION_MS,
    ({ expectedKey, expectedState }) => {
      const card = document.querySelector(`[data-mosaic-card-key="${expectedKey}"]`);
      const images = [...(card?.querySelectorAll("img") ?? [])];
      return images.length > 0
        && images.every((image) => image.dataset.mosaicImageState === expectedState);
    },
    { expectedKey: key, expectedState: state },
  );
}

async function descriptor(card) {
  return card.evaluate((article) => ({
    key: article.dataset.mosaicCardKey,
    activity: article.dataset.agentActivity,
    aria: article.getAttribute("aria-label"),
    imageOnly: article.classList.contains("mosaic-message-card--image-only"),
    text: article.querySelector(".message-body")?.textContent?.trim() ?? "",
    tool: article.querySelector(".message-badge--tool")?.textContent,
    agent: article.querySelector(".message-agent-name")?.getAttribute("aria-label"),
    names: [...article.querySelectorAll(".message-attachments img")]
      .map((image) => image.getAttribute("alt")),
    identities: [...article.querySelectorAll(".message-attachments img")]
      .map((image) => image.dataset.mediaIdentity),
    sources: [...article.querySelectorAll(".message-attachments img")]
      .map((image) => image.getAttribute("src")),
  }));
}

async function assertCard(card, expected, label) {
  const observed = await descriptor(card);
  assert.equal(observed.activity, "tool", `${label} retains typed tool activity`);
  assert.match(observed.aria, /^Tool result from /, `${label} exposes tool and agent attribution`);
  assert.equal(observed.tool, "TOOL", `${label} visibly identifies tool activity`);
  assert.match(observed.agent, /^Copy agent id for /, `${label} keeps accessible agent identity`);
  assert.equal(observed.imageOnly, expected.text.length === 0, `${label} chooses the image-only tier`);
  assert.equal(observed.text, expected.text, `${label} retains its bounded textual outcome`);
  assert.deepEqual(observed.names, expected.media.map(({ name }) => name), `${label} keeps image order`);
  assert.ok(observed.identities.every(Boolean), `${label} exposes every admitted media identity`);
  assert.equal(new Set(observed.identities).size, expected.unique, `${label} keeps exact byte identities`);
  assert.deepEqual(observed.sources, expected.media.map(({ data_url: source }) => source));
  const attachmentBox = await card.locator(".message-attachments").evaluate((attachments) => {
    const cardBox = attachments.closest(".mosaic-message-card").getBoundingClientRect();
    const box = attachments.getBoundingClientRect();
    return { card: cardBox.toJSON(), attachments: box.toJSON() };
  });
  assert.ok(
    attachmentBox.attachments.left >= attachmentBox.card.left - 1
      && attachmentBox.attachments.right <= attachmentBox.card.right + 1,
    `${label} keeps its scroll-owned image row inside the card`,
  );
  return observed;
}

async function assertRetained(console, expected, label) {
  for (const entry of expected) {
    const card = console.locator(`[data-mosaic-card-key="${entry.key}"]`);
    await card.waitFor({ state: "attached", timeout: TRANSITION_MS });
    assert.deepEqual(await descriptor(card), entry.descriptor, `${label} preserves ${entry.key}`);
  }
  assert.equal(
    await console.locator('.mosaic-message-card[data-agent-activity="tool"]')
      .evaluateAll((cards, keys) => cards.filter(
        (card) => keys.includes(card.dataset.mosaicCardKey),
      ).length, expected.map(({ key }) => key)),
    expected.length,
    `${label} preserves the exact tool-media card count`,
  );
  assert.deepEqual(
    await console.locator(".mosaic-message-card").evaluateAll((cards, keys) => cards
      .map((card) => card.dataset.mosaicCardKey)
      .filter((key) => keys.includes(key)), expected.map(({ key }) => key)),
    expected.map(({ key }) => key),
    `${label} preserves tool-media chronology`,
  );
}

export async function assertRuntimeToolMediaCards(
  page,
  console,
  lane,
  activityReplay,
  label,
  { boundary = true, history = false, reconnect = false } = {},
) {
  const cases = [
    {
      name: "mixed",
      text: "view image: capture.png succeeded",
      media: [png("mixed-capture.png")],
      state: "loaded",
      unique: 1,
    },
    {
      name: "image-only",
      text: "",
      media: [png("image-only-capture.png")],
      state: "loaded",
      unique: 1,
    },
    {
      name: "multi-image",
      text: "",
      media: [png("first-capture.png"), gif("second-capture.gif")],
      state: "loaded",
      unique: 2,
    },
    {
      name: "decode-error",
      text: "",
      media: [media("broken-capture.png", "image/png", BROKEN_PNG)],
      state: "error",
      unique: 1,
    },
  ];
  const baseline = await console.locator(
    '.mosaic-message-card[data-agent-activity="tool"]',
  ).count();
  await armPendingObservation(page);
  const retained = [];
  for (const expected of cases) {
    const injected = await activityReplay.injectAgentToolMedia(
      page,
      lane,
      expected.media,
      expected.text,
    );
    const card = console.locator(`[data-mosaic-card-key="${injected.key}"]`);
    await waitForImages(page, injected.key, expected.state);
    const observed = await assertCard(card, expected, `${label} ${expected.name}`);
    const previewHeight = expected.text.length === 0 ? 252 : 140;
    for (let index = 0; index < expected.media.length; index += 1) {
      const preview = card.locator(".message-attachments .message-image").nth(index);
      await assertImageLifecycle(preview, `${label} ${expected.name} image ${index + 1}`, expected.state);
      if (index === 0) {
        await assertImagePreview(preview, `${label} ${expected.name} image ${index + 1}`, previewHeight);
      } else {
        const box = await preview.locator("img").evaluate((image) => ({
          width: image.getBoundingClientRect().width,
          height: image.getBoundingClientRect().height,
          maxHeight: getComputedStyle(image).maxHeight,
        }));
        assert.ok(Math.abs(box.width - previewHeight) <= 1 && Math.abs(box.height - previewHeight) <= 1,
          `${label} ${expected.name} image ${index + 1} retains its square slot`);
        assert.equal(box.maxHeight, `${previewHeight}px`,
          `${label} ${expected.name} image ${index + 1} retains its stable cap`);
      }
      await assertFullSizeTarget(preview, `${label} ${expected.name} image ${index + 1}`,
        expected.media[index].data_url);
    }
    retained.push({ key: injected.key, descriptor: observed });
  }
  const pending = await page.evaluate(() => {
    Element.prototype.setAttribute = window.__toolMediaSetAttribute;
    Element.prototype.setAttributeNS = window.__toolMediaSetAttributeNs;
    return [...new Set(window.__toolMediaPendingNodes.map((image) => image.alt))];
  });
  for (const expected of cases.flatMap((entry) => entry.media.map(({ name }) => name))) {
    assert.ok(pending.includes(expected), `${label} observed ${expected} while pending`);
  }
  assert.equal(
    await console.locator('.mosaic-message-card[data-agent-activity="tool"]').count(),
    baseline + cases.length,
    `${label} renders one card per tool result`,
  );

  await openAndAssertIsolated(
    page,
    console.locator(`[data-mosaic-card-key="${retained[0].key}"] .message-image`).first(),
    `${label} tool image`,
    /^blob:/,
  );
  await activityReplay.replayAgentMessage(page, retained[0].key);
  await assertRetained(console, retained, `${label} duplicate live delivery`);

  if (boundary) {
    const compacted = await activityReplay.injectCompaction(page, lane, "Tool-media boundary");
    await console.locator(`[data-mosaic-card-key="${compacted.key}"]`)
      .waitFor({ state: "attached" });
    await assertRetained(console, retained, `${label} compaction boundary`);
  }

  if (history) {
    const expected = {
      name: "older-history",
      text: "view image: archived.gif succeeded",
      media: [gif("archived-capture.gif")],
      state: "loaded",
      unique: 1,
    };
    await chooseCanvasView(page, "Flow");
    const injected = await activityReplay.injectAgentToolMediaHistory(
      page,
      lane,
      expected.media,
      expected.text,
    );
    await chooseCanvasView(page, "Lanes");
    await console.getByRole("tab", { name: /^Agents/ }).click();
    const card = console.locator(`[data-mosaic-card-key="${injected.key}"]`);
    await waitForImages(page, injected.key, expected.state);
    retained.push({
      key: injected.key,
      descriptor: await assertCard(card, expected, `${label} older history`),
    });
    await assertRetained(console, retained, `${label} bounded older-history pagination`);
  }

  if (reconnect) {
    await page.reload({ waitUntil: "networkidle" });
    await chooseCanvasView(page, "Lanes");
    await console.getByRole("tab", { name: /^Agents/ }).click();
    await assertRetained(console, retained, `${label} reconnect snapshot`);
  }
  return retained;
}
