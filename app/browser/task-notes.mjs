import assert from "node:assert/strict";

import { chooseCanvasView } from "./canvas-fixture.mjs";

/// Exercise accepted, malformed, and absent task-note submissions through the UI.
export async function assertTaskNotes(page, task) {
  await chooseCanvasView(page, "Flow");
  await page.getByLabel("Task").fill(task);
  await page.getByLabel("Observation").fill("browser mutation reached the resident");
  await page.getByRole("button", { name: "Add note" }).click();
  await chooseCanvasView(page, "Chronology");
  await page.getByText(/Note appended to/).waitFor({ timeout: 10_000 });
  await chooseCanvasView(page, "Flow");
  await page.getByLabel("Task").fill("not a handle");
  await page.getByLabel("Observation").fill("the callback must explain this refusal");
  const addNote = page.getByRole("button", { name: "Add note" });
  assert.equal(await addNote.isEnabled(), true, "nonempty malformed input reaches the callback");
  await addNote.click();
  const refusal = page.getByRole("alert");
  await refusal.waitFor();
  assert.match(await refusal.innerText(), /Task must be .*both parts alphanumeric/);
  assert.match(await refusal.innerText(), /"not a handle"/);
  const tasks = await page.locator("#note-tasks option").evaluateAll(
    (options) => options.map((option) => option.value),
  );
  const absent = ["SPICE-aBcD", "SPICE-zYxW"].find((candidate) => !tasks.includes(candidate));
  assert.ok(absent, "an absent canonical task handle is available");
  await page.getByLabel("Task").fill(absent);
  await page.getByLabel("Observation").fill("this task does not exist");
  await addNote.click();
  await refusal.waitFor({ state: "detached" });
  await chooseCanvasView(page, "Chronology");
  await page.locator(".event.refusal").waitFor({ timeout: 10_000 });
}
