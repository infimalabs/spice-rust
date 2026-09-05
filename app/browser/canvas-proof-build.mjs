import assert from "node:assert/strict";

import { buildResident } from "./canvas-build.mjs";

assert.equal(process.argv.length, 4, "expected Git-common and command-root arguments");
await buildResident({ gitCommonDirectory: process.argv[2], commandRoot: process.argv[3] });
