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
    <div class="lineage-fleet">${surface(1)}${surface(4)}</div>
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
    const surfaces = [...fleet.children].map((surface) => {
      const rect = surface.getBoundingClientRect();
      const clipped = [...surface.querySelectorAll("button, textarea, table")].some((control) => {
        const controlRect = control.getBoundingClientRect();
        return controlRect.left < rect.left - 1 || controlRect.right > rect.right + 1;
      });
      return {
        top: Math.round(rect.top),
        width: rect.width,
        height: rect.height,
        contained: rect.left >= bounds.left - 1 && rect.right <= bounds.right + 1,
        overflow: surface.scrollWidth - surface.clientWidth,
        clipped,
      };
    });
    return {
      rows: new Set(surfaces.map(({ top }) => top)).size,
      height: bounds.height,
      overflowX: fleet.scrollWidth - fleet.clientWidth,
      overflowY: fleet.scrollHeight - fleet.clientHeight,
      surfaces,
    };
  });
}

function assertReadable(layout, label) {
  assert.ok(layout.overflowX <= 1, `${label} fleet has no horizontal overflow`);
  assert.ok(layout.surfaces.every(({ width, contained, overflow, clipped }) => (
    width >= 512 && contained && overflow <= 1 && !clipped
  )), `${label} surfaces remain readable: ${JSON.stringify(layout)}`);
  if (layout.rows > 1) {
    assert.ok(layout.overflowY > 0, `${label} wrapped rows use the fleet scroller`);
    assert.ok(layout.surfaces.every(({ height }) => height >= layout.height - 1));
  }
}

async function expectRows(page, width, rows) {
  await setWidth(page, width);
  const layout = await geometry(page);
  assert.equal(layout.rows, rows, `${width}px row count`);
  assertReadable(layout, `${width}px`);
  return layout;
}

async function setCount(page, count) {
  await page.locator(".lineage-fleet").evaluate((fleet, nextCount) => {
    while (fleet.childElementCount > nextCount) fleet.lastElementChild.remove();
    while (fleet.childElementCount < nextCount) {
      fleet.append(fleet.firstElementChild.cloneNode(true));
    }
  }, count);
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
        await expectRows(page, 932, 2);
        await setWidth(page, 1200);
        const weighted = await geometry(page);
        assert.ok(weighted.surfaces[0].width >= 512 && weighted.surfaces[1].width > 512);
        assertReadable(weighted, `${engine} weighted row`);

        for (const count of [2, 3, 4, 7]) {
          await setCount(page, count);
          const boundary = count * 512 + (count - 1) * 8;
          await expectRows(page, boundary - 1, 2);
          await expectRows(page, boundary, 1);
          await expectRows(page, boundary + 1, 1);
        }

        await setCount(page, 2);
        await page.setViewportSize({ width: 900, height: 700 });
        await page.locator(".mosaic-workspace").evaluate((workspace) => {
          workspace.style.width = "900px";
          workspace.style.height = "auto";
          workspace.querySelector(".lineage-fleet").style.height = "auto";
        });
        const documentFlow = await page.locator(".lineage-fleet").evaluate((fleet) => ({
          direction: getComputedStyle(fleet).flexDirection,
          wrap: getComputedStyle(fleet).flexWrap,
          overflowY: getComputedStyle(fleet).overflowY,
        }));
        assert.deepEqual(documentFlow, { direction: "column", wrap: "nowrap", overflowY: "visible" });
      } finally {
        await context.close();
        await browser.close();
      }
    });
  }
});
