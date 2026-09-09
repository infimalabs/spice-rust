import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { captureCursorFailureArtifact } from "./organization-menu.mjs";

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

test("global destinations mount in the shared header rather than the menu panel", async () => {
  const renderer = await readFile(resolve(root, "app/src/browser/view/chrome.rs"), "utf8");
  const header = renderer.slice(
    renderer.indexOf("pub(super) fn render_command_bar"),
    renderer.indexOf("fn render_navigation"),
  );
  const menu = renderer.slice(
    renderer.indexOf("fn render_canvas_menu"),
    renderer.indexOf("fn task_pills"),
  );

  assert.match(
    header,
    /<\/nav>\s*if \*\*render\.menu_open \{\s*\{render_navigation\(&render, runtime\)\}\s*\}\s*\{render_canvas_menu/,
    "the open-only destination strip is a sibling between task pills and the permanent trigger",
  );
  assert.doesNotMatch(
    menu,
    /canvas-menu-frame[\s\S]*render_navigation/,
    "the dropdown frame contains no duplicate destination strip",
  );
});

test("the shared shell gutter adds physical safe areas to its responsive base", async () => {
  const [chrome, mosaic, responsive] = await Promise.all([
    readFile(resolve(root, "app/chrome.css"), "utf8"),
    readFile(resolve(root, "app/mosaic.css"), "utf8"),
    readFile(resolve(root, "app/responsive.css"), "utf8"),
  ]);

  assert.match(
    chrome,
    /--canvas-shell-edge-start:\s*calc\(var\(--canvas-shell-edge-inline\) \+ env\(safe-area-inset-left, 0px\)\)/,
  );
  assert.match(
    chrome,
    /--canvas-shell-edge-end:\s*calc\(var\(--canvas-shell-edge-inline\) \+ env\(safe-area-inset-right, 0px\)\)/,
  );
  assert.doesNotMatch(chrome, /--canvas-shell-edge-(?:start|end):\s*max\(/);
  assert.match(responsive, /\.shell \{ --canvas-shell-edge-inline: 8px; \}/);
  for (const stylesheet of [chrome, mosaic]) {
    assert.match(
      stylesheet,
      /padding-inline:\s*var\(--canvas-shell-edge-start\) var\(--canvas-shell-edge-end\)/,
      "the header and workspace consume the same resolved shell edges",
    );
  }
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
