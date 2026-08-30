import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";

export const SHARD_FLAG = "--spice-shard";
export const SHARD_EVENT_PREFIX = "CANVAS-SHARD ";

const EXPECTED_SCENARIOS = [
  "launch-boundaries",
  "fixture-reaping",
  "light-readiness",
  "inline-task-chip",
  "agent-conversation",
  "firefox-restored-mosaic",
  "reference-composer",
  "authority-task-flow",
  "composer-lease",
  "variant-surfaces",
  "socket-resilience",
  "team-lifecycle",
  "resident-restart",
];

export const CANVAS_SHARDS = Object.freeze([
  Object.freeze({
    name: "ownership",
    scenarios: Object.freeze(["launch-boundaries", "fixture-reaping"]),
  }),
  Object.freeze({
    name: "readiness",
    scenarios: Object.freeze(["light-readiness"]),
  }),
  Object.freeze({
    name: "task-chip",
    scenarios: Object.freeze(["inline-task-chip"]),
  }),
  Object.freeze({
    name: "interaction",
    scenarios: Object.freeze([
      "agent-conversation",
      "firefox-restored-mosaic",
      "reference-composer",
      "authority-task-flow",
    ]),
  }),
  Object.freeze({
    name: "lease",
    // This shard crosses one production automatic-claim deadline. The rolling
    // lease itself consumes 100 seconds after the observed heartbeat, plus
    // restart catch-up under the ordinary browser load policy.
    timeoutMs: 180_000,
    scenarios: Object.freeze(["composer-lease"]),
  }),
  Object.freeze({
    name: "resilience",
    scenarios: Object.freeze([
      "variant-surfaces",
      "socket-resilience",
      "team-lifecycle",
    ]),
  }),
  Object.freeze({
    name: "restart",
    scenarios: Object.freeze(["resident-restart"]),
  }),
]);

/// Prove every named scenario belongs to exactly one independently runnable shard.
export function assertShardInventory(shards = CANVAS_SHARDS) {
  const names = shards.map((shard) => shard.name);
  assert.equal(new Set(names).size, names.length, "Canvas shard names are unique");
  const observed = shards.flatMap((shard) => shard.scenarios);
  assert.equal(
    new Set(observed).size,
    observed.length,
    "each Canvas scenario is registered exactly once",
  );
  assert.deepEqual(
    [...observed].sort(),
    [...EXPECTED_SCENARIOS].sort(),
    "the shard registry covers the complete Canvas scenario inventory",
  );
  assert.ok(shards.every((shard) => shard.scenarios.length > 0), "every shard owns a scenario");
}

/// Prove removal of a registration trips the same complete-inventory gate.
export function assertShardMutationGuard() {
  assert.throws(
    () => assertShardInventory(CANVAS_SHARDS.slice(1)),
    /complete Canvas scenario inventory/,
    "deleting one Canvas shard registration fails the named inventory assertion",
  );
}

export function shardNamed(name) {
  const shard = CANVAS_SHARDS.find((candidate) => candidate.name === name);
  assert.ok(shard, `Canvas shard is one of ${CANVAS_SHARDS.map(({ name }) => name).join(", ")}`);
  return shard;
}

export function emitShardEvent(event) {
  process.stdout.write(`${SHARD_EVENT_PREFIX}${JSON.stringify(event)}\n`);
}

/// Snapshot every process below this worker by exact parent relationships.
export function descendantPids(ancestor = process.pid) {
  const rows = execFileSync("ps", ["-axo", "pid=,ppid="], { encoding: "utf8" })
    .trim().split("\n").map((row) => row.trim().split(/\s+/).map(Number));
  const descendants = [];
  const frontier = [ancestor];
  while (frontier.length > 0) {
    const parent = frontier.shift();
    for (const [pid, ppid] of rows) {
      if (ppid === parent && !descendants.includes(pid)) {
        descendants.push(pid);
        frontier.push(pid);
      }
    }
  }
  return descendants;
}

/// Bind one worker invocation to the registry and account every scenario.
export function startShardWorker(arguments_) {
  assert.equal(arguments_.length, 2, `expected ${SHARD_FLAG} NAME`);
  assert.equal(arguments_[0], SHARD_FLAG, `expected ${SHARD_FLAG} NAME`);
  const shard = shardNamed(arguments_[1]);
  const started = Date.now();
  const cpu = process.cpuUsage();
  let next = 0;
  return {
    shard,
    owns: (name) => shard.name === name,
    async scenario(name, action) {
      assert.equal(name, shard.scenarios[next], `${shard.name} follows its scenario inventory`);
      emitShardEvent({ kind: "scenario-start", shard: shard.name, scenario: name });
      const scenarioStarted = Date.now();
      await action();
      emitShardEvent({
        kind: "scenario-end",
        shard: shard.name,
        scenario: name,
        elapsedMs: Date.now() - scenarioStarted,
      });
      next += 1;
    },
    finish() {
      assert.equal(next, shard.scenarios.length, `${shard.name} executes every scenario once`);
      const used = process.cpuUsage(cpu);
      emitShardEvent({
        kind: "summary",
        shard: shard.name,
        elapsedMs: Date.now() - started,
        cpuMs: Math.round((used.user + used.system) / 1_000),
      });
    },
  };
}
