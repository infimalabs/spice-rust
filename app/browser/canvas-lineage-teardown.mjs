// Focused proof for the non-throwing lineage teardown boundary.
import assert from "node:assert/strict";

import { cleanupLineageFixture } from "./canvas-lineage-life.mjs";

const liveResident = () => ({ child: { exitCode: null, signalCode: null } });

async function captureStderr(run) {
  const write = process.stderr.write;
  let output = "";
  process.stderr.write = (chunk) => {
    output += chunk.toString();
    return true;
  };
  try {
    await run();
  } finally {
    process.stderr.write = write;
  }
  return output;
}

async function teardownWhilePrimaryFails(request) {
  const primary = new Error("injected primary lineage scenario failure");
  let observed;
  let cleanupCalls = 0;
  const diagnostics = await captureStderr(async () => {
    try {
      try {
        throw primary;
      } finally {
        await cleanupLineageFixture({
          fixture: "focused-lineage-fixture",
          resident: liveResident(),
          owner: { async cleanup() { cleanupCalls += 1; } },
          request,
        });
      }
    } catch (error) {
      observed = error;
    }
  });
  return { cleanupCalls, diagnostics, observed, primary };
}

function assertPrimaryAndCleanup(result) {
  assert.equal(result.observed, result.primary, "teardown preserves the primary scenario error");
  assert.equal(result.cleanupCalls, 1, "owner cleanup runs once after the drain failure");
}

export async function assertLineageDrainFailureTeardown() {
  const snapshotFailure = new Error("injected lineage snapshot failure");
  const snapshot = await teardownWhilePrimaryFails(async () => { throw snapshotFailure; });
  assertPrimaryAndCleanup(snapshot);
  assert.equal(
    snapshot.diagnostics,
    `canvas lineage teardown could not drain lanes: ${snapshotFailure}\n`,
    "a lane snapshot failure is reported exactly once",
  );

  const drainFailure = new Error("injected second lane drain failure");
  const requests = [];
  const drain = await teardownWhilePrimaryFails(async (_fixture, args) => {
    requests.push(args);
    if (requests.length === 1) return { body: { lanes: [{ lane: "first" }, { lane: "second" }] } };
    if (requests.length === 2) return { body: {} };
    throw drainFailure;
  });
  assertPrimaryAndCleanup(drain);
  assert.deepEqual(
    requests,
    [["lane"], ["lane", "drain", "first"], ["lane", "drain", "second"]],
    "the failure from a later lane drain reaches the same reporting boundary",
  );
  assert.equal(
    drain.diagnostics,
    `canvas lineage teardown could not drain lanes: ${drainFailure}\n`,
    "a lane drain failure retains its original error",
  );

  let cleanupCalls = 0;
  let queries = 0;
  const diagnostics = await captureStderr(() => cleanupLineageFixture({
    fixture: "signal-exited-lineage-fixture",
    resident: { child: { exitCode: null, signalCode: "SIGTERM" } },
    owner: { async cleanup() { cleanupCalls += 1; } },
    request: async () => { queries += 1; },
  }));
  assert.equal(queries, 0, "a signal-exited resident is not queried for its lanes");
  assert.equal(cleanupCalls, 1, "an already-exited resident still reaches owner cleanup");
  assert.equal(diagnostics, "", "an already-exited resident needs no drain diagnostic");
}
