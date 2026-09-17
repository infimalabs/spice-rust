import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { launchBrowser } from "./headless-launch.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const identityPattern = /(?:\b[0-9a-f]{8}\/[0-9a-f]{8}\b|\b[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\b)/i;

async function boundaryDocument() {
  const [style, mosaic] = await Promise.all([
    readFile(resolve(root, "app/style.css"), "utf8"),
    readFile(resolve(root, "app/mosaic.css"), "utf8"),
  ]);
  return `<!doctype html>
    <style>${style}\n${mosaic}
      body { margin: 0; padding: 12px; background: var(--page); }
      .boundary-fixture { width: 276px; }
      .boundary-fixture .lane-mosaic-plane { height: auto; }
      .boundary-fixture .mosaic-message-card { --mosaic-measure-span: 12; }
    </style>
    <main class="team-surface boundary-fixture">
      <div class="lane-mosaic-plane" data-measurement-phase="base">
        <article
          class="mosaic-message-card mosaic-compaction-card mosaic-lifecycle-card frozen"
          aria-label="Bexley Integration Reliability Operator lifecycle: runtime timed out; code -1"
          data-card-kind="lifecycle"
          data-layout-phase="measuring"
          data-accent="3"
          data-accent-slot="3"
        >
          <span class="mosaic-compaction-rule" aria-hidden="true"></span>
          <span class="mosaic-compaction-label">
            <span class="mosaic-compaction-age">48m</span>
            <span class="mosaic-compaction-context">
              <span class="mosaic-compaction-agent">Bexley Integration Reliability Operator</span>
              <span class="message-key">runtime timed out; code -1</span>
            </span>
          </span>
          <span class="mosaic-compaction-rule" aria-hidden="true"></span>
        </article>
      </div>
    </main>`;
}

test("retained lifecycle dividers preserve compact accent geometry in Chromium and Firefox", async (t) => {
  const document = await boundaryDocument();
  for (const engine of ["chromium", "firefox"]) {
    await t.test(engine, async () => {
      const browser = await launchBrowser(engine);
      const context = await browser.newContext({ colorScheme: "dark", viewport: { width: 320, height: 180 } });
      const page = await context.newPage();
      try {
        await page.setContent(document);
        const geometry = await page.locator('[data-card-kind="lifecycle"]').evaluate((boundary) => {
          const rect = (node) => {
            const box = node.getBoundingClientRect();
            return { left: box.left, right: box.right, width: box.width, height: box.height };
          };
          const color = (node) => getComputedStyle(node).color;
          const agent = boundary.querySelector(".mosaic-compaction-agent");
          const status = boundary.querySelector(".message-key");
          const label = boundary.querySelector(".mosaic-compaction-label");
          const accentProbe = document.createElement("span");
          accentProbe.style.color = "var(--accent-slot)";
          boundary.append(accentProbe);
          const accent = color(accentProbe);
          accentProbe.remove();
          const inkProbe = document.createElement("span");
          inkProbe.style.color = "var(--ink)";
          boundary.append(inkProbe);
          const ink = color(inkProbe);
          inkProbe.remove();
          return {
            card: { ...rect(boundary), clientWidth: boundary.clientWidth, scrollWidth: boundary.scrollWidth },
            plane: rect(boundary.parentElement),
            label: boundary.getAttribute("aria-label"),
            labelChildren: [...label.children].map((child) => child.className),
            labelGap: getComputedStyle(label).gap,
            agent: {
              ...rect(agent),
              clientWidth: agent.clientWidth,
              scrollWidth: agent.scrollWidth,
              color: color(agent),
              overflow: getComputedStyle(agent).overflow,
              textOverflow: getComputedStyle(agent).textOverflow,
            },
            status: { color: color(status), text: status.textContent.trim() },
            accent,
            ink,
            rules: [...boundary.querySelectorAll(".mosaic-compaction-rule")].map((rule) => ({
              ...rect(rule),
              background: getComputedStyle(rule).backgroundColor,
            })),
          };
        });

        assert.equal(geometry.card.width, geometry.plane.width, `${engine} divider is full width`);
        assert.ok(geometry.card.scrollWidth <= geometry.card.clientWidth + 1, `${engine} divider does not overflow`);
        assert.equal(geometry.rules.length, 2, `${engine} divider has two rules`);
        assert.deepEqual(
          [geometry.labelChildren, geometry.labelGap],
          [["mosaic-compaction-age", "mosaic-compaction-context"], "4.2px"],
          `${engine} age and lifecycle context share the internal text rhythm`,
        );
        assert.ok(geometry.rules.every((rule) => rule.width >= 32 && rule.height >= 1));
        assert.ok(Math.abs(geometry.rules[0].left - geometry.card.left) <= 0.5);
        assert.ok(Math.abs(geometry.rules[1].right - geometry.card.right) <= 0.5);
        assert.ok(geometry.rules.every((rule) => rule.background === geometry.accent));
        assert.equal(geometry.agent.color, geometry.accent, `${engine} agent name uses its exact accent`);
        assert.equal(geometry.status.color, geometry.ink, `${engine} cause uses normal ink`);
        assert.equal(geometry.status.text, "runtime timed out; code -1");
        assert.equal(geometry.agent.overflow, "hidden");
        assert.equal(geometry.agent.textOverflow, "ellipsis");
        assert.ok(
          geometry.agent.scrollWidth > geometry.agent.clientWidth + 1,
          `${engine} narrow divider truncates the long agent name`,
        );
        assert.doesNotMatch(geometry.label, identityPattern, `${engine} accessible cause exposes no identity ID`);
      } finally {
        await context.close();
        await browser.close();
      }
    });
  }
});
