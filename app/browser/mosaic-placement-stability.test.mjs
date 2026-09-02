import assert from "node:assert/strict";
import test from "node:test";

import {
  assertCanonicalPlacementsStable,
  assertDeclaredPlacementMovement,
  captureCanonicalPlacements,
} from "./mosaic-placement-stability.mjs";

function placed(key, tuple) {
  return { key, tuple, bytes: JSON.stringify(tuple) };
}

function planeWithTrack(track) {
  return {
    evaluate: (capture, selected) => capture({
      querySelectorAll: () => [{
        dataset: {
          base: "0",
          mosaicCardKey: "card",
          rows: "4",
          span: "3",
          track,
        },
      }],
    }, selected),
  };
}

test("canonical placement capture admits only normalized safe integer spellings", async () => {
  for (const [raw, expected] of [["0", 0], ["7", 7], ["-7", -7]]) {
    assert.deepEqual(await captureCanonicalPlacements(planeWithTrack(raw)), [{
      key: "card",
      tuple: [expected, 0, 3, 4],
      bytes: JSON.stringify([expected, 0, 3, 4]),
    }]);
  }

  for (const raw of [
    undefined,
    "",
    " ",
    " 0",
    "0 ",
    "1.0",
    "NaN",
    "Infinity",
    "-Infinity",
    "01",
    "-0",
    "+1",
    "1e0",
    "9007199254740992",
  ]) {
    await assert.rejects(
      captureCanonicalPlacements(planeWithTrack(raw)),
      /card has a non-canonical committed placement/,
      `rejects ${JSON.stringify(raw)}`,
    );
  }
});

test("stable placement names the first unexpected card movement", () => {
  const before = [placed("a", [0, 0, 3, 4]), placed("b", [3, 0, 3, 4])];
  const after = [placed("a", [0, 1, 3, 4]), placed("b", [3, 2, 3, 4])];

  assert.throws(
    () => assertCanonicalPlacementsStable(before, after, "content-only update"),
    /card "a" moved unexpectedly; before=\[0,0,3,4\] after=\[0,1,3,4\]/,
  );
});

test("declared movement requires every exact target tuple and no unaffected movement", () => {
  const movement = Object.freeze({
    cause: "the target grows through an overlapping predecessor",
    label: "frozen content growth",
    targets: Object.freeze([
      Object.freeze({ key: "ripple", tuple: Object.freeze([0, -2, 6, 3]) }),
      Object.freeze({ key: "target", tuple: Object.freeze([0, 1, 6, 5]) }),
    ]),
  });
  const before = [
    placed("ripple", [0, 0, 6, 3]),
    placed("target", [0, 3, 6, 3]),
    placed("stable", [6, 0, 6, 3]),
  ];
  const after = [
    placed("ripple", [0, -2, 6, 3]),
    placed("target", [0, 1, 6, 5]),
    placed("stable", [6, 0, 6, 3]),
  ];

  assert.doesNotThrow(() => assertDeclaredPlacementMovement(before, after, movement));
  assert.throws(
    () => assertDeclaredPlacementMovement(before, [
      placed("ripple", [99, -99, 12, 999]),
      after[1],
      after[2],
    ], movement),
    /target "ripple" moved to an unexpected placement.*expected=\[0,-2,6,3\] actual=\[99,-99,12,999\]/,
  );
  assert.throws(
    () => assertDeclaredPlacementMovement(before, [
      after[0],
      after[1],
      placed("stable", [6, -1, 6, 3]),
    ], movement),
    /card "stable" moved unexpectedly.*before=\[6,0,6,3\] after=\[6,-1,6,3\]/,
  );
});
