import assert from "node:assert/strict";

import { chooseCanvasView, monitorPage } from "./canvas-fixture.mjs";
import { installSocketControl } from "./canvas-speech.mjs";

/// The shared clock's period, mirroring `TICK_MILLIS` in
/// `app/src/browser/clock.rs`. The assertion below reads a label that advances
/// on this cadence, so a drift here shows up as a timeout rather than as a
/// silently weaker test.
const TICK_MS = 1_000;

/// How long to wait for a label to cross a one-second boundary.
///
/// Four ticks rather than two: the wait starts at an arbitrary point inside a
/// second, and a machine under fleet load can lose most of a tick to scheduling
/// without the canvas being wrong about anything.
const BOUNDARY_WAIT_MS = 4 * TICK_MS;

/// How long the page must go without a delivery before the stall counts as
/// having taken hold. Half a tick, which is long enough to outlast a frame the
/// route forwarded just before it closed and short enough that the observation
/// still starts well inside the socket's tolerance for silence.
const QUIET_MS = 500;

/// How long to keep waiting for that quiet before calling the stall broken.
/// A gate that is not holding produces a steady stream, so this expires and
/// says so rather than letting the observation run against a live connection.
const SILENCE_DEADLINE_MS = 5_000;

/// Attributes that decide where a Mosaic card sits, and therefore the ones a
/// mere age tick must not touch. `style` carries the grid placement the plane
/// commits, so it belongs with the data attributes rather than beside them.
const PLACEMENT_ATTRIBUTES = ["data-track", "data-span", "data-base", "data-row", "data-rows", "style"];

/// Relative ages advance on a browser-local clock while the resident is silent.
///
/// The point under test is a negative one — that *nothing from the server* is
/// required — so the test has to make the silence real rather than hope for it.
/// A route holds every inbound frame in Node for the duration of the window and
/// counts what it held, so the page provably receives nothing while the label
/// is being watched, and the held frames are released afterwards so the
/// connection is never actually broken. The client tolerates 15s of silence
/// before it even probes (`LIVENESS_PROBE_MILLIS`), and this window is a few
/// seconds, so the stall stays well inside what the socket considers healthy.
///
/// One observation watches the composer's shared age and one stable assistant
/// card together, proving the label moved while the lattice did not.
export async function assertAgesTickWhileTheResidentIsSilent(browser, endpoint, lane) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  // The page-side frame counter below reads the socket the client actually
  // constructed, which only exists as a capturable object because this init
  // script wraps the constructor. It has to be installed on this context in
  // particular: the fixture installs it on the suite's context, and a fresh one
  // starts without it.
  await installSocketControl(context);
  const gate = { open: true, forwarded: 0, held: [], socket: null };
  const page = await context.newPage();
  const monitor = monitorPage(page);
  await page.routeWebSocket(/.*/, (socket) => {
    const server = socket.connectToServer();
    gate.socket = socket;
    server.onMessage((message) => {
      if (gate.open) {
        gate.forwarded += 1;
        socket.send(message);
        return;
      }
      gate.held.push(message);
    });
  });

  await page.goto(endpoint, { waitUntil: "domcontentloaded" });
  await chooseCanvasView(page, "Agents");
  const card = page
    .locator(
      `.mosaic-message-card[data-card-kind="agent-message"]`
        + `[data-agent-lane="${lane}"][data-layout-phase="committed"]`,
    )
    .first();
  await card.waitFor({ timeout: 15_000 });

  await countInboundFrames(page);
  gate.open = false;
  await settleIntoSilence(page);
  const advance = await observeOneBoundary(card, lane);
  const transport = await page.evaluate(() => ({
    delivered: window.__ageTickInbound,
    sockets: window.__canvasSockets.length,
    inbound: window.__canvasSockets.map((socket) => socket.__fixtureInboundFrames),
  }));
  Object.assign(transport, { forwarded: gate.forwarded, held: gate.held.length });
  await releaseHeldFrames(gate);
  assertAdvancedLocally(advance, transport);
  monitor.assertClean();
  await context.close();
}

/// Count frames the page actually receives, independently of the route.
///
/// The route already guarantees the stall, so this is the check on the check:
/// it reads the socket the client is really using, and a frame that reached the
/// page by any path the route does not see still lands here. Without it the
/// stall would be an argument about the harness rather than an observation, and
/// a label that advanced because a server frame arrived would be indis-
/// tinguishable from one that advanced on the clock.
async function countInboundFrames(page) {
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    window.__ageTickInbound = 0;
    socket.addEventListener("message", () => {
      window.__ageTickInbound += 1;
    });
  });
}

/// Wait for the stall to actually take hold, then start the count from zero.
///
/// Closing the gate stops Node forwarding, but a frame forwarded a moment
/// earlier can still be in flight, and it lands after the close — measured once
/// as exactly one frame arriving inside the window. Forgiving it would be wrong:
/// a frame delivered while the label moves is precisely what makes an advance
/// non-local, so the count has to keep counting those. What is wrong is opening
/// the window before the page is quiet. This waits for a stretch with no
/// delivery at all and only then zeroes the counter, so the observation begins
/// from a silence the page provably reached rather than one Node merely asked
/// for. A gate that never takes hold fails here, naming the count, instead of
/// looking like a label that would not move.
async function settleIntoSilence(page) {
  await page.evaluate(
    (config) => new Promise((resolve, reject) => {
      const socket = window.__canvasSockets.at(-1);
      if (!socket) {
        reject(new Error("canvas WebSocket was not captured"));
        return;
      }
      let quiet;
      const cleanup = () => {
        clearTimeout(quiet);
        clearTimeout(deadline);
        socket.removeEventListener("message", armQuiet);
      };
      const armQuiet = () => {
        clearTimeout(quiet);
        quiet = setTimeout(() => {
          cleanup();
          window.__ageTickInbound = 0;
          resolve();
        }, config.quietMs);
      };
      const deadline = setTimeout(() => {
        cleanup();
        reject(new Error(`frames kept arriving after the gate closed: ${window.__ageTickInbound}`));
      }, config.deadlineMs);
      socket.addEventListener("message", armQuiet);
      armQuiet();
    }),
    { quietMs: QUIET_MS, deadlineMs: SILENCE_DEADLINE_MS },
  );
}

/// Watch one age label cross a second boundary without letting go of the card.
///
/// Everything is read inside a single `evaluate` so the before and after
/// snapshots come from the same element instance: re-locating between them
/// would let a replaced card pass as a surviving one, which is exactly the
/// substitution the placement half of the clause is meant to catch.
async function observeOneBoundary(card, lane) {
  return card.evaluate(async (element, config) => {
    const age = document.querySelector(
      `.team-agent[data-agent-lane="${config.lane}"] .composer-shard-age-label[data-age-slot]`,
    );
    if (!age) throw new Error("the agent composer renders no age slot");
    const authored = Number(age.dataset.ageAuthored);
    if (!Number.isFinite(authored)) throw new Error("the age slot carries no authored instant");
    const snapshot = () => ({
      label: age.textContent?.trim(),
      box: roundBox(age.getBoundingClientRect()),
      cardBox: roundBox(element.getBoundingClientRect()),
      placement: Object.fromEntries(
        config.attributes.map((name) => [name, element.getAttribute(name)]),
      ),
    });
    const roundBox = (box) => ({
      x: Math.round(box.x),
      y: Math.round(box.y),
      width: Math.round(box.width),
      height: Math.round(box.height),
    });

    const writes = [];
    const record = (mutations) => {
      for (const mutation of mutations) {
        writes.push({
          attribute: mutation.attributeName,
          before: mutation.oldValue,
          after: element.getAttribute(mutation.attributeName),
        });
      }
    };
    const observer = new MutationObserver(record);
    observer.observe(element, {
      attributes: true,
      attributeOldValue: true,
      attributeFilter: config.attributes,
    });

    const before = snapshot();
    const startedAt = performance.now();
    await new Promise((resolve) => {
      let timeout;
      const labelObserver = new MutationObserver(() => {
        if (snapshot().label === before.label) return;
        clearTimeout(timeout);
        labelObserver.disconnect();
        resolve();
      });
      labelObserver.observe(age, { childList: true, characterData: true, subtree: true });
      timeout = setTimeout(() => {
        labelObserver.disconnect();
        resolve();
      }, config.waitMs);
    });
    const after = snapshot();
    const elapsed = performance.now() - startedAt;
    // `takeRecords` empties the queue whether or not anyone reads what it
    // returns, so discarding its result would delete exactly the evidence this
    // observation exists to collect: a placement write issued between the last
    // poll and this line, whose callback has not been dispatched yet.
    record(observer.takeRecords());
    observer.disconnect();

    return {
      before,
      after,
      elapsed,
      writes,
      authored,
      // What the page's own clock implies for the label just read, which is
      // what binds the observed text to a cause: a label that changed for any
      // other reason -- a re-render carrying stale text, a server frame that
      // slipped through -- lands outside this pair and fails rather than
      // counting as an advance.
      //
      // A pair rather than one string, because the label is the shared clock's
      // most recent publication and the render it triggers can be scheduled
      // behind it. Demanding sub-tick agreement would be demanding something
      // the design never promised; one tick is exactly the lag it does allow,
      // and a frozen label leaves even that window within two seconds.
      expected: [
        elapsedLabel(Date.now() - authored),
        elapsedLabel(Date.now() - config.tickMs - authored),
      ],
      connected: element.isConnected,
    };

    function elapsedLabel(millis) {
      const seconds = Math.trunc(millis / 1_000);
      if (seconds <= 0) return "now";
      if (seconds < 60) return `${seconds}s`;
      if (seconds < 3_600) return `${Math.trunc(seconds / 60)}m`;
      if (seconds < 86_400) return `${Math.trunc(seconds / 3_600)}h`;
      const days = Math.trunc(seconds / 86_400);
      return days <= 99 ? `${days}d` : "99d+";
    }
  }, {
    attributes: PLACEMENT_ATTRIBUTES,
    waitMs: BOUNDARY_WAIT_MS,
    tickMs: TICK_MS,
    lane,
  });
}

/// Let the stalled frames through and put the socket back in pass-through mode.
async function releaseHeldFrames(gate) {
  gate.open = true;
  for (const message of gate.held.splice(0)) gate.socket?.send(message);
}

function assertAdvancedLocally(advance, transport) {
  assert.equal(
    transport.delivered,
    0,
    `the page received no frame while the age advanced, so the advance is local: ${JSON.stringify(transport)}`,
  );
  assert.notEqual(
    advance.after.label,
    advance.before.label,
    `the age advanced within ${BOUNDARY_WAIT_MS}ms of silence: ${JSON.stringify(advance)}`,
  );
  assert.ok(
    advance.expected.includes(advance.after.label),
    `the advanced label is what the browser's own clock implies: ${JSON.stringify(advance)}`,
  );
  assert.ok(
    advance.elapsed <= BOUNDARY_WAIT_MS,
    `the advance arrived on the tick rather than late: ${JSON.stringify(advance)}`,
  );
  assert.equal(advance.connected, true, "the observed card is the one still in the document");
  assert.deepEqual(
    advance.after.box,
    advance.before.box,
    `the age slot keeps its bounding box across the tick: ${JSON.stringify(advance)}`,
  );
  assert.deepEqual(
    advance.after.cardBox,
    advance.before.cardBox,
    `the card keeps its bounding box across the tick: ${JSON.stringify(advance)}`,
  );
  assert.deepEqual(
    advance.after.placement,
    advance.before.placement,
    `the Mosaic placement attributes are unchanged: ${JSON.stringify(advance)}`,
  );
  assert.deepEqual(
    advance.writes,
    [],
    `a tick writes no placement attribute: ${JSON.stringify(advance.writes)}`,
  );
}
