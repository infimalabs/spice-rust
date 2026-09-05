import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { launchBrowser } from "./headless-launch.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

async function responsiveDocument() {
  const [style, mosaic, responsive] = await Promise.all([
    readFile(resolve(root, "app/style.css"), "utf8"),
    readFile(resolve(root, "app/mosaic.css"), "utf8"),
    readFile(resolve(root, "app/responsive.css"), "utf8"),
  ]);
  return `<!doctype html><style>${style}\n${mosaic}\n${responsive}
    body { margin: 0; }
    .mosaic-workspace { width: 932px; height: 600px; }
    .mosaic-workspace .content { padding: 0; }
    .lineage-fleet { height: 600px; }
  </style><main class="mosaic-workspace"><div class="content">
    <div class="lineage-fleet" tabindex="0" aria-label="Lineage lanes">${surface(1)}${surface(4)}</div>
  </div></main>`;
}

function surface(weight) {
  return `<section class="team-surface" data-lineage-weight="${weight}">
    <div class="team-toolbar"><button>Compose</button><button>Tasks</button></div>
    <textarea>draft</textarea><table><tbody><tr><td>Shared evidence</td><td>Readable value</td></tr></tbody></table>
  </section>`;
}

async function setWidth(page, width) {
  await page.locator(".mosaic-workspace").evaluate((element, value) => {
    element.style.width = `${value}px`;
  }, width);
  await page.evaluate(() => new Promise((resolveFrame) => requestAnimationFrame(resolveFrame)));
}

async function geometry(page) {
  return page.locator(".lineage-fleet").evaluate((fleet) => {
    const bounds = fleet.getBoundingClientRect();
    const style = getComputedStyle(fleet);
    const surfaces = [...fleet.children].map((surface) => {
      const rect = surface.getBoundingClientRect();
      const clipped = [...surface.querySelectorAll("button, textarea, table")].some((control) => {
        const controlRect = control.getBoundingClientRect();
        return controlRect.left < rect.left - 1 || controlRect.right > rect.right + 1;
      });
      return {
        top: Math.round(rect.top),
        left: rect.left,
        right: rect.right,
        width: rect.width,
        height: rect.height,
        direction: getComputedStyle(surface).direction,
        overflow: surface.scrollWidth - surface.clientWidth,
        clipped,
      };
    });
    return {
      rows: new Set(surfaces.map(({ top }) => top)).size,
      height: bounds.height,
      left: bounds.left,
      right: bounds.right,
      direction: style.direction,
      flexDirection: style.flexDirection,
      overflowStyleX: style.overflowX,
      overflowStyleY: style.overflowY,
      tabIndex: fleet.tabIndex,
      scrollLeft: fleet.scrollLeft,
      maxScroll: fleet.scrollWidth - fleet.clientWidth,
      overflowX: fleet.scrollWidth - fleet.clientWidth,
      overflowY: fleet.scrollHeight - fleet.clientHeight,
      pageOverflowX: document.documentElement.scrollWidth - innerWidth,
      surfaces,
    };
  });
}

function assertTrack(layout, label, overflow) {
  assert.equal(layout.rows, 1, `${label} remains one row`);
  assert.equal(layout.direction, "ltr");
  assert.equal(layout.flexDirection, "row");
  assert.equal(layout.overflowStyleX, "auto");
  assert.equal(layout.overflowStyleY, "hidden");
  assert.equal(layout.tabIndex, 0);
  assert.ok(layout.overflowY <= 1, `${label} has no vertical overflow`);
  assert.ok(layout.pageOverflowX <= 1, `${label} does not widen the page`);
  assert.ok(layout.surfaces.every(({ width, height, direction, overflow: inner, clipped }) => (
    width >= 512 && height <= layout.height + 1 && direction === "ltr" && inner <= 1 && !clipped
  )), `${label} surfaces remain readable: ${JSON.stringify(layout)}`);
  assert.ok(layout.surfaces.every((surface, index) => (
    index === 0 || layout.surfaces[index - 1].left < surface.left
  )), `${label} preserves DOM order west to east`);
  assert.equal(layout.maxScroll > 1, overflow, `${label} horizontal overflow`);
}

async function expectRightEdge(page, label, overflow = true) {
  await page.locator(".lineage-fleet").evaluate((fleet) => { fleet.scrollLeft = fleet.scrollWidth; });
  const layout = await geometry(page);
  assertTrack(layout, label, overflow);
  assert.ok(Math.abs(layout.scrollLeft - layout.maxScroll) <= 1, `${label} reaches the right edge`);
  assert.ok(
    Math.abs(layout.surfaces.at(-1).right - layout.right) <= 1,
    `${label} reveals newest lane: ${JSON.stringify(layout)}`,
  );
  return layout;
}

async function setCount(page, count) {
  await page.locator(".lineage-fleet").evaluate((fleet, nextCount) => {
    while (fleet.childElementCount > nextCount) fleet.lastElementChild.remove();
    while (fleet.childElementCount < nextCount) {
      fleet.append(fleet.firstElementChild.cloneNode(true));
    }
  }, count);
  await page.evaluate(() => new Promise((resolveFrame) => requestAnimationFrame(resolveFrame)));
}

test("lineage minima govern weighted fleet layout in Chromium and Firefox", async (t) => {
  const document = await responsiveDocument();
  for (const engine of ["chromium", "firefox"]) {
    await t.test(engine, async () => {
      const browser = await launchBrowser(engine);
      const context = await browser.newContext({ viewport: { width: 4000, height: 700 } });
      const page = await context.newPage();
      try {
        await page.setContent(document);
        await expectRightEdge(page, `${engine} initial two-lane track`);
        await setWidth(page, 1200);
        const weighted = await geometry(page);
        assert.ok(weighted.surfaces[0].width >= 512 && weighted.surfaces[1].width > 512);
        assertTrack(weighted, `${engine} weighted row`, false);

        for (const count of [2, 3, 7]) {
          await setCount(page, count);
          await setWidth(page, 932);
          await expectRightEdge(page, `${engine} ${count}-lane desktop track`);
        }

        await page.locator(".lineage-fleet").focus();
        await page.keyboard.press("ArrowLeft");
        await page.waitForTimeout(50);
        let traversed = await geometry(page);
        assert.ok(traversed.scrollLeft < traversed.maxScroll, `${engine} keyboard traverses west`);
        await page.locator(".lineage-fleet").evaluate((fleet) => { fleet.scrollLeft = fleet.scrollWidth; });
        await page.locator(".lineage-fleet").hover();
        await page.mouse.wheel(-300, 0);
        await page.waitForTimeout(50);
        traversed = await geometry(page);
        assert.ok(traversed.scrollLeft < traversed.maxScroll, `${engine} horizontal wheel traverses west`);

        await page.locator(".lineage-fleet").evaluate((fleet) => { fleet.scrollLeft = fleet.scrollWidth; });
        await setCount(page, 3);
        await expectRightEdge(page, `${engine} added lane remains at east edge`);
        await setCount(page, 2);
        await page.setViewportSize({ width: 700, height: 700 });
        await setWidth(page, 700);
        await expectRightEdge(page, `${engine} narrow track after removal and resize`);
      } finally {
        await context.close();
        await browser.close();
      }
    });
  }
});
