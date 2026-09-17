import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import test from "node:test";

import "./canvas-build.test.mjs";
import "./canvas-coordinator.test.mjs";
import "./canvas-fixture.test.mjs";
import "./canvas-lifecycle.test.mjs";
import "./canvas-process-ownership.test.mjs";
import "./mosaic-placement-stability.test.mjs";
import "./team-menu.test.mjs";

test("every Canvas test has exactly one fast or slow classification", async () => {
  const directory = new URL("./", import.meta.url);
  const files = (await readdir(directory, { withFileTypes: true }))
    .filter((entry) => entry.isFile() && entry.name.endsWith(".test.mjs"))
    .map((entry) => entry.name)
    .sort();
  const source = await readFile(new URL(import.meta.url), "utf8");
  const fast = [...source.matchAll(/^import "\.\/([^"/]+\.test\.mjs)";$/gm)]
    .map((match) => match[1]);
  const registry = JSON.parse(
    await readFile(new URL("../../test-classification.json", import.meta.url), "utf8"),
  );
  const slow = registry.tests.flatMap((entry) => entry.command ?? [])
    .filter((argument) => /^app\/browser\/[^/]+\.test\.mjs$/.test(argument))
    .map((argument) => argument.slice("app/browser/".length));
  const selections = [...fast, ...slow];
  const counts = new Map(files.map((file) => [file, 0]));
  for (const file of selections) counts.set(file, (counts.get(file) ?? 0) + 1);

  assert.deepEqual(
    [...counts].filter(([, count]) => count !== 1),
    [],
    "each Canvas test file must appear once across the default fast suite and explicit slow IDs",
  );
  assert.equal(selections.length, files.length, "classification cannot name nonexistent files");
});
