import assert from "node:assert/strict";
import { access, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { drainShardQueue } from "./canvas-coordinator.mjs";

function deferred() {
  let resolve;
  const promise = new Promise((settle) => { resolve = settle; });
  return { promise, resolve };
}

test("a failed restart drains a live lease through its final discovery command", async () => {
  for (let repetition = 0; repetition < 24; repetition += 1) {
    const fixture = await mkdtemp(join(tmpdir(), "spice-canvas-concurrent-lifecycle-"));
    const discovery = join(fixture, "canvas");
    await writeFile(discovery, `lease-generation-${repetition}\n`);
    const leaseStarted = deferred();
    const restartFailed = deferred();
    const finalCommand = deferred();
    const releaseLease = deferred();
    const order = [];
    let reported = false;
    let queuedRan = false;

    const coordinated = drainShardQueue(
      [{ name: "lease" }, { name: "restart" }, { name: "queued" }],
      2,
      async ({ name }) => {
        if (name === "lease") {
          leaseStarted.resolve();
          await restartFailed.promise;
          assert.equal(
            await readFile(discovery, "utf8"),
            `lease-generation-${repetition}\n`,
            "the peer failure cannot remove the live lease discovery artifact",
          );
          order.push("lease-final-command");
          finalCommand.resolve();
          await releaseLease.promise;
          await rm(fixture, { recursive: true });
          order.push("lease-teardown");
          return name;
        }
        if (name === "restart") {
          await leaseStarted.promise;
          order.push("restart-failure");
          restartFailed.resolve();
          throw new Error(`restart ownership failure ${repetition}`);
        }
        queuedRan = true;
        return name;
      },
    );
    const outcome = coordinated.then(
      () => null,
      (error) => {
        reported = true;
        return error;
      },
    );

    await finalCommand.promise;
    await Promise.resolve();
    assert.equal(reported, false, "the coordinator has not reported while a peer is still owned");
    assert.equal(queuedRan, false, "no unstarted shard is assigned after the first failure");
    assert.deepEqual(order, ["restart-failure", "lease-final-command"]);
    releaseLease.resolve();
    const failure = await outcome;
    assert.match(failure.message, new RegExp(`restart ownership failure ${repetition}$`));
    assert.deepEqual(order, ["restart-failure", "lease-final-command", "lease-teardown"]);
    await assert.rejects(() => access(fixture), { code: "ENOENT" });
  }
});
