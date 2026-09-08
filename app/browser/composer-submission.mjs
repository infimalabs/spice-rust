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

async function assertCommandPreview(send, expected, label) {
  let observed = !expected;
  for (let frame = 0; frame < 8; frame += 1) {
    observed = await send.evaluate((button) => {
      const style = getComputedStyle(button);
      return style.outlineStyle === "solid" && style.outlineWidth === "3px";
    });
    if (observed === expected) break;
    await new Promise((resolve) => setTimeout(resolve, 16));
  }
  assert.equal(observed, expected, label);
}

async function assertBroadcastPreview(form, expected, label) {
  const targets = form.locator(
    '.composer-shard--primary[data-broadcast-target-eligible="true"]',
  );
  assert.ok(await targets.count() >= 2, "the broadcast fixture has multiple occupied targets");
  const outlined = await targets.evaluateAll((nodes) => nodes.map((node) => {
    const style = getComputedStyle(node);
    return style.outlineStyle === "solid" && style.outlineWidth === "2px";
  }));
  assert.equal(outlined.every((value) => value === expected), true, label);
  assert.equal(
    await form.locator('.composer-shard--quote').evaluateAll((nodes) => nodes.some((node) => (
      getComputedStyle(node).outlineStyle === "solid"
    ))),
    false,
    "quoted composers never receive the broadcast preview",
  );
}

async function setTextareaValue(textbox, value) {
  await textbox.evaluate((textarea, next) => {
    textarea.value = next;
    textarea.dispatchEvent(new InputEvent("input", {
      bubbles: true,
      inputType: next.length === 0 ? "deleteContentBackward" : "insertText",
    }));
  }, value);
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
  const primaryTextbox = composer.getByRole("textbox");
  const quotedTextbox = quotedComposer.getByRole("textbox");
  const form = lane.locator("xpath=ancestor::form[1]");
  const send = form.locator(".organization-send");
  await primaryTextbox.fill("canvas primary body");
  await primaryTextbox.focus();
  await assertCommandPreview(send, false, "focus alone does not preview a keyboard submission");
  await page.keyboard.down("Control");
  await assertCommandPreview(
    send,
    true,
    "holding the command modifier outlines the focused eligible composer's action",
  );
  assert.equal(
    await primaryTextbox.evaluate((textarea) => document.activeElement === textarea),
    true,
    "the action preview coexists with textarea focus",
  );
  const [focusedComposerBorder, idleComposerBorder] = await Promise.all([
    composer.evaluate((node) => getComputedStyle(node).borderColor),
    quoteLane.locator(".composer-shard--primary").evaluate(
      (node) => getComputedStyle(node).borderColor,
    ),
  ]);
  assert.notEqual(
    focusedComposerBorder,
    idleComposerBorder,
    "the focused composer keeps its visible perimeter while the action preview is shown",
  );
  assert.equal(
    await primaryTextbox.getAttribute("aria-keyshortcuts"),
    "Control+Shift+Enter Meta+Shift+Enter",
    "the primary composer exposes the broadcast shortcut accessibly",
  );
  const broadcastHelp = await primaryTextbox.getAttribute("aria-describedby");
  assert.ok(broadcastHelp, "the primary composer names its broadcast help");
  assert.match(
    await page.locator(`#${broadcastHelp}`).innerText(),
    /broadcasts this primary message.*every occupied agent/i,
  );
  await page.keyboard.down("Shift");
  await assertBroadcastPreview(
    form,
    true,
    "command plus Shift previews every occupied team destination",
  );
  await page.keyboard.up("Shift");
  await assertBroadcastPreview(form, false, "releasing Shift clears every broadcast preview");
  await page.keyboard.up("Control");
  await assertCommandPreview(send, false, "releasing the command modifier clears the preview");

  await page.keyboard.down("Control");
  await setTextareaValue(primaryTextbox, "");
  assert.equal(await primaryTextbox.inputValue(), "", "the held-modifier edit empties the textarea");
  assert.equal(
    await composer.getAttribute("data-command-submit-eligible"),
    "false",
    "the empty primary draft becomes ineligible before preview styling",
  );
  await assertCommandPreview(send, false, "empty focused text clears the preview");
  await setTextareaValue(primaryTextbox, "canvas primary body");
  await assertCommandPreview(send, true, "restoring eligible focused text restores the preview");
  await quotedTextbox.focus();
  await assertCommandPreview(send, false, "a quoted composer never previews the primary action");
  await primaryTextbox.focus();
  await assertCommandPreview(send, true, "returning focus to the primary composer restores the preview");
  await send.evaluate((button) => { button.disabled = true; });
  await assertCommandPreview(send, false, "a disabled action cannot retain the preview");
  await send.evaluate((button) => { button.disabled = false; });
  await assertCommandPreview(send, true, "reenabling the eligible action restores the held preview");
  await form.locator(".organization-lifetime input").focus();
  await assertCommandPreview(send, false, "leaving the draft composer clears the preview");
  await page.keyboard.up("Control");

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

  await primaryTextbox.focus();
  await page.keyboard.down("Control");
  await assertCommandPreview(send, true, "the preview is visible immediately before keyboard submit");
  const repeatedSinglePrevented = await primaryTextbox.evaluate((textarea) => {
    const repeated = new KeyboardEvent("keydown", {
      bubbles: true,
      cancelable: true,
      ctrlKey: true,
      key: "Enter",
      repeat: true,
    });
    textarea.dispatchEvent(repeated);
    return repeated.defaultPrevented;
  });
  assert.equal(
    repeatedSinglePrevented,
    true,
    "a repeated ordinary keyboard submission is consumed without suppressing its single target",
  );
  await page.keyboard.up("Control");
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
  await composer.getByRole("textbox").fill("broadcast primary body");
  await pasteImages(composer, ["broadcast.png"]);
  await composer.getByRole("img", { name: "broadcast.png" }).waitFor();

  const secondAllQuote = quoteLane.locator(".composer-shard--quote").first();
  const secondPrimary = quoteLane.locator(".composer-shard--primary");
  await secondAllQuote.getByRole("textbox").fill("all-agents second quote reply");
  await secondPrimary.getByRole("textbox").fill("all-agents second primary");
  await pasteImages(secondPrimary, ["all-second.png"]);
  await secondPrimary.getByRole("img", { name: "all-second.png" }).waitFor();

  const source = composer.getByRole("textbox");
  const repeatedPrevented = await source.evaluate((textarea) => {
    const repeated = new KeyboardEvent("keydown", {
      bubbles: true,
      cancelable: true,
      ctrlKey: true,
      key: "Enter",
      repeat: true,
      shiftKey: true,
    });
    textarea.dispatchEvent(repeated);
    document.dispatchEvent(new KeyboardEvent("keyup", { bubbles: true, key: "Control" }));
    return repeated.defaultPrevented;
  });
  assert.equal(repeatedPrevented, true, "a repeated broadcast keydown is consumed");
  assert.equal(await source.inputValue(), "broadcast primary body");

  const broadcastTargets = form.locator(
    '.composer-shard--primary[data-broadcast-target-eligible="true"]',
  );
  const targetLanes = await broadcastTargets.evaluateAll((nodes) => (
    nodes.map((node) => node.dataset.shardTargetId)
  ));
  await source.focus();
  await page.keyboard.down("Control");
  await page.keyboard.down("Shift");
  await assertBroadcastPreview(form, true, "the submitted broadcast previews every target");
  await page.keyboard.press("Enter");
  await page.keyboard.up("Shift");
  await page.keyboard.up("Control");
  await source.evaluate((textarea) => {
    if (document.activeElement === textarea && textarea.value === "") return;
    return new Promise((resolve) => textarea.addEventListener("focus", resolve, { once: true }));
  });
  assert.equal(await source.inputValue(), "", "broadcast clears only the source primary body");
  assert.equal(await composer.getByRole("img", { name: "broadcast.png" }).count(), 0);
  assert.equal(await firstAllQuote.getByRole("textbox").inputValue(), "all-agents first quote reply");
  assert.equal(await secondPrimary.getByRole("textbox").inputValue(), "all-agents second primary");
  assert.equal(await secondAllQuote.getByRole("textbox").inputValue(), "all-agents second quote reply");
  assert.equal(await secondPrimary.getByRole("img", { name: "all-second.png" }).count(), 1);
  for (const targetLane of targetLanes) {
    const targetInbox = await reply(fixture, ["inbox", targetLane]);
    const delivered = targetInbox.body.lanes[0].pending.find(
      (directive) => directive.text.includes("broadcast primary body"),
    );
    assert.ok(delivered, `broadcast reaches occupied target ${targetLane}`);
    assert.deepEqual(delivered.attachments.map((attachment) => attachment.name), ["broadcast.png"]);
    assert.equal(delivered.text.includes("all-agents first quote reply"), false);
  }

  await composer.getByRole("textbox").fill("all-agents first primary");
  await pasteImages(composer, ["all-first.png"]);
  await composer.getByRole("img", { name: "all-first.png" }).waitFor();

  await lane.locator("xpath=ancestor::form[1]").locator(".organization-send").click();
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
