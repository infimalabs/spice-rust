import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdir, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  auditOwnership,
  processIdsMatchingPaths,
  readProcessTable,
} from "./canvas-process-ownership.mjs";

// Under the C locale /bin/ps caret-escapes non-UTF-8 argv bytes (0xff renders
// as M-^?), so the invalid-byte fixture below would never be observable. Pin a
// UTF-8 locale before any process-table read so every launcher sees raw bytes.
process.env.LC_ALL = "en_US.UTF-8";

const SLEEP = "setTimeout(() => {}, 30_000)";
const INVALID_ARGV = [
  "import os, sys",
  "executable = os.fsencode(sys.executable)",
  "os.execv(executable, [executable, b'-c', b'import time; time.sleep(30)', b'spice-invalid-\\xff'])",
].join("; ");

async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  const closed = once(child, "close");
  child.kill("SIGTERM");
  await closed;
}

async function observed(predicate) {
  const deadline = Date.now() + 5_000;
  while (Date.now() < deadline) {
    const table = readProcessTable();
    if (predicate(table)) return table;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  assert.fail("expected process-table condition was not observed");
}

test("ownership audit ignores unrelated non-UTF-8 argv without losing either owner", async () => {
  const root = await mkdtemp(join(tmpdir(), "spice-canvas-ownership-"));
  const fixture = join(root, "fixture");
  const linkedRoot = join(root, "lanes");
  await Promise.all([mkdir(fixture), mkdir(linkedRoot)]);
  const fixtureOwner = spawn(process.execPath, ["-e", SLEEP, fixture], { stdio: "ignore" });
  const laneOwner = spawn(process.execPath, ["-e", SLEEP, linkedRoot], { stdio: "ignore" });
  const invalid = spawn("python3", ["-c", INVALID_ARGV], { stdio: "ignore" });

  try {
    const table = await observed((snapshot) => {
      const marker = snapshot.indexOf(Buffer.from("spice-invalid-"));
      return marker >= 0 && snapshot[marker + "spice-invalid-".length] === 0xff;
    });
    assert.deepEqual(processIdsMatchingPaths(table, [fixture]), [fixtureOwner.pid]);
    assert.deepEqual(processIdsMatchingPaths(table, [linkedRoot]), [laneOwner.pid]);

    await Promise.all([stop(fixtureOwner), stop(laneOwner)]);
    await Promise.all([rm(fixture, { recursive: true }), rm(linkedRoot, { recursive: true })]);
    assert.deepEqual(
      await auditOwnership("invalid-byte regression", {
        pids: [fixtureOwner.pid, laneOwner.pid],
        fixture,
        linkedRoot,
      }),
      [fixtureOwner.pid, laneOwner.pid],
    );
  } finally {
    await Promise.all([stop(fixtureOwner), stop(laneOwner), stop(invalid)]);
    await rm(root, { recursive: true, force: true });
  }
});
