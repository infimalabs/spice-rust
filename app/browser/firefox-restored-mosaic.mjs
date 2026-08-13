import assert from "node:assert/strict";
import { join } from "node:path";

import { assertMosaicLattice } from "./agent-conversation.mjs";
import { chooseCanvasView, monitorPage } from "./canvas-fixture.mjs";
import { LIVENESS_PROBE_MS } from "./client-deadlines.mjs";
import { launchBrowser } from "./headless-launch.mjs";

export async function assertFirefoxRestoredMosaic(endpoint, artifacts) {
  const firefox = await launchBrowser("firefox");
  const context = await firefox.newContext({
    colorScheme: "dark",
    viewport: { width: 1717, height: 1470 },
  });
  const page = await context.newPage();
  const monitor = monitorPage(page);
  try {
    await page.goto(endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    const console = page.locator('[data-pane="agent-console"]');
    await console.getByRole("tab", { name: /^Compose/ }).click();
    const before = await assertMosaicLattice(page, false);
    assert.ok(before.length > 0, "Firefox receives restored Mosaic cards");

    await page.reload({ waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    await console.getByRole("tab", { name: /^Compose/ }).click();
    const restored = await assertMosaicLattice(page, false);
    assert.equal(restored.length, before.length, "Firefox reload restores every Mosaic card");

    await page.waitForTimeout(LIVENESS_PROBE_MS + 500);
    const settled = await assertMosaicLattice(page, false);
    const settledKeys = new Set(settled.map((card) => card.key));
    assert.ok(
      restored.every((card) => settledKeys.has(card.key)),
      "Firefox retains every restored card after the liveness heartbeat",
    );
    assert.ok(
      settled.length >= restored.length,
      "the liveness heartbeat never removes a restored Firefox card",
    );
    await page.screenshot({ path: join(artifacts, "firefox-restored-mosaic.png"), fullPage: true });
    monitor.assertClean();
  } finally {
    await context.close();
    await firefox.close();
  }
}
