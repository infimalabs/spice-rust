import assert from "node:assert/strict";

import {
  chooseCanvasView,
} from "./canvas-fixture.mjs";

/// Exercise accepted, malformed, and absent task-note submissions through the UI.
export async function assertTaskNotes(page, task) {
  const content = page.locator("main.content");
  await chooseCanvasView(page, "Work");
  await content.getByLabel("Task").fill(task);
  await content.getByLabel("Observation").fill("browser mutation reached the resident");
  await content.getByRole("button", { name: "Add note" }).click();
  assert.equal(await content.getByLabel("Observation").inputValue(), "");
  await content.getByLabel("Task").fill("not a handle");
  await content.getByLabel("Observation").fill("the callback must explain this refusal");
  const addNote = content.getByRole("button", { name: "Add note" });
  assert.equal(await addNote.isEnabled(), true, "nonempty malformed input reaches the callback");
  await addNote.click();
  const refusal = content.getByRole("alert");
  await refusal.waitFor();
  assert.match(
    await refusal.innerText(),
    /Task must be .*eight- or nine-character canonical task stamp/,
  );
  assert.match(await refusal.innerText(), /"not a handle"/);
}
