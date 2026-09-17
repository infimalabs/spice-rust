// Focused proof for the non-throwing team teardown boundary.
import assert from "node:assert/strict";

import { cleanupTeamFixture } from "./canvas-team-life.mjs";

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
  const primary = new Error("injected primary team scenario failure");
  let observed;
  let cleanupCalls = 0;
  const diagnostics = await captureStderr(async () => {
    try {
      try {
        throw primary;
      } finally {
        await cleanupTeamFixture({
          fixture: "focused-team-fixture",
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

export async function assertTeamDrainFailureTeardown() {
  const snapshotFailure = new Error("injected team snapshot failure");
  const snapshot = await teardownWhilePrimaryFails(async () => { throw snapshotFailure; });
  assertPrimaryAndCleanup(snapshot);
  assert.equal(
    snapshot.diagnostics,
    `canvas team teardown could not drain lanes: ${snapshotFailure}\n`,
    "a lane snapshot failure is reported exactly once",
  );

  const drainFailure = new Error("injected second lane drain failure");
  const requests = [];
  const drain = await teardownWhilePrimaryFails(async (_fixture, args) => {
    requests.push(args);
    if (requests.length === 1) {
      return { body: { lanes: [
        { lane: "first", occupant: "first-occupant" },
        { lane: "second", occupant: "second-occupant" },
      ] } };
    }
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
    `canvas team teardown could not drain lanes: ${drainFailure}\n`,
    "a lane drain failure retains its original error",
  );

  const vacancyRequests = [];
  let vacancyCleanupCalls = 0;
  await cleanupTeamFixture({
    fixture: "vacant-team-fixture",
    resident: liveResident(),
    owner: { async cleanup() { vacancyCleanupCalls += 1; } },
    request: async (_fixture, args) => {
      vacancyRequests.push(args);
      return vacancyRequests.length === 1
        ? { body: { lanes: [
          { lane: "vacant", occupant: null },
          { lane: "live", occupant: "live-occupant" },
        ] } }
        : { body: {} };
    },
  });
  assert.deepEqual(
    vacancyRequests,
    [["lane"], ["lane", "drain", "live"]],
    "cleanup skips a seat that is already vacant and drains the remaining live seat",
  );
  assert.equal(vacancyCleanupCalls, 1, "vacant-seat cleanup still reaches the fixture owner");

  let cleanupCalls = 0;
  let queries = 0;
  const diagnostics = await captureStderr(() => cleanupTeamFixture({
    fixture: "signal-exited-team-fixture",
    resident: { child: { exitCode: null, signalCode: "SIGTERM" } },
    owner: { async cleanup() { cleanupCalls += 1; } },
    request: async () => { queries += 1; },
  }));
  assert.equal(queries, 0, "a signal-exited resident is not queried for its lanes");
  assert.equal(cleanupCalls, 1, "an already-exited resident still reaches owner cleanup");
  assert.equal(diagnostics, "", "an already-exited resident needs no drain diagnostic");
}
