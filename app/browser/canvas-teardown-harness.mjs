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
import { ownFixture, recordedVendorPids } from "./canvas-lifecycle.mjs";

/// How the run ends, chosen by the parent to cover both teardown paths.
const mode = process.argv[2];
assert.ok(["idle", "fail"].includes(mode), `harness mode is idle or fail, not ${mode}`);

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
const heartbeat = setInterval(() => {}, 1_000);

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
  process.stdout.write(`${JSON.stringify({ ready: true, fixture, linkedRoot, vendors })}\n`);

  if (mode === "fail") throw new Error(INJECTED);
  // idle: the parent signals this process once it has read the line above.
  await new Promise(() => {});
} finally {
  clearInterval(heartbeat);
  await owner.cleanup();
}
