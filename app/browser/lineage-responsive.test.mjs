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
    <header class="team-toolbar">
      <span class="team-lights"><span class="team-light"></span></span>
      <nav class="team-mode-rail" role="tablist">
        ${["Compose", "Tasks", "Metrics", "Info"].map((label) => `<button class="team-mode">
          <span class="team-mode-label">${label}</span><span class="team-mode-badge">88</span>
        </button>`).join("")}
      </nav>
      <button class="team-menu" aria-label="Team details">•••</button>
    </header>
    <div class="composer-shard-heading">
      <span class="composer-shard-link">48m</span>
      <strong>Extraordinarily long configured agent identity on a long worktree branch</strong>
      <span class="composer-shard-image-indicators"></span><span></span>
    </div>
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
    const minimumToken = style.getPropertyValue("--lineage-surface-min-inline").trim();
    const surfaces = [...fleet.children].map((surface) => {
      const rect = surface.getBoundingClientRect();
      const surfaceStyle = getComputedStyle(surface);
      const toolbar = surface.querySelector(".team-toolbar");
      const identity = surface.querySelector(".composer-shard-heading strong");
      const clipped = [...surface.querySelectorAll("button, textarea, table")].some((control) => {
        const controlRect = control.getBoundingClientRect();
        return controlRect.left < rect.left - 1 || controlRect.right > rect.right + 1;
      });
      return {
        weight: Number(surface.dataset.lineageWeight),
        minWidth: Number.parseFloat(minimumToken) * Number.parseFloat(surfaceStyle.fontSize),
        minWidthToken: minimumToken,
        top: Math.round(rect.top),
        left: rect.left,
        right: rect.right,
        width: rect.width,
        height: rect.height,
        direction: surfaceStyle.direction,
        overflow: surface.scrollWidth - surface.clientWidth,
        clipped,
        toolbar: {
          height: toolbar.getBoundingClientRect().height,
          children: [...toolbar.children].map((child) => child.getBoundingClientRect().toJSON()),
        },
        identity: {
          clientWidth: identity.clientWidth,
          scrollWidth: identity.scrollWidth,
          overflow: getComputedStyle(identity).overflow,
          textOverflow: getComputedStyle(identity).textOverflow,
          whiteSpace: getComputedStyle(identity).whiteSpace,
        },
        labels: [...surface.querySelectorAll(".team-mode-label")].map((label) => ({
          box: label.getBoundingClientRect().toJSON(),
          button: label.parentElement.getBoundingClientRect().toJSON(),
          overflow: getComputedStyle(label).overflow,
          textOverflow: getComputedStyle(label).textOverflow,
          whiteSpace: getComputedStyle(label).whiteSpace,
        })),
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
  assert.ok(layout.surfaces.every(({ width, minWidth, minWidthToken, height, direction, overflow: inner, clipped }) => (
    width >= minWidth - 0.5
      && minWidthToken === "20em"
      && height <= layout.height + 1
      && direction === "ltr"
      && inner <= 1
      && !clipped
  )), `${label} surfaces remain readable: ${JSON.stringify(layout)}`);
  assert.ok(layout.surfaces.every((surface, index) => (
    index === 0 || layout.surfaces[index - 1].left < surface.left
  )), `${label} preserves DOM order west to east`);
  for (const surface of layout.surfaces) {
    assert.ok(
      surface.toolbar.height <= 47,
      `${label} toolbar does not grow: ${JSON.stringify(surface.toolbar)}`,
    );
    assert.ok(surface.toolbar.children.every((child, index, children) => (
      child.left >= surface.left - 0.5
        && child.right <= surface.right + 0.5
        && (index === 0 || children[index - 1].right <= child.left + 0.5)
    )), `${label} toolbar controls remain ordered and contained`);
    assert.ok(surface.labels.every(({ box, button, overflow: inner, textOverflow, whiteSpace }) => (
      box.left >= button.left - 0.5
        && box.right <= button.right + 0.5
        && inner === "hidden"
        && textOverflow === "ellipsis"
        && whiteSpace === "nowrap"
    )), `${label} mode labels compress inside their tabs`);
    assert.deepEqual(
      [surface.identity.overflow, surface.identity.textOverflow, surface.identity.whiteSpace],
      ["hidden", "ellipsis", "nowrap"],
      `${label} long identity owns controlled truncation`,
    );
    if (surface.width <= surface.minWidth + 1) {
      assert.ok(surface.identity.scrollWidth > surface.identity.clientWidth, `${label} identity truncates at floor`);
    }
  }
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

async function setWeights(page, weights) {
  await setCount(page, weights.length);
  await page.locator(".lineage-fleet").evaluate((fleet, values) => {
    [...fleet.children].forEach((surface, index) => {
      surface.dataset.lineageWeight = String(values[index]);
    });
  }, weights);
  await page.evaluate(() => new Promise((resolveFrame) => requestAnimationFrame(resolveFrame)));
}

function assertRatio(layout, expected, label) {
  const ratio = layout.surfaces[1].width / layout.surfaces[0].width;
  assert.ok(
    Math.abs(ratio - expected) <= expected * 0.04,
    `${label} renders ${expected}:1 within four percent: ${JSON.stringify(layout)}`,
  );
  assert.equal(layout.overflowX <= 1, true, `${label} does not overflow the fleet`);
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
        await setWidth(page, 2560);
        for (const [weights, expected] of [[[1, 1], 1], [[1, 2], 2], [[1, 6], 6]]) {
          await setWeights(page, weights);
          const weighted = await geometry(page);
          assertTrack(weighted, `${engine} ${expected}:1 weighted row`, false);
          assertRatio(weighted, expected, `${engine} ${expected}:1 weighted row`);
          assert.deepEqual(weighted.surfaces.map(({ weight }) => weight), weights);
        }

        await setWidth(page, 700);
        const floorActive = await geometry(page);
        assertTrack(floorActive, `${engine} floor-active weighted row`, false);
        assert.ok(floorActive.surfaces[0].width <= 321, `${engine} solo lane holds its floor`);
        assert.ok(floorActive.surfaces[1].width > floorActive.surfaces[0].width);

        await setWidth(page, 600);
        await expectRightEdge(page, `${engine} two-lane floor overflow`);

        await setWeights(page, [1]);
        await page.setViewportSize({ width: 280, height: 700 });
        await setWidth(page, 280);
        const subFloor = await expectRightEdge(page, `${engine} sub-floor viewport`);
        assert.equal(subFloor.surfaces[0].width, 320, `${engine} viewport cannot lower the lane floor`);

        await page.evaluate(() => { document.documentElement.style.fontSize = "20px"; });
        await setWeights(page, [1, 1]);
        await page.setViewportSize({ width: 700, height: 700 });
        await setWidth(page, 700);
        const zoomed = await expectRightEdge(page, `${engine} zoomed floor overflow`);
        assert.ok(zoomed.surfaces.every(({ width, minWidth }) => width === 400 && minWidth === 400));
        await page.evaluate(() => { document.documentElement.style.fontSize = ""; });

        await setWeights(page, [1, 3, 6]);
        await page.setViewportSize({ width: 932, height: 700 });
        await setWidth(page, 932);
        const threeLane = await expectRightEdge(page, `${engine} three weighted lane floors`);
        assert.deepEqual(threeLane.surfaces.map(({ weight }) => weight), [1, 3, 6]);

        for (const count of [3, 7]) {
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
        await setWeights(page, [1, 3, 6]);
        await expectRightEdge(page, `${engine} added lane remains at east edge`);
        await setCount(page, 2);
        await page.setViewportSize({ width: 700, height: 700 });
        await setWidth(page, 600);
        await expectRightEdge(page, `${engine} narrow track after removal and resize`);
      } finally {
        await context.close();
        await browser.close();
      }
    });
  }
});
