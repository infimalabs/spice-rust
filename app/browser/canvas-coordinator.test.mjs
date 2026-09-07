import assert from "node:assert/strict";
import { access, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { drainShardQueue, invocation } from "./canvas-coordinator.mjs";
import {
  assertShardInventory,
  CANVAS_BUILD_ACTION,
  CANVAS_SHARDS,
  canvasShardAction,
} from "./canvas-shards.mjs";

test("release 0.31.0 marker-only Canvas evidence is refused", () => {
  assert.throws(
    () => invocation(["--spice-coverage-receipt", "/tmp/marker"]),
    /marker-only evidence belongs to release 0\.31\.0.*canvas:build and canvas:shard:\*/,
  );
});

test("the canonical registry derives one build action and one action per shard", () => {
  assert.equal(CANVAS_BUILD_ACTION, "canvas:build");
  assert.deepEqual(
    CANVAS_SHARDS.map(({ name }) => canvasShardAction(name)),
    CANVAS_SHARDS.map(({ name }) => `canvas:shard:${name}`),
  );
  assert.throws(
    () => assertShardInventory(CANVAS_SHARDS.slice(1)),
    /complete Canvas scenario inventory/,
  );
});

test("default prebuilt explicit and queue execution admit one resident stack only", async () => {
  assert.equal(invocation([]).concurrency, 1);
  assert.equal(invocation(["--spice-prebuilt-app"]).concurrency, 1);
  assert.equal(invocation(["--spice-concurrency", "1"]).concurrency, 1);
  for (const refused of ["0", "2", "4", "not-a-number"]) {
    assert.throws(
      () => invocation(["--spice-concurrency", refused]),
      /must be an integer between 1 and 1/,
    );
  }

  let active = 0;
  let peak = 0;
  const completed = await drainShardQueue(
    [{ name: "first" }, { name: "second" }, { name: "third" }],
    invocation([]).concurrency,
    async ({ name }) => {
      active += 1;
      peak = Math.max(peak, active);
      await Promise.resolve();
      active -= 1;
      return name;
    },
  );
  assert.equal(peak, 1, "the default queue never overlaps resident/browser stacks");
  assert.deepEqual(completed, ["first", "second", "third"]);
});

test("speech and production media own fixed-horizon shards outside interaction", () => {
  const interaction = CANVAS_SHARDS.find(({ name }) => name === "interaction");
  const speech = CANVAS_SHARDS.find(({ name }) => name === "speech");
  const media = CANVAS_SHARDS.find(({ name }) => name === "media");
  assert.ok(interaction && speech && media, "interaction, speech, and media shards are registered");
  assert.equal(interaction.timeoutMs, 120_000);
  assert.equal(speech.timeoutMs, 120_000);
  assert.equal(media.timeoutMs, 120_000);
  assert.deepEqual(speech.scenarios, ["automatic-speech"]);
  assert.deepEqual(media.scenarios, ["media-exchange"]);
  assert.ok(!interaction.scenarios.includes("automatic-speech"));
  assert.ok(!interaction.scenarios.includes("media-exchange"));
});

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
