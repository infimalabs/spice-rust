// Canonical app and resident build used by every real Canvas browser fixture.
import { execFile } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execute = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
export const root = resolve(here, "../..");
export const prebuiltBinary = process.env.SPICE_CANVAS_BINARY; // env-policy: allow
export const binary = prebuiltBinary ?? join(root, "target/debug/spice");

export async function buildResident({ app = true } = {}) {
  try {
    if (app) {
      await execute("cargo", ["xtask", "app"], {
        cwd: root,
        maxBuffer: 32 * 1024 * 1024,
      });
    }
    if (!prebuiltBinary) {
      await execute("cargo", ["build", "-p", "spice", "--bin", "spice"], {
        cwd: root,
        maxBuffer: 16 * 1024 * 1024,
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
