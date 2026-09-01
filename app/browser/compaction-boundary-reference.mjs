import assert from "node:assert/strict";

import {
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

function channel(value) {
  return value <= 0.04045
    ? value / 12.92
    : ((value + 0.055) / 1.055) ** 2.4;
}

function luminance(color) {
  const components = color.match(/[\d.]+/g)?.slice(0, 3).map(Number);
  assert.equal(components?.length, 3, `computed color is RGB: ${color}`);
  const normalized = color.startsWith("color(srgb")
    ? components
    : components.map((component) => component / 255);
  return 0.2126 * channel(normalized[0])
    + 0.7152 * channel(normalized[1])
    + 0.0722 * channel(normalized[2]);
}

function contrast(foreground, background) {
  const lighter = Math.max(luminance(foreground), luminance(background));
  const darker = Math.min(luminance(foreground), luminance(background));
  return (lighter + 0.05) / (darker + 0.05);
}

function separated(one, two) {
  return one.right <= two.left + 0.5 || two.right <= one.left + 0.5
    || one.bottom <= two.top + 0.5 || two.bottom <= one.top + 0.5;
}

export async function waitForCompactionReady(page, divider, label) {
  const key = await divider.getAttribute("data-mosaic-card-key");
  assert.ok(key, `${label} boundary has a stable identity`);
  await waitForPage(page, `${label} boundary commits`, TRANSITION_MS, (expected) => {
    const boundary = document.querySelector(`[data-mosaic-card-key="${expected}"]`);
    const plane = boundary?.closest(".lane-mosaic-plane");
    return boundary?.dataset.layoutPhase === "committed"
      && boundary.dataset.track === "0"
      && boundary.dataset.span === "12"
      && plane?.dataset.measurementPhase === "ready"
      && Math.abs(Number(plane.dataset.committedWidth) - plane.clientWidth) <= 1;
  }, key);
  return key;
}

export async function resizeCompactionViewport(page, divider, viewport, label) {
  const key = await divider.getAttribute("data-mosaic-card-key");
  const before = await divider.evaluate((boundary) => (
    Number(boundary.closest(".lane-mosaic-plane").dataset.committedWidth)
  ));
  await page.setViewportSize(viewport);
  await waitForPage(page, `${label} width commits`, TRANSITION_MS, (expected) => {
    const boundary = document.querySelector(`[data-mosaic-card-key="${expected.key}"]`);
    const plane = boundary?.closest(".lane-mosaic-plane");
    return boundary?.dataset.layoutPhase === "committed"
      && plane?.dataset.measurementPhase === "ready"
      && Number(plane.dataset.committedWidth) !== expected.before
      && Math.abs(Number(plane.dataset.committedWidth) - plane.clientWidth) <= 1;
  }, { key, before });
}

export async function assertCompactionDivider(divider, label, expectedAttribution) {
  const geometry = await divider.evaluate((boundary) => {
    const rect = (node) => {
      const box = node.getBoundingClientRect();
      return {
        left: box.left,
        right: box.right,
        top: box.top,
        bottom: box.bottom,
        width: box.width,
        height: box.height,
      };
    };
    const measured = (node) => {
      const style = getComputedStyle(node);
      return {
        ...rect(node),
        background: style.backgroundColor,
        borderBottomWidth: style.borderBottomWidth,
        borderLeftWidth: style.borderLeftWidth,
        borderRadius: style.borderRadius,
        borderRightWidth: style.borderRightWidth,
        borderTopWidth: style.borderTopWidth,
        boxShadow: style.boxShadow,
        color: style.color,
        display: style.display,
        fontFamily: style.fontFamily,
        fontSize: style.fontSize,
        fontWeight: style.fontWeight,
        gap: style.gap,
        paddingBottom: style.paddingBottom,
        paddingLeft: style.paddingLeft,
        paddingRight: style.paddingRight,
        paddingTop: style.paddingTop,
      };
    };
    const plane = boundary.closest(".lane-mosaic-plane");
    const labelNode = boundary.querySelector(".mosaic-compaction-label");
    const agent = boundary.querySelector(".mosaic-compaction-agent");
    const context = boundary.querySelector(".message-key");
    const pageProbe = document.createElement("span");
    pageProbe.style.background = "var(--page)";
    boundary.append(pageProbe);
    const pageBackground = getComputedStyle(pageProbe).backgroundColor;
    pageProbe.remove();
    const cards = [...plane.querySelectorAll(
      '.mosaic-message-card[data-layout-phase="committed"]',
    )].map((card) => ({
      ...rect(card),
      key: card.dataset.mosaicCardKey,
      kind: card.dataset.cardKind,
      creationIndex: Number(card.dataset.creationIndex),
    }));
    return {
      card: {
        ...measured(boundary),
        clientWidth: boundary.clientWidth,
        scrollWidth: boundary.scrollWidth,
        creationIndex: Number(boundary.dataset.creationIndex),
      },
      plane: { ...rect(plane), clientWidth: plane.clientWidth },
      pageBackground,
      label: {
        ...measured(labelNode),
        text: labelNode.textContent.trim().replace(/\s+/g, " "),
        childClasses: [...labelNode.children].map((node) => node.className),
      },
      agent: { ...measured(agent), text: agent?.textContent?.trim() },
      context: { ...measured(context), text: context?.textContent?.trim() },
      contextGroup: measured(boundary.querySelector(".mosaic-compaction-context")),
      rules: [...boundary.querySelectorAll(".mosaic-compaction-rule")].map(measured),
      cards,
    };
  });

  assert.equal(geometry.card.display, "grid", `${label} is a grid divider`);
  assert.ok(geometry.card.width > 0 && geometry.card.height > 0, `${label} paints visible pixels`);
  assert.ok(Math.abs(geometry.card.left - geometry.plane.left) <= 0.5, `${label} starts at the plane edge`);
  assert.ok(Math.abs(geometry.card.right - geometry.plane.right) <= 0.5, `${label} ends at the plane edge`);
  assert.ok(Math.abs(geometry.card.width - geometry.plane.clientWidth) <= 1, `${label} uses the full plane width`);
  assert.equal(geometry.card.background, "rgba(0, 0, 0, 0)", `${label} has no panel surface`);
  assert.deepEqual(
    [
      geometry.card.borderTopWidth,
      geometry.card.borderRightWidth,
      geometry.card.borderBottomWidth,
      geometry.card.borderLeftWidth,
    ],
    ["0px", "0px", "0px", "0px"],
    `${label} has no card border or accent rail`,
  );
  assert.equal(geometry.card.borderRadius, "0px", `${label} has no card radius`);
  assert.equal(geometry.card.boxShadow, "none", `${label} has no card shadow`);
  assert.deepEqual(
    [
      geometry.card.paddingTop,
      geometry.card.paddingRight,
      geometry.card.paddingBottom,
      geometry.card.paddingLeft,
    ],
    ["0px", "0px", "0px", "0px"],
    `${label} has no card padding`,
  );
  assert.ok(geometry.card.scrollWidth <= geometry.card.clientWidth + 1, `${label} does not overflow`);

  assert.equal(geometry.rules.length, 2, `${label} has two rules`);
  assert.ok(
    geometry.rules.every((rule) => rule.width >= 24 && rule.height >= 1),
    `${label} rules have visible continuous geometry: ${JSON.stringify(geometry.rules)}`,
  );
  assert.equal(new Set(geometry.rules.map((rule) => rule.background)).size, 1);
  assert.ok(geometry.rules.every((rule) => rule.background !== "rgba(0, 0, 0, 0)"));
  assert.ok(geometry.rules[0].right <= geometry.label.left + 0.5);
  assert.ok(geometry.label.right <= geometry.rules[1].left + 0.5);

  assert.deepEqual(
    geometry.label.childClasses,
    ["mosaic-compaction-age", "", "mosaic-compaction-context"],
    `${label} keeps age, separator, and compacted-context hierarchy`,
  );
  assert.equal(geometry.agent.text, expectedAttribution);
  assert.equal(geometry.context.text, "compacted context");
  assert.ok(Number.parseFloat(geometry.contextGroup.gap) > 0, `${label} separates agent and context`);
  assert.match(geometry.label.fontFamily, /system-ui/);
  assert.equal(geometry.label.fontSize, "12px");
  assert.equal(geometry.label.fontWeight, "500");
  const labelContrast = contrast(geometry.label.color, geometry.pageBackground);
  const agentContrast = contrast(geometry.agent.color, geometry.pageBackground);
  const contextContrast = contrast(geometry.context.color, geometry.pageBackground);
  assert.ok(
    labelContrast >= 4.5,
    `${label} label contrast is accessible: ${JSON.stringify({
      foreground: geometry.label.color,
      background: geometry.pageBackground,
      contrast: labelContrast,
    })}`,
  );
  assert.ok(
    agentContrast >= 4.5,
    `${label} agent contrast is accessible: ${JSON.stringify({
      foreground: geometry.agent.color,
      background: geometry.pageBackground,
      contrast: agentContrast,
    })}`,
  );
  assert.equal(geometry.agent.color, geometry.context.color, `${label} accents the complete context label`);
  assert.ok(
    contextContrast >= 4.5,
    `${label} context contrast is accessible: ${JSON.stringify({
      foreground: geometry.context.color,
      background: geometry.pageBackground,
      contrast: contextContrast,
    })}`,
  );

  const before = geometry.cards.filter(
    (card) => card.creationIndex < geometry.card.creationIndex,
  );
  const after = geometry.cards.filter(
    (card) => card.creationIndex > geometry.card.creationIndex,
  );
  assert.ok(before.length > 0, `${label} has realistic pre-compaction cards`);
  assert.ok(after.length > 0, `${label} has realistic post-compaction cards`);
  assert.ok(
    before.every((card) => card.top >= geometry.card.bottom - 0.5),
    `${label} leaves the completed epoch below the divider`,
  );
  assert.ok(
    after.every((card) => card.bottom <= geometry.card.top + 0.5),
    `${label} starts the next epoch above the divider`,
  );
  for (let left = 0; left < geometry.cards.length; left += 1) {
    for (let right = left + 1; right < geometry.cards.length; right += 1) {
      assert.ok(
        separated(geometry.cards[left], geometry.cards[right]),
        `${label} cards ${geometry.cards[left].key} and ${geometry.cards[right].key} do not overlap`,
      );
    }
  }
  return geometry;
}

export async function assertUnavailableToolTerminal(boundary, label, expectedPreview) {
  const terminal = boundary.locator(
    '[data-terminal-status="unavailable-after-compaction"]',
  );
  assert.equal(await terminal.count(), 1, `${label} has one unresolved terminal observation`);
  const geometry = await terminal.evaluate((item) => {
    const rect = (node) => {
      const box = node.getBoundingClientRect();
      return { top: box.top, right: box.right, bottom: box.bottom, left: box.left };
    };
    const boundary = item.closest('[data-card-kind="compaction"]');
    const dividerLabel = boundary.querySelector(".mosaic-compaction-label");
    const preview = item.querySelector(".mosaic-compaction-tool");
    const status = item.querySelector(".mosaic-compaction-status");
    return {
      item: rect(item),
      boundary: rect(boundary),
      label: rect(dividerLabel),
      preview: { ...rect(preview), text: preview.textContent.trim() },
      status: { ...rect(status), text: status.textContent.trim() },
      scrollWidth: boundary.scrollWidth,
      clientWidth: boundary.clientWidth,
    };
  });
  assert.equal(geometry.preview.text, expectedPreview);
  assert.equal(
    geometry.status.text,
    "terminal result unavailable after context compaction",
  );
  assert.ok(geometry.item.top >= geometry.label.bottom - 0.5, `${label} status follows its divider`);
  assert.ok(geometry.item.left >= geometry.boundary.left - 0.5);
  assert.ok(geometry.item.right <= geometry.boundary.right + 0.5);
  assert.ok(geometry.item.bottom <= geometry.boundary.bottom + 0.5);
  assert.ok(geometry.scrollWidth <= geometry.clientWidth + 1, `${label} status does not overflow`);
}

export async function assertNoUnavailableToolTerminal(boundary, label, expectedAttribution) {
  const geometry = await boundary.evaluate((card) => {
    const rect = (node) => {
      const box = node.getBoundingClientRect();
      return {
        top: box.top,
        right: box.right,
        bottom: box.bottom,
        left: box.left,
        width: box.width,
        height: box.height,
      };
    };
    return {
      article: {
        ...rect(card),
        tag: card.tagName,
        label: card.getAttribute("aria-label"),
        rows: card.dataset.rows,
      },
      lattice: (() => {
        const plane = card.closest(".lane-mosaic-plane");
        return {
          gap: Number.parseFloat(getComputedStyle(plane).rowGap),
          module: Number(plane.dataset.module),
        };
      })(),
      children: [...card.children].map((child) => ({
        ...rect(child),
        tag: child.tagName,
        className: child.className,
      })),
      terminalLists: card.querySelectorAll(".mosaic-compaction-terminals").length,
      terminalStatuses: card.querySelectorAll("[data-terminal-status]").length,
      unavailableLabels: [...card.querySelectorAll("[aria-label]")]
        .map((node) => node.getAttribute("aria-label"))
        .filter((value) => value?.toLowerCase().includes("unavailable")),
      text: card.textContent.trim().replace(/\s+/g, " "),
    };
  });
  assert.equal(geometry.article.tag, "ARTICLE", `${label} remains an article`);
  assert.equal(
    geometry.article.label,
    `Context compacted by ${expectedAttribution}`,
    `${label} retains its accessible article label`,
  );
  assert.deepEqual(
    geometry.children.map(({ tag, className }) => [tag, className]),
    [
      ["SPAN", "mosaic-compaction-rule"],
      ["SPAN", "mosaic-compaction-label"],
      ["SPAN", "mosaic-compaction-rule"],
    ],
    `${label} contains only two rules and the center label`,
  );
  assert.equal(geometry.terminalLists, 0, `${label} has no terminal list`);
  assert.equal(geometry.terminalStatuses, 0, `${label} has no terminal status marker`);
  assert.deepEqual(geometry.unavailableLabels, [], `${label} has no hidden unavailable label`);
  assert.doesNotMatch(
    geometry.text,
    /terminal result unavailable after context compaction/i,
    `${label} has no hidden unavailable status text`,
  );
  const childTop = Math.min(...geometry.children.map((child) => child.top));
  const childBottom = Math.max(...geometry.children.map((child) => child.bottom));
  const expectedRows = Math.max(
    1,
    Math.ceil((geometry.children[1].height + geometry.lattice.gap) / geometry.lattice.module),
  );
  assert.equal(
    Number(geometry.article.rows),
    expectedRows,
    `${label} allocates rows from only its center label`,
  );
  assert.ok(
    Math.abs(
      geometry.article.height
        - (expectedRows * geometry.lattice.module - geometry.lattice.gap),
    ) <= 0.5,
    `${label} has exactly its label-derived lattice height: ${JSON.stringify(geometry)}`,
  );
  assert.ok(
    childTop >= geometry.article.top - 0.5
      && childBottom <= geometry.article.bottom + 0.5
      && Math.abs(
        (childTop - geometry.article.top) - (geometry.article.bottom - childBottom),
      ) <= 0.5,
    `${label} centers its only row without terminal-row height: ${JSON.stringify(geometry)}`,
  );
}
