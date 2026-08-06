// The connection deadlines the client sets for itself, read from the Rust source
// that defines them. Every browser bound derived from one of these was a
// hand-copied number until now, in two files that could not notice the original
// moving; reading them here means a bound cannot outlive the contract it cites.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const source = join(dirname(fileURLToPath(import.meta.url)), "..", "src", "browser.rs");
const declarations = readFileSync(source, "utf8");

/// Read one `u32` deadline out of the client, or refuse to guess at it.
///
/// A missing constant is not a zero and not a default. It means the client was
/// restructured and the relationship every bound below is justified by no longer
/// holds, so the only honest answer is to fail at import naming the constant --
/// before a suite starts measuring the app against a deadline it stopped keeping.
function readDeadlineMs(name) {
  const declared = declarations.match(new RegExp(`\\b${name}: u32 = ([0-9_]+);`));
  if (!declared) {
    throw new Error(
      `${source} no longer declares ${name}, so the browser bounds derived from it are unfounded`,
    );
  }
  return Number(declared[1].replaceAll("_", ""));
}

/// How long the client allows one connection attempt before it gives up.
///
/// A test bound under this expires before the client's own deadline does, so its
/// red says the suite is stricter than the contract rather than that anything is
/// broken. Any wait on a connection is therefore this plus slack, never less.
export const HANDSHAKE_TIMEOUT_MS = readDeadlineMs("CONNECTION_HANDSHAKE_TIMEOUT_MILLIS");

/// How long the client sleeps before retrying a dropped socket.
///
/// This is a flat delay with no backoff, slept before every retry, so it is a
/// floor rather than an estimate: no reconnect can be observed sooner than this,
/// and a bound near the offline flip's few milliseconds would fail every time.
export const RECONNECT_DELAY_MS = readDeadlineMs("RECONNECT_DELAY_MILLIS");
