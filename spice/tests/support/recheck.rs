//! The one bound every fixture wait puts on how long it goes without looking.
//!
//! Each battery that waits for a state to arrive owns its own clock, horizon,
//! observation and failure text, and those differ for good reasons. The one
//! thing they must not each decide is how long to leave that state unread, so
//! the decision lives here and they call it rather than restate it.

use std::thread;
use std::time::Duration;

/// How long a wait leaves its state unread before reading it again.
///
/// This is the only thing that observes an arriving state in these batteries. A
/// filesystem watcher was the other candidate and it was measured out rather
/// than argued out: the readiness, resident and steering suites -- 44 tests --
/// were run alternately, five runs of each configuration, at load averages
/// between 4.36 and 7.74 on this eleven-lane machine. With a 100ms poll watcher
/// they took 17.1, 15.8, 15.0, 14.6 and 14.7 seconds; with no watcher at all,
/// 18.5, 15.3, 14.7, 14.0 and 14.8 seconds. Every round of both was green. A
/// 0.02s gap between the two means sits well inside the 4.5s spread within a
/// single configuration, so a watcher buys nothing this constant was not
/// already delivering. The event-driven watcher the poller replaced, measured
/// the same way, was slower than both at 26.7, 19.4, 22.9, 20.5 and 17.5
/// seconds, which is what arming it costs.
///
/// The value came from the same kind of measurement. With these waits blocking
/// on a notification for their whole horizon, 14 of 43 arrivals timed out, and
/// every one of the 11 timeouts that were instrumented was a wait whose state
/// already held. Reading the state every 50ms instead took that to 0 timeouts
/// in 35 arrivals and the readiness suite from 34.5s to 19.0s.
const RECHECK: Duration = Duration::from_millis(50);

/// Leave the state unread for [`RECHECK`], or for whatever horizon is left.
///
/// `remaining` is the caller's whole remaining horizon, and returning long
/// before it is the point: nothing here is an answer, it is an invitation to
/// look again. Callers treat it that way, so a horizon is spent by their
/// observations rather than by one long sleep.
///
/// Sleeping `remaining` instead makes every caller blind for its whole horizon
/// at once, which is why no caller writes this bound itself.
pub(crate) fn pause(remaining: Duration) {
    thread::sleep(remaining.min(RECHECK));
}
