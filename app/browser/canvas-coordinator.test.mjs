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
  SHARD_FLAG,
  startShardWorker,
} from "./canvas-shards.mjs";
import {
  assertTelemetryComplete,
  createTelemetryLedger,
  recordTelemetryEvent,
} from "./canvas-telemetry.mjs";

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
    /every scenario declared by the Canvas worker/,
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

test("nested shard phases emit and validate stable wall and CPU telemetry", async () => {
  const events = [];
  const shard = CANVAS_SHARDS[0];
  const run = startShardWorker([SHARD_FLAG, shard.name], (event) => events.push(event));
  await run.phase("shard-setup", async () => {});
  await run.scenario(shard.scenarios[0], () => run.phase("outer", () => (
    run.phase("inner", async () => {})
  )));
  await run.scenario(shard.scenarios[1], async () => {});
  run.finish();

  const ledger = createTelemetryLedger(shard.name);
  for (const event of events) recordTelemetryEvent(ledger, event);
  assertTelemetryComplete(ledger);
  assert.deepEqual(
    ledger.phases.map(({ phase, path, outcome }) => ({ phase, path, outcome })),
    [
      { phase: "shard-setup", path: ["shard-setup"], outcome: "passed" },
      { phase: "inner", path: ["outer", "inner"], outcome: "passed" },
      { phase: "outer", path: ["outer"], outcome: "passed" },
    ],
  );
  assert.ok(ledger.phases.every((event) => (
    Number.isSafeInteger(event.elapsedMs) && event.elapsedMs >= 0
      && Number.isSafeInteger(event.cpuMs) && event.cpuMs >= 0
  )));
  const start = events.find((event) => event.kind === "phase-start");
  const end = events.find((event) => event.kind === "phase-end");
  assert.deepEqual(Object.keys(start), ["kind", "shard", "scenario", "phase", "path"]);
  assert.deepEqual(
    Object.keys(end),
    ["kind", "shard", "scenario", "phase", "path", "outcome", "elapsedMs", "cpuMs"],
  );
  assert.deepEqual(
    Object.keys(events.find((event) => event.kind === "scenario-end")),
    ["kind", "shard", "scenario", "outcome", "elapsedMs", "cpuMs"],
  );
  assert.deepEqual(
    Object.keys(events.find((event) => event.kind === "summary")),
    ["kind", "shard", "elapsedMs", "cpuMs"],
  );

  const failedEvents = [];
  const failed = startShardWorker(
    [SHARD_FLAG, shard.name],
    (event) => failedEvents.push(event),
  );
  await assert.rejects(
    () => failed.scenario(shard.scenarios[0], () => failed.phase("broken", async () => {
      throw new Error("phase failure");
    })),
    /phase failure/,
  );
  assert.deepEqual(
    failedEvents.filter((event) => event.kind.endsWith("-end"))
      .map(({ kind, outcome }) => ({ kind, outcome })),
    [
      { kind: "phase-end", outcome: "failed" },
      { kind: "scenario-end", outcome: "failed" },
    ],
  );
});

test("coordinator telemetry rejects malformed nesting and unbalanced completion", () => {
  const outside = createTelemetryLedger("fixture");
  assert.throws(
    () => recordTelemetryEvent(outside, {
      kind: "phase-start",
      shard: "fixture",
      scenario: "scenario",
      phase: "outside",
      path: ["outside"],
    }),
    /active scenario/,
  );

  const open = createTelemetryLedger("fixture");
  recordTelemetryEvent(open, {
    kind: "scenario-start", shard: "fixture", scenario: "scenario",
  });
  recordTelemetryEvent(open, {
    kind: "phase-start",
    shard: "fixture",
    scenario: "scenario",
    phase: "outer",
    path: ["outer"],
  });
  recordTelemetryEvent(open, {
    kind: "phase-start",
    shard: "fixture",
    scenario: "scenario",
    phase: "inner",
    path: ["outer", "inner"],
  });
  assert.throws(
    () => recordTelemetryEvent(open, {
      kind: "phase-end",
      shard: "fixture",
      scenario: "scenario",
      phase: "outer",
      path: ["outer"],
      outcome: "passed",
      elapsedMs: 1,
      cpuMs: 0,
    }),
    /LIFO order/,
  );
  assert.throws(() => assertTelemetryComplete(open), /active scenario/);
  assert.throws(
    () => recordTelemetryEvent(createTelemetryLedger("fixture"), {
      kind: "summary", shard: "fixture", elapsedMs: -1, cpuMs: 0,
    }),
    /nonnegative safe integer/,
  );

  const closed = createTelemetryLedger("fixture");
  recordTelemetryEvent(closed, {
    kind: "summary", shard: "fixture", elapsedMs: 1, cpuMs: 0,
  });
  assert.throws(
    () => recordTelemetryEvent(closed, {
      kind: "scenario-start", shard: "fixture", scenario: "late",
    }),
    /terminal telemetry event/,
  );

  const failed = createTelemetryLedger("fixture");
  recordTelemetryEvent(failed, {
    kind: "scenario-start", shard: "fixture", scenario: "scenario",
  });
  recordTelemetryEvent(failed, {
    kind: "scenario-end",
    shard: "fixture",
    scenario: "scenario",
    outcome: "failed",
    elapsedMs: 1,
    cpuMs: 0,
  });
  recordTelemetryEvent(failed, {
    kind: "summary", shard: "fixture", elapsedMs: 1, cpuMs: 0,
  });
  assert.throws(() => assertTelemetryComplete(failed), /no failed scenario/);
});

test("production media owns a fixed-horizon shard outside interaction", () => {
  const interaction = CANVAS_SHARDS.find(({ name }) => name === "interaction");
  const media = CANVAS_SHARDS.find(({ name }) => name === "media");
  assert.ok(interaction && media, "interaction and media shards are registered");
  assert.equal(interaction.timeoutMs, 120_000);
  assert.equal(media.timeoutMs, 120_000);
  assert.deepEqual(media.scenarios, ["media-exchange"]);
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
