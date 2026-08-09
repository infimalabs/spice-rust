import assert from "node:assert/strict";
import { execFileSync, spawn } from "node:child_process";
import { once } from "node:events";
import { access, writeFile } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

import { buildResident } from "./canvas-fixture.mjs";
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
const COVERAGE_RECEIPT = "BROWSER-1kKSWhZk browser coverage complete\n";
const SHARD_TIMEOUT_MS = 120_000;
const COORDINATOR_TIMEOUT_MS = 180_000;
const PROGRESS_MS = 30_000;
const TERMINATION_GRACE_MS = 30_000;
const CONCURRENCY = 2;

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

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

async function absent(path) {
  try {
    await access(path);
    return false;
  } catch (error) {
    return error.code === "ENOENT";
  }
}

function pgrepPath(path) {
  try {
    return execFileSync("pgrep", ["-f", path], { encoding: "utf8" })
      .trim().split("\n").filter(Boolean).map(Number);
  } catch (error) {
    if (error.status === 1) return [];
    throw error;
  }
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

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    void terminateAll(signal).finally(() => {
      process.removeAllListeners(signal);
      process.kill(process.pid, signal);
    });
  });
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
  const timer = setTimeout(() => {
    process.stderr.write(`canvas shard timeout name=${shard.name} phase=worker\n`);
    void terminate(child);
  }, SHARD_TIMEOUT_MS);
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
  assert.ok(elapsedMs < SHARD_TIMEOUT_MS, `${shard.name} finishes inside 120 seconds`);
  assert.ok(state.summary, `${shard.name} reports CPU and wall timing`);
  if (shard.name !== "ownership") {
    assert.ok(state.ownership, `${shard.name} reports its exact descendant and fixture ownership`);
  }
  if (state.ownership) {
    const pids = state.ownership.pids.filter((pid) => pid !== process.pid);
    assert.deepEqual(pids.filter(alive), [], `${shard.name} leaves no captured process alive`);
    assert.equal(await absent(state.ownership.fixture), true, `${shard.name} removes its fixture`);
    assert.equal(await absent(state.ownership.linkedRoot), true, `${shard.name} removes its lanes`);
    assert.deepEqual(
      [...pgrepPath(state.ownership.fixture), ...pgrepPath(state.ownership.linkedRoot)],
      [],
      `${shard.name} leaves no process matched by either exact fixture path`,
    );
    process.stdout.write(
      `canvas ownership audit name=${shard.name} pids=${pids.join(",")} fixture=absent lanes=absent pgrep=clean\n`,
    );
  }
  return { shard: shard.name, elapsedMs, ...state.summary, scenarios: state.scenarios };
}

async function coordinate() {
  assertShardInventory();
  assertShardMutationGuard();
  const receipt = coveragePath(process.argv.slice(2));
  const started = Date.now();
  await buildResident();
  const buildElapsedMs = Date.now() - started;
  assert.ok(buildElapsedMs < COORDINATOR_TIMEOUT_MS, "Canvas build leaves time for its shards");
  let peak = 0;
  const results = [];
  const queue = [...CANVAS_SHARDS];
  const progress = setInterval(() => {
    const running = [...active.keys()];
    process.stdout.write(
      `canvas shard progress elapsed_ms=${Date.now() - started} running=${running.join(",") || "none"} remaining=${queue.length}\n`,
    );
  }, PROGRESS_MS);
  const overall = setTimeout(() => {
    process.stderr.write("canvas coordinator timeout phase=shards elapsed_ms=180000\n");
    void terminateAll();
  }, COORDINATOR_TIMEOUT_MS - buildElapsedMs);
  try {
    const runners = Array.from({ length: CONCURRENCY }, async () => {
      while (queue.length > 0) {
        const shard = queue.shift();
        peak = Math.max(peak, active.size + 1);
        results.push(await runShard(shard));
      }
    });
    await Promise.all(runners);
  } catch (error) {
    await terminateAll();
    throw error;
  } finally {
    clearInterval(progress);
    clearTimeout(overall);
  }
  const elapsedMs = Date.now() - started;
  assert.ok(elapsedMs < COORDINATOR_TIMEOUT_MS, "Canvas coordinator finishes inside 180 seconds");
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

await coordinate();
