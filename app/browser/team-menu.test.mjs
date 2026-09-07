import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { captureCursorFailureArtifact } from "./team-menu.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

test("an empty task strip stays quiet without changing populated task pills", async () => {
  const [renderer, chrome] = await Promise.all([
    readFile(resolve(root, "app/src/browser/view/chrome.rs"), "utf8"),
    readFile(resolve(root, "app/chrome.css"), "utf8"),
  ]);

  assert.doesNotMatch(renderer, /No open tasks|task-pill-empty/);
  assert.doesNotMatch(chrome, /\.task-pill-empty\b/);
  assert.match(
    renderer,
    /<nav class="task-pill-strip" aria-label="Open task projects">\s*\{for task_pills\.into_iter\(\)\.map/,
    "the labeled strip stays mounted and renders only actual task pills",
  );
  assert.match(renderer, /class=\{classes!\(\s*"task-pill"/);
  assert.match(renderer, /<span class="task-pill-count">\{pill\.count\(\)\}<\/span>/);
});

test("cursor assertion failures capture their engine and viewport before rethrow", async () => {
  const captures = [];
  const page = {
    async screenshot(options) {
      captures.push(options);
    },
  };
  const failure = new Error("closed cursor count regressed");
  const path = "artifacts/canvas/canvas-menu-cursor-failure-firefox-332x720.png";

  await assert.rejects(
    captureCursorFailureArtifact(page, path, failure),
    (error) => error === failure,
  );
  assert.deepEqual(captures, [{ path, fullPage: false }]);
});
