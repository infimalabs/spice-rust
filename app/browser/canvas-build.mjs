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

async function installedSpice(resolvedGitCommonDirectory) {
  let common = resolvedGitCommonDirectory;
  if (common === undefined) {
    const { stdout } = await execute(
      "/usr/bin/git",
      ["rev-parse", "--path-format=absolute", "--git-common-dir"],
      { cwd: root },
    );
    common = stdout.endsWith("\n") ? stdout.slice(0, -1) : stdout;
  }
  const path = join(common, ".spice-install/bin/spice");
  const metadata = await lstat(path);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`Rust Spice appliance is not an executable regular file: ${path}`);
  }
  await access(path, constants.X_OK);
  return path;
}

export async function buildResident({ app = true, gitCommonDirectory, commandRoot = root } = {}) {
  try {
    const installed = !prebuiltBinary || app
      ? await installedSpice(gitCommonDirectory)
      : undefined;
    if (!prebuiltBinary) {
      const cargo = ["dev", "cargo", "--", "build", "--locked"];
      if (commandRoot !== root) {
        cargo.push("--manifest-path", join(root, "Cargo.toml"), "--target-dir", join(root, "target"));
      } else {
        cargo.push("--target-dir", "target");
      }
      cargo.push("-p", "spice", "--bin", "spice");
      await execute(installed, cargo, {
        cwd: commandRoot,
        maxBuffer: 16 * 1024 * 1024,
      });
    }
    if (app) {
      const xtask = commandRoot === root
        ? ["dev", "xtask", "--", "app"]
        : [
            "dev", "cargo", "--", "run", "--quiet", "--locked",
            "--manifest-path", join(root, "xtask/Cargo.toml"), "--bin", "xtask", "--",
            "__canvas-proof-app", root, root,
          ];
      await execute(installed, xtask, {
        cwd: commandRoot,
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
