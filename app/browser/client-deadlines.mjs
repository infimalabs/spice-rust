// The connection deadlines the client sets for itself, read from the Rust source
// that defines them. Every browser bound derived from one of these was a
// hand-copied number until now, in two files that could not notice the original
// moving; reading them here means a bound cannot outlive the contract it cites.
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "src");
const sources = new Map();

/// Read one `u32` deadline out of the client, or refuse to guess at it.
///
/// A missing constant is not a zero and not a default. It means the client was
/// restructured and the relationship every bound below is justified by no longer
/// holds, so the only honest answer is to fail at import naming the constant --
/// before a suite starts measuring the app against a deadline it stopped keeping.
///
/// The file is part of the citation rather than a search path: a constant that
/// moved between modules changed owner, and finding it anywhere would let a
/// bound keep its justification while the thing justifying it was rewritten.
function readDeadlineMs(name, file) {
  if (!sources.has(file)) {
    sources.set(file, readFileSync(join(root, file), "utf8"));
  }
  const declared = sources.get(file).match(new RegExp(`\\b${name}: u32 = ([0-9_]+);`));
  if (!declared) {
    throw new Error(
      `${join(root, file)} no longer declares ${name}, so the browser bounds derived from it are unfounded`,
    );
  }
  return Number(declared[1].replaceAll("_", ""));
}

/// How long the client allows one connection attempt before it gives up.
///
/// A test bound under this expires before the client's own deadline does, so its
/// red says the suite is stricter than the contract rather than that anything is
/// broken. Any wait on a connection is therefore this plus slack, never less.
export const HANDSHAKE_TIMEOUT_MS = readDeadlineMs(
  "CONNECTION_HANDSHAKE_TIMEOUT_MILLIS",
  "browser.rs",
);

/// How long the socket may stay silent before the client probes it, and again
/// before an unanswered probe ends the connection.
///
/// One interval is not the detection bound. A quiet board is silent too, so the
/// first interval only buys the right to ask; the second is the one during which
/// silence means something. A half-open socket is therefore detected after two
/// of these, which is what [`HALF_OPEN_DETECT_MS`] spends.
export const LIVENESS_PROBE_MS = readDeadlineMs("LIVENESS_PROBE_MILLIS", "browser.rs");

/// The shortest the client ever sleeps before retrying.
///
/// This replaced a flat delay. The floor still holds -- no reconnect can be
/// observed sooner -- but it is only the floor for the *first* retry after an
/// admitted connection, because the client resets its ladder there and nowhere
/// else. Every bound below reaches the client in exactly that state, so every
/// one of them means this rung.
///
/// The ladder's ceiling is deliberately not exported alongside it. A wait
/// spanning an outage long enough to climb would be entitled to that instead and
/// would have to say so, but no browser bound spans one, and the ceiling is
/// already pinned where it is decided by `reconnect.rs`'s own tests. Exporting it
/// here would add a tripwire guarding a caller that does not exist.
export const RECONNECT_BASE_MS = readDeadlineMs("BASE_MILLIS", "reconnect.rs");
