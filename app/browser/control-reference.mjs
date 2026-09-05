import assert from "node:assert/strict";

import { reply, TRANSITION_MS, waitForPageFunction } from "./canvas-fixture.mjs";
import { assertTeamActions } from "./team-actions.mjs";

const darkPalette = {
  quiet: "rgb(142, 162, 255)",
  drain: "rgb(230, 180, 78)",
  green: "rgb(116, 198, 157)",
  // One ink prints every submit state, the way the predecessor's `.primary`
  // reads a single `--button-accent-fg` and lets only the fill follow the
  // state accent. A maxed-only ink would be a second place to keep in step.
  pageInk: "rgb(16, 20, 17)",
};

const TASK_CHIP_FIXTURE = '[data-task-chip-fixture="matrix"]';
const GENERATED_MAX_TASK_HANDLE = "CONTROL-1kKbC3j4";
const LONG_ADMITTED_TASK_HANDLE = "EXTRAORDINARILYLONGTASKSTEM-1kKbC3j4";

async function selectLifetime(page, lineage, input, value, label) {
  await input.fill(value);
  await waitForPageFunction(
    page,
    `reference lifetime reaches ${label}`,
    TRANSITION_MS,
    ({ lineageId, expected }) => {
      const current = document.querySelector(
        `.team-surface[data-lineage-id="${lineageId}"] .team-composer-controls`,
      );
      return current?.dataset.lifetimePending === "false"
        && current.dataset.lifetime === expected;
    },
    { lineageId: lineage, expected: label },
  );
}

function measureReference(surface) {
  const measured = (node, selector) => {
    if (!node) throw new Error(`reference control is missing ${selector}`);
    const box = node.getBoundingClientRect();
    const style = getComputedStyle(node);
    return {
      left: box.left,
      right: box.right,
      top: box.top,
      bottom: box.bottom,
      width: box.width,
      height: box.height,
      background: style.backgroundColor,
      border: style.borderColor,
      borderStyle: style.borderStyle,
      borderWidth: style.borderWidth,
      borderRadius: style.borderRadius,
      boxShadow: style.boxShadow,
      color: style.color,
      display: style.display,
      fontSize: style.fontSize,
      gap: style.gap,
      paddingLeft: style.paddingLeft,
      paddingTop: style.paddingTop,
      clientWidth: node.clientWidth,
      scrollWidth: node.scrollWidth,
    };
  };
  const rect = (selector, root = surface) => measured(root.querySelector(selector), selector);
  const taskStrip = rect(".task-pill-strip", document);
  const toolbar = rect(".team-toolbar");
  const rail = rect(".team-mode-rail");
  const composer = rect(".team-composer");
  const controls = rect(".team-composer-controls");
  const activity = rect(".team-activity-grid");
  // The armed and maxed sliders carry the state accent blended 70% into the
  // resting line, the way the predecessor mixes it, so the hue role has to be
  // read back through the same mix rather than through the raw token.
  const probe = document.createElement("span");
  surface.append(probe);
  const mix = (token) => {
    probe.style.color = `color-mix(in srgb, var(${token}) 70%, var(--line))`;
    const resolved = getComputedStyle(probe).color;
    probe.style.color = "";
    return resolved;
  };
  probe.style.color = "var(--accent-1)";
  const armedAccent = getComputedStyle(probe).color;
  probe.style.color = "";
  const stateBorder = { quiet: mix("--accent-1"), drain: mix("--warning") };
  probe.remove();
  return {
    armedAccent,
    stateBorder,
    surface: {
      width: surface.getBoundingClientRect().width,
      clientWidth: surface.clientWidth,
      scrollWidth: surface.scrollWidth,
    },
    versionDisplays: document.querySelectorAll(".version-pill, [data-package-version]").length,
    taskStrip,
    taskPills: [...document.querySelectorAll(".task-pill")].map((pill) => {
      const count = pill.querySelector(".task-pill-count");
      return {
        ...measured(pill, ".task-pill"),
        tag: pill.tagName,
        stem: pill.dataset.taskStem,
        ready: pill.dataset.ready,
        active: pill.dataset.active,
        unavailable: pill.dataset.unavailable,
        hidden: pill.classList.contains("system"),
        countText: count?.textContent,
        count: measured(count, ".task-pill-count"),
      };
    }),
    toolbar,
    rail,
    composer,
    controls,
    activity,
    selected: rect(".team-mode.selected"),
    menu: rect(".team-menu"),
    speech: rect(".team-speech"),
    lifetime: rect(".team-lifetime"),
    send: rect(".team-send"),
    badges: [...surface.querySelectorAll(".team-mode-badge")].map((badge) => ({
      text: badge.textContent,
      background: getComputedStyle(badge).backgroundColor,
      width: badge.getBoundingClientRect().width,
      fontSize: getComputedStyle(badge).fontSize,
      paddingLeft: getComputedStyle(badge).paddingLeft,
      paddingTop: getComputedStyle(badge).paddingTop,
      boxShadow: getComputedStyle(badge).boxShadow,
    })),
    lights: [...surface.querySelectorAll(".team-light")].map((light) => ({
      width: light.getBoundingClientRect().width,
      height: light.getBoundingClientRect().height,
    })),
    agents: [...surface.querySelectorAll(".team-agent")].map((agent) => {
      const box = agent.getBoundingClientRect();
      return {
        left: box.left,
        right: box.right,
        top: box.top,
        bottom: box.bottom,
        width: box.width,
      };
    }),
    labels: [...surface.querySelectorAll(".team-slider span")].map((label) => ({
      text: label.textContent,
      clientWidth: label.clientWidth,
      scrollWidth: label.scrollWidth,
      textAlign: getComputedStyle(label).textAlign,
      textTransform: getComputedStyle(label).textTransform,
    })),
    modes: [...surface.querySelectorAll(".team-mode")].map((mode) => ({
      label: mode.firstElementChild?.textContent,
      selected: mode.classList.contains("selected"),
      badge: mode.querySelector(".team-mode-badge")?.textContent ?? null,
      background: getComputedStyle(mode).backgroundColor,
      border: getComputedStyle(mode).borderColor,
      borderWidth: getComputedStyle(mode).borderWidth,
      boxShadow: getComputedStyle(mode).boxShadow,
      color: getComputedStyle(mode).color,
      fontSize: getComputedStyle(mode).fontSize,
      gap: getComputedStyle(mode).gap,
      paddingLeft: getComputedStyle(mode).paddingLeft,
      paddingTop: getComputedStyle(mode).paddingTop,
      clientWidth: mode.clientWidth,
      scrollWidth: mode.scrollWidth,
    })),
  };
}

function assertReferenceGeometry(geometry) {
  assert.equal(geometry.versionDisplays, 0, "the task strip never becomes a version display");
  assert.ok(geometry.taskPills.length > 0, "the live board renders task-stem pills");
  const taskPillOrder = geometry.taskPills.map(({ hidden, stem }) => ({ hidden, stem }));
  assert.equal(
    new Set(taskPillOrder.map(({ hidden, stem }) => `${hidden}:${stem}`)).size,
    geometry.taskPills.length,
    "each live board visibility/stem pair renders exactly one pill",
  );
  assert.deepEqual(
    taskPillOrder,
    [...taskPillOrder].sort((left, right) => {
      if (left.hidden !== right.hidden) return Number(left.hidden) - Number(right.hidden);
      if (left.stem < right.stem) return -1;
      if (left.stem > right.stem) return 1;
      return 0;
    }),
    "public task stems precede system stems and each class is ascending",
  );
  assert.equal(geometry.taskStrip.gap, "6px", "task-pill-strip gap matches the canonical reference");
  for (const pill of geometry.taskPills) {
    const expected = pill.hidden
      ? String(Number(pill.ready) + Number(pill.active) + Number(pill.unavailable))
      : `${pill.ready}·${pill.active}·${pill.unavailable}`;
    assert.equal(pill.countText, expected, `${pill.stem} carries board-state counts, not a version`);
    assert.equal(
      pill.tag,
      "BUTTON",
      `${pill.stem} deliberately remains a Rust navigation control while matching the reference treatment`,
    );
    assert.equal(pill.fontSize, "12px", `${pill.stem} uses the reference pill type size`);
    assert.equal(pill.gap, "4px", `${pill.stem} uses the reference label/count gap`);
    assert.equal(pill.paddingLeft, "10px", `${pill.stem} uses reference inline padding`);
    assert.equal(pill.paddingTop, "2px", `${pill.stem} uses reference block padding`);
    assert.equal(pill.borderWidth, "1px", `${pill.stem} keeps the one-pixel pill outline`);
    assert.ok(
      Number.parseFloat(pill.borderRadius) >= pill.height / 2 - 1,
      `${pill.stem} remains capsule-shaped`,
    );
    assert.equal(pill.count.fontSize, "9px", `${pill.stem} count uses reference badge type`);
    assert.equal(pill.count.paddingLeft, "5px", `${pill.stem} count uses reference inline padding`);
    assert.equal(pill.count.paddingTop, "0px", `${pill.stem} count uses reference block padding`);
    assert.notEqual(pill.count.boxShadow, "none", `${pill.stem} count keeps its inset edge`);
  }
  assert.ok(
    geometry.rail.width / geometry.toolbar.width >= 0.82,
    "the four-mode rail remains the dominant toolbar element",
  );
  assert.equal(
    geometry.toolbar.display,
    "grid",
    "Rust deliberately uses a grid so the four reference modes retain equal widths",
  );
  assert.ok(
    geometry.toolbar.height >= 44 && geometry.toolbar.height <= 47,
    "the desktop toolbar keeps the compact reference height",
  );
  assert.ok(
    geometry.rail.height >= 34 && geometry.rail.height <= 37,
    "the rounded rail keeps the compact reference track",
  );
  assert.notEqual(geometry.selected.boxShadow, "none", "the selected mode has an inset treatment");
  const selected = geometry.modes.filter((mode) => mode.selected);
  const unselected = geometry.modes.filter((mode) => !mode.selected);
  assert.equal(selected.length, 1, "one mode alone is selected");
  assert.ok(
    unselected.every(
      (mode) => mode.boxShadow === "none" && mode.background !== selected[0].background,
    ),
    "the outlined selected pill is measurably distinct from every bare neighbour",
  );
  assert.ok(geometry.badges.length > 0, "the live rail exposes green count badges");
  assert.ok(
    geometry.modes.some((mode) => mode.badge === null),
    "a mode without a count renders no badge",
  );
  assert.ok(
    geometry.modes.filter((mode) => mode.badge !== null).every((mode) => Number(mode.badge) > 0),
    "a badge renders only for a positive count without duplicating any named mode's count",
  );
  assert.ok(
    geometry.badges.every(
      (badge) => badge.background === darkPalette.green
        && badge.width >= 19
        && badge.fontSize === "9px"
        && badge.paddingLeft === "5px"
        && badge.paddingTop === "0px"
        && badge.boxShadow !== "none",
    ),
    "the rail badges retain the dark-palette green treatment",
  );
  assert.equal(geometry.lights.length, 4, "the reference toolbar shows four agent lights");
  assert.ok(
    geometry.lights.every((light) => light.width === 9 && light.height === 9),
    "the four status lights stay legible",
  );
  assert.ok(
    Math.abs(geometry.menu.width - geometry.menu.height) <= 1 && geometry.menu.width === 30,
    "the details control matches the compact round reference button",
  );
  assert.ok(
    Number.parseFloat(geometry.menu.borderRadius) >= geometry.menu.width / 2 - 1,
    "the details control remains circular",
  );

  // The reference fixes the stack absolutely, not as a share of the composer:
  // three identical 112x36 pills, measured off the capture at its 2x ratio and
  // matched by the predecessor's own declarations (a 96px track inside 4px 7px
  // padding and a 1px border, under one 36px item-height token). Sizes derived
  // from the composer instead would move whenever the shard grid does.
  const controlShare = geometry.controls.width / geometry.composer.width;
  assert.ok(
    controlShare <= 0.15,
    `the control stack never dominates the composer, observed ${controlShare}`,
  );
  assert.equal(Math.round(geometry.controls.width), 112, "the stack keeps the reference pill width");
  assert.equal(Math.round(geometry.speech.height), 36, "Quiet uses the reference range capsule");
  assert.equal(Math.round(geometry.lifetime.height), 36, "Drain uses the reference range capsule");
  assert.equal(Math.round(geometry.send.height), 36, "the Drain action shares one item height");
  assert.equal(
    geometry.armedAccent,
    darkPalette.quiet,
    "the Rust armed accent matches the reference periwinkle",
  );
  assert.equal(geometry.speech.border, geometry.stateBorder.quiet, "Quiet uses cool periwinkle");
  assert.equal(geometry.lifetime.border, geometry.stateBorder.drain, "Drain uses amber/gold");
  assert.equal(geometry.send.background, darkPalette.drain, "Drain submit is filled amber");
  assert.equal(geometry.send.color, darkPalette.pageInk, "Drain submit keeps high-contrast ink");
  assert.deepEqual(
    geometry.labels.map(({ text }) => text),
    ["Quiet", "Drain"],
    "the control capsules carry centered lowercase-styled state labels",
  );
  assert.ok(
    geometry.labels.every((label) => label.scrollWidth <= label.clientWidth + 1),
    "control labels are not clipped",
  );
  assert.ok(
    geometry.labels.every(
      (label) => label.textAlign === "center" && label.textTransform === "lowercase",
    ),
    "control labels are visibly centered and lowercase",
  );
  assert.ok(
    geometry.modes.every((mode) => mode.scrollWidth <= mode.clientWidth + 1),
    "desktop mode labels and badges are not clipped",
  );
  assert.equal(geometry.agents.length, 4, "all four agent shards remain mounted");
  assert.ok(
    geometry.agents.every((agent) => agent.width >= 150),
    "each of the four composer shards retains usable width",
  );
  for (let index = 1; index < geometry.agents.length; index += 1) {
    assert.ok(
      geometry.agents[index - 1].right <= geometry.agents[index].left + 1,
      "the four composer shards do not overlap",
    );
  }
  assert.ok(
    geometry.agents.at(-1).right <= geometry.controls.left + 1,
    "the control stack does not overlap the agent grid",
  );
  assert.equal(
    Math.round(geometry.activity.top - geometry.controls.bottom),
    10,
    "the control rail keeps the reference gap above the activity lane",
  );
  assert.ok(
    geometry.agents.every((agent) => Math.round(geometry.activity.top - agent.bottom) === 10),
    "every composer shard keeps the reference gap above the activity lane",
  );
  assert.ok(
    geometry.surface.scrollWidth <= geometry.surface.clientWidth + 1,
    "the reference control treatment introduces no horizontal overflow",
  );
}

function assertReferenceGuardRejectsPerturbation(geometry) {
  const perturbed = structuredClone(geometry);
  perturbed.taskStrip.gap = "13px";
  assert.throws(
    () => assertReferenceGeometry(perturbed),
    (error) => error instanceof assert.AssertionError
      && error.message.includes("task-pill-strip gap matches the canonical reference"),
    "the measured-style guard reddens and names a deliberately perturbed property",
  );
}

export async function assertCanonicalInlineTaskCode(surface) {
  assert.equal(
    await surface.locator(".message-footer .message-task-chip").count(),
    0,
    "card footers never repeat a reported task handle",
  );
  const chip = surface.locator(".message-body .message-task-chip").first();
  await chip.waitFor();
  const handle = await chip.getAttribute("data-task-handle");
  assert.equal(
    handle,
    "CONTROL-1kKbC3j4",
    "canonical inline code becomes the reported-task chip",
  );
  assert.equal(await chip.innerText(), handle, "the task chip visibly names the reported handle");
  assert.equal(
    await chip.getAttribute("aria-label"),
    `Reported task ${handle}`,
    "the task chip names the same handle to assistive technology",
  );
  const label = chip.locator(":scope > .message-task-chip-label");
  assert.equal(await label.count(), 1, "the rendered task chip owns one constrained label");
  assert.equal(await label.innerText(), handle, "the rendered label preserves the complete handle");
  const card = chip.locator("xpath=ancestor::article[1]");
  assert.match(
    await card.locator(".message-body").innerText(),
    new RegExp(handle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
    "the chip is derived from the task handle the message body reports",
  );
  assert.equal(
    await card.locator(".message-footer .message-task-chip").count(),
    0,
    "the inline identifier has no duplicate footer chip",
  );
  assert.equal(
    await card.locator('.message-task-chip[data-task-handle="APP-a1"]').count(),
    0,
    "a broadly compatible legacy handle does not become a task affordance",
  );
  const legacyCode = card.locator(".message-body code").filter({ hasText: "APP-a1" });
  assert.equal(await legacyCode.count(), 1, "the legacy handle remains ordinary inline code");
  assert.equal(await legacyCode.innerText(), "APP-a1", "ordinary code preserves legacy prose");
  assert.equal(
    await card.locator(".message-body code").filter({ hasText: handle }).count(),
    0,
    "canonical inline code is replaced by its single task affordance",
  );
  const geometry = await chip.evaluate((element) => {
    const box = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    return {
      height: box.height,
      borderRadius: Number.parseFloat(style.borderRadius),
      border: style.borderColor,
      clientWidth: element.clientWidth,
      scrollWidth: element.scrollWidth,
    };
  });
  assert.ok(geometry.borderRadius >= geometry.height / 2 - 1, "the task marker is a chip");
  assert.equal(geometry.border, darkPalette.green, "the task chip uses the live green accent");
  assert.ok(geometry.scrollWidth <= geometry.clientWidth + 1, "the reported handle is not clipped");
}

function taskChipArtifact(artifactPath, variant) {
  return artifactPath.replace(/\.png$/u, `-${variant}.png`);
}

async function installTaskChipFixture(page, surface) {
  await surface.evaluate((element, handles) => {
    document.querySelector('[data-task-chip-fixture="matrix"]')?.remove();
    const renderedChip = element.querySelector(".message-body .message-task-chip");
    const sourceCard = renderedChip?.closest("article");
    if (!sourceCard) throw new Error("task-chip fixture requires one rendered message card");
    const chip = (handle) => `<span class="message-task-chip" data-task-handle="${handle}"`
      + ` aria-label="Reported task ${handle}" title="${handle}">`
      + `<span class="message-task-chip-label">${handle}</span></span>`;
    const host = document.createElement("div");
    host.className = "team-surface";
    host.dataset.taskChipFixture = "matrix";
    Object.assign(host.style, {
      position: "fixed",
      zIndex: "2147483647",
      top: "16px",
      right: "auto",
      bottom: "auto",
      left: "16px",
      width: "420px",
      minWidth: "0",
      height: "auto",
      margin: "0",
      display: "block",
      overflow: "visible",
      border: "0",
      borderRadius: "0",
      background: "transparent",
      boxShadow: "none",
      transition: "none",
    });
    const fixture = sourceCard.cloneNode(true);
    fixture.removeAttribute("id");
    fixture.querySelectorAll("[id]").forEach((node) => node.removeAttribute("id"));
    Object.assign(fixture.style, {
      position: "relative",
      top: "auto",
      right: "auto",
      bottom: "auto",
      left: "auto",
      width: "100%",
      minWidth: "0",
      height: "auto",
      margin: "0",
      gridColumn: "auto",
      transition: "none",
    });
    const body = fixture.querySelector(".message-body");
    if (!body) throw new Error("task-chip fixture requires a message body");
    body.innerHTML = `
      <p data-task-chip-case="ordinary"><span data-prose>Settled </span>${chip(handles.generated)}<span data-prose> in one pass.</span></p>
      <p data-task-chip-case="long"><span data-prose>Transferred </span>${chip(handles.long)}<span data-prose> with its identity intact.</span></p>
      <p data-task-chip-case="multi"><span data-prose>Related </span>${chip("CARDS-1kRZRDy7")}<span data-prose> and </span>${chip("MOSAIC-1kRYqQxy")}<span data-prose> remain distinct.</span></p>
      <div data-task-chip-case="quoted-first">
        <blockquote>
          <p><span data-prose>Quoted </span>${chip(handles.generated)}<span data-prose> first.</span></p>
          <blockquote><p><span data-prose>Nested </span>${chip(handles.generated)}<span data-prose> again.</span></p></blockquote>
        </blockquote>
        <p><span data-prose>Authored </span>${chip(handles.generated)}<span data-prose> and </span>${chip(handles.generated)}<span data-prose> remain highlighted.</span></p>
      </div>
    `;
    const footer = fixture.querySelector(".message-footer");
    if (!footer) throw new Error("task-chip fixture requires a message footer");
    footer.innerHTML = `
      <div class="message-footer-left"><span class="message-age">1m</span></div>
      <div class="message-footer-right">
        <button class="card-action card-action--copy" type="button" title="Copy this message" aria-label="Copy this message"><span class="card-action-icon" aria-hidden="true">&#x29C9;</span></button>
      </div>
    `;
    host.append(fixture);
    document.body.append(host);
  }, { generated: GENERATED_MAX_TASK_HANDLE, long: LONG_ADMITTED_TASK_HANDLE });
  const fixture = page.locator(TASK_CHIP_FIXTURE);
  await fixture.waitFor();
  return fixture;
}

async function measureTaskChipFixture(fixture) {
  return fixture.evaluate((host) => {
    const rect = (box) => ({
      top: box.top,
      right: box.right,
      bottom: box.bottom,
      left: box.left,
      width: box.width,
      height: box.height,
    });
    const textRect = (node) => {
      const range = document.createRange();
      range.selectNodeContents(node);
      return rect(range.getBoundingClientRect());
    };
    const textRects = (node) => {
      const range = document.createRange();
      range.selectNodeContents(node);
      return [...range.getClientRects()].map(rect);
    };
    const card = host.querySelector(".mosaic-message-card");
    const body = card?.querySelector(".message-body");
    const quote = card?.querySelector("blockquote");
    const action = card?.querySelector(".card-action");
    if (!card || !body || !quote || !action) {
      throw new Error("task-chip fixture is incomplete");
    }
    const inspectChip = (node) => {
      const style = getComputedStyle(node);
      const label = node.querySelector(".message-task-chip-label");
      if (!label) throw new Error("task-chip fixture requires a label");
      const labelStyle = getComputedStyle(label);
      return {
        handle: node.dataset.taskHandle,
        ariaLabel: node.getAttribute("aria-label"),
        title: node.title,
        tag: node.tagName,
        tabIndex: node.tabIndex,
        clientWidth: label.clientWidth,
        scrollWidth: label.scrollWidth,
        box: rect(node.getBoundingClientRect()),
        text: textRect(label),
        fontSize: Number.parseFloat(style.fontSize),
        lineHeight: Number.parseFloat(style.lineHeight),
        overflowX: style.overflowX,
        textOverflow: style.textOverflow,
        verticalAlign: style.verticalAlign,
        whiteSpace: style.whiteSpace,
        marginTop: style.marginTop,
        marginRight: style.marginRight,
        marginBottom: style.marginBottom,
        marginLeft: style.marginLeft,
        paddingTop: style.paddingTop,
        paddingRight: style.paddingRight,
        paddingBottom: style.paddingBottom,
        paddingLeft: style.paddingLeft,
        labelOverflowX: labelStyle.overflowX,
        labelTextOverflow: labelStyle.textOverflow,
        labelWhiteSpace: labelStyle.whiteSpace,
      };
    };
    return {
      card: {
        box: rect(card.getBoundingClientRect()),
        clientWidth: card.clientWidth,
        scrollWidth: card.scrollWidth,
      },
      body: {
        box: rect(body.getBoundingClientRect()),
        clientWidth: body.clientWidth,
        scrollWidth: body.scrollWidth,
      },
      quote: {
        fontSize: Number.parseFloat(getComputedStyle(quote).fontSize),
        lineHeight: Number.parseFloat(getComputedStyle(quote).lineHeight),
      },
      cases: [...card.querySelectorAll("[data-task-chip-case]")].map((row) => ({
        name: row.dataset.taskChipCase,
        box: rect(row.getBoundingClientRect()),
        clientWidth: row.clientWidth,
        scrollWidth: row.scrollWidth,
        prose: [...row.querySelectorAll("[data-prose]")].flatMap(textRects),
        chips: [...row.querySelectorAll(".message-task-chip")].map(inspectChip),
      })),
      action: {
        box: rect(action.getBoundingClientRect()),
        tabIndex: action.tabIndex,
        width: action.getBoundingClientRect().width,
        height: action.getBoundingClientRect().height,
      },
    };
  });
}

function assertTaskChipMeasurement(label, measured) {
  const tolerance = 1;
  const baselineRows = new Set();
  assert.ok(
    measured.card.scrollWidth <= measured.card.clientWidth + tolerance
      && measured.body.scrollWidth <= measured.body.clientWidth + tolerance,
    `${label} card and message body contain the matrix: ${JSON.stringify(measured)}`,
  );
  for (const row of measured.cases) {
    assert.ok(
      row.scrollWidth <= row.clientWidth + tolerance,
      `${label} ${row.name} prose has no horizontal overflow: ${JSON.stringify(row)}`,
    );
    for (const chip of row.chips) {
      assert.equal(chip.tag, "SPAN", `${label} ${chip.handle} remains an inline affordance`);
      assert.equal(chip.tabIndex, -1, `${label} ${chip.handle} does not enter the tab order`);
      assert.equal(chip.ariaLabel, `Reported task ${chip.handle}`);
      assert.equal(chip.title, chip.handle, `${label} ${chip.handle} retains its full hover label`);
      assert.equal(chip.whiteSpace, "nowrap", `${label} ${chip.handle} stays on one line`);
      assert.equal(chip.textOverflow, "ellipsis", `${label} ${chip.handle} declares ellipsis`);
      assert.equal(chip.overflowX, "hidden", `${label} ${chip.handle} clips only inside itself`);
      assert.equal(chip.labelWhiteSpace, "nowrap", `${label} ${chip.handle} label stays on one line`);
      assert.equal(chip.labelTextOverflow, "ellipsis", `${label} ${chip.handle} label paints ellipsis`);
      assert.equal(chip.labelOverflowX, "hidden", `${label} ${chip.handle} label clips its text`);
      assert.equal(chip.verticalAlign, "baseline", `${label} ${chip.handle} follows prose baseline`);
      assert.deepEqual(
        [chip.marginTop, chip.marginRight, chip.marginBottom, chip.marginLeft],
        ["2px", "0px", "2px", "0px"],
        `${label} ${chip.handle} uses the requested block-only margin`,
      );
      assert.deepEqual(
        [chip.paddingTop, chip.paddingRight, chip.paddingBottom, chip.paddingLeft],
        ["0px", "4px", "0px", "4px"],
        `${label} ${chip.handle} uses the requested inline-only padding`,
      );
      assert.ok(
        Math.abs(chip.fontSize - measured.quote.fontSize) <= 0.01
          && Math.abs(chip.lineHeight - measured.quote.lineHeight) <= 0.01,
        `${label} ${chip.handle} matches quoted prose type: ${JSON.stringify({ chip, quote: measured.quote })}`,
      );
      assert.ok(
        chip.box.left >= measured.body.box.left - tolerance
          && chip.box.right <= measured.body.box.right + tolerance,
        `${label} ${chip.handle} remains inside the message body: ${JSON.stringify(chip.box)}`,
      );
      const adjacentProse = row.prose.find((prose) => (
        prose.top <= chip.text.bottom && prose.bottom >= chip.text.top
      ));
      if (adjacentProse) {
        assert.ok(
          Math.abs(adjacentProse.bottom - chip.text.bottom) <= 2,
          `${label} ${chip.handle} shares the prose baseline: ${JSON.stringify({ chip, adjacentProse })}`,
        );
        baselineRows.add(row.name);
      }
    }
    for (let index = 1; index < row.chips.length; index += 1) {
      const previous = row.chips[index - 1].box;
      const current = row.chips[index].box;
      const separated = previous.right <= current.left + tolerance
        || previous.bottom <= current.top + tolerance;
      assert.ok(separated, `${label} ${row.name} chips do not overlap`);
    }
  }
  assert.ok(
    baselineRows.has("ordinary") && baselineRows.has("quoted-first"),
    `${label} ordinary and quote-nested chips share neighboring prose baselines`,
  );
  const quotedFirst = measured.cases.find(({ name }) => name === "quoted-first");
  assert.deepEqual(
    quotedFirst.chips.map(({ handle }) => handle),
    Array(4).fill(GENERATED_MAX_TASK_HANDLE),
    `${label} quoted, nested-quoted, and later prose occurrences all remain chips`,
  );
  const ordinary = measured.cases.find(({ name }) => name === "ordinary").chips[0];
  const long = measured.cases.find(({ name }) => name === "long").chips[0];
  assert.equal(ordinary.handle, GENERATED_MAX_TASK_HANDLE);
  assert.ok(
    ordinary.scrollWidth <= ordinary.clientWidth + tolerance,
    `${label} maximum generated handle remains complete`,
  );
  assert.equal(long.handle, LONG_ADMITTED_TASK_HANDLE);
  assert.ok(
    long.scrollWidth > long.clientWidth + tolerance,
    `${label} long admitted handle exercises the declared ellipsis`,
  );
  assert.equal(measured.action.tabIndex, 0, `${label} surrounding card action remains focusable`);
  assert.deepEqual(
    [measured.action.width, measured.action.height],
    [32, 32],
    `${label} surrounding card action retains its stable dimensions`,
  );
  assert.ok(
    measured.action.box.left >= measured.card.box.left
      && measured.action.box.right <= measured.card.box.right + tolerance,
    `${label} surrounding card action remains inside the card`,
  );
}

export async function assertTaskChipTypographyMatrix(page, surface, artifactPath) {
  await assertCanonicalInlineTaskCode(surface);
  const fixture = await installTaskChipFixture(page, surface);
  const rootFontSize = await page.evaluate(() => document.documentElement.style.fontSize);
  try {
    const ordinary = await measureTaskChipFixture(fixture);
    assertTaskChipMeasurement("ordinary", ordinary);
    await fixture.screenshot({ path: taskChipArtifact(artifactPath, "ordinary") });
    await fixture.locator('[data-task-chip-case="long"]').screenshot({
      path: taskChipArtifact(artifactPath, "long"),
    });
    await fixture.locator('[data-task-chip-case="multi"]').screenshot({
      path: taskChipArtifact(artifactPath, "multi"),
    });

    await fixture.evaluate((card) => { card.style.width = "236px"; });
    const narrow = await measureTaskChipFixture(fixture);
    assertTaskChipMeasurement("narrow", narrow);
    const action = fixture.locator(".card-action");
    await action.focus();
    assert.equal(await action.evaluate((node) => document.activeElement === node), true);
    await fixture.screenshot({ path: taskChipArtifact(artifactPath, "narrow") });

    await fixture.evaluate((card) => { card.style.width = "420px"; });
    await page.evaluate(() => { document.documentElement.style.fontSize = "20px"; });
    const zoomed = await measureTaskChipFixture(fixture);
    assertTaskChipMeasurement("zoomed", zoomed);
    assert.ok(
      zoomed.quote.fontSize > ordinary.quote.fontSize,
      `task-chip matrix follows root text zoom: ${JSON.stringify({ ordinary, zoomed })}`,
    );
    await fixture.screenshot({ path: taskChipArtifact(artifactPath, "zoom") });
  } finally {
    await page.evaluate((fontSize) => { document.documentElement.style.fontSize = fontSize; }, rootFontSize);
    await fixture.evaluate((node) => node.remove());
  }
}

/// The operator agent card carries its own control: renewing this agent has to
/// name both the action and the request it sends, stay on the exact agent lane
/// it was drawn for, and reach the resident rather than only the message list.
export async function assertAgentRenewalRequest(page, surface, fixture) {
  const surfaces = page.locator('[data-pane="agent-console"] .team-surface');
  if ((await surfaces.count()) === 2) {
    const primaryLineage = await surface.getAttribute("data-lineage-id");
    assert.ok(primaryLineage, "the primary control surface names its lineage");
    const first = surfaces.first();
    const secondaryRow = await first.getAttribute("data-lineage-id") === primaryLineage
      ? surfaces.nth(1)
      : first;
    const secondaryLineage = await secondaryRow.getAttribute("data-lineage-id");
    assert.ok(secondaryLineage, "the sibling control surface names its lineage");
    const secondary = page.locator(
      `[data-pane="agent-console"] .team-surface[data-lineage-id="${secondaryLineage}"]`,
    );
    await assertTeamActions(page, surface, secondary, primaryLineage);
  }
  const renewalRequest = "Renew your current task lease now.";
  const menu = surface
    .locator(
      ".composer-shard--primary .agent-action-menu:has(.agent-renew-request:not(:disabled))",
    )
    .first();
  await menu.waitFor();
  const composer = menu.locator(
    "xpath=ancestor::*[contains(concat(' ', normalize-space(@class), ' '), ' composer-shard--primary ')][1]",
  );
  const textbox = composer.getByRole("textbox");
  assert.equal(
    await menu.getAttribute("open"),
    null,
    "agent renewal starts inside a closed drawer",
  );
  const trigger = menu.locator(":scope > summary");
  const focus = menu.getByRole("button", { name: "Focus message editor" });
  const renew = menu.locator(".agent-renew-request:not(:disabled)");
  const vendorIcon = composer.locator(".composer-vendor-icon");
  await vendorIcon.waitFor();
  assert.equal(await renew.isVisible(), false, "renewal is hidden while its drawer is closed");
  await trigger.click();
  await renew.waitFor();
  assert.equal(await vendorIcon.isVisible(), false, "the open menu clears the body vendor icon");
  const geometry = await menu.evaluate((details) => {
    const composer = details.closest(".composer-shard--primary");
    const drawer = details.querySelector(".agent-action-drawer");
    const trigger = details.querySelector(":scope > summary");
    const heading = details.closest(".composer-shard-heading");
    const actions = [...details.querySelectorAll(".agent-action-drawer > button")];
    if (!composer || !drawer || !trigger || !heading || actions.length < 2) {
      throw new Error("agent menu geometry is incomplete");
    }
    const rect = (element) => element.getBoundingClientRect().toJSON();
    const composerBox = rect(composer);
    const drawerBox = rect(drawer);
    const hit = document.elementFromPoint(
      composerBox.left + composerBox.width / 2,
      composerBox.bottom - 8,
    );
    return {
      composer: composerBox,
      drawer: drawerBox,
      heading: rect(heading),
      trigger: rect(trigger),
      actions: actions.map((action) => ({
        box: rect(action),
        scrollWidth: action.scrollWidth,
        clientWidth: action.clientWidth,
        scrollHeight: action.scrollHeight,
        clientHeight: action.clientHeight,
      })),
      drawerOwnsComposerInterior: hit !== null && drawer.contains(hit),
    };
  });
  for (const edge of ["right", "bottom", "left"]) {
    assert.ok(
      Math.abs(geometry.drawer[edge] - geometry.composer[edge]) <= 1,
      `the open agent menu fills the composer body's ${edge} edge: ${JSON.stringify(geometry)}`,
    );
  }
  assert.ok(
    Math.abs(geometry.drawer.top - geometry.heading.bottom) <= 1,
    `the action grid replaces the editor below the retained header: ${JSON.stringify(geometry)}`,
  );
  assert.ok(
    geometry.actions.every((action) => (
      action.box.width > 0 && action.box.height >= 72 &&
      action.scrollWidth <= action.clientWidth && action.scrollHeight <= action.clientHeight
    )),
    `every menu action is a full-size unclipped label/detail control: ${JSON.stringify(geometry.actions)}`,
  );
  assert.ok(
    geometry.trigger.left >= geometry.composer.left &&
      geometry.trigger.right <= geometry.composer.right &&
      geometry.trigger.top >= geometry.composer.top &&
      geometry.trigger.bottom <= geometry.composer.bottom,
    `the hamburger remains inside the composer while its full overlay is open: ${JSON.stringify(geometry)}`,
  );
  assert.equal(
    geometry.drawerOwnsComposerInterior,
    true,
    "the open drawer receives interaction across the composer's covered interior",
  );
  await focus.click();
  assert.equal(await menu.getAttribute("open"), null, "focusing the editor closes its overlay");
  const textboxId = await textbox.getAttribute("id");
  assert.ok(textboxId, "the primary message editor has a stable focus target");
  await waitForPageFunction(
    page,
    "the focus action returns interaction to this agent's message editor",
    TRANSITION_MS,
    (id) => document.activeElement?.id === id,
    textboxId,
  );

  await trigger.click();
  await renew.waitFor();
  const siblingEditor = surface.locator(".team-agent").nth(1).getByRole("textbox");
  await siblingEditor.click();
  assert.equal(
    await menu.getAttribute("open"),
    null,
    "clicking outside the action menu dismisses it",
  );
  await trigger.click();
  await renew.waitFor();
  assert.equal(
    (await renew.innerText()).replaceAll("\n", " "),
    "Renew this agent request renewal",
    "the renewal action names both the command and its request",
  );
  const renewalLane = await renew.evaluate(
    (button) => button.closest(".team-agent")?.dataset.agentLane,
  );
  assert.ok(renewalLane, "the renewal control retains its exact agent lane");
  await renew.click();
  assert.equal(await menu.getAttribute("open"), null, "renewing closes the full composer overlay");
  const renewalInbox = await reply(fixture, ["inbox", renewalLane]);
  assert.ok(
    renewalInbox.body.lanes[0].pending.some(
      (directive) => directive.text === renewalRequest,
    ),
    "Renew this agent routes a residency-witnessed renewal request through the resident",
  );
  assert.equal(
    await surface.locator(`.mosaic-message-card[data-agent-lane="${renewalLane}"]`)
      .filter({ hasText: renewalRequest }).count(),
    0,
    "an unacknowledged renewal request is absent from the conversation",
  );
  assert.equal(await renew.isVisible(), false, "renewal hides again when its drawer closes");
}

export async function captureReferenceControls(page, team, artifactPath) {
  const surface = team.locator('.team-surface[data-lineage-weight="4"]').first();
  await surface.waitFor();
  const controls = surface.locator(".team-composer-controls");
  const lineage = await surface.getAttribute("data-lineage-id");
  assert.ok(lineage, "the reference controls belong to an identified lineage");
  const speech = controls.locator('.team-speech input[type="range"]');
  const lifetime = controls.locator('.team-lifetime input[type="range"]');
  const textarea = surface.locator(".team-agent textarea").first();
  const initial = {
    speech: await speech.inputValue(),
    lifetime: await lifetime.inputValue(),
    body: await textarea.inputValue(),
  };
  const lifeLabels = ["Steer", "Drive", "Drain"];

  try {
    await speech.fill("0");
    assert.equal(
      await controls.getAttribute("data-speech-mode"),
      "Quiet",
      "the native speech range updates the rendered control state",
    );
    if (initial.lifetime === "2") {
      await selectLifetime(page, lineage, lifetime, "1", "Drive");
    }
    await selectLifetime(page, lineage, lifetime, "2", "Drain");
    await textarea.fill("Reference control geometry");
    const drain = controls.getByRole("button", { name: "Drain", exact: true });
    await drain.waitFor();
    assert.equal(await drain.isEnabled(), true, "a real draft enables the Drain action");
    assert.equal(await speech.inputValue(), "0", "the native speech range selects Quiet");
    assert.equal(await lifetime.inputValue(), "2", "the native lifetime range selects Drain");
    const geometry = await surface.evaluate(measureReference);
    assertReferenceGuardRejectsPerturbation(geometry);
    assertReferenceGeometry(geometry);
    await page.screenshot({ path: artifactPath, fullPage: false });
  } finally {
    await textarea.fill(initial.body);
    if (await lifetime.inputValue() !== initial.lifetime) {
      await selectLifetime(
        page,
        lineage,
        lifetime,
        initial.lifetime,
        lifeLabels[Number(initial.lifetime)],
      );
    }
    await speech.fill(initial.speech);
    assert.equal(await textarea.inputValue(), initial.body, "the reference draft is restored");
    assert.equal(
      await lifetime.inputValue(),
      initial.lifetime,
      "the server-backed lifetime policy is restored",
    );
    assert.equal(await speech.inputValue(), initial.speech, "the local speech mode is restored");
  }
}
