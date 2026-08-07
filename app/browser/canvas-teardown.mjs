// Executable proof that a Canvas fixture reaps what it launched.
//
// The leak this covers is invisible to every other test in the suite, because
// its symptom is not a wrong answer but a process that is still there after the
// answer was given. Asserting on it needs a fixture that really launched a
// vendor and a run that really ended, so every scenario below drives the real
// fixture in a child process (`canvas-teardown-harness.mjs`) and then ends that
// child one of the three ways a run actually ends badly: interrupted, failed,
// and never ready. The third is the one this file used to leak from itself --
// a launch that gives up has a live child nobody downstream ever receives.
//
// Every scenario watches the *exact* pids the stand-ins enrolled for themselves.
// Nothing here searches for processes by name. That is not only a safety rule
// for a machine running eleven worktrees at once -- a name search would also
// make the assertion vacuous, since it would happily find a peer battery's
// stand-ins and report this fixture's own as reaped.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { access, rm } from "node:fs/promises";
import { createInterface } from "node:readline";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { childExited } from "./canvas-child-state.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const harness = join(here, "canvas-teardown-harness.mjs");

/// Launch the real harness one generation below a disposable parent. The
/// intermediate relays readiness and diagnostics but deliberately owns no
/// cleanup, so killing it tests only the harness's parent-death path.
const INTERMEDIATE_PARENT = `
  import { spawn } from "node:child_process";
  const harness = spawn(process.execPath, [process.argv[1], process.argv[2]], {
    cwd: process.cwd(),
    stdio: ["ignore", "pipe", "pipe"],
  });
  harness.stdout.pipe(process.stdout);
  harness.stderr.pipe(process.stderr);
  await new Promise(() => {});
`;

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
///
/// Every way this can fail leaves a child that is nobody's yet: the scenario
/// never received it, so nothing downstream can end it. That is why the whole
/// readiness section is guarded rather than the individual throw sites -- there
/// are four of them, and the next one added would otherwise be the fifth leak.
async function launchHarness(mode, throughIntermediate = false) {
  const args = throughIntermediate
    ? ["--input-type=module", "--eval", INTERMEDIATE_PARENT, harness, mode]
    : [harness, mode];
  const child = spawn(process.execPath, args, {
    cwd: root,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let diagnostics = "";
  child.stderr.on("data", (chunk) => { diagnostics += chunk.toString(); });
  // Taken at spawn time so it cannot miss the event. `close` rather than `exit`
  // because it is the one that promises the child's streams were drained first,
  // which is what makes the diagnostics below complete rather than partial.
  const closed = once(child, "close");
  const lines = createInterface({ input: child.stdout });
  // Neither of these carries the diagnostics itself. Both are raised while the
  // child is still being read, so what they could quote is whatever happened to
  // have arrived; the guard below appends the complete text once, on its own
  // lines, after the child is gone.
  const failedEarly = once(child, "exit").then(([code, ended]) => {
    throw new Error(
      `teardown harness (${mode}) exited ${ended ?? code} before announcing a vendor`,
    );
  });
  const announced = (async () => {
    for await (const line of lines) {
      const trimmed = line.trim();
      if (trimmed.startsWith("{")) return JSON.parse(trimmed);
    }
    throw new Error(`teardown harness (${mode}) closed stdout`);
  })();
  const timeout = new Promise((_, fail) => {
    setTimeout(() => fail(new Error(`teardown harness (${mode}) never became ready`)), READY_MS)
      .unref();
  });
  try {
    const ready = await Promise.race([announced, failedEarly, timeout]);
    assert.ok(Number.isInteger(ready.harness), "the harness announces its own pid");
    assert.ok(ready.vendors.length > 0, "the harness announces the stand-ins it launched");
    assert.deepEqual(
      ready.vendors.filter((pid) => !alive(pid)),
      [],
      "every announced stand-in is running before the run is ended",
    );
    return { child, ready, diagnostics: () => diagnostics };
  } catch (failure) {
    await endHarness(child, closed);
    // Re-raised as it arrived, so the run still reports why the launch failed.
    // The diagnostics are appended rather than substituted, and only now that
    // the child is gone, because until then they are whatever had happened to
    // arrive -- and what the harness reached before it stopped announcing is
    // the only account of what this launch was responsible for.
    //
    // They go on their own lines so the text is read back the way it was
    // written: a caller parsing an announcement out of a failure then reads the
    // same line-oriented stream the ready path reads from stdout.
    failure.message = `${failure.message}\nharness diagnostics:\n${diagnostics || "<none>"}`;
    throw failure;
  }
}

/// End a harness child that was never handed to a scenario, and wait it out.
///
/// SIGTERM first because the harness answers it: `ownFixture` installs handlers
/// for the three catchable termination signals and re-raises after running
/// cleanup, so this is the signal that reaps the resident and its stand-ins
/// rather than scattering them. SIGKILL is the escalation for a child that will
/// not, and it is still better than the alternative -- a child left running is
/// reparented to pid 1 the moment the spec that gave up on it exits.
async function endHarness(child, closed) {
  child.kill("SIGTERM");
  if (await arrives(closed, REAP_MS)) return;
  child.kill("SIGKILL");
  await arrives(closed, REAP_MS);
}

/// Whether an event arrived inside a bound.
///
/// The wait is on the event itself, so a child that ends promptly is noticed at
/// once; the bound only decides how long a child that will not end may hold the
/// run, and never becomes the thing being waited on.
function arrives(event, bound) {
  return new Promise((settle) => {
    const timer = setTimeout(() => settle(false), bound);
    timer.unref();
    event.then(() => settle(true), () => settle(true));
  });
}

/// Wait for a harness child to exit, tolerating one that already has.
///
/// `once` only hears events emitted after it is attached, so attaching it to a
/// child that has already gone waits forever. Every wait below is reached after
/// an await -- the readiness handshake -- and that await is exactly the window
/// in which the child can exit, which is why the guard is not decoration here.
/// Where a wait can be attached before the thing that ends the child, that is
/// the better shape and is what `launchHarness` does with `closed`.
///
/// The resolved pair is the one the `exit` event carries, in the same order, so
/// a caller reads a signal exit as [null, "SIGINT"] whether it observed the
/// event or arrived after it.
function exits(child) {
  return childExited(child)
    ? Promise.resolve([child.exitCode, child.signalCode])
    : once(child, "exit");
}

/// Signal one target, ignoring one that has already gone.
function signal(target, name) {
  try {
    process.kill(target, name);
  } catch {
    // ESRCH is the expected case: whatever this named is already gone.
  }
}

/// Reap what a failing scenario would otherwise leave running and on disk.
///
/// A no-op on the green path, where every pid is already gone and both
/// directories are already removed. It earns its place on the red path, which
/// is reached precisely when the fixture did not reap: reporting that failure
/// without this would leave the very leak under test alive, one resident and
/// its occupants per red run, on a machine where eleven worktrees run batteries
/// at once. Never raises, because the failure being reported is the scenario's.
async function reapResidue(ready) {
  const reported = [ready.harness, ready.resident, ...ready.vendors];
  const owned = reported.filter((pid) => Number.isInteger(pid) && pid > 1);
  for (const name of ["SIGTERM", "SIGKILL"]) {
    const living = owned.filter(alive);
    if (living.length === 0) break;
    for (const pid of living) {
      // A stand-in leads its own process group, so the negative pid reaches it
      // and whatever it is currently blocked in. A resident leads no group, so
      // that call finds nothing and the direct one is what ends it.
      signal(-pid, name);
      signal(pid, name);
    }
    await survivors(living, REAP_MS);
  }
  await rm(ready.fixture, { recursive: true, force: true }).catch(() => {});
  await rm(ready.linkedRoot, { recursive: true, force: true }).catch(() => {});
}

/// Require one owned directory to be absent for the exact filesystem reason.
async function assertGone(path, message) {
  let code = null;
  try {
    await access(path);
  } catch (error) {
    code = error.code;
  }
  assert.equal(code, "ENOENT", message);
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
  const [code, ended] = await exits(child);
  try {
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
      ended,
      "SIGINT",
      `an interrupted run still terminates by its signal, not code ${code}`,
    );
    await assertGone(ready.fixture, "the fixture directory is removed");
    await assertGone(ready.linkedRoot, "the linked worktree root is removed");
  } finally {
    await reapResidue(ready);
  }
}

/// A run that fails after launching a vendor reaps, and still fails.
///
/// This is the path the suite takes whenever an assertion goes red, which is
/// the common case in practice: the leak was reproduced far more often by
/// failing tests than by interrupted ones.
async function assertFailedRunReaps() {
  const { child, ready, diagnostics } = await launchHarness("fail");
  const [code, ended] = await exits(child);
  try {
    const living = await survivors(ready.vendors, REAP_MS);
    assert.deepEqual(
      living,
      [],
      `a failed fixture reaps every stand-in it launched: ${diagnostics()}`,
    );
    assert.equal(ended, null, "a failed run ends by its own exit rather than a signal");
    assert.notEqual(code, 0, "a failed run keeps reporting failure after cleaning up");
    assert.match(
      diagnostics(),
      /injected harness failure after vendor launch/,
      "the original failure is what the run reports, not an error raised by teardown",
    );
    await assertGone(ready.fixture, "the fixture directory is removed");
    await assertGone(ready.linkedRoot, "the linked worktree root is removed");
  } finally {
    await reapResidue(ready);
  }
}

/// A launch that never becomes ready reaps, and still reports not becoming ready.
///
/// The two scenarios above go wrong after the harness handed its fixture over.
/// This one goes wrong before, which is the case nothing downstream can clean
/// up: the child is still the launcher's, and until this was guarded, giving up
/// on it left a real resident and a real stand-in running under a spec that had
/// already exited.
///
/// The `mute` harness is what makes it deterministic. Shortening `READY_MS`
/// would lose the race too, but it would lose it before the resident exists,
/// and an assertion that nothing survived would then be true of a run that
/// never launched anything. `mute` reaches the same live state the other two
/// scenarios reach and only then stops announcing, so what has to be reaped
/// here is exactly what the leak leaves behind.
async function assertFailedLaunchReaps() {
  let failure = null;
  let launched = null;
  try {
    launched = await launchHarness("mute");
  } catch (error) {
    failure = error;
  }
  if (launched) {
    // Unreachable unless `mute` announced on stdout after all, and a live child
    // is not something to hand back to the assertion below.
    launched.child.kill("SIGTERM");
    await exits(launched.child);
  }
  assert.ok(failure, "a harness that stops announcing does not produce a ready launch");
  assert.match(
    failure.message,
    /closed stdout/,
    `a failed launch reports its own failure: ${failure.message}`,
  );
  const announcement = failure.message
    .split("\n")
    .map((line) => line.trim())
    .find((line) => line.startsWith("{"));
  assert.ok(
    announcement,
    `a failed launch reports what the harness reached: ${failure.message}`,
  );
  const ready = JSON.parse(announcement);
  try {
    const living = await survivors([ready.resident, ...ready.vendors], REAP_MS);
    assert.deepEqual(
      living,
      [],
      `a failed launch reaps the resident and every stand-in under it: ${failure.message}`,
    );
    await assertGone(ready.fixture, "the fixture directory of a failed launch is removed");
    await assertGone(
      ready.linkedRoot,
      "the linked worktree root of a failed launch is removed",
    );
  } finally {
    await reapResidue(ready);
  }
}

/// A harness whose parent is killed still notices reparenting and cleans itself.
async function assertParentDeathReaps() {
  const { child: parent, ready, diagnostics } = await launchHarness("idle", true);
  const parentExit = exits(parent);
  parent.kill("SIGKILL");
  const [code, ended] = await parentExit;
  try {
    assert.equal(
      ended,
      "SIGKILL",
      `the intermediate parent is killed rather than exiting with code ${code}`,
    );
    const owned = [ready.harness, ready.resident, ...ready.vendors];
    const living = await survivors(owned, REAP_MS);
    assert.deepEqual(
      living,
      [],
      `a parentless harness reaps itself, its resident, and every stand-in: ${diagnostics()}`,
    );
    await assertGone(ready.fixture, "a parentless harness removes its fixture directory");
    await assertGone(ready.linkedRoot, "a parentless harness removes its lanes directory");
  } finally {
    await reapResidue(ready);
  }
}

/// Prove the fixture owns its processes across every way a run ends badly.
export async function assertFixtureTeardownReaps() {
  await assertInterruptedRunReaps();
  await assertFailedRunReaps();
  await assertFailedLaunchReaps();
  await assertParentDeathReaps();
}
