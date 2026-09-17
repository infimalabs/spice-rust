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
      const backgroundAlpha = (color) => {
        const slashAlpha = color.match(/\/\s*([\d.]+)\s*\)/)?.[1];
        if (slashAlpha) return number(slashAlpha);
        const channels = color.match(/[\d.]+/g)?.map(number) ?? [];
        return channels.length === 4 ? channels[3] : color === "transparent" ? 0 : 1;
      };
      return {
        backgroundAlpha: backgroundAlpha(style.backgroundColor),
        borderBottom: number(style.borderBottomWidth),
        borderBottomRightRadius: number(style.borderBottomRightRadius),
        borderBottomLeftRadius: number(style.borderBottomLeftRadius),
        borderBottomStyle: style.borderBottomStyle,
        borderLeft: number(style.borderLeftWidth),
        borderLeftStyle: style.borderLeftStyle,
        borderRight: number(style.borderRightWidth),
        borderRightStyle: style.borderRightStyle,
        borderTop: number(style.borderTopWidth),
        borderTopRightRadius: number(style.borderTopRightRadius),
        borderTopLeftRadius: number(style.borderTopLeftRadius),
        borderTopStyle: style.borderTopStyle,
        clientWidth: quote.clientWidth,
        fontSize: number(style.fontSize),
        lineHeight: number(style.lineHeight),
        marginBottom: number(style.marginBottom),
        marginLeft: number(style.marginLeft),
        marginRight: number(style.marginRight),
        marginTop: number(style.marginTop),
        paddingBottom: number(style.paddingBottom),
        paddingLeft: number(style.paddingLeft),
        paddingRight: number(style.paddingRight),
        paddingTop: number(style.paddingTop),
        parentWidth: parentBox?.width ?? null,
        railOffset: parentBox ? box.left - parentBox.left : null,
        rightEdgeInset: parentBox ? parentBox.right - box.right : null,
        scrollWidth: quote.scrollWidth,
        width: box.width,
      };
    });
    const paragraphs = [...element.querySelectorAll("blockquote p")].map((paragraph) => {
      const style = getComputedStyle(paragraph);
      return {
        fontSize: number(style.fontSize),
        lineHeight: number(style.lineHeight),
        marginBottom: number(style.marginBottom),
      };
    });
    const lists = [...element.querySelectorAll("blockquote ul, blockquote ol")].map((list) => {
      const style = getComputedStyle(list);
      return {
        marginBottom: number(style.marginBottom),
        paddingLeft: number(style.paddingLeft),
      };
    });
    const body = element.querySelector(".markdown-body");
    const bodyStyle = getComputedStyle(body);
    const outsideParagraphs = [...body.children]
      .filter((child) => child.tagName === "P")
      .map((paragraph) => {
        const style = getComputedStyle(paragraph);
        return {
          fontSize: number(style.fontSize),
          lineHeight: number(style.lineHeight),
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
      typography: {
        bodyFontSize: number(bodyStyle.fontSize),
        codeFontSizes: [...element.querySelectorAll("blockquote code")]
          .map((code) => number(getComputedStyle(code).fontSize)),
        outsideParagraphs,
      },
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
      observed.typography.outsideParagraphs.length > 0,
      `${label} ${kind} retains prose outside its quote stack`,
    );
    assert.ok(
      observed.card.scrollWidth <= observed.card.clientWidth + 1,
      `${label} ${kind} card has no horizontal overflow: ${JSON.stringify(observed.card)}`,
    );
    assert.ok(
      observed.card.scrollHeight <= observed.card.clientHeight + 1,
      `${label} ${kind} card has no clipped vertical content: ${JSON.stringify(observed.card)}`,
    );
    for (const [index, quote] of observed.quotes.entries()) {
      assert.ok(
        near(quote.borderLeft, 2) && quote.borderLeftStyle === "solid",
        `${label} ${kind} quote ${index} keeps one solid 2px left rail`,
      );
      assert.ok(
        [
          [quote.borderTop, quote.borderTopStyle],
          [quote.borderRight, quote.borderRightStyle],
          [quote.borderBottom, quote.borderBottomStyle],
        ].every(([width, style]) => near(width, 0) && style === "none"),
        `${label} ${kind} quote ${index} has no top, right, or bottom border`,
      );
      assert.ok(
        [
          quote.borderTopLeftRadius,
          quote.borderTopRightRadius,
          quote.borderBottomRightRadius,
          quote.borderBottomLeftRadius,
        ].every((radius) => near(radius, 12)),
        `${label} ${kind} quote ${index} is rounded around its single rail`,
      );
      const expectedAlpha = index === 0 ? 0.08 : 0.12;
      assert.ok(
        near(quote.backgroundAlpha, expectedAlpha),
        `${label} ${kind} quote ${index} keeps its translucent layer: ${quote.backgroundAlpha}`,
      );
      assert.ok(
        near(quote.fontSize, 13) && quote.fontSize < observed.typography.bodyFontSize,
        `${label} ${kind} quote ${index} is one compact 13px tier: ${quote.fontSize}`,
      );
      assert.ok(
        near(quote.fontSize, observed.quotes[0].fontSize),
        `${label} ${kind} quote ${index} does not recursively shrink: ${quote.fontSize}`,
      );
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
      assert.ok(
        near(quote.marginLeft, 0),
        `${label} ${kind} quote ${index} does not accumulate left margin: ${quote.marginLeft}`,
      );
      assert.ok(
        near(quote.marginRight, 0),
        `${label} ${kind} quote ${index} does not accumulate right margin: ${quote.marginRight}`,
      );
      assert.ok(near(quote.paddingTop, 7) && near(quote.paddingBottom, 7),
        `${label} ${kind} quote ${index} replaces the removed horizontal edges with inset`);
      const expectedLeft = 5;
      const expectedRight = expectedLeft + 2;
      assert.ok(near(quote.paddingLeft, expectedLeft) && near(quote.paddingRight, expectedRight),
        `${label} ${kind} quote ${index} keeps compact inline content spacing`);
      assert.ok(quote.scrollWidth <= quote.clientWidth + 1,
        `${label} ${kind} quote ${index} wraps without horizontal overflow`);
      if (quote.railOffset !== null) {
        assert.ok(near(quote.railOffset, 7),
          `${label} ${kind} quote ${index} advances only by its parent rail and inset: ${quote.railOffset}`);
        assert.ok(quote.parentWidth - quote.width <= 20 + PIXEL_TOLERANCE,
          `${label} ${kind} quote ${index} retains usable line width: ${quote.parentWidth - quote.width}`);
        assert.ok(near(quote.rightEdgeInset, 7),
          `${label} ${kind} quote ${index} keeps only its parent inset at the right edge`);
      }
    }
    assert.ok(observed.paragraphs.every(({ fontSize, lineHeight, marginBottom }) => (
      near(fontSize, 13) &&
      lineHeight < observed.typography.outsideParagraphs[0].lineHeight &&
      marginBottom <= 6 + PIXEL_TOLERANCE
    )),
      `${label} ${kind} quote paragraphs keep a six-pixel rhythm`);
    assert.ok(
      observed.typography.outsideParagraphs.every(({ fontSize }) => (
        near(fontSize, observed.typography.bodyFontSize)
      )),
      `${label} ${kind} prose outside quotes retains the body type tier`,
    );
    assert.ok(
      observed.typography.codeFontSizes.every((fontSize) => near(fontSize, 12)),
      `${label} ${kind} quote code retains its 12px monospace tier`,
    );
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
