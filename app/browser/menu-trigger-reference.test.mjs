import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { launchBrowser } from "./headless-launch.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function triggerDocument() {
  const [style, mosaic, chrome] = await Promise.all([
    readFile(resolve(root, "app/style.css"), "utf8"),
    readFile(resolve(root, "app/mosaic.css"), "utf8"),
    readFile(resolve(root, "app/chrome.css"), "utf8"),
  ]);
  return `<!doctype html><style>${style}\n${mosaic}\n${chrome}</style>
    <header class="commandbar commandbar--mosaic">
      <div class="canvas-disclosure" data-expanded="false">
        <nav class="task-pill-strip" aria-label="Open task projects">
          <button class="task-pill"><span>canvas</span><span class="task-pill-count">1</span></button>
        </nav>
        <details class="canvas-menu">
          <summary aria-label="Open Spice Canvas menu" aria-haspopup="menu" aria-expanded="false"
            title="Open Spice Canvas menu">
            <span class="canvas-menu-flame" aria-hidden="true">🔥</span>
            <span class="canvas-menu-pepper" aria-hidden="true">🌶️</span>
          </summary>
        </details>
      </div>
    </header>`;
}

async function appearance(trigger) {
  return trigger.evaluate((node) => {
    const box = (element) => {
      const rect = element.getBoundingClientRect();
      return { left: rect.left, top: rect.top, width: rect.width, height: rect.height };
    };
    const style = getComputedStyle(node);
    const stamp = node.querySelector(".canvas-menu-flame");
    const stampStyle = getComputedStyle(stamp);
    const flame = getComputedStyle(stamp, "::before");
    return {
      trigger: box(node),
      stamp: box(stamp),
      pepper: box(node.querySelector(".canvas-menu-pepper")),
      background: style.backgroundColor,
      border: style.borderColor,
      padding: [style.paddingTop, style.paddingRight, style.paddingBottom, style.paddingLeft],
      stampBackground: stampStyle.backgroundColor,
      flameAnimation: flame.animationName,
      flameOpacity: flame.opacity,
      accent: style.getPropertyValue("--canvas-menu-accent").trim(),
      warning: getComputedStyle(document.documentElement).getPropertyValue("--warning").trim(),
      name: node.getAttribute("aria-label"),
      hasTextLabel: Boolean(node.querySelector(".canvas-menu-label")),
    };
  });
}

function assertCutout(observed, label) {
  assert.deepEqual([observed.trigger.width, observed.trigger.height], [35, 30], `${label} outer size`);
  assert.deepEqual([observed.stamp.width, observed.stamp.height], [31, 25], `${label} stamp size`);
  assert.deepEqual(observed.padding, ["0px", "0px", "0px", "0px"], `${label} balanced padding`);
  assert.ok(Math.abs(observed.stamp.left - observed.trigger.left - 2) <= 0.5, `${label} left gutter`);
  assert.ok(Math.abs(observed.trigger.left + observed.trigger.width
    - observed.stamp.left - observed.stamp.width - 2) <= 0.5, `${label} right gutter`);
  assert.ok(Math.abs(
    observed.pepper.left + observed.pepper.width / 2
      - observed.stamp.left - observed.stamp.width / 2,
  ) <= 0.5, `${label} pepper centers in its stamp`);
  assert.equal(observed.name, "Open Spice Canvas menu");
  assert.equal(observed.hasTextLabel, false, `${label} remains wordless`);
}

test("Spice trigger stays centered across modes and interaction states", async (t) => {
  const document = await triggerDocument();
  for (const engine of ["chromium", "firefox"]) {
    await t.test(engine, async () => {
      const browser = await launchBrowser(engine);
      try {
        for (const colorScheme of ["light", "dark"]) {
          for (const width of [1200, 332]) {
            const context = await browser.newContext({ colorScheme, viewport: { width, height: 720 } });
            const page = await context.newPage();
            try {
              await page.setContent(document);
              const trigger = page.locator(".canvas-menu > summary");
              const label = `${engine} ${colorScheme} ${width}px`;
              const inactive = await appearance(trigger);
              assertCutout(inactive, `${label} inactive`);
              assert.equal(inactive.flameOpacity, "0");

              await trigger.hover();
              const hovered = await appearance(trigger);
              assertCutout(hovered, `${label} hovered`);
              assert.notDeepEqual([hovered.background, hovered.border], [inactive.background, inactive.border]);

              await trigger.focus();
              assert.ok(await trigger.evaluate((node) => node === document.activeElement));
              assertCutout(await appearance(trigger), `${label} focused`);

              await page.evaluate(() => { document.documentElement.style.fontSize = "32px"; });
              assertCutout(await appearance(trigger), `${label} zoomed`);
              await page.evaluate(() => { document.documentElement.style.fontSize = ""; });

              await page.mouse.move(0, 0);
              await trigger.evaluate((node) => node.blur());
              const settledInactive = await appearance(trigger);
              await trigger.evaluate((node) => node.classList.add("canvas-menu-trigger--fast"));
              const active = await appearance(trigger);
              assertCutout(active, `${label} fast`);
              assert.equal(active.accent, active.warning);
              assert.notDeepEqual(
                [active.background, active.border],
                [settledInactive.background, settledInactive.border],
              );
              assert.equal(active.flameAnimation, "canvas-fast-mode-pulse");

              await page.emulateMedia({ reducedMotion: "reduce" });
              const reduced = await appearance(trigger);
              assertCutout(reduced, `${label} reduced motion`);
              assert.equal(reduced.flameAnimation, "none");
              assert.equal(reduced.flameOpacity, "0.82");
              await page.emulateMedia({ reducedMotion: "no-preference" });

              await trigger.evaluate((node) => { node.parentElement.open = true; });
              const pressed = await appearance(trigger);
              assertCutout(pressed, `${label} pressed`);
              assert.notDeepEqual([pressed.background, pressed.border], [inactive.background, inactive.border]);
            } finally {
              await context.close();
            }
          }
        }
      } finally {
        await browser.close();
      }
    });
  }
});
