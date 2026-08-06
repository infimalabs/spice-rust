import assert from "node:assert/strict";

import { TRANSITION_MS, waitForPageFunction } from "./canvas-fixture.mjs";

const darkPalette = {
  quiet: "rgb(138, 169, 232)",
  drain: "rgb(230, 180, 78)",
  green: "rgb(115, 199, 159)",
  drainInk: "rgb(21, 18, 10)",
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
  return {
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

  const controlShare = geometry.controls.width / geometry.composer.width;
  assert.ok(
    controlShare >= 0.12 && controlShare <= 0.15,
    `the control stack occupies 12–15% of the composer, observed ${controlShare}`,
  );
  assert.ok(geometry.speech.height >= 54, "Quiet uses a tall rounded range capsule");
  assert.ok(geometry.lifetime.height >= 54, "Drain uses a tall rounded range capsule");
  assert.ok(geometry.send.height >= 50, "the Drain action is visually substantial");
  assert.equal(geometry.speech.border, darkPalette.quiet, "Quiet uses cool periwinkle");
  assert.equal(geometry.lifetime.border, darkPalette.drain, "Drain uses amber/gold");
  assert.equal(geometry.send.background, darkPalette.drain, "Drain submit is filled amber");
  assert.equal(geometry.send.color, darkPalette.drainInk, "Drain submit keeps high-contrast ink");
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

async function assertReportedTaskChip(surface) {
  const chip = surface.locator(".message-task-chip").first();
  await chip.waitFor();
  const handle = await chip.getAttribute("data-task-handle");
  assert.ok(handle, "the reported-task chip carries its canonical handle");
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
    await assertReportedTaskChip(surface);
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
