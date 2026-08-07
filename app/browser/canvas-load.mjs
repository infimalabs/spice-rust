// What the machine looked like at the moment a browser wait gave up.
//
// This exists because a red run could not answer the first question anyone asks
// of it: was the product wrong, or was the box busy. Eleven worktrees share this
// machine, so a load average several times the core count is ordinary here, and
// a reader looking at a bare timeout had no way to rule it in or out without
// re-running the suite -- by which time the load that produced it is gone.
//
// The measurements say load is very rarely the answer, which is exactly why the
// reading belongs in the report rather than in the bound. Driving
// offline-cycle.mjs, the smallest cycle that reproduces the reported flake, the
// offline flip runs 3ms at load average 26.8 on 12 cores and 78ms worst at load
// average 69.5 rising to 124.6 -- past the 85.52 that produced the failure this
// module was written for, and still twenty-five times inside the 2000ms bound,
// 3 of 3 passing. So a timeout on one of these waits is a defect or a tail
// event, and widening the bounds to cover load would have bought a green by
// making every genuine failure slower to notice, on every machine, forever.
//
// A reader who sees this line at factor 1 knows load is not the explanation. A
// reader who sees it far above 1 knows to ask for a second run before opening a
// regression. That distinction is the whole value, and it costs one line.
import assert from "node:assert/strict";
import { cpus, loadavg } from "node:os";

/// How far this machine is oversubscribed right now, never below one.
///
/// The one-minute average rather than the five, because it is the one that still
/// describes the machine the wait just ran on. Held at or above one so the
/// reading always reads as a multiplier of a machine that is keeping up.
function loadFactor() {
  const cores = cpus().length || 1;
  const [recent] = loadavg();
  return Math.max(1, recent / cores);
}

/// The load reading carried by a failed wait's report.
export function describeLoad() {
  const cores = cpus().length || 1;
  const [recent] = loadavg();
  return {
    loadAverage: Math.round(recent * 100) / 100,
    cores,
    factor: Math.round(loadFactor() * 100) / 100,
  };
}

/// The field that both writes the reading and recognises one already written.
///
/// One constant decides both sides. A change to the report's shape therefore
/// cannot leave the boundary appending a second copy under a name the
/// already-carries check no longer matches.
const LOAD_FIELD = "  load: ";

/// Carry the reading on a failure that reached the top of the suite without one.
///
/// `observedWait` reads the load for the waits routed through it, and almost
/// nothing is routed through it. canvas.spec.mjs makes 51 raw locator waits and
/// its helper modules make 43 more, none of them wrapped, and 34 of the spec's
/// name no bound at all and inherit Playwright's 30000ms default. Every one of
/// those raises a timeout naming only a selector, which is the ambiguity this
/// module exists to remove, at the call sites where it was never removed.
///
/// Wrapping them one at a time would cover the waits somebody remembered to
/// wrap. The suite is one script, so every rejection out of every one of those
/// modules unwinds to a single boundary instead; reading the load there covers
/// the waits nobody instrumented, including the ones not yet written.
///
/// Reading at the boundary rather than at the wait is sound because the reading
/// is a one-minute average: it does not move measurably while an error unwinds,
/// so the number is still the machine the wait actually ran on. A failure that
/// already carries a reading is returned untouched, because a second reading of
/// one moment would disagree with the first and there is no way to tell which
/// of the two a reader should believe.
export function withLoadReading(error) {
  const message = error instanceof Error ? error.message : String(error);
  if (message.includes(LOAD_FIELD)) return error;
  return new Error(`${message}\n${LOAD_FIELD}${JSON.stringify(describeLoad())}`, { cause: error });
}

/// Prove the boundary reads the load, reads all of it, and reads it once.
///
/// Deleting the catch at the foot of canvas.spec.mjs, or the append above,
/// returns all 94 of those waits to an unattributable bare timeout with the
/// whole suite still green. This is the assertion that makes either edit fail.
export function assertBoundaryLoadReading() {
  const bare = withLoadReading(new Error("canvas wait failed: bare locator timeout"));
  assert.match(bare.message, /load: \{"loadAverage":\d/, "a bare timeout gains the reading");
  assert.match(
    bare.message,
    /"cores":\d+,"factor":\d/,
    "the boundary carries the whole reading rather than its first field alone",
  );
  assert.match(bare.message, /^canvas wait failed: bare locator timeout$/m, "the original survives");
  assert.ok(bare.cause, "the timeout that reached the boundary survives as the cause");
  assert.equal(bare.message.match(/load: /g).length, 1, "the reading is written once");
  assert.equal(withLoadReading(bare), bare, "a failure already carrying a reading is left alone");
}
