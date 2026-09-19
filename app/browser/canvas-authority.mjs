import assert from "node:assert/strict";

import { chooseCanvasView } from "./canvas-fixture.mjs";

export async function assertAuthoritySurface(page, seeded) {
  await chooseCanvasView(page, "Work");
  const currentWork = page.locator(".section").filter({ hasText: "Current work" });
  await currentWork.getByText("Browser fixture task", { exact: true }).waitFor();
  await currentWork.getByText("Continuation witness", { exact: true }).waitFor();

  await chooseCanvasView(page, "Info");
  const topology = page.locator("main.content");
  await topology.getByText("Resident topology", { exact: true }).waitFor();
  await topology.getByText("claude", { exact: true }).first().waitFor();
  assert.match(await topology.innerText(), new RegExp(seeded.lane.slice(0, 8)));
}
