// Reference-viewport proof for the real Canvas fixture.
import assert from "node:assert/strict";
import { join } from "node:path";

import {
  agentNames,
  assertFourAgentDensity,
  assertMosaicLattice,
} from "./agent-conversation.mjs";
import { captureReferenceControls } from "./control-reference.mjs";
import { chooseCanvasView } from "./canvas-fixture.mjs";

export async function captureReferenceViewport(page, artifacts) {
  await chooseCanvasView(page, "Agents");
  const team = page.locator('[data-pane="agent-console"]');
  await team.getByRole("tab", { name: /^Info/ }).click();
  for (const name of agentNames) {
    await team.getByText(name, { exact: true }).waitFor();
  }
  await team.getByText("0 pending", { exact: true }).waitFor();
  await team.getByText("running", { exact: true }).first().waitFor();
  await page.screenshot({
    path: join(artifacts, "canvas-reference-agent-details-dark.png"),
    fullPage: false,
  });
  await team.getByRole("tab", { name: /^Compose/ }).click();
  await team.locator(".mosaic-message-card").nth(3).waitFor({ timeout: 10_000 });
  assert.equal(
    await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches),
    true,
    "the reference capture explicitly exercises dark mode",
  );
  await assertFourAgentDensity(team);
  assert.equal(
    await team.locator(".team-surface").getAttribute("data-lineage-weight"),
    "4",
    "the reference viewport retains the four-agent lineage weight",
  );
  await assertMosaicLattice(page, false);
  const pixels = await page.evaluate(() => ({
    width: innerWidth,
    height: innerHeight,
    background: getComputedStyle(document.body).backgroundColor,
    bodyWidth: document.body.scrollWidth,
  }));
  assert.deepEqual([pixels.width, pixels.height], [1000, 568]);
  assert.equal(
    await page.locator(".mosaic-workspace > .topbar").count(),
    0,
    "the live lineage has no redundant operator control bar",
  );
  assert.equal(pixels.bodyWidth, 1000, "the reference viewport has no horizontal overflow");
  assert.equal(pixels.background, "rgb(11, 15, 12)", "the captured surface uses the dark palette");
  await captureReferenceControls(
    page,
    team,
    join(artifacts, "canvas-reference-four-agent-dark.png"),
  );
}
