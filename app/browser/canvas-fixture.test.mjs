import assert from "node:assert/strict";
import test from "node:test";

import { fixtureCommandEnv } from "./canvas-fixture.mjs";

function restoreEnvironment(name, value) {
  if (value === undefined) delete process.env[name];
  else process.env[name] = value;
}

test("fixture operator commands do not inherit the ambient Spice lane", () => {
  const previousLane = process.env.SPICE_RS_LANE;
  try {
    process.env.SPICE_RS_LANE = "ambient-agent-lane";

    const operator = fixtureCommandEnv();
    assert.equal(operator.SPICE_RS_LANE, undefined);

    const lane = fixtureCommandEnv("fixture-lane");
    assert.equal(lane.SPICE_RS_LANE, "fixture-lane");
  } finally {
    restoreEnvironment("SPICE_RS_LANE", previousLane);
  }
});
