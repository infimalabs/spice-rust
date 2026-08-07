// What the machine looked like at the moment a browser wait gave up.
//
// This exists because a red run could not answer the first question anyone asks
// of it: was the product wrong, or was the box busy. Eleven worktrees share this
// machine, so a load average several times the core count is ordinary here, and
// a reader looking at a bare timeout had no way to rule it in or out without
// re-running the suite -- by which time the load that produced it is gone.
//
// The measurements say load is very rarely the answer, which is exactly why the
// reading belongs in the report rather than in the bound. Driving
// offline-cycle.mjs, the smallest cycle that reproduces the reported flake, the
// offline flip runs 3ms at load average 26.8 on 12 cores and 78ms worst at load
// average 69.5 rising to 124.6 -- past the 85.52 that produced the failure this
// module was written for, and still twenty-five times inside the 2000ms bound,
// 3 of 3 passing. So a timeout on one of these waits is a defect or a tail
// event, and widening the bounds to cover load would have bought a green by
// making every genuine failure slower to notice, on every machine, forever.
//
// A reader who sees this line at factor 1 knows load is not the explanation. A
// reader who sees it far above 1 knows to ask for a second run before opening a
// regression. That distinction is the whole value, and it costs one line.
import { cpus, loadavg } from "node:os";

/// How far this machine is oversubscribed right now, never below one.
///
/// The one-minute average rather than the five, because it is the one that still
/// describes the machine the wait just ran on. Held at or above one so the
/// reading always reads as a multiplier of a machine that is keeping up.
function loadFactor() {
  const cores = cpus().length || 1;
  const [recent] = loadavg();
  return Math.max(1, recent / cores);
}

/// The load reading carried by a failed wait's report.
export function describeLoad() {
  const cores = cpus().length || 1;
  const [recent] = loadavg();
  return {
    loadAverage: Math.round(recent * 100) / 100,
    cores,
    factor: Math.round(loadFactor() * 100) / 100,
  };
}
