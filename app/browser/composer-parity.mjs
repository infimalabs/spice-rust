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
