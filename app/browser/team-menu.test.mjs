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

test("the theme authority resolves before styles and exposes one current preference path", async () => {
  const [index, style, renderer] = await Promise.all([
    readFile(resolve(root, "app/index.html"), "utf8"),
    readFile(resolve(root, "app/style.css"), "utf8"),
    readFile(resolve(root, "app/src/browser/view/team_menu.rs"), "utf8"),
  ]);
  const prepaint = "<script data-canvas-theme>";
  assert.ok(
    index.indexOf(prepaint) >= 0
      && index.indexOf(prepaint) < index.indexOf('<link data-trunk rel="css"'),
  );
  assert.match(index, /const key = "spice:canvas-theme";/);
  assert.match(index, /stored === "light" \|\| stored === "dark" \? stored : "system"/);
  assert.match(index, /if \(next === "system"\) window\.localStorage\.removeItem\(key\)/);
  assert.match(index, /media\.addEventListener\("change", \(\) => \{\s*if \(root\.dataset\.canvasTheme === "system"\)/);
  assert.match(style, /:root\[data-canvas-effective-theme="dark"\]/);
  assert.doesNotMatch(style, /@media \(prefers-color-scheme: dark\)/);
  assert.match(renderer, /\[\("light", "Light"\), \("system", "System"\), \("dark", "Dark"\)\]/);
});

test("the product version and retained action payload each keep one owner", async () => {
  const [renderer, connection, mailbox, browser, compatibility, chrome, responsive] = await Promise.all([
    readFile(resolve(root, "app/src/browser/view/team_menu.rs"), "utf8"),
    readFile(resolve(root, "app/src/browser/connection.rs"), "utf8"),
    readFile(resolve(root, "app/src/browser/action_mailbox.rs"), "utf8"),
    readFile(resolve(root, "app/src/browser.rs"), "utf8"),
    readFile(resolve(root, "app/browser/canvas-compatibility.mjs"), "utf8"),
    readFile(resolve(root, "app/chrome.css"), "utf8"),
    readFile(resolve(root, "app/responsive.css"), "utf8"),
  ]);

  assert.match(browser, /struct CanvasUpdate \{[\s\S]*loaded_generation: wire::Digest,/);
  assert.match(connection, /loaded_generation: loaded_generation\.clone\(\),/);
  const authored = connection.slice(
    connection.indexOf("async fn dispatch_authored_action"),
    connection.indexOf("async fn send_fast_mode"),
  );
  assert.match(connection, /struct BorrowedSteerSend<'a>[\s\S]*message: &'a str,[\s\S]*attachments: &'a \[DirectiveAttachment\]/);
  assert.match(connection, /struct BorrowedTaskNote<'a>[\s\S]*note: &'a str/);
  assert.doesNotMatch(authored, /message\.clone\(\)|attachments\.clone\(\)|note\.clone\(\)/);
  assert.doesNotMatch(connection, /DemandQueue|drain_demands|send_demand_as/);
  assert.match(mailbox, /in_flight: Option<InFlightAction>[\s\S]*topology_ids: HashSet<CanonicalUuid>/);
  assert.match(renderer, /if let Some\(update\) = update \{[\s\S]*<button[\s\S]*class="canvas-menu-version canvas-menu-version--update"/);
  assert.match(renderer, /<strong\s+class="canvas-menu-version"[\s\S]*tabindex="0"/);
  assert.match(renderer, /<span class="canvas-menu-version-label">\{label\}<\/span>/);
  assert.match(renderer, /<span class="canvas-menu-version-indicator" aria-hidden="true">\{"↻"\}<\/span>/);
  assert.match(renderer, /"Reload Spice\{update_product\} to update;/);
  for (const detail of [
    "Current product:",
    "Loaded app generation:",
    "Available Canvas protocol:",
    "Served app generation:",
    "Action: Reload to update",
  ]) assert.ok(renderer.includes(detail), `update details include ${detail}`);
  assert.match(renderer, /data-canvas-update-version=\{update\.version\.to_string\(\)\}/);
  assert.match(renderer, /data-canvas-update-generation=\{update\.generation\.to_string\(\)\}/);
  assert.doesNotMatch(renderer, /class="canvas-menu-update"|Update \{generation\}/);
  assert.doesNotMatch(chrome, /\.canvas-menu-update\b/);
  assert.match(chrome, /\.canvas-menu-footer:has\(\.canvas-menu-version--update\) \{\s*grid-template-columns: minmax\(min-content, 1fr\) minmax\(0, auto\);/);
  assert.match(chrome, /\.canvas-menu-version--update \{[\s\S]*display: inline-flex;[\s\S]*flex: 0 1 auto;[\s\S]*max-width: 100%;/);
  assert.match(chrome, /\.canvas-menu-version-label \{[\s\S]*min-width: 0;[\s\S]*text-overflow: ellipsis;/);
  assert.match(chrome, /\.canvas-menu-version-indicator \{[\s\S]*flex: 0 0 auto;/);
  assert.match(responsive, /\.canvas-menu-version--update \{ padding-inline: 5px; \}/);
  assert.match(compatibility, /button\.canvas-menu-version--update/);
  assert.doesNotMatch(compatibility, /\.canvas-menu-update\b|Reload Spice .* app/);
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
