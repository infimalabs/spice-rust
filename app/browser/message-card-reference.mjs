import assert from "node:assert/strict";
import { join } from "node:path";

import {
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

// Mosaic reserves whole CSS rows while text contributes fractional intrinsic
// heights, leaving at most the final 2.5px rounding remainder inside padding.
const INSET_TOLERANCE = 3;

const referencePalettes = {
  dark: {
    page: "rgb(16, 20, 17)",
    panel: "rgb(24, 29, 25)",
    control: "rgb(20, 25, 21)",
  },
  light: {
    page: "rgb(247, 248, 244)",
    panel: "rgb(255, 255, 255)",
    control: "rgb(255, 255, 255)",
  },
};

async function assertReferencePalette(console, mode) {
  const measured = await console.locator(".team-surface").first().evaluate((surface) => {
    const background = (selector) => getComputedStyle(
      selector ? surface.querySelector(selector) : surface,
    ).backgroundColor;
    const probe = document.createElement("span");
    surface.append(probe);
    const token = (name) => {
      probe.style.background = `var(${name})`;
      const value = getComputedStyle(probe).backgroundColor;
      probe.style.background = "";
      return value;
    };
    const values = {
      page: token("--page"),
      panel: token("--panel"),
      control: token("--control"),
      controlSoft: token("--control-surface-soft"),
      body: getComputedStyle(document.body).backgroundColor,
      surface: background(),
      toolbar: background(".team-toolbar"),
      rail: background(".team-mode-rail"),
      selectedMode: background(".team-mode.selected"),
      menu: background(".team-menu"),
      ticker: background(".ticker"),
      pane: background('.team-pane:not([hidden])'),
      agent: background(".team-agent"),
      card: background('.mosaic-message-card[data-layout-phase="committed"]'),
      cardAction: background(".mosaic-message-card .card-action"),
    };
    probe.remove();
    return values;
  });
  const expected = referencePalettes[mode];
  assert.deepEqual(
    {
      page: measured.page,
      panel: measured.panel,
      control: measured.control,
    },
    expected,
    `${mode} palette copies the reference surface ladder`,
  );
  assert.equal(measured.body, expected.page, `${mode} body owns the page surface`);
  for (const owner of ["surface", "toolbar", "ticker", "pane", "card"]) {
    assert.equal(measured[owner], expected.panel, `${mode} ${owner} owns the panel surface`);
  }
  for (const owner of ["selectedMode", "agent"]) {
    assert.equal(measured[owner], expected.control, `${mode} ${owner} owns the control surface`);
  }
  for (const owner of ["rail", "menu", "cardAction"]) {
    assert.equal(
      measured[owner],
      measured.controlSoft,
      `${mode} ${owner} owns the soft control surface`,
    );
  }
  if (mode === "dark") {
    assert.equal(
      new Set([expected.page, measured.controlSoft, expected.control, expected.panel]).size,
      4,
      "dark mode keeps four visibly distinct neighboring surface steps",
    );
  }
}

async function assertMessageCardLayout(console, label) {
  const planes = await console.locator(".lane-mosaic-plane").evaluateAll((elements) => (
    elements.map((plane) => [...plane.querySelectorAll(
      '.mosaic-message-card[data-layout-phase="committed"]',
    )].map((card) => {
      const box = card.getBoundingClientRect();
      const style = getComputedStyle(card);
      const painted = [...card.querySelectorAll([
        ".message-body > *",
        ".message-attachments img",
        ".message-footer button",
        ".message-footer time",
        ".message-footer-separator",
      ].join(", "))];
      return {
        key: card.dataset.mosaicCardKey,
        left: box.left,
        right: box.right,
        top: box.top,
        bottom: box.bottom,
        innerBottom: box.bottom - Number.parseFloat(style.borderBottomWidth),
        contentBottom: Math.max(...painted.map(
          (child) => child.getBoundingClientRect().bottom,
        )),
      };
    }))
  ));
  for (const cards of planes) {
    assert.ok(
      cards.every((card) => card.contentBottom <= card.innerBottom + 0.5),
      `${label} cards contain their visible content: ${JSON.stringify(cards)}`,
    );
    for (let left = 0; left < cards.length; left += 1) {
      for (let right = left + 1; right < cards.length; right += 1) {
        const one = cards[left];
        const two = cards[right];
        const separated = one.right <= two.left + 0.5 || two.right <= one.left + 0.5
          || one.bottom <= two.top + 0.5 || two.bottom <= one.top + 0.5;
        assert.ok(
          separated,
          `${label} cards ${one.key} and ${two.key} do not overlap`,
        );
      }
    }
  }
}

export async function assertNaturalMessageFooterInsets(console, label) {
  const cards = await console.locator(
    '[data-card-kind="agent-message"][data-layout-phase="committed"]',
  ).evaluateAll((articles) => articles.map((article) => {
    const card = article.getBoundingClientRect();
    const cardStyle = getComputedStyle(article);
    const footer = article.querySelector(".message-footer");
    const footerBox = footer.getBoundingClientRect();
    const footerStyle = getComputedStyle(footer);
    const actions = [...footer.querySelectorAll(".card-action")]
      .map((action) => action.getBoundingClientRect());
    return {
      key: article.dataset.mosaicCardKey,
      height: card.height,
      cardTop: card.top,
      cardBottom: card.bottom,
      cardClientHeight: article.clientHeight,
      cardOffsetHeight: article.offsetHeight,
      gridTemplateRows: cardStyle.gridTemplateRows,
      alignContent: cardStyle.alignContent,
      footerHeight: footerBox.height,
      footerTop: footerBox.top,
      footerBottom: footerBox.bottom,
      footerAlignSelf: footerStyle.alignSelf,
      footerMarginTop: footerStyle.marginTop,
      actionHeights: actions.map((action) => action.height),
      actionBottoms: actions.map((action) => footerBox.bottom - action.bottom),
      insets: {
        bottom: card.bottom - Number.parseFloat(cardStyle.borderBottomWidth) - footerBox.bottom,
        left: footerBox.left - card.left - Number.parseFloat(cardStyle.borderLeftWidth),
        right: card.right - Number.parseFloat(cardStyle.borderRightWidth) - footerBox.right,
      },
      padding: {
        bottom: Number.parseFloat(cardStyle.paddingBottom),
        left: Number.parseFloat(cardStyle.paddingLeft),
        right: Number.parseFloat(cardStyle.paddingRight),
      },
    };
  }));
  assert.ok(cards.length >= 2, `${label} has short and tall message cards`);
  const heights = cards.map((card) => card.height);
  assert.ok(
    Math.max(...heights) > Math.min(...heights),
    `${label} exercises distinct short and tall card reservations`,
  );
  for (const card of cards) {
    assert.ok(
      Math.abs(card.insets.bottom - card.padding.bottom) <= INSET_TOLERANCE,
      `${label} card ${card.key} footer ends at the natural bottom padding: ${JSON.stringify(card)}`,
    );
    assert.ok(
      Math.abs(card.insets.left - card.padding.left) <= 0.5
        && Math.abs(card.insets.right - card.padding.right) <= 0.5,
      `${label} card ${card.key} footer keeps the natural side padding: ${JSON.stringify(card)}`,
    );
    assert.ok(
      card.actionHeights.length === 3
        && card.actionHeights.every((height) => height <= card.footerHeight + 0.5)
        && card.actionBottoms.every((inset) => Math.abs(inset) <= 0.5),
      `${label} card ${card.key} controls reach the natural footer bottom edge: ${JSON.stringify(card)}`,
    );
  }
}

function assertReferenceGeometry(geometry, agentLabels) {
  assert.equal(geometry.card.fontSize, "16px", "message body keeps the reference type size");
  assert.equal(geometry.card.lineHeight, "20px", "message body keeps the reference line rhythm");
  assert.match(geometry.card.fontFamily, /system-ui/, "message prose uses the system sans stack");
  assert.equal(geometry.card.paddingLeft, "9px", "message card keeps reference inline padding");
  assert.equal(geometry.card.paddingTop, "9px", "message card keeps reference block padding");
  assert.equal(geometry.grid.paddingLeft, "11px", "message plane keeps the reference origin gutter");
  assert.equal(geometry.grid.paddingTop, "11px", "message plane keeps the reference origin gutter");
  assert.equal(geometry.card.borderRadius, "10px", "message card keeps the reference radius");
  assert.equal(geometry.card.background, geometry.panel, "message card uses the neutral panel surface");
  assert.equal(geometry.card.borderLeftWidth, "1px", "the card has no left accent rail");
  assert.equal(geometry.card.borderRightWidth, "3px", "the producer accent stays on the right");
  assert.equal(
    geometry.card.borderLeftColor,
    geometry.card.borderBottomColor,
    "the left edge uses the same soft perimeter border as the card",
  );
  assert.notEqual(
    geometry.card.borderLeftColor,
    geometry.card.borderRightColor,
    "the right producer edge remains distinguishable from the soft perimeter",
  );
  assert.equal(geometry.inlineCode.background, "rgba(0, 0, 0, 0)");
  assert.equal(geometry.inlineCode.paddingLeft, "0px");
  assert.match(geometry.inlineCode.fontFamily, /monospace/);
  assert.ok(geometry.inlineCodeText, "the inline-code treatment is measured on real prose");

  assert.equal(geometry.footer.display, "flex");
  assert.equal(geometry.footer.height, 32, "the footer follows the natural action height");
  assert.equal(geometry.footer.fontSize, "12px");
  assert.equal(geometry.footer.marginTop, "8px");
  assert.deepEqual(
    geometry.footerChildren,
    ["message-footer-left", "message-footer-right"],
    "age and agent actions occupy the reference footer clusters",
  );
  assert.equal(geometry.classifiers, 0, "the generic agent classifier is absent");
  assert.doesNotMatch(geometry.footerText, /(?:^|\s)agent(?:\s|$)/i);
  assert.ok(
    agentLabels.includes(geometry.agent.text),
    "the visible footer uses an exact configured name or worktree basename",
  );
  assert.equal(geometry.agent.fontSize, "12px");
  assert.equal(geometry.agent.fontWeight, "400");
  assert.match(geometry.agent.fontFamily, /system-ui/);
  assert.equal(
    /^Copy agent id for [0-9a-f]{8}\/[0-9a-f]{8} · [^\n·]+(?: · [^\n·]+)?$/i.test(
      geometry.agent.ariaLabel,
    ),
    true,
    "the accessible copy control retains the complete unellipsized attribution",
  );
  assert.match(geometry.agent.title, /^Copy agent id\n[0-9a-f-]{36}$/);
  assert.equal(geometry.agent.overflow, "hidden");
  assert.equal(geometry.agent.textOverflow, "ellipsis");
  assert.equal(geometry.agent.whiteSpace, "nowrap");
  assert.ok(
    geometry.agent.scrollWidth <= geometry.agent.clientWidth,
    "the compact visible identity fits without ellipsis",
  );
  assert.equal(geometry.actions.gap, "6px");
  assert.equal(geometry.actions.buttons.length, 3);
  assert.ok(
    geometry.actions.buttons.every((button) => button.width === 32 && button.height === 32),
    "each message action occupies the reference 32px circle",
  );
}

async function waitForCommittedResize(page, console, viewport) {
  const priorWidths = await console.locator(".lane-mosaic-plane").evaluateAll(
    (planes) => planes.map((plane) => Number(plane.dataset.committedWidth)),
  );
  await page.setViewportSize({ width: viewport.width, height: viewport.height });
  await waitForPage(page, `${viewport.label} message-card width commits`, TRANSITION_MS,
    (before) => {
      const planes = [...document.querySelectorAll(
        '[data-pane="agent-console"] .lane-mosaic-plane',
      )];
      return planes.length === before.length && planes.every((plane, index) => (
        plane.dataset.measurementPhase === "ready"
        && Number(plane.dataset.committedWidth) !== before[index]
        && Math.abs(Number(plane.dataset.committedWidth) - plane.clientWidth) <= 1
      ));
    }, priorWidths);
}

async function captureReferenceView(page, console, artifacts, viewport, mode) {
  await waitForCommittedResize(page, console, viewport);
  await assertMessageCardLayout(console, viewport.label);
  await assertNaturalMessageFooterInsets(console, `${mode} ${viewport.label}`);
  await assertReferencePalette(console, mode);
  const cards = console.locator(
    '[data-card-kind="agent-message"][data-layout-phase="committed"]',
  );
  const fits = await cards.evaluateAll((articles) => articles.map((article) => ({
    key: article.dataset.mosaicCardKey,
    width: article.getBoundingClientRect().width,
    cardClientWidth: article.clientWidth,
    cardScrollWidth: article.scrollWidth,
    footerClientWidth: article.querySelector(".message-footer").clientWidth,
    footerScrollWidth: article.querySelector(".message-footer").scrollWidth,
  })));
  assert.ok(
    fits.every((fit) => fit.cardScrollWidth <= fit.cardClientWidth + 1),
    `${viewport.label} cards have no horizontal overflow: ${JSON.stringify(fits)}`,
  );
  assert.ok(
    fits.every((fit) => fit.footerScrollWidth <= fit.footerClientWidth + 1),
    `${viewport.label} footers have no horizontal overflow: ${JSON.stringify(fits)}`,
  );
  const widest = fits.reduce((selected, candidate) => (
    candidate.width > selected.width ? candidate : selected
  ));
  const screenshotCard = console.locator(`[data-mosaic-card-key="${widest.key}"]`);
  assert.equal(
    await screenshotCard.evaluate((article) => getComputedStyle(article).backgroundColor),
    referencePalettes[mode].panel,
    `${viewport.label} capture explicitly exercises ${mode} mode`,
  );
  await screenshotCard.screenshot({
    path: join(artifacts, `canvas-message-card-${viewport.label}-${mode}.png`),
  });
}

export async function assertReferenceMessageCard(
  page,
  console,
  card,
  artifacts,
  agentLabels,
) {
  const geometry = await card.evaluate((article) => {
    const measured = (node) => {
      const box = node.getBoundingClientRect();
      const style = getComputedStyle(node);
      return {
        width: box.width,
        height: box.height,
        background: style.backgroundColor,
        borderBottomColor: style.borderBottomColor,
        borderLeftColor: style.borderLeftColor,
        borderLeftWidth: style.borderLeftWidth,
        borderRadius: style.borderRadius,
        borderRightColor: style.borderRightColor,
        borderRightWidth: style.borderRightWidth,
        display: style.display,
        fontFamily: style.fontFamily,
        fontSize: style.fontSize,
        fontWeight: style.fontWeight,
        gap: style.gap,
        lineHeight: style.lineHeight,
        marginTop: style.marginTop,
        paddingLeft: style.paddingLeft,
        paddingTop: style.paddingTop,
      };
    };
    const footer = article.querySelector(".message-footer");
    const agent = article.querySelector(".message-agent-name");
    const actions = article.querySelector(".message-actions");
    const inlineCode = article.closest(".team-surface")?.querySelector(".message-body code");
    const grid = article.closest(".team-activity-grid");
    const panelProbe = document.createElement("span");
    panelProbe.style.background = "var(--panel)";
    article.append(panelProbe);
    const panel = getComputedStyle(panelProbe).backgroundColor;
    panelProbe.remove();
    return {
      card: measured(article),
      panel,
      grid: measured(grid),
      footer: measured(footer),
      footerChildren: [...footer.children].map((node) => node.className),
      footerText: footer.innerText,
      classifiers: footer.querySelectorAll(".message-key, .message-classifiers").length,
      agent: {
        ...measured(agent),
        ariaLabel: agent.getAttribute("aria-label"),
        clientWidth: agent.clientWidth,
        overflow: getComputedStyle(agent).overflow,
        scrollWidth: agent.scrollWidth,
        text: agent.innerText,
        textOverflow: getComputedStyle(agent).textOverflow,
        title: agent.title,
        whiteSpace: getComputedStyle(agent).whiteSpace,
      },
      actions: {
        ...measured(actions),
        buttons: [...actions.querySelectorAll("button")].map(measured),
      },
      inlineCode: measured(inlineCode),
      inlineCodeText: inlineCode?.textContent ?? "",
    };
  });
  assertReferenceGeometry(geometry, agentLabels);

  const original = page.viewportSize();
  for (const mode of ["dark", "light"]) {
    await page.emulateMedia({ colorScheme: mode });
    await captureReferenceView(page, console, artifacts, {
      label: "reference", width: 648, height: 726,
    }, mode);
    await captureReferenceView(page, console, artifacts, {
      label: "mobile", width: 390, height: 720,
    }, mode);
  }
  await waitForCommittedResize(page, console, { label: "restored reference", ...original });
  await assertMessageCardLayout(console, "restored reference");
  await page.emulateMedia({ colorScheme: null });
}
