// Focused proof that an exit reported by signal is already terminal at the
// shared fixture teardown boundary.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { childExited } from "./canvas-child-state.mjs";
import { ownFixture } from "./canvas-lifecycle.mjs";

export async function assertSignalExitedChildSkipsTeardown() {
  const fixture = await mkdtemp(join(tmpdir(), "spice-canvas-signal-exit-"));
  const child = spawn(
    process.execPath,
    ["-e", 'process.stdout.write("ready\\n"); setInterval(() => {}, 1_000)'],
    { stdio: ["ignore", "pipe", "ignore"] },
  );
  const killChild = child.kill.bind(child);
  let owner;
  let exitObserved = false;
  try {
    const [ready] = await once(child.stdout, "data");
    assert.equal(ready.toString(), "ready\n", "the real child reaches its live state");
    const exited = once(child, "exit");
    assert.equal(killChild("SIGTERM"), true, "the real child accepts the terminating signal");
    const [code, signal] = await exited;
    exitObserved = true;
    assert.equal(code, null, "a signal-terminated child has no numeric exit code");
    assert.equal(signal, "SIGTERM", "the exit event names the terminating signal");
    assert.equal(child.exitCode, null, "Node keeps exitCode null after a signal exit");
    assert.equal(child.signalCode, "SIGTERM", "Node records the signal on the child");
    const gracefulAttempts = [];
    const escalationAttempts = [];
    owner = ownFixture({
      fixture,
      stop: async () => {
        gracefulAttempts.push("graceful shutdown");
        throw new Error("teardown attempted graceful shutdown after the child exit event");
      },
    });
    owner.ownResident({ child });
    child.kill = (attemptedSignal) => {
      escalationAttempts.push(attemptedSignal);
      throw new Error(`teardown attempted ${attemptedSignal} after the child exit event`);
    };

    await owner.cleanup();
    assert.deepEqual(gracefulAttempts, [], "teardown does not gracefully stop an exited child");
    assert.deepEqual(
      escalationAttempts,
      [],
      "teardown sends neither SIGTERM nor SIGKILL after the exit event",
    );
    assert.equal(childExited(child), true, "the shared rule recognizes the signal exit");
  } finally {
    // The event observation is intentionally independent of `childExited`: the
    // mutation check removes half of that rule and must still end immediately.
    if (!exitObserved) {
      const exited = once(child, "exit");
      killChild("SIGKILL");
      await exited;
    }
    if (owner) await owner.cleanup();
    else await rm(fixture, { recursive: true, force: true });
  }
}
