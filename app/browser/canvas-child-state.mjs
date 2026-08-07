// One definition of whether Node has observed a Canvas child exit.
//
// A process terminated by a signal keeps `exitCode` null and records the
// terminating signal in `signalCode`, so neither field alone describes every
// exit. Lifecycle callers use the positive form to avoid each site inventing
// its own inverse "still running" predicate.
export function childExited(child) {
  return child.exitCode !== null || child.signalCode !== null;
}
