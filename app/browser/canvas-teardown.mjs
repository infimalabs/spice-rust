// Executable proof that a Canvas fixture reaps what it launched.
//
// The leak this covers is invisible to every other test in the suite, because
// its symptom is not a wrong answer but a process that is still there after the
// answer was given. Asserting on it needs a fixture that really launched a
// vendor and a run that really ended, so both scenarios below drive the real
// fixture in a child process (`canvas-teardown-harness.mjs`) and then end that
// child the two ways a run actually ends badly: interrupted, and failed.
//
// Both scenarios watch the *exact* pids the stand-ins enrolled for themselves.
// Nothing here searches for processes by name. That is not only a safety rule
// for a machine running eleven worktrees at once -- a name search would also
// make the assertion vacuous, since it would happily find a peer battery's
// stand-ins and report this fixture's own as reaped.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { access } from "node:fs/promises";
import { createInterface } from "node:readline";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const harness = join(here, "canvas-teardown-harness.mjs");

/// Longest the harness is given to reach a live vendor stand-in.
const READY_MS = 60_000;

/// Longest teardown may take once the run has been ended.
///
/// The owner's own escalation ladder is bounded well inside this: a graceful
/// shutdown, then SIGTERM, then SIGKILL, each with its own wait. A reap that
/// has not happened by here is not slow, it is not happening.
const REAP_MS = 30_000;

/// Whether a pid still names a live process.
function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

/// Wait until every named pid is gone, and report the ones that stayed.
async function survivors(pids, bound) {
  const deadline = Date.now() + bound;
  let living = pids.filter(alive);
  while (living.length > 0 && Date.now() < deadline) {
    await new Promise((wake) => setTimeout(wake, 25));
    living = pids.filter(alive);
  }
  return living;
}

/// Start the harness and wait until it reports a confirmed-live stand-in.
///
/// Returns the child together with what it announced, so a scenario can end the
/// run and then judge it against pids that were observed running rather than
/// merely written down.
async function launchHarness(mode) {
  const child = spawn(process.execPath, [harness, mode], {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let diagnostics = "";
  child.stderr.on("data", (chunk) => { diagnostics += chunk.toString(); });
  const lines = createInterface({ input: child.stdout });
  const failedEarly = once(child, "exit").then(([code, signal]) => {
    throw new Error(
      `teardown harness (${mode}) exited ${signal ?? code} before announcing a vendor: ${diagnostics || "<no diagnostics>"}`,
    );
  });
  const announced = (async () => {
    for await (const line of lines) {
      const trimmed = line.trim();
      if (trimmed.startsWith("{")) return JSON.parse(trimmed);
    }
    throw new Error(`teardown harness (${mode}) closed stdout: ${diagnostics || "<none>"}`);
  })();
  const timeout = new Promise((_, fail) => {
    setTimeout(() => fail(new Error(`teardown harness (${mode}) never became ready`)), READY_MS)
      .unref();
  });
  const ready = await Promise.race([announced, failedEarly, timeout]);
  assert.ok(ready.vendors.length > 0, "the harness announces the stand-ins it launched");
  assert.deepEqual(
    ready.vendors.filter((pid) => !alive(pid)),
    [],
    "every announced stand-in is running before the run is ended",
  );
  return { child, ready, diagnostics: () => diagnostics };
}

/// Whether a path is still present, used to prove teardown ordering.
async function present(path) {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
}

/// A run stopped by a catchable signal still reaps, and still reports stopping.
///
/// SIGINT stands for the whole class the fixture promises to survive. It is the
/// one an operator actually sends, and before this it did nothing but kill the
/// spec: the `finally` never ran, so the resident and every stand-in under it
/// were left behind with no owner and reparented to pid 1.
async function assertInterruptedRunReaps() {
  const { child, ready, diagnostics } = await launchHarness("idle");
  child.kill("SIGINT");
  const [code, signal] = await once(child, "exit");
  const living = await survivors(ready.vendors, REAP_MS);
  assert.deepEqual(
    living,
    [],
    `an interrupted fixture reaps every stand-in it launched: ${diagnostics()}`,
  );
  // The interruption must survive its own cleanup. A teardown that swallowed
  // the signal and exited 0 would leave a caller -- a shell, a CI runner, the
  // verifier -- believing an interrupted run had succeeded.
  assert.equal(
    signal,
    "SIGINT",
    `an interrupted run still terminates by its signal, not code ${code}`,
  );
  assert.equal(await present(ready.fixture), false, "the fixture directory is removed after");
  assert.equal(await present(ready.linkedRoot), false, "the linked worktree root is removed after");
}

/// A run that fails after launching a vendor reaps, and still fails.
///
/// This is the path the suite takes whenever an assertion goes red, which is
/// the common case in practice: the leak was reproduced far more often by
/// failing tests than by interrupted ones.
async function assertFailedRunReaps() {
  const { child, ready, diagnostics } = await launchHarness("fail");
  const [code, signal] = await once(child, "exit");
  const living = await survivors(ready.vendors, REAP_MS);
  assert.deepEqual(
    living,
    [],
    `a failed fixture reaps every stand-in it launched: ${diagnostics()}`,
  );
  assert.equal(signal, null, "a failed run ends by its own exit rather than a signal");
  assert.notEqual(code, 0, "a failed run keeps reporting failure after cleaning up");
  assert.match(
    diagnostics(),
    /injected harness failure after vendor launch/,
    "the original failure is what the run reports, not an error raised by teardown",
  );
  assert.equal(await present(ready.fixture), false, "the fixture directory is removed after");
  assert.equal(await present(ready.linkedRoot), false, "the linked worktree root is removed after");
}

/// Prove the fixture owns its processes across both ways a run ends badly.
export async function assertFixtureTeardownReaps() {
  await assertInterruptedRunReaps();
  await assertFailedRunReaps();
}
