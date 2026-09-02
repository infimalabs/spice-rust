import assert from "node:assert/strict";

export async function assertImageLifecycle(preview, label, expectedState) {
  const observed = await preview.evaluate((element) => {
    const image = element.querySelector("img");
    const status = element.querySelector("[data-mosaic-image-status]");
    return {
      imageState: image?.dataset.mosaicImageState,
      imageVisibility: image ? getComputedStyle(image).visibility : null,
      statusState: status?.dataset.mosaicImageStatus,
      statusText: status?.textContent,
      statusDisplay: status ? getComputedStyle(status).display : null,
    };
  });
  const terminalText = expectedState === "error" ? "Image unavailable" : "Loading image";
  assert.equal(observed.imageState, expectedState, `${label} exposes its image lifecycle`);
  assert.equal(observed.statusState, expectedState, `${label} exposes its visible status lifecycle`);
  if (expectedState === "loaded") {
    assert.equal(observed.imageVisibility, "visible", `${label} reveals the loaded image`);
    assert.equal(observed.statusDisplay, "none", `${label} retires the loading surface`);
  } else {
    assert.equal(observed.imageVisibility, "hidden", `${label} suppresses blank or broken media`);
    assert.notEqual(observed.statusDisplay, "none", `${label} keeps a visible status surface`);
    assert.equal(observed.statusText, terminalText, `${label} names its visible state`);
  }
}

export async function assertOwningImageRevision(card, label, expectedRevision = 1) {
  const observed = await card.evaluate((element) => ({
    card: Number(element.dataset.mosaicImageRevision),
    images: [...element.querySelectorAll("img")].map((image) => ({
      revision: Number(image.dataset.mosaicImageRevision),
      measuredWidth: Number(image.dataset.mosaicImageMeasuredWidth),
    })),
  }));
  assert.equal(observed.card, expectedRevision, `${label} increments one owning card revision`);
  assert.ok(observed.images.length, `${label} owns at least one image`);
  assert.ok(observed.images.every((image) => image.revision === expectedRevision),
    `${label} applies one shared revision to every owned image: ${JSON.stringify(observed)}`);
  assert.ok(observed.images.every((image) => image.measuredWidth >= 24),
    `${label} records one measurable whole-card width: ${JSON.stringify(observed)}`);
}

export async function imageCardInvariant(card) {
  return card.evaluate((element) => {
    const body = element.querySelector(".message-body")?.cloneNode(true);
    for (const status of body?.querySelectorAll("[data-mosaic-image-status]") ?? []) status.remove();
    return {
      key: element.dataset.mosaicCardKey,
      ariaLabel: element.getAttribute("aria-label"),
      lane: element.dataset.agentLane,
      activity: element.dataset.agentActivity,
      producer: element.dataset.producerTargetId,
      span: element.dataset.span,
      text: body?.textContent,
      attribution: element.querySelector(".message-agent-name")?.getAttribute("aria-label"),
      commands: [...element.querySelectorAll("button")].map((button) => button.getAttribute("aria-label")),
    };
  });
}
