import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { access, mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { VENDOR_PID_LEDGER } from "./canvas-fixture.mjs";
import { ownFixture } from "./canvas-lifecycle.mjs";

test("concurrent fixture cleanup callers join the complete teardown", async () => {
  const fixture = await mkdtemp(join(tmpdir(), "spice-canvas-cleanup-join-"));
  const linkedRoot = await mkdtemp(join(tmpdir(), "spice-canvas-cleanup-lanes-"));
  const stopStarted = Promise.withResolvers();
  const allowStop = Promise.withResolvers();
  const child = new EventEmitter();
  child.exitCode = null;
  child.signalCode = null;
  child.kill = () => assert.fail("the completed graceful stop must not escalate");
  const owner = ownFixture({
    fixture,
    linkedRoot,
    stop: async () => {
      stopStarted.resolve();
      await allowStop.promise;
      child.exitCode = 0;
    },
  });
  owner.ownResident({ child });

  const first = owner.cleanup();
  await stopStarted.promise;
  let secondFinished = false;
  const second = owner.cleanup().then(() => { secondFinished = true; });
  await Promise.resolve();
  assert.equal(secondFinished, false, "a concurrent caller cannot report partial cleanup complete");
  await access(fixture);
  await access(linkedRoot);

  allowStop.resolve();
  await Promise.all([first, second]);
  await assert.rejects(() => access(fixture), { code: "ENOENT" });
  await assert.rejects(() => access(linkedRoot), { code: "ENOENT" });
  assert.equal(owner.cleaned, true, "the owner is complete only after its directories are gone");
});

test("fixture cleanup joins a recorded group after its leader exits", async () => {
  const fixture = await mkdtemp(join(tmpdir(), "spice-canvas-cleanup-group-"));
  const linkedRoot = await mkdtemp(join(tmpdir(), "spice-canvas-cleanup-group-lanes-"));
  const leader = 424_242;
  await writeFile(join(fixture, VENDOR_PID_LEDGER), `${leader}\n`);
  let leaderAlive = true;
  let childAlive = true;
  let settlementRounds = 0;
  let settledWhileChildAlive = null;
  let settledAfterChildExit = null;
  let pathsPresentDuringSettlement = false;
  const delivered = [];
  const processes = {
    kill(target, signal) {
      if (signal === 0) {
        if (target === -leader && childAlive) return;
        if (target === leader && leaderAlive) return;
        const error = new Error("fixture process target is absent");
        error.code = "ESRCH";
        throw error;
      }
      delivered.push({ target, signal });
      leaderAlive = false;
    },
    async settled(predicate) {
      settlementRounds += 1;
      settledWhileChildAlive = predicate();
      await access(fixture);
      await access(linkedRoot);
      pathsPresentDuringSettlement = true;
      childAlive = false;
      settledAfterChildExit = predicate();
      return settledAfterChildExit;
    },
  };
  const owner = ownFixture({ fixture, linkedRoot, processes });

  await owner.cleanup();

  assert.equal(settlementRounds, 1, "cleanup joins the group after graceful termination");
  assert.deepEqual(
    delivered,
    [{ target: -leader, signal: "SIGTERM" }],
    "teardown signals only the recorded process group before it settles",
  );
  assert.equal(leaderAlive, false, "the recorded leader exits during termination");
  assert.equal(settledWhileChildAlive, false, "leader exit alone does not settle its group");
  assert.equal(settledAfterChildExit, true, "the group settles after its final child exits");
  assert.equal(pathsPresentDuringSettlement, true, "owned paths remain until the group settles");
  await assert.rejects(() => access(fixture), { code: "ENOENT" });
  await assert.rejects(() => access(linkedRoot), { code: "ENOENT" });
});
