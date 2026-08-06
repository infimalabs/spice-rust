import assert from "node:assert/strict";

import { chooseCanvasView, reply } from "./canvas-fixture.mjs";

export const nestedQuoteMessage = [
  "> > The final review found no alternate execution path.",
  "> >",
  "> > `cargo xtask verify` is green.",
  ">",
  "> Why it matters: nested context remains visible.",
  "",
  "The author words belong in the composer band.",
].join("\n");
const nestedQuotePreview = "The author words belong in the composer band.";
export const entirelyQuotedMessage = [
  "> > Borrowed context has no authored preview.",
  "> > A second borrowed line stays reachable.",
].join("\n");

// A deep link that merely looks like one proves nothing, so this resolves the
// href against the document and requires it to land on an actual message card.
const resolvesToACard = (link) => {
  const href = link.getAttribute("href") ?? "";
  if (!href.startsWith("#")) {
    return false;
  }
  const target = document.getElementById(href.slice(1));
  return target?.classList.contains("mosaic-message-card") ?? false;
};

export async function assertMosaicComposer(page, fixture, seeded, artifactPath) {
  await chooseCanvasView(page, "Agents");
  const team = page.locator('[data-pane="agent-console"]');
  await team.getByRole("tab", { name: /^Compose/ }).click();
  const lane = page.locator(`.team-agent[data-agent-lane="${seeded.lane}"]`);
  const activity = team.locator(`.mosaic-message-card[data-agent-lane="${seeded.lane}"]`);
  const quoteLane = page.locator(`.team-agent[data-agent-lane="${seeded.quoteLane}"]`);
  const quoteActivity = team.locator(
    `.mosaic-message-card[data-agent-lane="${seeded.quoteLane}"]`,
  );
  await activity.filter({ hasText: "browser direction survives streaming" }).waitFor();

  const nestedCard = quoteActivity
    .filter({ hasText: nestedQuotePreview })
    .getByRole("button", { name: "Quote this message" });
  await nestedCard.click();
  let previewBand = quoteLane
    .locator(".composer-shard--draft")
    .filter({ hasText: nestedQuotePreview });
  await previewBand.waitFor();
  let previewLabel = await previewBand.locator(".composer-shard-locus").innerText();
  assert.equal(previewLabel, nestedQuotePreview);
  assert.doesNotMatch(previewLabel, /[<>]/, "the producer preview contains no quote marker");
  assert.equal(
    await previewBand.getAttribute("title"),
    nestedQuoteMessage,
    "the complete nested message remains reachable from the band",
  );
  assert.equal(
    await previewBand.locator(".composer-shard-menu").count(),
    0,
    "a draft shard carries no agent menu; only the primary shard does",
  );
  assert.equal(
    await quoteLane.locator(".composer-shard--primary .composer-shard-menu").count(),
    1,
    "the primary shard of the same agent still carries exactly one agent menu",
  );
  assert.equal(
    await previewBand.locator(".composer-shard-locus").evaluate(resolvesToACard),
    true,
    "the draft shard's deep link resolves to the message card it was quoted from",
  );
  assert.equal(
    await quoteLane
      .locator(".composer-shard--primary .composer-shard-locus")
      .evaluate(resolvesToACard),
    true,
    "the primary shard's deep link resolves to its own point in the message stream",
  );
  await nestedCard.click();
  assert.equal(
    await quoteLane.locator(".composer-shard--draft").count(),
    1,
    "quoting the same message identity reveals its existing band",
  );
  const repeatedQuoteId = await previewBand.getByRole("textbox").getAttribute("id");
  await page.waitForFunction(
    (id) => document.activeElement?.id === id,
    repeatedQuoteId,
  );
  assert.equal(
    await previewBand
      .getByRole("textbox")
      .evaluate((textarea) => document.activeElement === textarea),
    true,
    "repeated quote identity focuses the existing reply",
  );

  await quoteActivity
    .filter({ hasText: "Borrowed context has no authored preview." })
    .getByRole("button", { name: "Quote this message" })
    .click();
  assert.equal(
    await quoteLane.locator(".composer-shard--draft").count(),
    2,
    "two distinct messages retain two simultaneous quote bands",
  );
  // Two drafts overflow the agent's bounded height, and the stack must answer
  // that by scrolling rather than by clipping the shards out of reach. Both
  // readings are required: the overflow rule alone would hold on a stack short
  // enough never to use it, and the height alone cannot say the excess is
  // reachable.
  const stack = await quoteLane.evaluate((element) => ({
    overflowY: getComputedStyle(element).overflowY,
    scrollHeight: element.scrollHeight,
    clientHeight: element.clientHeight,
  }));
  assert.equal(stack.overflowY, "auto", "the shard stack scrolls its own overflow");
  assert.ok(
    stack.scrollHeight > stack.clientHeight,
    `two drafts exceed the agent height (${stack.scrollHeight} > ${stack.clientHeight})`,
  );
  previewBand = quoteLane.locator(".composer-shard--draft").first();
  await previewBand.waitFor();
  previewLabel = await previewBand.locator(".composer-shard-locus").innerText();
  assert.equal(previewLabel, "quoted message", "an absent preview has a visible placeholder");
  assert.ok(previewLabel.trim(), "the absent-preview band is not blank");
  assert.doesNotMatch(previewLabel, /[<>]/);
  assert.equal(await previewBand.getAttribute("title"), entirelyQuotedMessage);

  await activity
    .filter({ hasText: "browser direction survives streaming" })
    .getByRole("button", { name: "Quote this message" })
    .click();
  const composer = lane.locator(".mosaic-composer");
  const quotedComposer = lane.locator(".composer-shard--draft");
  await quotedComposer.waitFor();
  await composer.getByLabel("Markdown preview").waitFor();
  assert.equal(
    await composer.locator("blockquote").innerText(),
    "browser direction survives streaming",
  );
  await quotedComposer.getByRole("textbox").fill("canvas quote reply");
  assert.equal(await composer.getByRole("textbox").inputValue(), "");
  const pixel = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
    "base64",
  );
  await composer.locator('input[type="file"]').setInputFiles([
    { name: "pixel.png", mimeType: "image/png", buffer: pixel },
    { name: "pixel-copy.png", mimeType: "image/png", buffer: pixel },
  ]);
  const attachmentControls = composer.locator(".composer-attachment-list");
  await attachmentControls.getByRole("img", { name: "pixel.png" }).waitFor();
  await attachmentControls.getByRole("img", { name: "pixel-copy.png" }).waitFor();
  assert.equal(
    await composer.getByLabel("Markdown preview").locator("img").count(),
    2,
    "draft images are appended to the generated Markdown preview",
  );
  await page.screenshot({ path: artifactPath, fullPage: true });
  await quotedComposer.getByRole("textbox").press("Control+Enter");
  await activity.filter({ hasText: "canvas quote reply" }).waitFor({
    timeout: 10_000,
  });
  assert.equal(
    await lane
      .locator("textarea[data-primary-composer-target-id]")
      .evaluate((textarea) => document.activeElement === textarea),
    true,
    "quote keyboard submission returns focus to that target's primary composer",
  );
  assert.equal(
    await quoteLane.locator(".composer-shard--draft").count(),
    2,
    "target keyboard submission leaves sibling target drafts unchanged",
  );
  assert.equal(
    await activity.filter({ hasText: "canvas quote reply" }).locator(".message-body img").count(),
    2,
    "durable image links render through the card Markdown body",
  );
  const quoted = await reply(fixture, ["inbox", seeded.lane]);
  const quotedDirective = quoted.body.lanes[0].pending.find(
    (directive) => directive.text.includes("canvas quote reply"),
  );
  assert.ok(quotedDirective, "quote reply reaches the lane inbox");
  assert.match(quotedDirective.text, /> browser direction survives streaming/);
  assert.match(quotedDirective.text, /!\[pixel\.png\]\(data:image\/png;base64,/);
  assert.equal(quotedDirective.attachments.length, 2);
  assert.deepEqual(
    quotedDirective.attachments.map((attachment) => attachment.name).sort(),
    ["pixel-copy.png", "pixel.png"],
  );
  for (const attachment of quotedDirective.attachments) {
    assert.equal(attachment.content_type, "image/png");
    assert.match(attachment.data_url, /^data:image\/png;base64,/);
  }

  await lane.evaluate((target) => {
    const transfer = new DataTransfer();
    transfer.setData("text/plain", "dragged card direction");
    target.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
    }));
  });
  await activity.filter({ hasText: "dragged card direction" }).waitFor({
    timeout: 10_000,
  });
  const dropped = await reply(fixture, ["inbox", seeded.lane]);
  assert.ok(
    dropped.body.lanes[0].pending.some(
      (directive) => directive.text === "> dragged card direction",
    ),
    "dropping text on a lane sends quoted steering",
  );
}
