import assert from "node:assert/strict";
import test from "node:test";

import { settleAndArmTransitionWitness } from "./mosaic-transitions.mjs";

test("late measurement settlement completes before a backfill probe can arm", async () => {
  const calls = [];
  let releaseSettlement;
  const settled = new Promise((resolve) => { releaseSettlement = resolve; });
  const witness = { page: "page", plane: "plane" };
  const pending = settleAndArmTransitionWitness(witness, {
    async settle(page, plane) {
      calls.push(["settle", page, plane]);
      await settled;
    },
    async scroll(page, plane) {
      calls.push(["scroll", page, plane]);
      return "scroll-authority";
    },
    async arm(plane) {
      calls.push(["arm", plane]);
      return "placement-authority";
    },
  });

  await Promise.resolve();
  assert.deepEqual(calls, [["settle", "page", "plane"]]);
  releaseSettlement();
  assert.deepEqual(await pending, {
    witness,
    scroll: "scroll-authority",
    armed: "placement-authority",
  });
  assert.deepEqual(calls, [
    ["settle", "page", "plane"],
    ["scroll", "page", "plane"],
    ["arm", "plane"],
  ]);
});
