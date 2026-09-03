import assert from "node:assert/strict";

import {
  assertComposerAttachmentTreatment,
  assertComposerImageIndicators,
  pasteImages,
} from "./composer-image-indicators.mjs";
import { assertNoDraftPreview } from "./composer-shards.mjs";
import { reply } from "./canvas-fixture.mjs";

function assertDocumentOrder(directive, parts, label) {
  const positions = parts.map((part) => directive.text.indexOf(part));
  assert.ok(
    positions[0] === 0 &&
      positions.every((position, index) => index === 0 || position > positions[index - 1]),
    `${label} assembles one target document in visible order: ${JSON.stringify(positions)}`,
  );
}

/// Exercise both submission doors against complete target documents. Keyboard
/// submission sends only the focused agent; the lineage button sends every
/// nonempty agent draft. Both must preserve each composer's text-then-upload
/// order and keep uploads bound to the target whose heading showed them.
export async function assertCompleteComposerSubmission({
  page,
  fixture,
  lane,
  activity,
  quoteLane,
  quoteActivity,
  seeded,
  composer,
  quotedComposer,
  quoteSource,
  quoteSourceTitle,
  artifactPath,
}) {
  await assertNoDraftPreview(lane, 2);
  await quotedComposer.getByRole("textbox").fill("canvas quote reply");
  assert.equal(await composer.getByRole("textbox").inputValue(), "");
  await composer.getByRole("textbox").fill("canvas primary body");
  await assertComposerImageIndicators(page, composer, ["pixel.png", "pixel-copy.png"]);
  await pasteImages(quotedComposer, ["quoted-pixel.png"]);
  await quotedComposer.getByRole("img", { name: "quoted-pixel.png" }).waitFor();
  await assertComposerAttachmentTreatment(quotedComposer, "quoted composer");
  assert.equal(
    await composer.getByRole("img", { name: "quoted-pixel.png", exact: true }).count(),
    0,
    "an image pasted into a quoted composer stays out of the primary heading",
  );
  assert.equal(
    await quotedComposer.getByRole("img", { name: "pixel.png", exact: true }).count(),
    0,
    "primary images stay out of the quoted composer heading",
  );
  await assertNoDraftPreview(lane, 2);
  await page.screenshot({ path: artifactPath, fullPage: true });

  await quotedComposer.getByRole("textbox").press("Control+Enter");
  await quotedComposer.waitFor({ state: "detached", timeout: 10_000 });
  const primaryTextarea = lane.locator("textarea[data-primary-composer-target-id]");
  await primaryTextarea.evaluate((textarea) => {
    if (document.activeElement === textarea) return;
    return new Promise((resolve) => textarea.addEventListener("focus", resolve, { once: true }));
  });
  assert.equal(
    await primaryTextarea.evaluate((textarea) => document.activeElement === textarea),
    true,
    "quote keyboard submission returns focus to that target's primary composer",
  );
  assert.equal(
    await quoteLane.locator(".composer-shard--quote").count(),
    2,
    "target keyboard submission leaves sibling target drafts unchanged",
  );
  assert.equal(await activity.filter({ hasText: "canvas quote reply" }).count(), 0);

  const quoted = await reply(fixture, ["inbox", seeded.lane]);
  const quotedDirective = quoted.body.lanes[0].pending.find(
    (directive) => directive.text.includes("canvas quote reply"),
  );
  assert.ok(quotedDirective, "quote reply reaches the lane inbox");
  assertDocumentOrder(
    quotedDirective,
    [
      `> ${quoteSourceTitle}`,
      "canvas quote reply",
      "![quoted-pixel.png](data:image/png;base64,",
      "canvas primary body",
      "![pixel.png](data:image/png;base64,",
    ],
    "keyboard submission",
  );
  assert.equal(quotedDirective.attachments.length, 3);
  assert.deepEqual(
    quotedDirective.attachments.map((attachment) => attachment.name).sort(),
    ["pixel-copy.png", "pixel.png", "quoted-pixel.png"],
  );
  for (const attachment of quotedDirective.attachments) {
    assert.equal(attachment.content_type, "image/png");
    assert.match(attachment.data_url, /^data:image\/png;base64,/);
  }

  await quoteSource.getByRole("button", { name: "Quote this message" }).click();
  const firstAllQuote = lane.locator(".composer-shard--quote");
  await firstAllQuote.getByRole("textbox").fill("all-agents first quote reply");
  await composer.getByRole("textbox").fill("all-agents first primary");
  await pasteImages(composer, ["all-first.png"]);
  await composer.getByRole("img", { name: "all-first.png" }).waitFor();

  const secondAllQuote = quoteLane.locator(".composer-shard--quote").first();
  const secondPrimary = quoteLane.locator(".composer-shard--primary");
  await secondAllQuote.getByRole("textbox").fill("all-agents second quote reply");
  await secondPrimary.getByRole("textbox").fill("all-agents second primary");
  await pasteImages(secondPrimary, ["all-second.png"]);
  await secondPrimary.getByRole("img", { name: "all-second.png" }).waitFor();

  await lane.locator("xpath=ancestor::form[1]").locator(".team-send").click();
  await firstAllQuote.waitFor({ state: "detached", timeout: 10_000 });
  await secondAllQuote.waitFor({ state: "detached", timeout: 10_000 });
  const allFirstInbox = await reply(fixture, ["inbox", seeded.lane]);
  const allSecondInbox = await reply(fixture, ["inbox", seeded.quoteLane]);
  const allFirst = allFirstInbox.body.lanes[0].pending.find(
    (directive) => directive.text.includes("all-agents first primary"),
  );
  const allSecond = allSecondInbox.body.lanes[0].pending.find(
    (directive) => directive.text.includes("all-agents second primary"),
  );
  assert.equal(await activity.filter({ hasText: "all-agents first primary" }).count(), 0);
  assert.equal(await quoteActivity.filter({ hasText: "all-agents second primary" }).count(), 0);
  for (const [directive, replyText, primaryText, attachment] of [
    [allFirst, "all-agents first quote reply", "all-agents first primary", "all-first.png"],
    [allSecond, "all-agents second quote reply", "all-agents second primary", "all-second.png"],
  ]) {
    assert.ok(directive, `${primaryText} reaches its target through the all-agent button`);
    assertDocumentOrder(
      directive,
      ["> ", replyText, primaryText, `![${attachment}](data:image/png;base64,`],
      "all-agent submission",
    );
    assert.deepEqual(
      directive.attachments.map((item) => item.name),
      [attachment],
      `${primaryText} carries only its own target upload`,
    );
  }
}
