import assert from "node:assert/strict";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath, pathToFileURL } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

function restoreEnvironment(name, value) {
  if (value === undefined) delete process.env[name];
  else process.env[name] = value;
}

test("a prebuilt resident cannot absorb exact-appliance app publication", async () => {
  const fixture = await mkdtemp(join(tmpdir(), "spice-canvas-build-"));
  const browser = join(fixture, "app/browser");
  const modulePath = join(browser, "canvas-build.mjs");
  const ledger = join(fixture, "app-publication.argv");
  const previousBinary = process.env.SPICE_CANVAS_BINARY;
  const previousLedger = process.env.SPICE_APP_CALL_LEDGER;

  try {
    await mkdir(browser, { recursive: true });
    await writeFile(modulePath, await readFile(join(here, "canvas-build.mjs")));

    const appliance = join(fixture, ".git/.spice-install/bin/spice");
    await mkdir(dirname(appliance), { recursive: true });
    await writeFile(appliance, [
      "#!/bin/sh",
      "/usr/bin/printf '%s\\n' \"$*\" > \"$SPICE_APP_CALL_LEDGER\"",
      "/usr/bin/printf '%s\\n' 'exact appliance rejected app publication' >&2",
      "exit 23",
      "",
    ].join("\n"));
    await chmod(appliance, 0o755);

    process.env.SPICE_CANVAS_BINARY = "/usr/bin/true";
    process.env.SPICE_APP_CALL_LEDGER = ledger;
    const build = await import(pathToFileURL(modulePath));

    await assert.rejects(build.buildResident({
      app: true,
      gitCommonDirectory: join(fixture, ".git"),
    }), (error) => {
      assert.equal(error.cause?.code, 23);
      assert.match(error.message, /stderr=exact appliance rejected app publication/);
      return true;
    });
    assert.equal(await readFile(ledger, "utf8"), "dev xtask -- app\n");
  } finally {
    restoreEnvironment("SPICE_CANVAS_BINARY", previousBinary);
    restoreEnvironment("SPICE_APP_CALL_LEDGER", previousLedger);
    await rm(fixture, { recursive: true, force: true });
  }
});
