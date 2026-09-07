import assert from "node:assert/strict";

import {
  chooseCanvasView,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

const NOTE_RESPONSE_MS = 10_000;

function absentHandleWithKnownStamp(handle, tasks) {
  const separator = handle.indexOf("-");
  assert.ok(separator > 0, `canonical task handle has a stem: ${handle}`);
  const stamp = handle.slice(separator + 1);
  const absent = ["SPICE", "CANVAS", "GATE", "TASK", "BOARD"]
    .map((stem) => `${stem}-${stamp}`)
    .find((candidate) => candidate !== handle && !tasks.includes(candidate));
  assert.ok(absent, "an absent canonical task handle is available");
  return absent;
}

/// Exercise accepted, malformed, and absent task-note submissions through the UI.
export async function assertTaskNotes(page, task) {
  const content = page.locator("main.content");
  await chooseCanvasView(page, "Flow");
  await content.getByLabel("Task").fill(task);
  await content.getByLabel("Observation").fill("browser mutation reached the resident");
  await content.getByRole("button", { name: "Add note" }).click();
  await chooseCanvasView(page, "Chronology");
  await content.getByText(/Note appended to/).waitFor({ timeout: 10_000 });
  await chooseCanvasView(page, "Flow");
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
  const tasks = await content.locator("#note-tasks option").evaluateAll(
    (options) => options.map((option) => option.value),
  );
  const absent = absentHandleWithKnownStamp(task, tasks);
  await content.getByLabel("Task").fill(absent);
  await content.getByLabel("Observation").fill("this task does not exist");
  await addNote.click();
  await refusal.waitFor({ state: "detached" });
  await chooseCanvasView(page, "Chronology");
  await waitForPage(
    page,
    "absent task note refusal is recorded",
    NOTE_RESPONSE_MS,
    () => [...document.querySelectorAll(".event.refusal strong")]
      .some((title) => title.textContent === "task::unknown"),
    undefined,
    {
      observed: () => page.evaluate(() =>
        [...document.querySelectorAll(".event")].map((event) => ({
          className: event.className,
          title: event.querySelector("strong")?.textContent,
          detail: event.querySelector("p")?.textContent,
        }))),
    },
  );
}
