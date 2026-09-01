import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

import { buildResident } from "./canvas-fixture.mjs";
import { auditOwnership } from "./canvas-process-ownership.mjs";
import {
  assertShardInventory,
  assertShardMutationGuard,
  CANVAS_SHARDS,
  SHARD_EVENT_PREFIX,
  SHARD_FLAG,
} from "./canvas-shards.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const worker = join(here, "canvas.spec.mjs");
const COVERAGE_FLAG = "--spice-coverage-receipt";
const PREBUILT_APP_FLAG = "--spice-prebuilt-app";
const CONCURRENCY_FLAG = "--spice-concurrency";
const COVERAGE_RECEIPT = "BROWSER-1kKSWhZk browser coverage complete\n";
const SHARD_TIMEOUT_MS = 120_000;
const COORDINATOR_TIMEOUT_MS = 270_000;
const PROGRESS_MS = 30_000;
const TERMINATION_GRACE_MS = 30_000;
const RESIDENT_CONCURRENCY = 4;
const MAX_CONCURRENCY = 5;

function coveragePath(arguments_) {
  if (arguments_.length === 0) return null;
  assert.deepEqual(
    arguments_.slice(0, 1),
    [COVERAGE_FLAG],
    `expected ${COVERAGE_FLAG} FILE or no arguments`,
  );
  assert.equal(arguments_.length, 2, `expected ${COVERAGE_FLAG} FILE or no arguments`);
  assert.ok(arguments_[1] && !arguments_[1].startsWith("--"), "coverage receipt names a file");
  return arguments_[1];
}

function concurrencyFrom(arguments_) {
  const indexes = arguments_.flatMap((argument, index) =>
    argument === CONCURRENCY_FLAG ? [index] : []);
  assert.ok(indexes.length <= 1, `${CONCURRENCY_FLAG} is singular`);
  if (indexes.length === 0) {
    return { concurrency: RESIDENT_CONCURRENCY, remaining: arguments_ };
  }
  const index = indexes[0];
  const concurrency = Number(arguments_[index + 1]);
  assert.ok(
    Number.isSafeInteger(concurrency)
      && concurrency >= RESIDENT_CONCURRENCY
      && concurrency <= MAX_CONCURRENCY,
    `${CONCURRENCY_FLAG} must be an integer between ${RESIDENT_CONCURRENCY} and ${MAX_CONCURRENCY}`,
  );
  return {
    concurrency,
    remaining: arguments_.filter((_, candidate) => candidate !== index && candidate !== index + 1),
  };
}

function invocation(arguments_) {
  const prebuiltFlags = arguments_.filter((argument) => argument === PREBUILT_APP_FLAG);
  assert.ok(prebuiltFlags.length <= 1, `${PREBUILT_APP_FLAG} is singular`);
  const { concurrency, remaining } = concurrencyFrom(
    arguments_.filter((argument) => argument !== PREBUILT_APP_FLAG),
  );
  return {
    concurrency,
    prebuiltApp: prebuiltFlags.length === 1,
    receipt: coveragePath(remaining),
  };
}

function bounded(event, milliseconds) {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(false), milliseconds);
    timer.unref();
    event.then(
      () => { clearTimeout(timer); resolve(true); },
      () => { clearTimeout(timer); resolve(true); },
    );
  });
}

const active = new Map();
let terminating = false;

async function terminate(child, signal = "SIGTERM") {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const closed = once(child, "close");
  child.kill(signal);
  if (await bounded(closed, TERMINATION_GRACE_MS)) return;
  try {
    process.kill(-child.pid, "SIGKILL");
  } catch {
    child.kill("SIGKILL");
  }
  await bounded(closed, TERMINATION_GRACE_MS);
}

async function terminateAll(signal = "SIGTERM") {
  if (terminating) return;
  terminating = true;
  await Promise.all([...active.values()].map(({ child }) => terminate(child, signal)));
}

function installTerminationHandlers() {
  for (const signal of ["SIGINT", "SIGTERM"]) {
    process.on(signal, () => {
      void terminateAll(signal).finally(() => {
        process.removeAllListeners(signal);
        process.kill(process.pid, signal);
      });
    });
  }
}

function observeEvents(child, state) {
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    process.stdout.write(`${line}\n`);
    if (!line.startsWith(SHARD_EVENT_PREFIX)) return;
    try {
      const event = JSON.parse(line.slice(SHARD_EVENT_PREFIX.length));
      if (event.kind === "scenario-end") state.scenarios.push(event);
      if (event.kind === "ownership") state.ownership = event;
      if (event.kind === "summary") state.summary = event;
    } catch (error) {
      state.eventError = error;
      void terminate(child);
    }
  });
  child.stderr.pipe(process.stderr);
}

async function runShard(shard) {
  const started = Date.now();
  process.stdout.write(`canvas shard start name=${shard.name}\n`);
  const child = spawn(process.execPath, [worker, SHARD_FLAG, shard.name], {
    cwd: root,
    detached: true,
    env: { ...process.env, SPICE_CANVAS_PREBUILT: "1" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const state = { child, scenarios: [], ownership: null, summary: null, eventError: null };
  active.set(shard.name, state);
  observeEvents(child, state);
  const closed = once(child, "close");
  const timeoutMs = shard.timeoutMs ?? SHARD_TIMEOUT_MS;
  const timer = setTimeout(() => {
    process.stderr.write(`canvas shard timeout name=${shard.name} phase=worker\n`);
    void terminate(child);
  }, timeoutMs);
  const [code, signal] = await closed;
  clearTimeout(timer);
  active.delete(shard.name);
  const elapsedMs = Date.now() - started;
  process.stdout.write(
    `canvas shard end name=${shard.name} elapsed_ms=${elapsedMs} code=${code} signal=${signal}\n`,
  );
  if (state.eventError) throw state.eventError;
  assert.equal(signal, null, `${shard.name} shard is not interrupted`);
  assert.equal(code, 0, `${shard.name} shard succeeds`);
  assert.deepEqual(
    state.scenarios.map(({ scenario }) => scenario),
    shard.scenarios,
    `${shard.name} executes its registered scenarios exactly once and in order`,
  );
  assert.ok(
    elapsedMs < timeoutMs,
    `${shard.name} finishes inside ${Math.floor(timeoutMs / 1000)} seconds`,
  );
  assert.ok(state.summary, `${shard.name} reports CPU and wall timing`);
  if (shard.name !== "ownership") {
    assert.ok(state.ownership, `${shard.name} reports its exact descendant and fixture ownership`);
  }
  if (state.ownership) {
    assert.equal(
      state.ownership.workerPid,
      child.pid,
      `${shard.name} reports the worker whose close event is its completion boundary`,
    );
    const pids = await auditOwnership(shard.name, state.ownership);
    process.stdout.write(
      `canvas ownership audit name=${shard.name} pids=${pids.join(",")} fixture=absent lanes=absent process_table=clean\n`,
    );
  }
  return { shard: shard.name, elapsedMs, ...state.summary, scenarios: state.scenarios };
}

/// Stop assigning work after the first failure, but drain every shard already running.
///
/// A worker owns its fixture until its final command and teardown complete. Ending
/// peers merely because an unrelated worker failed lets signal teardown race those
/// commands and remove their discovery files underneath them. External coordinator
/// termination still uses `terminateAll`; an ordinary shard failure stays local.
export async function drainShardQueue(queue, concurrency, run) {
  const results = [];
  let firstFailure = null;
  const runners = Array.from({ length: concurrency }, async () => {
    while (queue.length > 0 && firstFailure === null) {
      const shard = queue.shift();
      try {
        results.push(await run(shard));
      } catch (error) {
        if (firstFailure === null) firstFailure = error;
      }
    }
  });
  await Promise.all(runners);
  if (firstFailure) throw firstFailure;
  return results;
}

async function coordinate() {
  installTerminationHandlers();
  assertShardInventory();
  assertShardMutationGuard();
  const { concurrency, prebuiltApp, receipt } = invocation(process.argv.slice(2));
  const started = Date.now();
  await buildResident({ app: !prebuiltApp });
  const buildElapsedMs = Date.now() - started;
  assert.ok(buildElapsedMs < COORDINATOR_TIMEOUT_MS, "Canvas build leaves time for its shards");
  process.stdout.write(
    `canvas coordinator start concurrency=${concurrency} shards=${CANVAS_SHARDS.length}\n`,
  );
  let peak = 0;
  let results = [];
  const queue = [...CANVAS_SHARDS];
  const progress = setInterval(() => {
    const running = [...active.keys()];
    process.stdout.write(
      `canvas shard progress elapsed_ms=${Date.now() - started} running=${running.join(",") || "none"} remaining=${queue.length}\n`,
    );
  }, PROGRESS_MS);
  const overall = setTimeout(() => {
    process.stderr.write(
      `canvas coordinator timeout phase=shards elapsed_ms=${COORDINATOR_TIMEOUT_MS}\n`,
    );
    void terminateAll();
  }, COORDINATOR_TIMEOUT_MS - buildElapsedMs);
  try {
    results = await drainShardQueue(queue, concurrency, async (shard) => {
      peak = Math.max(peak, active.size + 1);
      return runShard(shard);
    });
  } finally {
    clearInterval(progress);
    clearTimeout(overall);
  }
  const elapsedMs = Date.now() - started;
  assert.ok(
    elapsedMs < COORDINATOR_TIMEOUT_MS,
    `Canvas coordinator finishes inside ${Math.floor(COORDINATOR_TIMEOUT_MS / 1000)} seconds`,
  );
  assert.deepEqual(
    results.map(({ shard }) => shard).sort(),
    CANVAS_SHARDS.map(({ name }) => name).sort(),
    "every Canvas shard succeeds exactly once",
  );
  const scenarios = results.flatMap((result) => result.scenarios);
  const slowest = scenarios.reduce(
    (current, candidate) => candidate.elapsedMs > current.elapsedMs ? candidate : current,
    { scenario: "none", elapsedMs: 0 },
  );
  const cpuMs = results.reduce((sum, result) => sum + result.cpuMs, 0);
  process.stdout.write(
    `canvas coordinator end elapsed_ms=${elapsedMs} cpu_ms=${cpuMs} peak_concurrency=${peak} slowest=${slowest.scenario} slowest_ms=${slowest.elapsedMs}\n`,
  );
  if (receipt) await writeFile(receipt, COVERAGE_RECEIPT);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await coordinate();
}
