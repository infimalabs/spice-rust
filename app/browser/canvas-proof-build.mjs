import assert from "node:assert/strict";

import { buildResident } from "./canvas-build.mjs";

assert.equal(process.argv.length, 3, "expected one Git-common argument");
await buildResident({ gitCommonDirectory: process.argv[2] });
