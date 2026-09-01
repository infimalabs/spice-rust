// One idempotent teardown boundary for a Canvas fixture and everything it launched.
//
// A fixture owns two kinds of durable thing: temporary directories, and
// processes. Only the directories were ever cleaned reliably. The resident was
// stopped gracefully on the happy path and SIGKILLed on the unhappy one, and
// SIGKILL is precisely the signal that cannot be handled, so a killed resident
// never got to shut its occupants down. Those occupants are not in the
// resident's process group either -- `sill/src/spawn/launch.rs` gives every
// launched occupant its own group with `process_group(0)` -- so nothing about
// killing the resident reaches them. They were reparented to pid 1 and stayed
// there, one set per run, for as long as the machine was up.
//
// The rules this module keeps, in the order they matter:
//
//   1. Prefer the resident's own shutdown. `spice down` drains supervised
//      occupants, which is the only path that ends them the way production
//      does; signals are an escalation for when that path is unavailable or
//      does not finish.
//   2. Escalate only against *recorded* ownership. Every target here was either
//      spawned by this process or enrolled itself in this fixture's own ledger.
//      Nothing is discovered by matching a process name: eleven worktrees share
//      this machine, and `pkill -f fake-claude` would end a peer's battery as
//      readily as this one's.
//   3. Clean processes before directories. The ledger naming the processes
//      lives inside the fixture directory, so removing it first would throw the
//      ownership away and leave the processes unreachable.
//   4. Run at most once per owner, and survive partial construction. Teardown
//      is reached from normal completion, from a failed assertion, and from a
//      signal, including a signal that arrives while setup is half done.
import { once } from "node:events";
import { readFile, rm } from "node:fs/promises";
import { join } from "node:path";

import { childExited } from "./canvas-child-state.mjs";
import { VENDOR_PID_LEDGER, stopResident } from "./canvas-fixture.mjs";

/// Signals a spec process can be asked to stop by and still clean up.
///
/// SIGKILL is deliberately absent: it cannot be caught, so no teardown can be
/// promised against it. That is the whole reason this module exists rather than
/// the fixture continuing to SIGKILL its own resident.
const TERMINATION_SIGNALS = ["SIGINT", "SIGTERM", "SIGHUP"];

/// How long the resident is given to shut its own occupants down.
const GRACEFUL_MS = 10_000;

/// How long a signalled process is given to exit before the next escalation.
const ESCALATION_MS = 5_000;

/// How often a bounded wait re-reads whether a process is still there.
const POLL_MS = 25;

/// Owners with cleanup still outstanding, and the handlers installed for them.
///
/// Module-level because process signal handlers are process-global; keeping the
/// set here is what lets a nested scenario add and remove its own owner without
/// either fixture installing a second set of handlers or removing the other's.
const outstanding = new Set();
let installed = null;

/// The process world the reap signals through and decides liveness from.
///
/// Injected as one object rather than as loose functions because its two halves
/// have to agree: a check that substitutes liveness but not waiting would spend
/// the real escalation bound waiting for a process that was never going to
/// change, and one that substitutes waiting alone would signal real pids. Each
/// half is a boundary this module cannot reach past in a test -- the kernel's
/// answer about another process, and the passage of time.
const REAL_PROCESSES = {
  kill: (pid, signal) => process.kill(pid, signal),
  settled: (predicate) => settles(predicate, ESCALATION_MS),
};

/// Whether a pid or negative process-group id still names a live process.
///
/// Signal 0 performs the permission and existence check without delivering
/// anything, so this asks the kernel rather than inferring liveness from an
/// exit event that a detached or reparented process would never send us.
function alive(target, processes) {
  try {
    processes.kill(target, 0);
    return true;
  } catch (error) {
    // EPERM means the process exists and is simply not ours to signal, which is
    // still alive for our purposes and must not read as gone.
    return error.code === "EPERM";
  }
}

/// Wait until a predicate holds, up to a bound, and report whether it did.
///
/// Returns rather than throws, because every caller wants to escalate on
/// failure rather than abandon the rest of teardown.
async function settles(predicate, bound) {
  const deadline = Date.now() + bound;
  while (Date.now() < deadline) {
    if (predicate()) return true;
    await new Promise((wake) => setTimeout(wake, POLL_MS));
  }
  return predicate();
}

/// Signal one recorded process group, ignoring one that has already gone.
///
/// The negative pid addresses the group rather than the leader alone. Each
/// occupant is its own group leader, so this reaches the shell stand-in *and*
/// the `sleep` it is currently blocked in, which a signal to the leader alone
/// would leave running for up to a second after its parent had gone.
function signalGroup(pid, signal, processes) {
  try {
    processes.kill(-pid, signal);
  } catch {
    // ESRCH is the expected and desirable case: the group is already empty.
  }
}

/// Read back the pids the stand-ins of this fixture enrolled for themselves.
///
/// An absent ledger is not a failure. It is what a fixture looks like when it
/// was torn down before any vendor ever launched, which is exactly the partial
/// initialization this must survive.
export async function recordedVendorPids(fixture) {
  try {
    const ledger = await readFile(join(fixture, VENDOR_PID_LEDGER), "utf8");
    return [
      ...new Set(
        ledger
          .split("\n")
          .map((line) => Number.parseInt(line.trim(), 10))
          .filter((pid) => Number.isInteger(pid) && pid > 1),
      ),
    ];
  } catch {
    return [];
  }
}

/// Take ownership of a fixture's directories and, later, its processes.
///
/// Returns the owner rather than taking a callback so that a scenario can keep
/// using its existing `try`/`finally` shape: the only requirement is that
/// `cleanup` is reached, and a signal arriving before that reaches it instead.
/// The resident stop operation is injectable so focused checks can observe this
/// boundary without replacing the real child whose state it decides from, and
/// the process world for the same reason: the pids a stand-in records are not
/// ones a check can conjure into an unkillable state for real. The process world
/// governs the recorded-pid reap alone. The resident is escalated against
/// through its own child handle, which `stop` and that handle already stand in
/// for, so substituting a process world here does not quietly disarm it.
export function ownFixture({
  fixture,
  linkedRoot,
  stop = stopResident,
  processes = REAL_PROCESSES,
}) {
  const owner = {
    fixture,
    linkedRoot,
    resident: null,
    cleaned: false,
    cleanupPromise: null,

    /// Record the resident this fixture is responsible for.
    ///
    /// Called after `startResident` resolves, and re-called after a restart,
    /// because the restarted resident is a different process and the first one
    /// is already gone.
    ownResident(resident) {
      owner.resident = resident;
      return resident;
    },

    /// End every owned process, then remove every owned directory.
    ///
    /// Idempotent: the second call returns without re-signalling anything, so
    /// the ordinary `finally` path and a signal that raced it cannot both drive
    /// teardown. Never throws, because it runs on the failure path and an error
    /// raised here would replace the failure the caller is trying to report.
    cleanup() {
      if (owner.cleanupPromise) return owner.cleanupPromise;
      owner.cleanupPromise = (async () => {
        try {
          // Read the ledger before anything is removed: it lives inside the
          // fixture directory, and it is the only record of what to reap.
          const vendors = await recordedVendorPids(fixture);
          await attempt(() => stopOwnedResident(owner, stop), "could not stop its resident");
          await attempt(
            () => reapRecordedVendors(vendors, processes),
            "could not reap its stand-ins",
          );
          await attempt(() => rm(fixture, { recursive: true, force: true }), `left ${fixture}`);
          if (linkedRoot) {
            await attempt(
              () => rm(linkedRoot, { recursive: true, force: true }),
              `left ${linkedRoot}`,
            );
          }
        } finally {
          owner.cleaned = true;
          outstanding.delete(owner);
          if (outstanding.size === 0) removeHandlers();
        }
      })();
      return owner.cleanupPromise;
    },
  };
  outstanding.add(owner);
  installHandlers();
  return owner;
}

/// Run one teardown step, reporting rather than raising if it cannot finish.
///
/// Two properties depend on this. Steps stay independent: a resident that
/// refuses to stop must not skip the reap that ends the stand-ins it left
/// behind, nor the directories neither of them owns. And a step that fails
/// stays non-fatal: teardown runs inside the caller's `finally`, so throwing
/// would report trouble here in place of the assertion that sent us here. The
/// failure still goes to stderr, where it is visible without being mistaken
/// for the reason the run ended.
async function attempt(step, note) {
  try {
    await step();
  } catch (error) {
    process.stderr.write(`canvas fixture ${note}: ${error}\n`);
  }
}

/// Ask the resident to shut down, and escalate against it alone if it will not.
async function stopOwnedResident(owner, stop) {
  const resident = owner.resident;
  if (!resident || childExited(resident.child)) return;
  try {
    await withBound(stop(owner.fixture, resident), GRACEFUL_MS);
  } catch {
    // A resident that cannot be asked to stop is escalated against below. The
    // reason it refused is not actionable here and must not mask the caller's.
  }
  if (childExited(resident.child)) return;
  for (const signal of ["SIGTERM", "SIGKILL"]) {
    const exited = once(resident.child, "exit");
    resident.child.kill(signal);
    const stopped = await Promise.race([
      exited.then(() => true),
      settles(() => childExited(resident.child), ESCALATION_MS),
    ]);
    if (stopped) return;
  }
}

/// End the recorded stand-ins, which the resident's own shutdown leaves running.
///
/// Not a backstop for the killed-resident case alone, though it covers that too.
/// Deleting this step reds the *failure* scenario, whose resident is asked to
/// stop gracefully and does: a completed `spice down` returns with these
/// stand-ins still alive, so this is what actually ends them on every path.
///
/// Escalation that runs out is reported, not assumed to have worked. A stand-in
/// still alive after SIGKILL is one the kernel would not let us end -- a pid
/// that outlived its fixture and belongs to no one, which is precisely the
/// residue this teardown exists to prevent. Silence there would leave the next
/// run to discover it, so the survivors are named and raised, and `attempt`
/// turns that into a stderr line without ending the rest of cleanup.
async function reapRecordedVendors(vendors, processes) {
  // A leader can exit while a child remains in its group. Probe the negative
  // group id so settlement means every member is gone, not merely the leader.
  const living = () => vendors.filter((pid) => alive(-pid, processes));
  if (living().length === 0) return;
  for (const signal of ["SIGTERM", "SIGKILL"]) {
    for (const pid of living()) signalGroup(pid, signal, processes);
    if (await processes.settled(() => living().length === 0)) return;
  }
  // Recomputed rather than reusing the last round's set: escalation is what the
  // report is about, and a stand-in that died as SIGKILL landed is reaped, not
  // surviving. Sorted so a run that leaks two of them reads the same every time.
  const survivors = living().sort((left, right) => left - right);
  if (survivors.length === 0) return;
  throw new Error(`stand-ins survived SIGKILL: ${survivors.join(", ")}`);
}

/// Bound a promise that has no timeout of its own.
///
/// `spice down` talks to a resident that may be wedged, and an unbounded wait
/// there would hang teardown itself -- turning a reported failure into a run
/// that never finishes, which is strictly worse than a noisy escalation.
function withBound(work, bound) {
  return new Promise((settle, fail) => {
    const timer = setTimeout(() => fail(new Error("teardown step exceeded its bound")), bound);
    work.then(settle, fail).finally(() => clearTimeout(timer));
  });
}

/// Clean every outstanding owner, then let the original signal take effect.
///
/// The signal is re-raised against the default disposition rather than
/// translated into an exit code, so a caller waiting on this process still sees
/// the termination it asked for instead of a plain success or a synthetic
/// status that no longer names the signal.
function onTermination(signal) {
  return () => {
    void (async () => {
      // `cleanup` promises not to throw, and this holds it to that promise
      // rather than trusting it. Nothing catches a rejection thrown from a
      // signal handler: it would become an unhandled rejection, replacing the
      // interruption the caller asked for with a crash, and abandoning the
      // owners queued behind this one at the exact moment they must be cleaned.
      for (const owner of [...outstanding]) {
        try {
          await owner.cleanup();
        } catch (error) {
          process.stderr.write(`canvas fixture teardown after ${signal}: ${error}\n`);
        }
      }
      removeHandlers();
      process.kill(process.pid, signal);
    })();
  };
}

/// Install the process-global handlers once, however many fixtures are open.
///
/// A nested scenario runs while its parent's fixture is still owned, so this
/// must be shared rather than per-fixture: two installations would clean the
/// same owners twice and re-raise the signal twice.
function installHandlers() {
  if (installed) return;
  installed = new Map(
    TERMINATION_SIGNALS.map((signal) => [signal, onTermination(signal)]),
  );
  for (const [signal, handler] of installed) process.on(signal, handler);
}

/// Remove them again once nothing is owned.
///
/// Without this a suite that opens and closes several fixtures would leave a
/// handler behind for each, and the accumulated listeners would eventually trip
/// Node's max-listeners warning while silently changing the process's default
/// response to being asked to stop.
function removeHandlers() {
  if (!installed) return;
  for (const [signal, handler] of installed) process.off(signal, handler);
  installed = null;
}
