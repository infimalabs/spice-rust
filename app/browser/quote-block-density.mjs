import assert from "node:assert/strict";
import { join } from "node:path";

const PIXEL_TOLERANCE = 0.75;

function near(actual, expected) {
  return Math.abs(actual - expected) <= PIXEL_TOLERANCE;
}

async function observeQuoteCard(card) {
  return card.evaluate((element) => {
    const number = (value) => Number.parseFloat(value);
    const cardBox = element.getBoundingClientRect();
    const quotes = [...element.querySelectorAll("blockquote")].map((quote) => {
      const style = getComputedStyle(quote);
      const box = quote.getBoundingClientRect();
      const parent = quote.parentElement?.closest("blockquote");
      const parentBox = parent?.getBoundingClientRect();
      return {
        borderLeft: number(style.borderLeftWidth),
        clientWidth: quote.clientWidth,
        marginBottom: number(style.marginBottom),
        marginTop: number(style.marginTop),
        paddingBottom: number(style.paddingBottom),
        paddingLeft: number(style.paddingLeft),
        paddingRight: number(style.paddingRight),
        paddingTop: number(style.paddingTop),
        parentWidth: parentBox?.width ?? null,
        railOffset: parentBox ? box.left - parentBox.left : null,
        scrollWidth: quote.scrollWidth,
        width: box.width,
      };
    });
    const paragraphs = [...element.querySelectorAll("blockquote p")].map((paragraph) => ({
      marginBottom: number(getComputedStyle(paragraph).marginBottom),
    }));
    const lists = [...element.querySelectorAll("blockquote ul, blockquote ol")].map((list) => {
      const style = getComputedStyle(list);
      return {
        marginBottom: number(style.marginBottom),
        paddingLeft: number(style.paddingLeft),
      };
    });
    return {
      card: {
        clientHeight: element.clientHeight,
        clientWidth: element.clientWidth,
        scrollHeight: element.scrollHeight,
        scrollWidth: element.scrollWidth,
        width: cardBox.width,
      },
      formatting: {
        code: element.querySelectorAll("blockquote code").length,
        listItems: element.querySelectorAll("blockquote li").length,
        strong: element.querySelectorAll("blockquote strong").length,
      },
      paragraphs,
      lists,
      quotes,
    };
  });
}

export async function assertQuoteBlockDensity(cards, label) {
  const observations = {};
  for (const [kind, card] of Object.entries(cards)) {
    assert.equal(await card.count(), 1, `${label} has one ${kind} quote card`);
    const observed = await observeQuoteCard(card);
    observations[kind] = observed;
    assert.ok(observed.quotes.length >= 2, `${label} ${kind} preserves nested quote depth`);
    assert.ok(
      observed.card.scrollWidth <= observed.card.clientWidth + 1,
      `${label} ${kind} card has no horizontal overflow: ${JSON.stringify(observed.card)}`,
    );
    assert.ok(
      observed.card.scrollHeight <= observed.card.clientHeight + 1,
      `${label} ${kind} card has no clipped vertical content: ${JSON.stringify(observed.card)}`,
    );
    for (const [index, quote] of observed.quotes.entries()) {
      assert.ok(near(quote.borderLeft, 3), `${label} ${kind} quote ${index} keeps a 3px rail`);
      assert.ok(
        index === 0
          ? quote.marginTop >= 0 && quote.marginTop <= 6 + PIXEL_TOLERANCE
          : near(quote.marginTop, 6),
        `${label} ${kind} quote ${index} has compact top rhythm: ${quote.marginTop}`,
      );
      assert.ok(
        quote.marginBottom >= 0 && quote.marginBottom <= 6 + PIXEL_TOLERANCE,
        `${label} ${kind} quote ${index} has compact bottom rhythm: ${quote.marginBottom}`,
      );
      assert.ok(near(quote.paddingTop, 6) && near(quote.paddingBottom, 6),
        `${label} ${kind} quote ${index} has compact block padding`);
      const expectedInline = index === 0 ? 8 : 6;
      assert.ok(near(quote.paddingLeft, expectedInline) && near(quote.paddingRight, expectedInline),
        `${label} ${kind} quote ${index} has bounded inline padding`);
      assert.ok(quote.scrollWidth <= quote.clientWidth + 1,
        `${label} ${kind} quote ${index} wraps without horizontal overflow`);
      if (quote.railOffset !== null) {
        assert.ok(near(quote.railOffset, 8),
          `${label} ${kind} quote ${index} advances one 8px nesting step: ${quote.railOffset}`);
        assert.ok(quote.parentWidth - quote.width <= 18,
          `${label} ${kind} quote ${index} retains usable line width`);
      }
    }
    assert.ok(observed.paragraphs.every(({ marginBottom }) => marginBottom <= 6 + PIXEL_TOLERANCE),
      `${label} ${kind} quote paragraphs keep a six-pixel rhythm`);
    assert.ok(observed.lists.every(({ marginBottom, paddingLeft }) => (
      marginBottom <= 6 + PIXEL_TOLERANCE && near(paddingLeft, 18)
    )), `${label} ${kind} quote lists stay compact`);
  }
  assert.ok(observations.user.formatting.code > 0, `${label} user quote retains inline code`);
  assert.ok(observations.user.formatting.strong > 0, `${label} user quote retains emphasis`);
  assert.ok(observations.user.formatting.listItems >= 2, `${label} user quote retains its list`);
  return observations;
}

export async function captureQuoteBlockDensity(page, cards, artifacts, prefix) {
  for (const [kind, card] of Object.entries(cards)) {
    // Committed Mosaic cards can sit outside their overflow-clipped viewport.
    // Clone at the measured width so the artifact contains the complete paint.
    const token = await card.evaluate((element) => {
      const id = crypto.randomUUID();
      const host = document.createElement("section");
      host.className = "team-surface";
      host.dataset.quoteCaptureHost = id;
      host.style.width = `${Math.min(element.getBoundingClientRect().width, window.innerWidth)}px`;
      host.style.maxWidth = "none";
      host.style.minHeight = "0";
      host.style.overflow = "visible";
      host.style.position = "fixed";
      host.style.top = "0";
      host.style.left = "0";
      host.style.zIndex = "2147483647";
      const clone = element.cloneNode(true);
      clone.dataset.quoteCaptureCard = id;
      clone.style.position = "relative";
      clone.style.inset = "auto";
      clone.style.height = "auto";
      clone.style.visibility = "visible";
      host.append(clone);
      document.body.append(host);
      return id;
    });
    const clone = page.locator(`[data-quote-capture-card="${token}"]`);
    try {
      await clone.screenshot({ path: join(artifacts, `${prefix}-${kind}.png`) });
    } finally {
      await page.locator(`[data-quote-capture-host="${token}"]`).evaluate((host) => host.remove());
    }
  }
}
