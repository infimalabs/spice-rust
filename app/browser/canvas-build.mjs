// Canonical app and resident build used by every real Canvas browser fixture.
import { execFile } from "node:child_process";
import { constants } from "node:fs";
import { access, lstat } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
export const root = resolve(here, "../..");
export const prebuiltBinary = process.env.SPICE_CANVAS_BINARY; // env-policy: allow
export const binary = prebuiltBinary ?? join(root, "target/debug/spice");

async function installedSpice() {
  const { stdout } = await execute(
    "/usr/bin/git",
    ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    { cwd: root },
  );
  const common = stdout.endsWith("\n") ? stdout.slice(0, -1) : stdout;
  const path = join(common, ".spice-install/bin/spice");
  const metadata = await lstat(path);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`Rust Spice appliance is not an executable regular file: ${path}`);
  }
  await access(path, constants.X_OK);
  return path;
}

export async function buildResident({ app = true } = {}) {
  try {
    const installed = !prebuiltBinary || app ? await installedSpice() : undefined;
    if (!prebuiltBinary) {
      await execute(installed, [
        "dev", "cargo", "--", "build", "--locked", "-p", "spice", "--bin", "spice",
      ], {
        cwd: root,
        maxBuffer: 16 * 1024 * 1024,
      });
    }
    if (app) {
      await execute(installed, ["dev", "xtask", "--", "app"], {
        cwd: root,
        maxBuffer: 32 * 1024 * 1024,
      });
    }
  } catch (error) {
    const stdout = error?.stdout ?? "<none>";
    const stderr = error?.stderr ?? "<none>";
    throw new Error(
      `canvas spec could not build its resident: stdout=${stdout} stderr=${stderr}`,
      { cause: error },
    );
  }
}
