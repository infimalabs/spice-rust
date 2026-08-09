// A real Canvas fixture, run as its own process so it can be interrupted.
//
// This is the subject of `canvas-teardown.mjs`, not a test itself. It is a
// separate process because the defect it exists to catch is only reachable
// that way: the question is what survives when the process running a fixture
// is stopped, and a test cannot stop itself and still observe the result.
//
// It builds the fixture through exactly the production helpers the browser
// suite uses -- `configureFixture`, `ownFixture`, `startResident` -- so that a
// regression in the shared teardown boundary shows up here. A hand-rolled
// imitation of a fixture would prove only that the imitation cleans up.
import assert from "node:assert/strict";
import { mkdtemp, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { configureFixture, reply, startResident } from "./canvas-fixture.mjs";
import { launchBrowser } from "./headless-launch.mjs";
import { ownFixture, recordedVendorPids } from "./canvas-lifecycle.mjs";
import { descendantPids } from "./canvas-shards.mjs";

/// How the run ends, chosen by the parent to cover every teardown path.
///
/// `idle` waits to be signalled and `fail` raises after launching a vendor:
/// both hand a live fixture to the parent first. `mute` reaches the same live
/// state and then withholds the announcement, which is the one case where the
/// parent never takes the fixture over and the child is still the parent's to
/// end. Keeping it a mode of *this* harness rather than a stand-in is what
/// makes that case run against a real resident and a real stand-in.
const mode = process.argv[2];
assert.ok(
  ["idle", "fail", "mute"].includes(mode),
  `harness mode is idle, fail or mute, not ${mode}`,
);

/// Longest the stand-in is given to launch and enrol itself.
const LAUNCH_MS = 30_000;

/// Deliberate failure raised after the vendor is confirmed live.
///
/// Text the parent matches on, so the failure path is proven to report the
/// scenario's own failure rather than anything teardown raised over it.
const INJECTED = "injected harness failure after vendor launch";

const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-teardown-")));
const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-teardown-lanes-")));
const owner = ownFixture({ fixture, linkedRoot });

/// Keep the event loop alive while the parent decides how to end this run.
///
/// A bare unresolved promise would not: with no referenced handle pending, Node
/// considers the loop drained and exits cleanly, which would make the interrupt
/// this harness exists to receive arrive at a process that had already gone.
/// The same tick also notices a parent that has already gone. The absolute
/// value matters: a harness first observed after reparenting already reads 1,
/// so comparing against an earlier sample would miss that state forever.
let endIdleWait = () => {};
const heartbeat = setInterval(() => {
  if (process.ppid === 1) endIdleWait();
}, 1_000);

let browser;
try {
  const worktrees = await configureFixture(fixture, linkedRoot, 1);
  owner.ownResident(await startResident(fixture));
  const pending = await reply(fixture, ["trust"]);
  await reply(fixture, ["trust", "grant", pending.body.digest]);
  await reply(fixture, ["lane", "add", worktrees[0], "--life", "Drive", "--vendor", "claude"]);

  // Report only pids that are confirmed live. Announcing a pid the fixture had
  // merely written down would let the parent "prove" a reap against a process
  // that was never running, which is the one way this whole regression could
  // pass while the leak it targets survived untouched.
  const deadline = Date.now() + LAUNCH_MS;
  let vendors = [];
  while (Date.now() < deadline) {
    vendors = (await recordedVendorPids(fixture)).filter((pid) => {
      try {
        process.kill(pid, 0);
        return true;
      } catch (error) {
        return error.code === "EPERM";
      }
    });
    if (vendors.length > 0) break;
    await new Promise((wake) => setTimeout(wake, 50));
  }
  assert.ok(vendors.length > 0, "the fixture launched at least one vendor stand-in");
  const beforeBrowser = new Set(descendantPids());
  browser = await launchBrowser();
  await new Promise((wake) => setTimeout(wake, 100));
  const firstBrowserSnapshot = new Set(
    descendantPids().filter((pid) => !beforeBrowser.has(pid)),
  );
  await new Promise((wake) => setTimeout(wake, 500));
  const browsers = descendantPids().filter(
    (pid) => firstBrowserSnapshot.has(pid) && !beforeBrowser.has(pid),
  );
  assert.ok(browsers.length > 0, "the fixture launched at least one browser descendant");
  // The resident is named alongside its occupants because it is the other half
  // of what leaks. A parent that only knew the stand-in pids could watch every
  // one of them go and still have left a `spice up` behind.
  const resident = owner.resident.child.pid;
  const announcement = JSON.stringify({
    ready: true,
    harness: process.pid,
    fixture,
    linkedRoot,
    resident,
    browsers,
    vendors,
  });
  if (mode === "mute") {
    // Announce where the parent collects diagnostics rather than where it reads
    // readiness, then close stdout so the parent's readiness race loses at the
    // exact moment this fixture is fully alive. That is the state the leak
    // needs, and the parent still learns what it has to account for.
    process.stderr.write(`${announcement}\n`);
    process.stdout.end();
  } else {
    process.stdout.write(`${announcement}\n`);
  }

  if (mode === "fail") throw new Error(INJECTED);
  // idle and mute: a live parent ends this process after reading the line. If
  // that parent is gone instead, the heartbeat resolves this same wait so the
  // fixture still crosses its normal finally boundary.
  await new Promise((resolve) => { endIdleWait = resolve; });
} finally {
  clearInterval(heartbeat);
  try {
    if (browser) await browser.close();
  } finally {
    await owner.cleanup();
  }
}
