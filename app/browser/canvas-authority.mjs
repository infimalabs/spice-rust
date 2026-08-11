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
  assert.match(trustText, /trust::pended/);
  assert.match(trustText, /trust::granted/);
  assert.match(trustText, /granted/);
}

// Carry claims to a terminal so the fold has phases it can price.
//
// The fixture's own seeding never closes a claim, so without this the effort
// surface only ever shows phases it withheld -- and "no sizing reaches a
// withheld row" is satisfied for free by a surface that shows no sizing at all.
// The proven rows this creates are what make the controls below cost something.
//
// Several claims are carried rather than one because aggregates are grouped on
// the exact `(launch identity, phase)` pair, and one group would be satisfied
// by a key that had dropped either half. Every lane in this fixture launches
// from one `[vendor.claude]` section and there is no per-lane model override, so
// the identity half cannot be moved here; the phase half can, since closing a
// claim advances its task's flow and a later claim opens the phase it advanced
// to.
// Run one seeding under a driven lineage and put the lineage back afterwards.
//
// Allocation is gated three ways, all of which answer `board::allocator-empty`
// -- see the inventory on `assertHeldTaskCount`. The gate this owns is the
// lineage's life: `steer` admits only operator-pinned projects, and this fixture
// pins none, so the team is driven for exactly as long as the seeding takes.
// Both effort seedings need that window, and a driven lineage left behind would
// keep launching stand-ins under later assertions, so it is spelled once here
// rather than opened and closed by each of them.
export async function driveTeam(page) {
  await chooseCanvasView(page, "Agents");
  const team = page.locator('[data-pane="agent-console"]');
  const lifetime = team.locator('.team-lifetime input[type="range"]');
  await lifetime.fill("1");
  await team.getByRole("button", { name: "Drive", exact: true }).waitFor();
  return async () => {
    await lifetime.fill("0");
    await team.getByRole("button", { name: "Steer", exact: true }).waitFor();
  };
}
