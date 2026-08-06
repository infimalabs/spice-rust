/// Repeat the smallest cycle that reproduces the unobserved offline flip.
///
/// The canvas suite is one script whose full run is multi-minute work, so
/// repeating it cannot produce a distribution inside the time budget and a
/// handful of runs cannot justify a wall-clock bound. The failure lives in a
/// cycle of seconds -- start the resident, open the page, choose the view, stop
/// the resident, wait for offline -- so that cycle is what this harness drives,
/// from the same fixture module the suite uses, so a pass rate here describes
/// the suite's wait rather than a re-implementation of it.
///
/// Two things are recorded per iteration and both are load-bearing. A failure
/// carries the instrument's own report, whose shape names the cause: `rendered`
/// false with `mosaic` true is the layout hiding the status, `rendered` true
/// with a `status online` class is a page that never processed the close, and
/// `rendered` true with `status offline` is the flip having happened unseen. A
/// success carries how long the flip actually took, because a bound can only be
/// justified against the distribution of the thing it bounds.
///
/// Usage: node app/browser/offline-cycle.mjs [--iterations N] [--bound MS]

import { mkdtemp, mkdir, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { once } from "node:events";
import { chromium } from "playwright";

import {
  buildResident,
  chooseCanvasView,
  configureFixture,
  describeConnection,
  describeResident,
  installBrowserSpeech,
  installSocketControl,
  seedAuthority,
  startResident,
  stopResident,
} from "./canvas-fixture.mjs";

function option(name, fallback) {
  const index = process.argv.indexOf(name);
  return index === -1 ? fallback : Number(process.argv[index + 1]);
}

const iterations = option("--iterations", 20);
const bound = option("--bound", 10_000);

/// Run the cycle once and report what the wait saw.
///
/// The resident is stopped inside the measured region because that is the event
/// the page must observe; the elapsed time is taken from the stop returning, so
/// it measures the page's reaction and not the subprocess teardown.
async function cycle(fixture, browser, seed) {
  const resident = await startResident(fixture);
  // The incident's page carried seeded authority and a populated event stream.
  // A bare page gives the renderer almost nothing to do, which is precisely the
  // condition the starvation candidate needs in order not to appear. The seed
  // is written once and lives in the fixture's journal, so later residents
  // serve it without re-seeding, which the trust grant would refuse anyway.
  if (seed) await seedAuthority(fixture);
  const context = await browser.newContext({
    colorScheme: "light",
    viewport: { width: 1440, height: 900 },
  });
  await installBrowserSpeech(context);
  await installSocketControl(context);
  const page = await context.newPage();
  try {
    await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Chronology");
    await stopResident(fixture, resident);
    const started = Date.now();
    try {
      await page.waitForSelector(".status.offline", { timeout: bound });
      const elapsed = Date.now() - started;
      // The companion wait shares the view gate and the close that drives it,
      // so it is measured here too rather than inheriting a bound justified
      // only against the status flip.
      await page.locator(".event.fault").waitFor({ timeout: bound });
      return { passed: true, elapsed, fault: Date.now() - started };
    } catch (error) {
      return {
        passed: false,
        elapsed: Date.now() - started,
        connection: await describeConnection(page),
        resident: await describeResident(resident),
        error: String(error).split("\n")[0],
      };
    }
  } finally {
    await context.close();
    if (resident.child.exitCode === null) {
      const exited = once(resident.child, "exit");
      resident.child.kill("SIGKILL");
      await exited;
    }
  }
}

await buildResident();
const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-offline-cycle-")));
const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-offline-lanes-")));
await mkdir(join(fixture, "artifacts"), { recursive: true });
await configureFixture(fixture, linkedRoot);

const browser = await chromium.launch({ headless: true });
const results = [];
try {
  for (let index = 0; index < iterations; index += 1) {
    const result = await cycle(fixture, browser, index === 0);
    results.push(result);
    const detail = result.passed
      ? `passed in ${result.elapsed}ms`
      : `FAILED after ${result.elapsed}ms connection=${JSON.stringify(result.connection)} resident=${JSON.stringify(result.resident)}`;
    process.stdout.write(`cycle ${index + 1}/${iterations}: ${detail}\n`);
  }
} finally {
  await browser.close();
}

const passed = results.filter((result) => result.passed);
function distribution(label, field) {
  const values = passed.map((result) => result[field]).sort((left, right) => left - right);
  const at = (fraction) =>
    values.length ? values[Math.min(values.length - 1, Math.floor(values.length * fraction))] : null;
  return `${label}: min=${values[0] ?? "-"}ms p50=${at(0.5) ?? "-"}ms p90=${at(0.9) ?? "-"}ms max=${values[values.length - 1] ?? "-"}ms`;
}
process.stdout.write(
  [
    "",
    `pass rate: ${passed.length}/${results.length} at bound ${bound}ms`,
    distribution("observed offline flip", "elapsed"),
    distribution("observed fault event", "fault"),
    "",
  ].join("\n"),
);
process.exitCode = passed.length === results.length ? 0 : 1;
