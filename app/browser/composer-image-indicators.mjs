import assert from "node:assert/strict";

/// One 1x1 PNG reused for every attachment: these observations measure the
/// thumbnail box and its treatment, so the pixels only have to decode.
const PIXEL_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const FILLER = ["fill-1.png", "fill-2.png", "fill-3.png", "fill-4.png", "fill-5.png", "fill-6.png"];

/// With the Images button gone, a paste onto the primary textarea is how the
/// composer really receives a draft image, so the observation attaches the same
/// way an operator does instead of reaching for a control that no longer exists.
export async function pasteImages(composer, names) {
  await composer.getByRole("textbox").evaluate((textarea, seed) => {
    const bytes = Uint8Array.from(atob(seed.encoded), (character) => character.charCodeAt(0));
    const transfer = new DataTransfer();
    for (const name of seed.names) {
      transfer.items.add(new File([bytes], name, { type: "image/png" }));
    }
    textarea.dispatchEvent(
      new ClipboardEvent("paste", { bubbles: true, cancelable: true, clipboardData: transfer }),
    );
  }, { encoded: PIXEL_PNG, names });
}

/// `composer-shard-image-indicator` is the heading's reserved name for a draft
/// image; thumbnail is what it looks like, so the readings below keep that word
/// while every selector uses the reserved one.
function measureHeading(heading) {
  return heading.evaluate((header) => {
    const box = (selector) => {
      const found = header.querySelector(selector);
      return found ? found.getBoundingClientRect().toJSON() : null;
    };
    const strip = header.querySelector(".composer-shard-image-indicators");
    return {
      header: header.getBoundingClientRect().toJSON(),
      link: box(".composer-shard-link"),
      title: box("strong"),
      thumbs: box(".composer-shard-image-indicators"),
      menu: box(".agent-action-menu, .composer-shard-heading > button"),
      strip: strip && {
        clientWidth: strip.clientWidth,
        scrollWidth: strip.scrollWidth,
        display: getComputedStyle(strip).display,
        overflowX: getComputedStyle(strip).overflowX,
        rows: new Set(
          [...strip.querySelectorAll(".composer-shard-image-indicator")].map((thumb) =>
            Math.round(thumb.getBoundingClientRect().top),
          ),
        ).size,
        images: [...strip.querySelectorAll(".composer-shard-image-indicator img")].map((image) => ({
          alt: image.alt,
          box: image.getBoundingClientRect().toJSON(),
          chip: image.closest(".composer-shard-image-indicator").getBoundingClientRect().toJSON(),
          objectFit: getComputedStyle(image).objectFit,
        })),
      },
    };
  });
}

export async function assertComposerAttachmentTreatment(composer, label) {
  const measured = await measureHeading(composer.locator(".composer-shard-heading"));
  assert.ok(measured.strip?.images.length > 0, `${label} exposes at least one draft image`);
  assert.ok(
    measured.strip.images.every((image) => (
      image.chip.width === 26 && image.chip.height === 26 &&
      image.box.width === 24 && image.box.height === 24 && image.objectFit === "cover"
    )),
    `${label} images fill square 26px chips: ${JSON.stringify(measured.strip.images)}`,
  );
  assert.ok(
    measured.title.width > 0 &&
      measured.title.right <= measured.thumbs.left + 0.5 &&
      measured.thumbs.right <= measured.menu.left + 0.5,
    `${label} images preserve the label and trailing control: ${JSON.stringify(measured)}`,
  );
}

/// Exercise the picker-less composer: attachments fill square reference chips,
/// and filling the row to MAX_ATTACHMENTS displaces neither the title, the deep
/// link, nor the primary-only hamburger.
export async function assertComposerImageIndicators(page, composer, names) {
  const heading = composer.locator(".composer-shard-heading");
  const bare = await measureHeading(heading);
  assert.equal(
    await composer.locator('input[type="file"]').count(),
    0,
    "the composer keeps no file input behind the deleted Images button",
  );
  assert.equal(
    await page.locator(".image-picker, .mosaic-composer-actions").count(),
    0,
    "no lane anywhere on the canvas still carries an Images button",
  );
  assert.doesNotMatch(
    await composer.innerText(),
    /\d\s*\/\s*8/,
    "no attachment count control survives in the composer",
  );
  // Hiding the empty strip costs the heading a grid column, which slides the
  // hamburger left until the first image lands. Named here so that regression
  // reports its own cause rather than the displacement it produces below.
  assert.ok(
    bare.strip && bare.strip.display !== "none",
    `an empty indicator strip still holds its heading column: ${JSON.stringify(bare.strip)}`,
  );

  await pasteImages(composer, [...names, ...FILLER]);
  const thumbs = heading.locator(
    ".composer-shard-image-indicators .composer-shard-image-indicator",
  );
  await thumbs.nth(names.length + FILLER.length - 1).waitFor();
  for (const name of [...names, ...FILLER]) {
    await heading.getByRole("img", { name }).waitFor();
  }

  const thumb = await thumbs.first().locator("img").evaluate((image) => {
    const style = getComputedStyle(image);
    return {
      width: image.getBoundingClientRect().width,
      height: image.getBoundingClientRect().height,
      borderRadius: style.borderRadius,
      objectFit: style.objectFit,
    };
  });
  assert.deepEqual(
    { width: thumb.width, height: thumb.height, borderRadius: thumb.borderRadius, objectFit: thumb.objectFit },
    { width: 24, height: 24, borderRadius: "3px", objectFit: "cover" },
    "a draft image fills the reference-sized square chip with a cover crop",
  );

  const filled = await measureHeading(heading);
  await assertComposerAttachmentTreatment(composer, "primary composer");
  assert.equal(filled.strip.rows, 1, "eight thumbnails stay on the single heading row");
  assert.ok(
    filled.thumbs.width <= filled.header.width / 2 && filled.title.width > 0,
    `a full row of thumbnails claims at most half the heading: ${JSON.stringify(filled)}`,
  );
  // A clipped strip and a scrolling strip report the same widths, so the
  // computed overflow is the only reading that tells them apart.
  assert.ok(
    ["auto", "scroll"].includes(filled.strip.overflowX),
    `the thumbnail strip scrolls its overflow rather than clipping it: ${JSON.stringify(filled.strip)}`,
  );
  assert.ok(
    filled.strip.scrollWidth > filled.strip.clientWidth,
    `a full row of attachments overflows the bounded strip: ${JSON.stringify(filled.strip)}`,
  );
  assert.ok(
    filled.title.right <= filled.thumbs.left + 0.5 && filled.thumbs.right <= filled.menu.left + 0.5,
    `thumbnails sit between the title and the hamburger without overlapping either: ${JSON.stringify(filled)}`,
  );
  assert.ok(
    filled.strip.images.every((image, index, images) => (
      index === 0 || images[index - 1].box.left > image.box.left
    )),
    `attachment DOM order fills the strip from right to left: ${JSON.stringify(filled.strip.images)}`,
  );
  assert.ok(
    Math.abs(filled.link.left - bare.link.left) < 0.5 &&
      Math.abs(filled.menu.right - bare.menu.right) < 0.5,
    `attachments displace neither the deep link nor the hamburger: ${JSON.stringify({ bare, filled })}`,
  );

  // Each file is read asynchronously, so the stack order is not the paste order;
  // remove by name to prove the click removes the thumbnail it decorates.
  for (const name of FILLER) {
    await heading
      .locator(`.composer-shard-image-indicator[aria-label="Remove image ${name}"]`)
      .click();
  }
  assert.equal(await thumbs.count(), names.length, "clicking a heading thumbnail removes it");
  assert.deepEqual(
    (await heading
      .locator(".composer-shard-image-indicator img")
      .evaluateAll((images) => images.map((image) => image.getAttribute("alt")))).sort(),
    [...names].sort(),
    "removal leaves exactly the attachments that were not clicked",
  );
}
