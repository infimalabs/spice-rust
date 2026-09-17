import assert from "node:assert/strict";
import { join } from "node:path";

import {
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

// Mosaic reserves whole CSS rows while text contributes fractional intrinsic
// heights, leaving at most the final 2.5px rounding remainder inside padding.
const INSET_TOLERANCE = 3;
const BASELINE_TOLERANCE = 1;
const PIXEL_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

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
      ].join(", "))];
      const paintedBoxes = painted
        .map((child) => child.getBoundingClientRect())
        .filter((child) => child.width > 0 || child.height > 0);
      return {
        key: card.dataset.mosaicCardKey,
        left: box.left,
        right: box.right,
        top: box.top,
        bottom: box.bottom,
        innerBottom: box.bottom - Number.parseFloat(style.borderBottomWidth),
        contentBottom: paintedBoxes.length > 0
          ? Math.max(...paintedBoxes.map((child) => child.bottom))
          : null,
      };
    }))
  ));
  for (const cards of planes) {
    assert.ok(
      cards.every((card) => card.contentBottom === null || card.contentBottom <= card.innerBottom + 0.5),
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
    const agent = footer.querySelector(".message-agent-name");
    const agentLabel = footer.querySelector(".message-agent-label");
    const agentStyle = getComputedStyle(agent);
    const labelStyle = getComputedStyle(agentLabel);
    const agentBox = agent.getBoundingClientRect();
    const labelBox = agentLabel.getBoundingClientRect();
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
      actionTops: actions.map((action) => action.top - footerBox.top),
      actionBottoms: actions.map((action) => footerBox.bottom - action.bottom),
      identity: {
        agentClientHeight: agent.clientHeight,
        agentClientWidth: agent.clientWidth,
        agentOverflow: agentStyle.overflow,
        agentOverflowWrap: agentStyle.overflowWrap,
        agentRight: agentBox.right,
        agentScrollHeight: agent.scrollHeight,
        agentScrollWidth: agent.scrollWidth,
        agentTextOverflow: agentStyle.textOverflow,
        agentWhiteSpace: agentStyle.whiteSpace,
        labelClientHeight: agentLabel.clientHeight,
        labelClientWidth: agentLabel.clientWidth,
        labelOverflow: labelStyle.overflow,
        labelOverflowWrap: labelStyle.overflowWrap,
        labelRight: labelBox.right,
        labelScrollHeight: agentLabel.scrollHeight,
        labelScrollWidth: agentLabel.scrollWidth,
        labelTextOverflow: labelStyle.textOverflow,
        labelWhiteSpace: labelStyle.whiteSpace,
        actionLeft: actions[0]?.left ?? null,
        separatorCount: footer.querySelectorAll(".message-footer-separator").length,
      },
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
      card.actionHeights.length === 0
        || (
          card.actionHeights.length === 3
          && card.actionHeights.every((height) => height <= card.footerHeight + 0.5)
          && card.actionTops.every((inset) => inset >= -0.5)
          && card.actionBottoms.every((inset) => inset >= -0.5)
        ),
      `${label} card ${card.key} controls stay within the natural footer: ${JSON.stringify(card)}`,
    );
    assert.ok(
      card.identity.agentOverflow === "visible"
        && card.identity.agentOverflowWrap === "anywhere"
        && card.identity.agentTextOverflow === "clip"
        && card.identity.agentWhiteSpace === "normal"
        && card.identity.labelOverflow === "visible"
        && card.identity.labelOverflowWrap === "anywhere"
        && card.identity.labelTextOverflow === "clip"
        && card.identity.labelWhiteSpace === "normal"
        && card.identity.agentScrollWidth <= card.identity.agentClientWidth + 1
        && card.identity.agentScrollHeight <= card.identity.agentClientHeight + 1
        && card.identity.labelScrollWidth <= card.identity.labelClientWidth + 1
        && card.identity.labelScrollHeight <= card.identity.labelClientHeight + 1,
      `${label} card ${card.key} renders its complete identity: ${JSON.stringify(card)}`,
    );
    assert.ok(
      card.identity.separatorCount === 0
        && (card.actionHeights.length === 0
          ? card.identity.actionLeft === null
          : Math.abs(card.identity.actionLeft - card.identity.agentRight - 6) <= 1
            && Math.abs(card.identity.actionLeft - card.identity.labelRight - 6) <= 1),
      `${label} card ${card.key} separates identity and actions without a visible dot: ${JSON.stringify(card)}`,
    );
  }
}

function assertReferenceGeometry(geometry, agentLabels) {
  assert.equal(geometry.card.fontSize, "16px", "message body keeps the reference type size");
  assert.equal(geometry.card.lineHeight, "20px", "message body keeps the reference line rhythm");
  assert.match(geometry.card.fontFamily, /system-ui/, "message prose uses the system sans stack");
  assert.equal(geometry.card.paddingLeft, "9px", "message card keeps reference inline padding");
  assert.equal(geometry.card.paddingTop, "9px", "message card keeps reference block padding");
  assert.equal(geometry.grid.paddingLeft, "0px", "message viewport has no inline clipping inset");
  assert.equal(geometry.grid.paddingTop, "0px", "message viewport has no block clipping inset");
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

  assert.equal(geometry.footer.display, "grid");
  assert.equal(geometry.footer.height, 32, "the footer is exactly one natural action row");
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
  assert.equal(geometry.agent.overflow, "visible");
  assert.equal(geometry.agent.overflowWrap, "anywhere");
  assert.equal(geometry.agent.textOverflow, "clip");
  assert.equal(geometry.agent.whiteSpace, "normal");
  assert.ok(geometry.agent.scrollWidth <= geometry.agent.clientWidth + 1);
  assert.ok(geometry.agent.scrollHeight <= geometry.agent.clientHeight + 1);
  assert.ok(geometry.agent.height <= geometry.footer.height, "identity stays inside the footer");
  assert.equal(geometry.actions.gap, "6px");
  assert.equal(geometry.actions.buttons.length, 3);
  assert.ok(
    geometry.actions.buttons.every((button) => button.width === 32 && button.height === 32),
    "each message action occupies the reference 32px circle",
  );
  assert.deepEqual(
    [geometry.actions.speechIcon.width, geometry.actions.speechIcon.height],
    [15, 15],
    "the message play icon matches the predecessor's 15px viewport",
  );
  assert.ok(
    geometry.actions.buttons.every((button) => button.borderRadius === "50%"),
    "each message action keeps the reference circular hit target",
  );
  assert.ok(
    geometry.actions.buttons.every(
      (button) => button.borderLeftColor !== geometry.controlLineSoft,
    ),
    "each message action mixes its producer accent into the soft control border",
  );
}

async function assertActionInteraction(page, card) {
  const producer = card.locator(".message-agent-name");
  const labels = ["Speak this message", "Quote this message", "Copy this message"];
  const rowGeometry = () => card.evaluate((article) => {
    const cardBox = article.getBoundingClientRect();
    const box = (node) => {
      const bounds = node.getBoundingClientRect();
      return {
        bottom: Math.round(bounds.bottom - cardBox.top),
        height: Math.round(bounds.height),
        top: Math.round(bounds.top - cardBox.top),
      };
    };
    return {
      cardHeight: Math.round(cardBox.height),
      footer: box(article.querySelector(".message-footer")),
      right: box(article.querySelector(".message-footer-right")),
      actions: box(article.querySelector(".message-actions")),
    };
  });
  const before = await rowGeometry();
  await producer.focus();
  for (const label of labels) {
    await page.keyboard.press("Tab");
    assert.equal(
      await page.evaluate(() => document.activeElement?.getAttribute("aria-label")),
      label,
      `keyboard order reaches ${label}`,
    );
    assert.equal(
      await page.evaluate(() => getComputedStyle(document.activeElement).outlineStyle),
      "solid",
      `${label} retains the shared visible focus ring`,
    );
  }

  const quote = card.getByRole("button", { name: "Quote this message" });
  const box = await quote.boundingBox();
  assert.ok(box, "the quote control has a pointer hit target");
  const center = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
  const resting = await quote.evaluate((button) => getComputedStyle(button).backgroundColor);
  await page.mouse.move(center.x, center.y);
  await page.mouse.down();
  const pressed = await quote.evaluate((button) => getComputedStyle(button).backgroundColor);
  assert.notEqual(pressed, resting, "pointer press gives the quote control visible feedback");
  await page.mouse.move(0, 0);
  await page.mouse.up();
  assert.deepEqual(
    await rowGeometry(),
    before,
    "focus, hover, and pressed action states keep the footer on its original row",
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
        && Math.abs(Number(plane.dataset.committedWidth)
          - plane.querySelector(":scope > .lane-mosaic-track").clientWidth) <= 1
      ));
    }, priorWidths);
}

async function captureReferenceView(page, console, artifacts, viewport, mode, agentLabels) {
  await waitForCommittedResize(page, console, viewport);
  await assertMessageCardLayout(console, viewport.label);
  await assertNaturalMessageFooterInsets(console, `${mode} ${viewport.label}`);
  await assertReferencePalette(console, mode);
  const cards = console.locator(
    '[data-card-kind="agent-message"][data-layout-phase="committed"]',
  );
  const fits = await cards.evaluateAll((articles) => articles.map((article) => {
    const bounds = (node) => {
      const box = node.getBoundingClientRect();
      return {
        bottom: box.bottom,
        height: box.height,
        left: box.left,
        right: box.right,
        top: box.top,
        width: box.width,
      };
    };
    const baseline = (node) => {
      const sheet = [...document.styleSheets]
        .find((candidate) => candidate.href?.endsWith("mosaic.css"));
      const index = sheet.cssRules.length;
      sheet.insertRule([
        ".message-footer-baseline-probe {",
        "display: inline-block; width: 0; height: 0; padding: 0; margin: 0; border: 0;",
        "}",
      ].join(" "), index);
      const probe = document.createElement("span");
      probe.className = "message-footer-baseline-probe";
      node.append(probe);
      const position = probe.getBoundingClientRect().top;
      probe.remove();
      sheet.deleteRule(index);
      return position;
    };
    const footer = article.querySelector(".message-footer");
    const footerLeft = article.querySelector(".message-footer-left");
    const footerRight = article.querySelector(".message-footer-right");
    const agent = article.querySelector(".message-agent-name");
    const agentLabel = agent.querySelector(".message-agent-label");
    const age = article.querySelector(".message-age");
    const actions = article.querySelector(".message-actions");
    return {
      key: article.dataset.mosaicCardKey,
      width: article.getBoundingClientRect().width,
      cardClientWidth: article.clientWidth,
      cardScrollWidth: article.scrollWidth,
      footer: {
        ...bounds(footer),
        clientWidth: footer.clientWidth,
        scrollWidth: footer.scrollWidth,
      },
      footerLeft: bounds(footerLeft),
      footerRight: {
        ...bounds(footerRight),
        flexWrap: getComputedStyle(footerRight).flexWrap,
      },
      agent: {
        ...bounds(agent),
        ariaLabel: agent.getAttribute("aria-label"),
        clientHeight: agent.clientHeight,
        clientWidth: agent.clientWidth,
        overflow: getComputedStyle(agent).overflow,
        labelClientWidth: agentLabel.clientWidth,
        labelClientHeight: agentLabel.clientHeight,
        labelOverflow: getComputedStyle(agentLabel).overflow,
        labelOverflowWrap: getComputedStyle(agentLabel).overflowWrap,
        labelRight: agentLabel.getBoundingClientRect().right,
        labelScrollHeight: agentLabel.scrollHeight,
        labelScrollWidth: agentLabel.scrollWidth,
        labelTextOverflow: getComputedStyle(agentLabel).textOverflow,
        labelWhiteSpace: getComputedStyle(agentLabel).whiteSpace,
        scrollHeight: agent.scrollHeight,
        scrollWidth: agent.scrollWidth,
        text: agent.textContent.trim(),
        baseline: baseline(agent.querySelector(".message-agent-label")),
        textOverflow: getComputedStyle(agent).textOverflow,
        whiteSpace: getComputedStyle(agent).whiteSpace,
      },
      age: { ...bounds(age), baseline: baseline(age) },
      separatorCount: article.querySelectorAll(".message-footer-separator").length,
      actionGroup: actions ? bounds(actions) : null,
      actions: [...article.querySelectorAll(".message-actions button")].map((button) => {
        const icon = button.querySelector("[data-card-action-icon]");
        return {
          ariaLabel: button.getAttribute("aria-label"),
          icon: icon ? {
            kind: icon.dataset.cardActionIcon,
            ...bounds(icon),
          } : null,
          ...bounds(button),
        };
      }),
    };
  }));
  assert.ok(
    fits.every((fit) => fit.cardScrollWidth <= fit.cardClientWidth + 1),
    `${viewport.label} cards have no horizontal overflow: ${JSON.stringify(fits)}`,
  );
  assert.ok(
    fits.every((fit) => fit.footer.scrollWidth <= fit.footer.clientWidth + 1),
    `${viewport.label} footers have no horizontal overflow: ${JSON.stringify(fits)}`,
  );
  assert.ok(
    fits.every((fit) => (
      agentLabels.includes(fit.agent.text)
        && fit.agent.ariaLabel.endsWith(` · ${fit.agent.text}`)
        && fit.agent.overflow === "visible"
        && fit.agent.labelOverflow === "visible"
        && fit.agent.labelOverflowWrap === "anywhere"
        && fit.agent.labelTextOverflow === "clip"
        && fit.agent.labelWhiteSpace === "normal"
        && fit.agent.textOverflow === "clip"
        && fit.agent.whiteSpace === "normal"
        && fit.agent.scrollWidth <= fit.agent.clientWidth + 1
        && fit.agent.scrollHeight <= fit.agent.clientHeight + 1
        && fit.agent.labelScrollWidth <= fit.agent.labelClientWidth + 1
        && fit.agent.labelScrollHeight <= fit.agent.labelClientHeight + 1
    )),
    `${viewport.label} renders every exact agent identity without clipping: ${JSON.stringify(fits)}`,
  );
  assert.ok(
    agentLabels.every((label) => fits.some((fit) => fit.agent.text === label)),
    `${viewport.label} exercises each configured name and worktree fallback`,
  );
  assert.equal(
    new Set(fits.map((fit) => fit.age.width)).size,
    1,
    `${viewport.label} message ages share one fixed-width slot`,
  );
  assert.ok(
    fits.every((fit) => fit.age.width > 0 && fit.age.height > 0),
    `${viewport.label} message ages remain visible`,
  );
  assert.ok(
    fits.every((fit) => (
      fit.footer.height >= 32
        && fit.footerLeft.top >= fit.footer.top
        && fit.footerLeft.bottom <= fit.footer.bottom
        && fit.footerRight.bottom === fit.footer.bottom
        && fit.footerRight.height >= 32
        && fit.footerRight.flexWrap === "nowrap"
        && fit.agent.top >= fit.footer.top
        && fit.agent.bottom <= fit.footer.bottom
        && fit.age.right <= fit.footerRight.left
    )),
    `${viewport.label} footer clusters remain bounded as identities wrap: ${JSON.stringify(fits)}`,
  );
  assert.ok(
    fits.every((fit) => (
      fit.actionGroup === null || (
        fit.actionGroup.top >= fit.footer.top
        && fit.actionGroup.bottom <= fit.footer.bottom
        && fit.agent.right <= fit.actionGroup.left
        && Math.abs(fit.actionGroup.left - fit.agent.right - 6) <= 1
        && Math.abs(fit.actionGroup.left - fit.agent.labelRight - 6) <= 1
      )
    )),
    `${viewport.label} footer actions stay coherent and separated: ${JSON.stringify(fits)}`,
  );
  const baselineFailures = fits.filter((fit) => (
    (fit.agent.height <= 16
      && Math.abs(fit.age.baseline - fit.agent.baseline) > BASELINE_TOLERANCE)
  )).map((fit) => ({
    actions: fit.actions.length,
    age: fit.age.baseline,
    agent: fit.agent.baseline,
    key: fit.key,
  }));
  assert.deepEqual(
    baselineFailures,
    [],
    `${viewport.label} footer text shares one measured baseline`,
  );
  const actionLabels = ["Speak this message", "Quote this message", "Copy this message"];
  assert.ok(
    fits.every((fit) => (
      fit.actions.length === 0
        || (fit.actions.length === actionLabels.length
          && fit.actions.every((action, index) => (
            action.ariaLabel === actionLabels[index]
              && action.width === 32
              && action.height === 32
              && (index !== 0 || (
                action.icon?.kind === "play"
                  && action.icon.width === 15
                  && action.icon.height === 15
              ))
          )))
    )),
    `${viewport.label} keeps accessible 32px actions in keyboard order: ${JSON.stringify(fits)}`,
  );
  assert.ok(
    fits.every((fit) => fit.separatorCount === 0),
    `${viewport.label} renders no message-footer separator nodes: ${JSON.stringify(fits)}`,
  );
  assert.ok(
    fits.some((fit) => fit.actions.length === 0),
    `${viewport.label} includes a real no-action footer: ${JSON.stringify(fits)}`,
  );
  assert.ok(
    fits.some((fit) => fit.actions.length === actionLabels.length),
    `${viewport.label} retains the ordinary three-action footer variant`,
  );
  if (viewport.label === "mobile") {
    assert.ok(
      fits.some((fit) => fit.agent.labelClientHeight > 16),
      `mobile proves long identities use the bounded multi-row footer: ${JSON.stringify(fits)}`,
    );
  }
  const screenshotTarget = fits.reduce((selected, candidate) => (
    candidate.agent.text.length > selected.agent.text.length ? candidate : selected
  ));
  const screenshotCard = console.locator(`[data-mosaic-card-key="${screenshotTarget.key}"]`);
  assert.equal(
    await screenshotCard.evaluate((article) => getComputedStyle(article).backgroundColor),
    referencePalettes[mode].panel,
    `${viewport.label} capture explicitly exercises ${mode} mode`,
  );
  await screenshotCard.screenshot({
    path: join(artifacts, `canvas-message-card-${viewport.label}-${mode}.png`),
  });
  const noActionTarget = fits.find((fit) => fit.actions.length === 0);
  await console.locator(`[data-mosaic-card-key="${noActionTarget.key}"]`).screenshot({
    path: join(artifacts, `canvas-message-card-no-actions-${viewport.label}-${mode}.png`),
  });
}

async function retainNoActionCard(page, console, activityReplay, lane) {
  const message = await activityReplay.injectAgentProse(page, lane, "", [{
    name: "visual-only.png",
    content_type: "image/png",
    data_url: `data:image/png;base64,${PIXEL_PNG}`,
  }]);
  const card = console.locator(`[data-mosaic-card-key="${message.key}"]`);
  await card.waitFor({ state: "attached", timeout: 10_000 });
  await waitForPage(page, "attachment-only message commits", TRANSITION_MS, (key) => {
    const candidate = document.querySelector(`[data-mosaic-card-key="${key}"]`);
    return candidate?.dataset.layoutPhase === "committed"
      && candidate.querySelector("img")?.dataset.mosaicImageState === "loaded";
  }, message.key);
  return card;
}

async function retainFinalCard(page, console, activityReplay, lane) {
  const text = "> [ACK] Acknowledged directive\n\nFinal message color reference";
  const message = await activityReplay.injectClassifiedAgentProse(page, lane, text, {
    acknowledgements: 1,
    tastes: 0,
  });
  const card = console.locator(`[data-mosaic-card-key="${message.key}"]`);
  await card.waitFor({ state: "attached", timeout: 10_000 });
  await waitForPage(page, "ordinary precursor commits", TRANSITION_MS, (key) => {
    const candidate = document.querySelector(`[data-mosaic-card-key="${key}"]`);
    return candidate?.dataset.layoutPhase === "committed"
      && candidate.dataset.messageFinal === "false"
      && !candidate.classList.contains("final")
      && candidate.querySelector(".message-badge--final") === null;
  }, message.key);
  await activityReplay.reviseAgentFinal(page, message.key);
  await waitForPage(page, "final message commits", TRANSITION_MS, (key) => {
    const candidate = document.querySelector(`[data-mosaic-card-key="${key}"]`);
    return candidate?.dataset.layoutPhase === "committed"
      && candidate.classList.contains("final")
      && [...candidate.querySelectorAll(".message-badge")]
        .map((badge) => badge.textContent).join(",") === "FINAL"
      && candidate.querySelector("blockquote > p > .message-ack-pill")?.textContent === "ACK";
  }, message.key);
  return card;
}

async function messageStateColors(card) {
  return card.evaluate((article) => {
    const canvas = document.createElement("canvas");
    canvas.width = 1;
    canvas.height = 1;
    const context = canvas.getContext("2d", { willReadFrequently: true });
    const rgba = (expression) => {
      context.clearRect(0, 0, 1, 1);
      context.fillStyle = expression;
      context.fillRect(0, 0, 1, 1);
      return [...context.getImageData(0, 0, 1, 1).data];
    };
    const probe = document.createElement("span");
    article.append(probe);
    const color = (expression) => {
      probe.style.color = expression;
      const resolved = getComputedStyle(probe).color;
      probe.style.color = "";
      return resolved;
    };
    const background = (expression) => {
      probe.style.background = expression;
      const resolved = getComputedStyle(probe).backgroundColor;
      probe.style.background = "";
      return resolved;
    };
    const style = getComputedStyle(article);
    const footer = article.querySelector(".message-footer");
    const footerBounds = footer.getBoundingClientRect();
    const footerRight = footer.querySelector(".message-footer-right");
    const footerRightBounds = footerRight.getBoundingClientRect();
    const finalBadge = article.querySelector(".message-badge--final");
    const badgeBounds = finalBadge?.getBoundingClientRect();
    const badgeGeometry = [...article.querySelectorAll(".message-badge, .message-ack-pill")].map((badge) => {
      const bounds = badge.getBoundingClientRect();
      const badgeStyle = getComputedStyle(badge);
      return {
        text: badge.textContent,
        display: badgeStyle.display,
        clientWidth: badge.clientWidth,
        scrollWidth: badge.scrollWidth,
        height: bounds.height,
        minHeight: badgeStyle.minHeight,
        padding: badgeStyle.padding,
        borderWidth: badgeStyle.borderWidth,
        borderRadius: badgeStyle.borderRadius,
        fontSize: badgeStyle.fontSize,
        fontFamily: badgeStyle.fontFamily,
        fontWeight: badgeStyle.fontWeight,
        left: bounds.left,
        right: bounds.right,
        top: bounds.top,
        bottom: bounds.bottom,
        counts: badge.querySelectorAll(".message-badge-count").length,
      };
    });
    const ackPill = article.querySelector(".message-ack-pill");
    const ackQuote = ackPill?.closest("blockquote");
    const result = {
      key: article.dataset.mosaicCardKey,
      classes: [...article.classList],
      background: style.backgroundColor,
      backgroundRgba: rgba(style.backgroundColor),
      borderLeft: style.borderLeftColor,
      borderRight: style.borderRightColor,
      borderTop: style.borderTopColor,
      boxShadow: style.boxShadow,
      panel: background("var(--panel)"),
      finalAccent: color("var(--accent-1)"),
      finalTint: background("color-mix(in srgb, var(--accent-1) 8%, var(--panel))"),
      producerAccent: color("var(--accent-slot, var(--accent))"),
      speechBorder: color("color-mix(in srgb, var(--warning) 70%, var(--line))"),
      speechAccent: color("var(--warning)"),
      footerGeometry: {
        height: footerBounds.height,
        clientWidth: footer.clientWidth,
        scrollWidth: footer.scrollWidth,
        rightStart: footerRightBounds.left,
      },
      badgeGeometry,
      finalBadge: finalBadge && {
        text: finalBadge.textContent,
        ariaLabel: finalBadge.getAttribute("aria-label"),
        role: finalBadge.getAttribute("role"),
        background: getComputedStyle(finalBadge).backgroundColor,
        color: getComputedStyle(finalBadge).color,
        count: finalBadge.querySelectorAll(".message-badge-count").length,
        left: badgeBounds.left,
        right: badgeBounds.right,
        top: badgeBounds.top,
        bottom: badgeBounds.bottom,
        footerLeft: footerBounds.left,
        footerRight: footerBounds.right,
        footerTop: footerBounds.top,
        footerBottom: footerBounds.bottom,
        footerRightLeft: footerRightBounds.left,
      },
      badgeTexts: [...article.querySelectorAll(".message-badge, .message-ack-pill")]
        .map((badge) => badge.textContent),
      badgeBorderWidths: [...article.querySelectorAll(".message-badge, .message-ack-pill")]
        .map((badge) => getComputedStyle(badge).borderWidth),
      badgeCounts: article.querySelectorAll(".message-badge-count").length,
      detachedClassifiers: article.querySelectorAll(".message-classifiers").length,
      ackQuote: ackQuote && {
        headsQuote: ackQuote.firstElementChild?.matches("p:has(> .message-ack-pill:first-child)"),
        borderLeftColor: getComputedStyle(ackQuote).borderLeftColor,
        accent: color("var(--accent)"),
        pillColor: getComputedStyle(ackPill).color,
        bodyText: article.querySelector(".message-body")?.innerText ?? "",
      },
    };
    probe.remove();
    return result;
  });
}

function rgbDistance(left, right) {
  return Math.hypot(...left.slice(0, 3).map((channel, index) => channel - right[index]));
}

async function captureFinalMessageStates(
  page,
  console,
  ordinaryCard,
  card,
  artifacts,
  mode,
  viewport,
) {
  await waitForCommittedResize(page, console, viewport);
  const ordinary = await messageStateColors(ordinaryCard);
  const final = await messageStateColors(card);
  const cardOrder = await console.locator(".mosaic-message-card").evaluateAll((cards) => (
    cards.map((candidate) => candidate.dataset.mosaicCardKey)
  ));
  const backgroundDelta = rgbDistance(ordinary.backgroundRgba, final.backgroundRgba);
  assert.equal(
    cardOrder.indexOf(final.key),
    cardOrder.indexOf(ordinary.key) + 1,
    `${mode} ${viewport.label} compares adjacent ordinary and final cards`,
  );
  assert.ok(!ordinary.classes.includes("final"), `${mode} ${viewport.label} comparator is ordinary`);
  assert.equal(ordinary.finalBadge, null, `${mode} ${viewport.label} ordinary card has no FINAL badge`);
  assert.equal(ordinary.background, ordinary.panel, `${mode} ordinary preserves the panel background`);
  assert.equal(
    ordinary.borderRight,
    ordinary.producerAccent,
    `${mode} ordinary preserves producer identity`,
  );
  assert.ok(final.classes.includes("final"), `${mode} ${viewport.label} retains final state`);
  assert.deepEqual(
    {
      text: final.finalBadge?.text,
      ariaLabel: final.finalBadge?.ariaLabel,
      role: final.finalBadge?.role,
      background: final.finalBadge?.background,
      color: final.finalBadge?.color,
      count: final.finalBadge?.count,
    },
    {
      text: "FINAL",
      ariaLabel: "FINAL message classification",
      role: "listitem",
      background: final.finalAccent,
      color: final.panel,
      count: 0,
    },
    `${mode} ${viewport.label} renders one accessible uncounted blue FINAL badge`,
  );
  assert.deepEqual(
    final.badgeTexts,
    ["ACK", "FINAL"],
    `${mode} ${viewport.label} keeps inline ACK distinct from footer FINAL`,
  );
  assert.deepEqual(
    final.badgeBorderWidths,
    ["0px", "0px"],
    `${mode} ${viewport.label} keeps FINAL and ACK borderless inside the card`,
  );
  assert.deepEqual(
    { counts: final.badgeCounts, detached: final.detachedClassifiers },
    { counts: 0, detached: 0 },
    `${mode} ${viewport.label} renders the ACK pill uncounted with no detached header`,
  );
  assert.equal(final.badgeGeometry.length, 2, `${mode} ${viewport.label} measures FINAL and ACK`);
  for (const badge of final.badgeGeometry) {
    assert.deepEqual(
      {
        minHeight: badge.minHeight,
        padding: badge.padding,
        borderWidth: badge.borderWidth,
        borderRadius: badge.borderRadius,
        fontFamily: badge.fontFamily,
        fontWeight: badge.fontWeight,
        counts: badge.counts,
      },
      {
        minHeight: "17px",
        padding: "0px 4px",
        borderWidth: "0px",
        borderRadius: "999px",
        fontFamily: "ui-monospace, SFMono-Regular, Menlo, monospace",
        fontWeight: "700",
        counts: 0,
      },
      `${mode} ${viewport.label} gives ${badge.text} the shared inline-badge geometry`,
    );
  }
  const [ackGeometry, finalGeometry] = final.badgeGeometry;
  assert.equal(ackGeometry.text, "ACK", `${mode} ${viewport.label} measures inline ACK first`);
  assert.equal(finalGeometry.text, "FINAL", `${mode} ${viewport.label} measures footer FINAL second`);
  assert.deepEqual(
    [ackGeometry.display, finalGeometry.display],
    ["inline-flex", "flex"],
    `${mode} ${viewport.label} flows ACK inline with quoted prose and FINAL as a footer item`,
  );
  assert.deepEqual(
    {
      headsQuote: final.ackQuote?.headsQuote,
      borderLeftColor: final.ackQuote?.borderLeftColor,
      pillColor: final.ackQuote?.pillColor,
    },
    {
      headsQuote: true,
      borderLeftColor: final.ackQuote?.accent,
      pillColor: final.ackQuote?.accent,
    },
    `${mode} ${viewport.label} nests the ACK pill at the head of its quote on the accent rail`,
  );
  assert.doesNotMatch(
    final.ackQuote?.bodyText ?? "",
    /\[ACK\]/u,
    `${mode} ${viewport.label} keeps the authenticated ACK marker presentation-only`,
  );
  assert.ok(
    final.badgeGeometry.every((badge) => badge.scrollWidth <= badge.clientWidth + 1),
    `${mode} ${viewport.label} keeps inline and footer classifier content unclipped`,
  );
  assert.ok(
    finalGeometry.right <= final.footerGeometry.rightStart,
    `${mode} ${viewport.label} keeps footer FINAL clear of adjacent metadata`,
  );
  assert.equal(
    final.footerGeometry.height,
    ordinary.footerGeometry.height,
    `${mode} ${viewport.label} badges do not expand the message footer`,
  );
  assert.ok(
    final.footerGeometry.scrollWidth <= final.footerGeometry.clientWidth + 1,
    `${mode} ${viewport.label} footer keeps its responsive contents contained`,
  );
  assert.ok(
    final.finalBadge.left >= final.finalBadge.footerLeft
      && final.finalBadge.right <= final.finalBadge.footerRight
      && final.finalBadge.top >= final.finalBadge.footerTop
      && final.finalBadge.bottom <= final.finalBadge.footerBottom
      && final.finalBadge.right <= final.finalBadge.footerRightLeft,
    `${mode} ${viewport.label} contains the FINAL badge without obscuring attribution or actions`,
  );
  assert.ok(!final.classes.includes("now-playing"), `${mode} ${viewport.label} starts silent`);
  assert.ok(
    backgroundDelta > 0,
    `${mode} ${viewport.label} final background differs from adjacent ordinary background: `
      + `${ordinary.backgroundRgba} versus ${final.backgroundRgba}`,
  );
  assert.equal(final.background, final.finalTint, `${mode} final uses the reference tint role`);
  assert.equal(final.borderTop, final.finalAccent, `${mode} final uses the cool top rule`);
  assert.equal(final.borderRight, final.producerAccent, `${mode} final preserves producer identity`);
  assert.ok(final.boxShadow.includes(final.finalAccent), `${mode} final paints its top inset rule`);
  await card.screenshot({
    path: join(artifacts, `canvas-message-final-${viewport.label}-${mode}.png`),
  });

  await card.getByLabel("Speak this message").click();
  await waitForPage(page, "message speech becomes current", TRANSITION_MS, (key) => (
    document.querySelector(`[data-mosaic-card-key="${key}"]`)?.classList.contains("now-playing")
  ), await card.getAttribute("data-mosaic-card-key"));
  const speaking = await messageStateColors(card);
  assert.ok(speaking.classes.includes("now-playing"), `${mode} message exposes active speech`);
  assert.equal(speaking.background, speaking.finalTint, `${mode} speech preserves final tint`);
  assert.equal(speaking.borderLeft, speaking.speechBorder, `${mode} speech uses the warm border`);
  assert.equal(speaking.borderRight, speaking.producerAccent, `${mode} speech preserves producer identity`);
  assert.ok(speaking.boxShadow.includes(speaking.speechAccent), `${mode} speech paints its left inset rule`);
  assert.ok(speaking.boxShadow.includes(speaking.finalAccent), `${mode} speech preserves the final top rule`);
  assert.equal(speaking.finalBadge?.background, speaking.finalAccent, `${mode} speech preserves the FINAL badge`);
  await card.screenshot({
    path: join(artifacts, `canvas-message-speaking-${viewport.label}-${mode}.png`),
  });
  await page.evaluate(() => window.__speechUtterances.at(-1)?.onend?.());
  await waitForPage(page, "message speech state clears", TRANSITION_MS, (key) => (
    !document.querySelector(`[data-mosaic-card-key="${key}"]`)?.classList.contains("now-playing")
  ), await card.getAttribute("data-mosaic-card-key"));
}

export async function assertReferenceMessageCard(
  page,
  console,
  card,
  artifacts,
  agentLabels,
  activityReplay,
  lane,
) {
  const ordinaryCard = await retainNoActionCard(page, console, activityReplay, lane);
  const finalCard = await retainFinalCard(page, console, activityReplay, lane);
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
    panelProbe.style.borderColor = "var(--control-line-soft)";
    const controlLineSoft = getComputedStyle(panelProbe).borderLeftColor;
    panelProbe.remove();
    return {
      card: measured(article),
      panel,
      controlLineSoft,
      grid: measured(grid),
      footer: measured(footer),
      footerChildren: [...footer.children].map((node) => node.className),
      footerText: footer.innerText,
      classifiers: footer.querySelectorAll(".message-key, .message-classifiers").length,
      agent: {
        ...measured(agent),
        ariaLabel: agent.getAttribute("aria-label"),
        clientHeight: agent.clientHeight,
        clientWidth: agent.clientWidth,
        overflow: getComputedStyle(agent).overflow,
        overflowWrap: getComputedStyle(agent).overflowWrap,
        scrollHeight: agent.scrollHeight,
        scrollWidth: agent.scrollWidth,
        text: agent.innerText,
        textOverflow: getComputedStyle(agent).textOverflow,
        title: agent.title,
        whiteSpace: getComputedStyle(agent).whiteSpace,
      },
      actions: {
        ...measured(actions),
        buttons: [...actions.querySelectorAll("button")].map(measured),
        speechIcon: measured(actions.querySelector("[data-card-action-icon]")),
      },
      inlineCode: measured(inlineCode),
      inlineCodeText: inlineCode?.textContent ?? "",
    };
  });
  assertReferenceGeometry(geometry, agentLabels);
  await assertActionInteraction(page, card);

  const original = page.viewportSize();
  const rootFontSize = await page.evaluate(() => document.documentElement.style.fontSize);
  try {
    for (const mode of ["dark", "light"]) {
      await page.emulateMedia({ colorScheme: mode });
      await captureReferenceView(page, console, artifacts, {
        label: "reference", width: 648, height: 726,
      }, mode, agentLabels);
      await captureReferenceView(page, console, artifacts, {
        label: "mobile", width: 390, height: 720,
      }, mode, agentLabels);
      await captureFinalMessageStates(page, console, ordinaryCard, finalCard, artifacts, mode, {
        label: "reference", width: 648, height: 726,
      });
      await captureFinalMessageStates(page, console, ordinaryCard, finalCard, artifacts, mode, {
        label: "mobile", width: 390, height: 720,
      });
    }
    await page.emulateMedia({ colorScheme: "dark" });
    await page.evaluate(() => { document.documentElement.style.fontSize = "20px"; });
    await captureFinalMessageStates(page, console, ordinaryCard, finalCard, artifacts, "dark", {
      label: "zoomed", width: 648, height: 726,
    });
    await finalCard.screenshot({ path: join(artifacts, "canvas-message-final-zoomed-dark.png") });
  } finally {
    await page.evaluate((fontSize) => {
      document.documentElement.style.fontSize = fontSize;
    }, rootFontSize);
    await page.emulateMedia({ colorScheme: null });
  }
  await waitForCommittedResize(page, console, { label: "restored reference", ...original });
  await assertMessageCardLayout(console, "restored reference");
}
