/// Repeat the smallest cycle that reproduces the unobserved offline flip, and
/// time every status transition that cycle passes through on the way.
///
/// The canvas suite is one script whose full run is multi-minute work, so
/// repeating it cannot produce a distribution inside the time budget and a
/// handful of runs cannot justify a wall-clock bound. The failure lives in a
/// cycle of seconds -- start the resident, open the page, choose the view, stop
/// the resident, wait for offline -- so that cycle is what this harness drives,
/// from the same fixture module the suite uses, so a pass rate here describes
/// the suite's wait rather than a re-implementation of it.
///
/// The suite waits on three distinct transitions and they are not one thing
/// measured three times. A cold connect follows a navigation that has already
/// reached `networkidle`, so the page only has to finish its handshake. A live
/// reconnect follows the page closing its own socket while the resident keeps
/// serving, so the client's retry schedule governs it and nothing the page did
/// can shorten it. The offline flip follows a close the page already holds. One
/// bound quoted across all three would be justified by whichever happened to be
/// measured, so each is timed separately here and reported as its own
/// distribution.
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
///                                           [--view Agents|Chronology|Flow|Magnitude|Topology]
///
/// Pointing `--view` at Agents reproduces the reported incident, because the
/// mosaic has no status element for the wait to ever see.

import { mkdtemp, mkdir, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { once } from "node:events";

import { childExited } from "./canvas-child-state.mjs";
import { launchBrowser } from "./headless-launch.mjs";
import { installBrowserSpeech, installSocketControl } from "./canvas-speech.mjs";

import {
  buildResident,
  chooseCanvasView,
  configureFixture,
  describeConnection,
  describeResident,
  seedAuthority,
  startResident,
  stopResident,
} from "./canvas-fixture.mjs";

/// The views the cycle can be pointed at, which is the whole set the canvas has.
///
/// `Agents` is the mosaic and is the reason this is an option at all: `view.rs`
/// renders the topbar carrying `.status` only outside the mosaic, so a cycle
/// pointed there waits for an element that is not in the document. That is the
/// configuration that proved the reported failure was a deaf wait rather than a
/// slow transition, and a harness that cannot be pointed back at it cannot
/// demonstrate the failure it was built to diagnose.
const VIEWS = ["Agents", "Chronology", "Flow", "Magnitude", "Topology"];

function argument(name) {
  const index = process.argv.indexOf(name);
  return index === -1 ? null : { given: true, raw: process.argv[index + 1] };
}

function refuse(name, raw, expected) {
  const seen = raw === undefined ? "no value" : JSON.stringify(raw);
  throw new Error(`${name} needs ${expected}, got ${seen}`);
}

/// Read a numeric option, refusing anything that would silently measure nothing.
///
/// `Number(undefined)` and `Number("Agents")` are both NaN, and a NaN iteration
/// count makes the loop body unreachable while the closing comparison of zero
/// passes against zero results still exits 0. A harness whose entire product is
/// a pass rate must not be able to report success having run no cycles, so an
/// unusable value is refused here instead of being absorbed into a green.
function numericOption(name, fallback) {
  const given = argument(name);
  if (!given) return fallback;
  const value = Number(given.raw);
  if (!Number.isInteger(value) || value < 1) {
    refuse(name, given.raw, "a positive whole number");
  }
  return value;
}

function viewOption(name, fallback) {
  const given = argument(name);
  if (!given) return fallback;
  if (!VIEWS.includes(given.raw)) refuse(name, given.raw, `one of ${VIEWS.join(", ")}`);
  return given.raw;
}

const iterations = numericOption("--iterations", 20);
const bound = numericOption("--bound", 10_000);
const view = viewOption("--view", "Chronology");

/// Time one transition, carrying the instrument's view out when it never lands.
///
/// Every measured wait goes through here, so a timeout names the transition that
/// went unobserved rather than the selector that expired, and so no number in
/// the report can be quoted against a transition it was not taken from.
async function timed(page, resident, transition, run) {
  const started = Date.now();
  try {
    await run();
    return Date.now() - started;
  } catch (error) {
    throw Object.assign(new Error(`${transition} was never observed`), {
      report: {
        transition,
        elapsed: Date.now() - started,
        connection: await describeConnection(page),
        resident: await describeResident(resident),
        error: String(error).split("\n")[0],
      },
    });
  }
}

/// Run the cycle once and report what each wait saw.
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
    await chooseCanvasView(page, view);
    try {
      const connect = await timed(page, resident, "cold connect reports online", () =>
        page.waitForSelector(".status.online", { timeout: bound }),
      );
      await page.evaluate(() => {
        const socket = window.__canvasSockets.at(-1);
        if (!socket) throw new Error("canvas WebSocket was not captured");
        socket.close(4000, "cycle reconnect");
      });
      // The reconnect is timed from offline being on screen rather than from the
      // close, because that is where the suite's wait arms: it asserts the
      // offline copy first and only then waits to come back. Timing from the
      // close would charge this number for a flip the suite waits out under its
      // own bound, and would quote a reconnect bound nobody's wait experiences.
      await timed(page, resident, "the socket closing reports offline", () =>
        page.waitForSelector(".status.offline", { timeout: bound }),
      );
      const reconnect = await timed(page, resident, "live reconnect reports online", () =>
        page.waitForSelector(".status.online", { timeout: bound }),
      );
      await stopResident(fixture, resident);
      const started = Date.now();
      const elapsed = await timed(page, resident, "the resident stopping reports offline", () =>
        page.waitForSelector(".status.offline", { timeout: bound }),
      );
      // The companion wait shares the view gate and the close that drives it,
      // so it is measured here too rather than inheriting a bound justified
      // only against the status flip.
      await page.locator(".event.fault").waitFor({ timeout: bound });
      return { passed: true, connect, reconnect, elapsed, fault: Date.now() - started };
    } catch (error) {
      return { passed: false, ...(error.report ?? { error: String(error).split("\n")[0] }) };
    }
  } finally {
    await context.close();
    if (!childExited(resident.child)) {
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

const browser = await launchBrowser();
const results = [];
try {
  for (let index = 0; index < iterations; index += 1) {
    const result = await cycle(fixture, browser, index === 0);
    results.push(result);
    const detail = result.passed
      ? `connect ${result.connect}ms reconnect ${result.reconnect}ms offline ${result.elapsed}ms`
      : `FAILED on ${result.transition ?? "an unnamed step"} after ${result.elapsed}ms connection=${JSON.stringify(result.connection)} resident=${JSON.stringify(result.resident)}`;
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
    `pass rate: ${passed.length}/${results.length} at bound ${bound}ms in the ${view} view`,
    distribution("observed cold connect", "connect"),
    distribution("observed live reconnect", "reconnect"),
    distribution("observed offline flip", "elapsed"),
    distribution("observed fault event", "fault"),
    "",
  ].join("\n"),
);
process.exitCode = passed.length === results.length ? 0 : 1;
