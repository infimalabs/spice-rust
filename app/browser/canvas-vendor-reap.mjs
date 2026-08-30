// Focused proof that stand-ins outliving the reap are named on stderr, and that
// a reap which ends every one of them says nothing at all.
import assert from "node:assert/strict";
import { access, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { VENDOR_PID_LEDGER } from "./canvas-fixture.mjs";
import { ownFixture } from "./canvas-lifecycle.mjs";

/// Pids the fake process world answers for and the real kernel is never asked
/// about. Nothing here reaches `process.kill`, so no unrelated process on this
/// machine -- including a peer worktree's -- can be signalled by this check.
///
/// Two of them, recorded in the order below rather than sorted, because one pid
/// cannot tell a report that names every survivor apart from one that names the
/// first, nor a deliberately ordered report from an accidentally ordered one.
const UNREACHABLE = [424_242, 131_313];

/// What every recorded group is signalled with, in the order escalation reaches
/// it. Spelled out rather than read from the module under test, which would
/// agree with whatever that module decided to send.
const ESCALATION = ["SIGTERM", "SIGKILL"];

/// Collect what teardown reports while it runs.
///
/// `attempt` writes to stderr rather than throwing, so the diagnostic is only
/// observable here. Restored in `finally`: leaving the capture installed would
/// silence every later assertion in the same process.
async function reporting(run) {
  const written = [];
  const real = process.stderr.write.bind(process.stderr);
  process.stderr.write = (chunk) => {
    written.push(String(chunk));
    return true;
  };
  try {
    await run();
  } finally {
    process.stderr.write = real;
  }
  return written.join("");
}

/// Run a body against a fixture whose ledger records the given stand-ins.
///
/// The directories are removed here as well as by the teardown under test. That
/// is not redundancy for its own sake: the mutations these assertions exist to
/// catch include ones that make `cleanup` raise, and a leak check that leaks its
/// own directories whenever it fails is an instance of the defect it tests for.
async function withLedgeredFixture(pids, body) {
  const fixture = await mkdtemp(join(tmpdir(), "spice-canvas-reap-"));
  const linkedRoot = await mkdtemp(join(tmpdir(), "spice-canvas-reap-lanes-"));
  try {
    await writeFile(join(fixture, VENDOR_PID_LEDGER), `${pids.join("\n")}\n`);
    await body({ fixture, linkedRoot });
  } finally {
    await rm(fixture, { recursive: true, force: true });
    await rm(linkedRoot, { recursive: true, force: true });
  }
}

/// Refuse a signal the way the kernel does, with the code teardown reads.
function refuse(code) {
  const error = new Error(`${code} from the substituted process world`);
  error.code = code;
  throw error;
}

/// Assert a path is gone, and gone by absence rather than by any other error.
async function assertRemoved(path, what) {
  await assert.rejects(() => access(path), { code: "ENOENT" }, `${what} is still removed`);
}

export async function assertSurvivingVendorsAreReported() {
  await withLedgeredFixture(UNREACHABLE, async ({ fixture, linkedRoot }) => {
    const delivered = [];
    let roundsCompleted = 0;
    const processes = {
      // Present but not ours: the liveness probe and every delivery refuse
      // alike, which is the state a stand-in reparented to another session
      // presents. `alive` reads EPERM as living, so this pid never goes away.
      kill(pid, signal) {
        if (signal !== 0) delivered.push({ pid, signal });
        refuse("EPERM");
      },
      // Reports each round complete immediately. The production escalation
      // bound is what this check must not spend; whether escalation actually
      // worked is decided by `kill` above, not by how long anything waited.
      settled: async () => {
        roundsCompleted += 1;
        return false;
      },
    };
    const owner = ownFixture({ fixture, linkedRoot, processes });

    // Teardown is driven from a `finally` under an already-failing scenario,
    // which is the shape that makes the non-fatal requirement observable.
    let reported;
    const written = await reporting(async () => {
      try {
        try {
          throw new Error("scenario failure");
        } finally {
          await owner.cleanup();
        }
      } catch (error) {
        reported = error;
      }
    });

    assert.deepEqual(
      delivered,
      ESCALATION.flatMap((signal) => UNREACHABLE.map((pid) => ({ pid: -pid, signal }))),
      "both escalation rounds are attempted against every recorded group",
    );
    assert.equal(roundsCompleted, 2, "each round is awaited to completion before the next");
    assert.match(
      written,
      // Spelled out rather than rebuilt from UNREACHABLE: a check that
      // recomputes the order it is testing agrees with any order it is given.
      /could not reap its stand-ins:.*stand-ins survived SIGKILL: 131313, 424242/,
      "every survivor is named on stderr, ascending, through the non-fatal boundary",
    );
    assert.equal(reported?.message, "scenario failure", "teardown leaves the run's failure");
    // The steps after the diagnostic still ran: a survivor report that escaped
    // `attempt` would have left both of these behind.
    await assertRemoved(fixture, "the fixture directory");
    await assertRemoved(linkedRoot, "the linked root");
  });
}

export async function assertFullyReapedVendorsStaySilent() {
  await withLedgeredFixture(UNREACHABLE, async ({ fixture, linkedRoot }) => {
    let ended = false;
    const processes = {
      kill(pid, signal) {
        if (signal === 0) {
          if (ended) refuse("ESRCH");
          return;
        }
        if (signal === "SIGKILL") ended = true;
      },
      // Never settles, so the survivor set is decided by the check after the
      // loop rather than by an early return -- which is the state a report made
      // unconditional would speak from, and the only state that can catch one.
      settled: async () => false,
    };
    const owner = ownFixture({ fixture, linkedRoot, processes });

    const written = await reporting(() => owner.cleanup());

    assert.equal(ended, true, "escalation reached SIGKILL, so the final check is the empty one");
    assert.equal(written, "", "a reap that ends every stand-in writes nothing");
  });
}
