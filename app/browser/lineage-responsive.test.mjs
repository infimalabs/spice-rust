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
    return {
      direction: getComputedStyle(fleet).flexDirection,
      surfaces: [...fleet.children].map((surface) => {
        const rect = surface.getBoundingClientRect();
        return {
          width: rect.width,
          contained: rect.left >= bounds.left - 1 && rect.right <= bounds.right + 1,
          overflow: surface.scrollWidth - surface.clientWidth,
        };
      }),
    };
  });
}

async function expectDirection(page, width, direction) {
  await setWidth(page, width);
  assert.equal((await geometry(page)).direction, direction, `${width}px direction`);
}

test("lineage minima govern weighted fleet layout in Chromium and Firefox", async (t) => {
  const document = await responsiveDocument();
  for (const engine of ["chromium", "firefox"]) {
    await t.test(engine, async () => {
      const browser = await launchBrowser(engine);
      const context = await browser.newContext({ viewport: { width: 1800, height: 700 } });
      const page = await context.newPage();
      try {
        await page.setContent(document);
        const narrow = await geometry(page);
        assert.equal(narrow.direction, "column", `${engine} stacks at 932px`);
        assert.ok(narrow.surfaces.every(
          ({ width, contained, overflow }) => width >= 512 && contained && overflow <= 1,
        ));
        await expectDirection(page, 1031, "column");
        await expectDirection(page, 1032, "row");
        await expectDirection(page, 1033, "row");
        await setWidth(page, 1200);
        const weighted = await geometry(page);
        assert.ok(weighted.surfaces[0].width >= 512 && weighted.surfaces[1].width > 512);

        await page.locator(".lineage-fleet").evaluate((fleet) => {
          fleet.append(fleet.firstElementChild.cloneNode(true));
        });
        await expectDirection(page, 1551, "column");
        await expectDirection(page, 1552, "row");
        await expectDirection(page, 1553, "row");
      } finally {
        await context.close();
        await browser.close();
      }
    });
  }
});
