import assert from "node:assert/strict";

import { chooseCanvasView } from "./canvas-fixture.mjs";

export async function assertAuthoritySurface(page, seeded) {
  await chooseCanvasView(page, "Flow");
  const currentWork = page.locator(".section").filter({ hasText: "Current work" });
  await currentWork.getByText("Browser fixture task", { exact: true }).waitFor();
  await currentWork.getByText("Continuation witness", { exact: true }).waitFor();

  await chooseCanvasView(page, "Magnitude");
  await page.getByRole("heading", { name: "Consent budgets", exact: true }).waitFor();
  await page.getByText("browser", { exact: true }).waitFor();
  assert.match(await page.locator(".section").filter({ hasText: "Consent budgets" }).innerText(), /2\s+2/);

  await chooseCanvasView(page, "Topology");
  const topology = page.locator("main.content");
  await topology.getByText("Resident topology", { exact: true }).waitFor();
  await topology.getByText("claude", { exact: true }).first().waitFor();
  assert.match(await topology.innerText(), new RegExp(seeded.lane.slice(0, 8)));

  await chooseCanvasView(page, "Chronology");
  await page.getByText("browser direction survives streaming", { exact: true }).waitFor();
  const trust = page.locator('[data-pane="trust"]');
  await trust.waitFor();
  const trustText = await trust.innerText();
  // Fresh operator subscriptions begin live at their barrier rather than
  // replaying historical facts. The independent Trust subject still carries
  // the current durable head.
  assert.match(trustText, /0 transitions/);
  assert.match(trustText, /granted/);
}
