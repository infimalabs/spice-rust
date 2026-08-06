import assert from "node:assert/strict";
import { join } from "node:path";

import { observedBoundedWait, TRANSITION_MS } from "./canvas-fixture.mjs";

/// Geometry read off the Python Spice reference capture, attachment
/// 6cb304ba784a940d13b5741390528f964f73937102c6a15e3e4539e3ed40968f/01-image.png,
/// at the 2x device pixel ratio it was taken with. The reference stacks the
/// speech slider, the lifetime slider, and the send button as three identical
/// fully-rounded pills, so one width and one height bind all three.
const referenceItemWidth = 112;
const referenceItemHeight = 36;
const referenceGap = 6;
const referenceRadius = "999px";
const referenceLabelFontSize = "11px";
const referenceSendFontSize = "16px";

/// The knob and the unfilled track are deliberately unasserted. Neither Spice
/// authors a thumb or track pseudo-element -- both set `accent-color` and
/// nothing else -- so the reference's white ring over a light track is the
/// capture browser's own control chrome, and headless Chromium draws a flat
/// accent disc from byte-identical CSS. Pinning it here would pin the user
/// agent rather than the Rust surface.
///
/// The exact accent hues are the second named divergence. The reference draws
/// its armed and maxed states from the predecessor palette (`--final-accent`
/// #8ea2ff and `--say-accent` #f2c14e in dark); Rust keeps its own `--accent-2`
/// #8aa9e8 and `--warning` #e6b44e, which every other Rust surface already
/// shares. The role each colour plays is what the reference fixes, so the roles
/// are asserted through the live tokens below and the hues are left alone
/// rather than forking a control-only palette.
const states = [
  { name: "steer", speech: "1", lifetime: "0" },
  { name: "drain", speech: "1", lifetime: "2" },
  { name: "quiet", speech: "0", lifetime: "0" },
  { name: "narrate", speech: "2", lifetime: "2" },
];

function describeStack(node) {
  const probe = document.createElement("span");
  node.append(probe);
  const resolve = (expression) => {
    probe.style.color = expression;
    const resolved = getComputedStyle(probe).color;
    probe.style.color = "";
    return resolved;
  };
  const tokens = {
    armed: resolve("var(--accent-2)"),
    maxed: resolve("var(--warning)"),
    muted: resolve("var(--muted)"),
    page: resolve("var(--page)"),
  };
  probe.remove();

  const measure = (element) => {
    const style = getComputedStyle(element);
    const box = element.getBoundingClientRect();
    return {
      width: Math.round(box.width),
      height: Math.round(box.height),
      top: Math.round(box.top),
      left: Math.round(box.left),
      bottom: Math.round(box.bottom),
      radius: style.borderTopLeftRadius,
      family: style.fontFamily,
      classes: [...element.classList],
    };
  };
  const slider = (selector) => {
    const element = node.querySelector(selector);
    const label = element.querySelector("span");
    const labelStyle = getComputedStyle(label);
    return {
      ...measure(element),
      value: element.querySelector("input").value,
      label: label.textContent.trim(),
      labelColor: labelStyle.color,
      labelFontSize: labelStyle.fontSize,
      labelFamily: labelStyle.fontFamily,
      labelTransform: labelStyle.textTransform,
    };
  };
  const button = node.querySelector(".team-send");
  const buttonStyle = getComputedStyle(button);
  return {
    tokens,
    speech: slider(".team-speech"),
    lifetime: slider(".team-lifetime"),
    send: {
      ...measure(button),
      text: button.textContent.trim(),
      disabled: button.disabled,
      fontSize: buttonStyle.fontSize,
      background: buttonStyle.backgroundColor,
      color: buttonStyle.color,
    },
  };
}

function assertReferenceShape(stack, state) {
  const items = [stack.speech, stack.lifetime, stack.send];
  const named = ["speech slider", "lifetime slider", "send button"];
  for (const [index, item] of items.entries()) {
    assert.equal(
      item.width,
      referenceItemWidth,
      `${state} ${named[index]} keeps the reference pill width`,
    );
    assert.equal(
      item.height,
      referenceItemHeight,
      `${state} ${named[index]} keeps the reference pill height`,
    );
    assert.equal(item.radius, referenceRadius, `${state} ${named[index]} stays fully rounded`);
    assert.equal(
      item.left,
      items[0].left,
      `${state} ${named[index]} shares one left edge with the stack`,
    );
  }
  for (let index = 1; index < items.length; index += 1) {
    assert.equal(
      items[index].top - items[index - 1].bottom,
      referenceGap,
      `${state} ${named[index]} sits one reference gap below ${named[index - 1]}`,
    );
  }
  for (const slider of [stack.speech, stack.lifetime]) {
    assert.equal(
      slider.labelFontSize,
      referenceLabelFontSize,
      `${state} ${slider.label} label keeps the reference type size`,
    );
    assert.match(slider.labelFamily, /mono/i, `${state} ${slider.label} label stays monospace`);
    // The source text is capitalised; the reference renders it lower-case, so
    // the transform is the thing that has to hold.
    assert.equal(
      slider.labelTransform,
      "lowercase",
      `${state} ${slider.label} label renders lower-case like the reference`,
    );
  }
  assert.equal(
    stack.send.fontSize,
    referenceSendFontSize,
    `${state} send button keeps the reference type size`,
  );
  assert.match(stack.send.family, /mono/i, `${state} send button stays monospace`);
  assert.equal(stack.send.color, stack.tokens.page, `${state} send button prints on the page ink`);
}

/// One accent drives a state everywhere it shows: the reference tints the
/// slider border and its label with the same colour it fills the send button
/// with, so a state that only reached one of the three would read as drift.
function assertReferenceState(stack, expected) {
  const accent = expected.accent === null ? null : stack.tokens[expected.accent];
  const slider = stack[expected.control];
  assert.equal(
    slider.label.toLowerCase(),
    expected.label,
    `${expected.name} names the reference label`,
  );
  assert.equal(slider.value, expected.value, `${expected.name} holds the reference slider value`);
  if (expected.class === null) {
    assert.ok(
      !slider.classes.includes("armed") && !slider.classes.includes("maxed"),
      `${expected.name} leaves the resting slider untreated`,
    );
    assert.equal(slider.labelColor, stack.tokens.muted, `${expected.name} keeps a muted label`);
    return;
  }
  assert.ok(
    slider.classes.includes(expected.class),
    `${expected.name} carries the reference ${expected.class} treatment`,
  );
  assert.equal(slider.labelColor, accent, `${expected.name} tints its label with the state accent`);
  if (expected.control !== "lifetime") return;
  assert.ok(
    stack.send.classes.includes(expected.class),
    `${expected.name} carries the send treatment with the slider`,
  );
  assert.equal(stack.send.text, expected.send, `${expected.name} names the reference send label`);
  assert.equal(
    stack.send.background,
    accent,
    `${expected.name} fills the send button with the same state accent`,
  );
}

const expectations = new Map([
  ["steer", [
    { name: "steer", control: "lifetime", value: "0", label: "steer", class: "armed", accent: "armed", send: "Steer" },
    { name: "speak", control: "speech", value: "1", label: "speak", class: null, accent: null },
  ]],
  ["drain", [
    { name: "drain", control: "lifetime", value: "2", label: "drain", class: "maxed", accent: "maxed", send: "Drain" },
  ]],
  ["quiet", [
    { name: "quiet", control: "speech", value: "0", label: "quiet", class: "armed", accent: "armed" },
  ]],
  ["narrate", [
    { name: "narrate", control: "speech", value: "2", label: "narrate", class: "maxed", accent: "maxed" },
  ]],
]);

/// Below 720px the reference narrows the same stack rather than letting it
/// crowd the agent cards, and it narrows every item together: a 68px track
/// inside 3px 5px padding and a 1px border is 80 wide, under a 34px item
/// height. Without this the fallback would be the one block of the reference
/// treatment nothing could red.
const narrowItemWidth = 80;
const narrowItemHeight = 34;

async function assertNarrowStack(page, controls, viewport) {
  await page.setViewportSize({ width: 680, height: viewport.height });
  try {
    await controls
      .locator(".team-speech")
      .evaluate(
        (node, expected) =>
          new Promise((settle, fail) => {
            const deadline = Date.now() + 10_000;
            const poll = () => {
              const measured = Math.round(node.getBoundingClientRect().width);
              if (measured === expected) return settle();
              if (Date.now() > deadline) {
                return fail(new Error(`narrow speech slider stalled at ${measured}px`));
              }
              requestAnimationFrame(poll);
            };
            poll();
          }),
        narrowItemWidth,
      );
    const stack = await controls.evaluate(describeStack);
    const items = [stack.speech, stack.lifetime, stack.send];
    const named = ["speech slider", "lifetime slider", "send button"];
    for (const [index, item] of items.entries()) {
      assert.equal(item.width, narrowItemWidth, `narrow ${named[index]} keeps the fallback width`);
      assert.equal(
        item.height,
        narrowItemHeight,
        `narrow ${named[index]} keeps the fallback height`,
      );
    }
    for (let index = 1; index < items.length; index += 1) {
      assert.equal(
        items[index].top - items[index - 1].bottom,
        referenceGap,
        `narrow ${named[index]} keeps the reference gap below ${named[index - 1]}`,
      );
    }
  } finally {
    await page.setViewportSize(viewport);
  }
}

/// Capture the lineage control stack at the reference's own states and refuse
/// any material drift from the Python Spice reference image.
///
/// The console must already hold a seated lineage, and the caller's viewport
/// must be wider than the 720px breakpoint where the reference itself narrows
/// the stack. The composer is given a draft so the send button is captured
/// enabled, the way the reference shows it, and the draft is withdrawn again so
/// the surrounding conversation flow resumes untouched.
export async function assertReferenceControlStack(page, console_, artifacts) {
  const viewport = page.viewportSize();
  const width = viewport?.width ?? 0;
  assert.ok(width > 720, "the reference control stack is measured above the narrow breakpoint");
  const controls = console_.locator(".team-composer-controls").first();
  await controls.waitFor({ timeout: 15_000 });
  const draft = console_.locator(".team-surface .mosaic-composer textarea").first();
  const restored = await draft.inputValue();
  await draft.fill("reference control capture");
  const speech = controls.locator('.team-speech input[type="range"]');
  const lifetime = controls.locator('.team-lifetime input[type="range"]');
  const send = controls.locator(".team-send");

  for (const state of states) {
    await speech.fill(state.speech);
    await lifetime.fill(state.lifetime);
    for (const expected of expectations.get(state.name)) {
      await controls
        .locator(`.team-${expected.control} span`)
        .filter({ hasText: expected.label })
        .waitFor({ timeout: 10_000 });
    }
    // A slider label can render before its lane is live. A trial click waits
    // for actionability on the send button itself without dispatching it.
    await observedBoundedWait(
      page,
      `${state.name} reference send control becomes live`,
      TRANSITION_MS,
      () => send.click({ trial: true, timeout: TRANSITION_MS }),
    );
    await controls.screenshot({ path: join(artifacts, `controls-${state.name}.png`) });
    const stack = await controls.evaluate(describeStack);
    assert.ok(!stack.send.disabled, "the reference states are captured with a live send button");
    assertReferenceShape(stack, state.name);
    for (const expected of expectations.get(state.name)) assertReferenceState(stack, expected);
  }

  await assertNarrowStack(page, controls, viewport);
  await speech.fill("1");
  await lifetime.fill("0");
  await console_.getByRole("button", { name: "Steer", exact: true }).waitFor();
  await draft.fill(restored);
}
