import assert from "node:assert/strict";

import { reply, TRANSITION_MS, waitForPageFunction } from "./canvas-fixture.mjs";

const darkPalette = {
  quiet: "rgb(138, 169, 232)",
  drain: "rgb(230, 180, 78)",
  green: "rgb(115, 199, 159)",
  // One ink prints every submit state, the way the predecessor's `.primary`
  // reads a single `--button-accent-fg` and lets only the fill follow the
  // state accent. A maxed-only ink would be a second place to keep in step.
  pageInk: "rgb(11, 15, 12)",
};

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
  probe.style.color = "var(--accent-2)";
  const armedAccent = getComputedStyle(probe).color;
  probe.style.color = "";
  const stateBorder = { quiet: mix("--accent-2"), drain: mix("--warning") };
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
      return { left: box.left, right: box.right, top: box.top, width: box.width };
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
  assert.equal(
    new Set(geometry.taskPills.map((pill) => pill.stem)).size,
    geometry.taskPills.length,
    "each live board stem renders exactly one pill",
  );
  assert.equal(geometry.taskStrip.gap, "6px", "task-pill-strip gap matches the Python reference");
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
    "the Rust armed accent keeps its named periwinkle divergence",
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
      && error.message.includes("task-pill-strip gap matches the Python reference"),
    "the measured-style guard reddens and names a deliberately perturbed property",
  );
}

export async function assertCanonicalInlineTaskCode(surface) {
  assert.equal(
    await surface.locator(".message-meta .message-task-chip").count(),
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
  const card = chip.locator("xpath=ancestor::article[1]");
  assert.match(
    await card.locator(".message-body").innerText(),
    new RegExp(handle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")),
    "the chip is derived from the task handle the message body reports",
  );
  assert.equal(
    await card.locator(".message-meta .message-task-chip").count(),
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

async function assertInlineTaskChip(surface) {
  await assertCanonicalInlineTaskCode(surface);
  const chiplessCard = surface
    .locator('.mosaic-message-card[data-card-kind="agent-message"]:not(:has(.message-task-chip))')
    .first();
  await chiplessCard.waitFor();
  assert.equal(
    await chiplessCard.locator(".message-task-chip").count(),
    0,
    "a card with no useful inline identifier omits the chip entirely",
  );
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
    await assertInlineTaskChip(surface);
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
