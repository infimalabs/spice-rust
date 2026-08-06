import assert from "node:assert/strict";

const darkPalette = {
  quiet: "rgb(138, 169, 232)",
  drain: "rgb(230, 180, 78)",
  green: "rgb(115, 199, 159)",
  drainInk: "rgb(21, 18, 10)",
};

async function selectLifetime(page, lineage, input, value, label) {
  await input.fill(value);
  await page.waitForFunction(({ lineageId, expected }) => {
    const current = document.querySelector(
      `.team-surface[data-lineage-id="${lineageId}"] .team-composer-controls`,
    );
    return current?.dataset.lifetimePending === "false"
      && current.dataset.lifetime === expected;
  }, { lineageId: lineage, expected: label });
}

function measureReference(surface) {
  const rect = (selector) => {
    const node = surface.querySelector(selector);
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
      borderRadius: style.borderRadius,
      boxShadow: style.boxShadow,
      color: style.color,
      clientWidth: node.clientWidth,
      scrollWidth: node.scrollWidth,
    };
  };
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
      background: getComputedStyle(badge).backgroundColor,
      width: badge.getBoundingClientRect().width,
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
      clientWidth: mode.clientWidth,
      scrollWidth: mode.scrollWidth,
    })),
  };
}

function assertReferenceGeometry(geometry) {
  assert.ok(
    geometry.rail.width / geometry.toolbar.width >= 0.82,
    "the four-mode rail remains the dominant toolbar element",
  );
  assert.ok(geometry.toolbar.height >= 58, "the desktop toolbar has reference-scale height");
  assert.ok(geometry.rail.height >= 46, "the rounded rail has a substantial inset track");
  assert.notEqual(geometry.selected.boxShadow, "none", "the selected mode has an inset treatment");
  assert.equal(geometry.badges.length >= 2, true, "the live rail exposes green count badges");
  assert.ok(
    geometry.badges.every((badge) => badge.background === darkPalette.green && badge.width >= 19),
    "the rail badges retain the dark-palette green treatment",
  );
  assert.equal(geometry.lights.length, 4, "the reference toolbar shows four agent lights");
  assert.ok(
    geometry.lights.every((light) => light.width >= 10 && light.height >= 10),
    "the four status lights stay legible",
  );
  assert.ok(
    Math.abs(geometry.menu.width - geometry.menu.height) <= 1 && geometry.menu.width >= 38,
    "the details control is a substantial round button",
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
