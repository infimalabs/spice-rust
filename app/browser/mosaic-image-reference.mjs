import assert from "node:assert/strict";

export async function submitAssistantActivity(page, console, lane, text, activityReplay) {
  const injected = await activityReplay.injectAgentProse(page, lane, text);
  return console.locator(`[data-mosaic-card-key="${injected.key}"]`);
}

export async function assertImagePreview(preview, label, expectedHeight) {
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

export async function imageOnlyTierGeometry(card) {
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

export function assertOneImageOnlyTier(reference, observed, label) {
  assert.equal(observed.stacks, 1, `${label} renders exactly one card-wide image stack`);
  assert.ok(observed.contained, `${label} keeps the image tier inside its body and card`);
  assert.ok(Math.abs(observed.stackHeight - 252) <= 1,
    `${label} has one 252px stack: ${JSON.stringify(observed)}`);
  assert.ok(observed.bodyHeight <= reference.bodyHeight + 1,
    `${label} body does not exceed one image tier: ${JSON.stringify({ reference, observed })}`);
  assert.ok(observed.cardScrollHeight <= reference.cardScrollHeight + 1,
    `${label} card content does not sum image tiers: ${JSON.stringify({ reference, observed })}`);
}

export async function assertFullSizeTarget(preview, label, expectedHref) {
  assert.equal(await preview.getAttribute("target"), "_blank", `${label} opens in a new tab`);
  assert.equal(await preview.getAttribute("rel"), "noopener", `${label} isolates its opener`);
  assert.match(await preview.getAttribute("aria-label"), /^Open .+ full size$/,
    `${label} names its full-size action`);
  assert.match(await preview.getAttribute("title"), /^Open .+ full size$/,
    `${label} exposes a discoverable tooltip`);
  if (expectedHref) assert.equal(await preview.getAttribute("href"), expectedHref, `${label} targets its source`);
}

export async function openAndAssertIsolated(page, preview, label, expectedUrl) {
  const opened = page.waitForEvent("popup");
  await preview.click();
  const popup = await opened;
  await popup.waitForLoadState("domcontentloaded");
  if (expectedUrl instanceof RegExp) assert.match(popup.url(), expectedUrl, `${label} opens full size`);
  else assert.equal(popup.url(), expectedUrl, `${label} opens full size`);
  assert.equal(await popup.evaluate(() => window.opener), null, `${label} has no opener`);
  await popup.close();
}
