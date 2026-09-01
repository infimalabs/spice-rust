import assert from "node:assert/strict";
import { EventEmitter } from "node:events";
import { access, mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { ownFixture } from "./canvas-lifecycle.mjs";

test("concurrent fixture cleanup callers join the complete teardown", async () => {
  const fixture = await mkdtemp(join(tmpdir(), "spice-canvas-cleanup-join-"));
  const linkedRoot = await mkdtemp(join(tmpdir(), "spice-canvas-cleanup-lanes-"));
  const stopStarted = Promise.withResolvers();
  const allowStop = Promise.withResolvers();
  const child = new EventEmitter();
  child.exitCode = null;
  child.signalCode = null;
  child.kill = () => assert.fail("the completed graceful stop must not escalate");
  const owner = ownFixture({
    fixture,
    linkedRoot,
    stop: async () => {
      stopStarted.resolve();
      await allowStop.promise;
      child.exitCode = 0;
    },
  });
  owner.ownResident({ child });

  const first = owner.cleanup();
  await stopStarted.promise;
  let secondFinished = false;
  const second = owner.cleanup().then(() => { secondFinished = true; });
  await Promise.resolve();
  assert.equal(secondFinished, false, "a concurrent caller cannot report partial cleanup complete");
  await access(fixture);
  await access(linkedRoot);

  allowStop.resolve();
  await Promise.all([first, second]);
  await assert.rejects(() => access(fixture), { code: "ENOENT" });
  await assert.rejects(() => access(linkedRoot), { code: "ENOENT" });
  assert.equal(owner.cleaned, true, "the owner is complete only after its directories are gone");
});
