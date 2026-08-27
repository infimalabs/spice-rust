/// Repeat the smallest packed-history cycle that can expose a partial thread.
///
/// `assertPackedHistory` first waits for the complete facts backlog, then
/// changes to Topology, waits for only the first causal row, and immediately
/// requires the walk to say `root`. This harness performs only those waits on
/// repeated reloads of one page against one resident. The fixture seats one
/// lane because that creates the same multi-page chain used by the suite:
/// `15 -> 14 -> 13 -> 3 -> 2 -> 1` with root 1 and two rows per wire page.
///
/// The optional frame delay is diagnostic, not a replacement server. A
/// Playwright WebSocket route forwards every real message unchanged and in the
/// same direction, delaying only server-to-page delivery. That widens the
/// interval in which an admitted partial page has rendered and its requested
/// continuation has not. On every observed dangle the harness reads the real
/// resident's authoritative thread and waits for that same DOM to reach root,
/// distinguishing a partial render from a journal that genuinely lacks one.
///
/// BROWSER-1kKZtzzF measured 0/200 dangles without delay and 12/20 reads as
/// `dangling at 2` with a 150ms per-frame delay. In all 12, the concurrent
/// authority read reported root 1 and all seven rows, and the DOM reached root
/// 11.8-69.8ms later. A 500ms delay caught the earlier `dangling at 13`
/// boundary 5/5 times.
///
/// Historical signature, verbatim: seen once in four full canvas runs on
/// 2026-08-06 inside the developer xtask verifier at load average 17.55, and not
/// reproduced in three further runs including one at load average 21-28.
///
/// Usage: node app/browser/lineage-cycle.mjs [cycles] [frame-delay-ms]

import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtemp, realpath, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { childExited } from "./canvas-child-state.mjs";
import { launchBrowser } from "./headless-launch.mjs";
import {
  buildResident,
  chooseCanvasView,
  configureFixture,
  journalExtent,
  reply,
  seedAuthority,
  startResident,
  TRANSITION_MS,
  waitForPageFunction,
} from "./canvas-fixture.mjs";

const cycles = Number.parseInt(process.argv[2] ?? "100", 10);
assert.ok(Number.isInteger(cycles) && cycles > 0, "cycle count is a positive integer");
const frameDelayMs = Number.parseInt(process.argv[3] ?? "0", 10);
assert.ok(
  Number.isInteger(frameDelayMs) && frameDelayMs >= 0,
  "frame delay is a non-negative integer",
);
// A dangle at 13 needs at most two delayed continuation frames to reach root.
const rootSettleMs = TRANSITION_MS + frameDelayMs * 2;

await buildResident();
const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-lineage-cycle-")));
const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-lineage-linked-")));
await configureFixture(fixture, linkedRoot, 1);

let resident;
let browser;
const labels = new Map();
const dangling = [];
let baseline;
try {
  resident = await startResident(fixture);
  await seedAuthority(fixture);
  await reply(fixture, [
    "lane", "add", fixture, "--name", "Lineage", "--life", "Steer", "--vendor", "claude",
  ]);
  const extent = await journalExtent(fixture);
  baseline = (await reply(fixture, ["thread", String(extent.cursor)])).body;
  browser = await launchBrowser();
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const page = await context.newPage();
  if (frameDelayMs > 0) {
    await page.routeWebSocket(/.*/, (socket) => {
      const server = socket.connectToServer();
      server.onMessage((message) => {
        setTimeout(() => socket.send(message), frameDelayMs);
      });
    });
  }

  for (let cycle = 1; cycle <= cycles; cycle += 1) {
    await page.goto(resident.discovery.endpoint, { waitUntil: "domcontentloaded" });
    await chooseCanvasView(page, "Chronology");
    const journal = page.locator('[data-pane="commands"]');
    const facts = journal.locator(".event strong");
    await facts.nth(extent.facts - 1).waitFor();
    assert.equal(await facts.count(), extent.facts);
    await journal.locator('[data-subject="facts"]').allTextContents();
    await journal.locator('[data-subject="facts"] span').evaluateAll((nodes) => nodes.map(
      (node) => node.textContent?.trim(),
    ));
    await facts.evaluateAll((nodes) => nodes.map((node) => node.textContent));

    await chooseCanvasView(page, "Topology");
    const thread = page.locator('[data-pane="lineage"]');
    await thread.locator("tbody tr").first().waitFor();
    const count = thread.locator(".section-count");
    const label = await count.innerText();
    labels.set(label, (labels.get(label) ?? 0) + 1);
    if (label.startsWith("dangling at ")) {
      const observedAt = performance.now();
      const authority = await reply(fixture, ["thread", String(extent.cursor)]);
      await waitForPageFunction(
        page,
        "packed lineage reaches root",
        rootSettleMs,
        () => {
          const found = document.querySelector('[data-pane="lineage"] .section-count');
          return found?.textContent === "root";
        },
        undefined,
        { observed: () => count.innerText() },
      );
      dangling.push({
        cycle,
        label,
        authorityRoot: authority.body.root,
        authorityRows: authority.body.entries.rows.map((row) => row.seq),
        rootAfterMs: Math.round((performance.now() - observedAt) * 10) / 10,
      });
    }
  }
  await context.close();
} finally {
  if (browser) await browser.close();
  if (resident && !childExited(resident.child)) {
    const exited = once(resident.child, "exit");
    resident.child.kill("SIGKILL");
    await exited;
  }
  await rm(fixture, { recursive: true, force: true });
  await rm(linkedRoot, { recursive: true, force: true });
}

process.stdout.write(`${JSON.stringify({
  cycles,
  frameDelayMs,
  baseline: {
    root: baseline.root,
    rows: baseline.entries.rows.map((row) => ({ seq: row.seq, cause: row.cause })),
  },
  labels: Object.fromEntries(labels),
  dangling,
}, null, 2)}\n`);
