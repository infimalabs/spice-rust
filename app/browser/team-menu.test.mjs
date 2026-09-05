import assert from "node:assert/strict";
import test from "node:test";

import { captureCursorFailureArtifact } from "./team-menu.mjs";

test("cursor assertion failures capture their engine and viewport before rethrow", async () => {
  const captures = [];
  const page = {
    async screenshot(options) {
      captures.push(options);
    },
  };
  const failure = new Error("closed cursor count regressed");
  const path = "artifacts/canvas/canvas-menu-cursor-failure-firefox-332x720.png";

  await assert.rejects(
    captureCursorFailureArtifact(page, path, failure),
    (error) => error === failure,
  );
  assert.deepEqual(captures, [{ path, fullPage: false }]);
});
