import assert from "node:assert/strict";

function withoutStatus(className) {
  return className.replace(/ (?:queued|running|running-stale|bound|unbound)$/, "");
}

/// Hold quoted composer chrome to the primary structure while proving the age
/// stays pinned to the message that minted this quote.
export async function assertQuotedComposerParity(quotedComposer, primaryHeadingChildren) {
  const heading = quotedComposer.locator(".composer-shard-heading");
  const children = await heading.locator(":scope > *").evaluateAll((elements) =>
    elements.map((element) => ({
      className: element.className,
      tagName: element.tagName,
      text: element.textContent?.trim(),
    })),
  );
  assert.deepEqual(
    children.slice(0, 3).map((child) => withoutStatus(child.className)),
    primaryHeadingChildren.slice(0, 3).map((child) => withoutStatus(child.className)),
    "quote and primary headers share age, prose, and image-indicator chrome",
  );
  assert.deepEqual(
    [children.length, children[3]?.tagName],
    [4, "BUTTON"],
    "the quote close button replaces only the primary hamburger",
  );
  assert.notEqual(
    children[0]?.text,
    "↩",
    "the quote header shows relative time rather than a return-arrow substitute",
  );

  const provenance = await heading.locator(".composer-shard-age").evaluate((link) => {
    const source = document.querySelector(link.getAttribute("href"));
    return {
      heading: link.querySelector("[data-age-authored]")?.dataset.ageAuthored,
      source: source?.querySelector("[data-age-authored]")?.dataset.ageAuthored,
    };
  });
  assert.ok(provenance.heading, "the quoted composer renders a relative source age");
  assert.equal(
    provenance.heading,
    provenance.source,
    "the quoted composer age remains pinned to its exact source message",
  );
}

function readComposerTreatment(composer) {
  return composer.evaluate((band) => {
    const box = (element) => element.getBoundingClientRect().toJSON();
    const style = (element) => getComputedStyle(element);
    const heading = band.querySelector(".composer-shard-heading");
    const age = heading?.querySelector(".composer-shard-age");
    const prose = heading?.querySelector(".composer-shard-prose");
    const textarea = band.querySelector("textarea");
    const control = heading?.querySelector(":scope > button, .agent-action-menu > summary");
    if (!heading || !age || !prose || !textarea || !control) {
      throw new Error("composer reference chrome is incomplete");
    }
    return {
      band: {
        borderWidth: style(band).borderTopWidth,
        borderRadius: style(band).borderRadius,
        boxShadow: style(band).boxShadow,
        background: style(band).backgroundColor,
      },
      heading: {
        height: box(heading).height,
        borderBottomWidth: style(heading).borderBottomWidth,
      },
      age: {
        fontSize: style(age).fontSize,
        decoration: style(age).textDecorationLine,
        underlineOffset: style(age).textUnderlineOffset,
      },
      prose: { fontSize: style(prose).fontSize, width: box(prose).width },
      textarea: {
        fontSize: style(textarea).fontSize,
        placeholder: textarea.placeholder,
        outlineStyle: style(textarea).outlineStyle,
      },
      control: { width: box(control).width, height: box(control).height },
    };
  });
}

function assertReferenceBand(kind, treatment) {
  assert.deepEqual(
    [treatment.band.borderWidth, treatment.band.borderRadius],
    ["1px", "7px"],
    `${kind} composer keeps one soft reference perimeter`,
  );
  assert.notEqual(treatment.band.boxShadow, "none", `${kind} composer keeps the inset treatment`);
  assert.deepEqual(
    [treatment.heading.height, treatment.heading.borderBottomWidth],
    [30, "2px"],
    `${kind} composer uses the 30px header and 2px accent divider`,
  );
  assert.deepEqual(
    [treatment.age.fontSize, treatment.prose.fontSize, treatment.textarea.fontSize],
    ["12px", "12px", "14px"],
    `${kind} composer uses the readable reference type ramp`,
  );
  assert.match(treatment.age.decoration, /underline/, `${kind} linked relatime is underlined`);
  assert.equal(treatment.age.underlineOffset, "2px", `${kind} relatime keeps its underline offset`);
  assert.ok(treatment.prose.width > 0, `${kind} header label retains visible width`);
  assert.deepEqual(
    [treatment.control.width, treatment.control.height],
    [22, 22],
    `${kind} trailing header control keeps the reference circular hit box`,
  );
}

async function assertComposerTargets(primary, quoted, placeholder) {
  const [quotedPlaceholder, primaryPlaceholder] = await Promise.all([
    quoted.getByRole("textbox").getAttribute("placeholder"),
    primary.getByRole("textbox").getAttribute("placeholder"),
  ]);
  assert.equal(
    quotedPlaceholder,
    primaryPlaceholder,
    "quoted and primary editors describe the same target without vendor prose",
  );

  const lines = placeholder.split("\n");
  assert.ok(lines[0]?.trim(), "the placeholder names its agent or branch target");
  assert.doesNotMatch(
    lines[0] || "",
    /^(?:claude|codex|openai),/i,
    "vendor identity is absent from placeholder prose",
  );
  assert.match(lines[1] || "", /^\d+ pending, /, "placeholder retains pending and liveness");
  const targetLines = await primary
    .locator("xpath=ancestor::*[contains(concat(' ', normalize-space(@class), ' '), ' team-surface ')][1]")
    .locator(".composer-shard--primary textarea")
    .evaluateAll((textareas) => textareas.map((textarea) => textarea.placeholder.split("\n")[0]));
  assert.ok(
    targetLines.some((line) => line.includes(", ")),
    `a configured agent placeholder names both agent and branch: ${JSON.stringify(targetLines)}`,
  );
}

async function assertFocusedComposer(primary) {
  await primary.getByRole("textbox").focus();
  const focused = await readComposerTreatment(primary);
  assert.deepEqual(
    [focused.band.borderWidth, focused.textarea.outlineStyle],
    ["1px", "none"],
    "editor focus uses the single composer perimeter instead of a thick nested outline",
  );
}

function readPrimaryIdentity(primary) {
  return primary.evaluate((band) => {
    const icon = band.querySelector(".composer-vendor-icon");
    const menuIcon = band.querySelector(".composer-menu-icon");
    const parent = band.closest(".team-agent");
    if (!icon || !menuIcon || !parent) throw new Error("primary identity chrome is incomplete");
    const iconBox = icon.getBoundingClientRect();
    const bandBox = band.getBoundingClientRect();
    const menuBox = menuIcon.getBoundingClientRect();
    return {
      vendor: icon.getAttribute("data-composer-vendor-icon"),
      title: icon.getAttribute("title"),
      aria: icon.getAttribute("aria-label"),
      parentTitle: parent.getAttribute("title"),
      hasVector: Boolean(icon.querySelector("svg path")),
      icon: {
        width: iconBox.width,
        height: iconBox.height,
        right: bandBox.right - iconBox.right,
        bottom: bandBox.bottom - iconBox.bottom,
      },
      menu: {
        width: menuBox.width,
        height: menuBox.height,
        bars: menuIcon.children.length,
      },
    };
  });
}

function assertPrimaryIdentity(identity) {
  assert.ok(["claude", "codex", "openai"].includes(identity.vendor));
  assert.ok(identity.hasVector, "recognized vendor renders its real vector emblem");
  assert.equal(identity.title, identity.aria, "vendor hover and accessible identity stay aligned");
  assert.equal(identity.title, identity.parentTitle, "vendor icon owns the standing identity tooltip");
  assert.deepEqual(
    identity.icon,
    { width: 18, height: 18, right: 9, bottom: 9 },
    "vendor emblem occupies the 8px inset inside the one-pixel reference perimeter",
  );
  assert.deepEqual(identity.menu, { width: 11, height: 8, bars: 3 });
}

/// Hold the two live composer variants to the predecessor's visual hierarchy.
/// These are computed-style and geometry observations rather than screenshot
/// pixels, so failures name the broken contract directly at narrow lane widths.
export async function assertComposerReferenceTreatment(primary, quoted) {
  const [primaryStyle, quoteStyle] = await Promise.all([
    readComposerTreatment(primary),
    readComposerTreatment(quoted),
  ]);
  assertReferenceBand("primary", primaryStyle);
  assertReferenceBand("quoted", quoteStyle);
  assert.notEqual(
    primaryStyle.band.background,
    quoteStyle.band.background,
    "quoted context remains subtly distinct from the primary composer",
  );
  await assertComposerTargets(primary, quoted, primaryStyle.textarea.placeholder);
  await assertFocusedComposer(primary);
  assertPrimaryIdentity(await readPrimaryIdentity(primary));
}
