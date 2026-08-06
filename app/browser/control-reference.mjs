import assert from "node:assert/strict";

import { reply } from "./canvas-fixture.mjs";

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
  const stateBorder = { quiet: mix("--accent-2"), drain: mix("--warning") };
  probe.remove();
  return {
    stateBorder,
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

/// The operator agent card carries its own control: renewing this agent has to
/// name both the action and the request it sends, stay on the exact agent lane
/// it was drawn for, and reach the resident rather than only the message list.
export async function assertAgentRenewalRequest(surface, fixture) {
  const renewalRequest = "Renew your current task lease now.";
  const renew = surface.locator(".agent-renew-request:not(:disabled)").first();
  await renew.waitFor();
  assert.equal(
    (await renew.innerText()).replaceAll("\n", " "),
    "Renew this agent request renewal",
    "the operator card names both the action and its renewal request",
  );
  const renewalLane = await renew.evaluate(
    (button) => button.closest(".team-agent")?.dataset.agentLane,
  );
  assert.ok(renewalLane, "the renewal control retains its exact agent lane");
  await renew.click();
  await surface
    .locator(`.mosaic-message-card[data-agent-lane="${renewalLane}"]`)
    .filter({ hasText: renewalRequest })
    .waitFor({ timeout: 10_000 });
  const renewalInbox = await reply(fixture, ["inbox", renewalLane]);
  assert.ok(
    renewalInbox.body.lanes[0].pending.some(
      (directive) => directive.text === renewalRequest,
    ),
    "Renew this agent routes a residency-witnessed renewal request through the resident",
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
    await assertReportedTaskChip(surface);
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
