//! The one bound every fixture wait puts on a block it might not be woken from.
//!
//! Each battery that waits on a filesystem notification owns its own clock,
//! horizon, observation and failure text, and those differ for good reasons. The
//! one thing they must not each decide is how long to trust a signal that may
//! never come, so that decision lives here and they call it rather than restate
//! it.

use std::sync::mpsc;
use std::time::Duration;

/// How long a wait will sit on a silent channel before reading its state again.
///
/// The signal remains the fast path and answers in microseconds; this is a floor
/// beneath it, not a replacement for it. A filesystem notification can be
/// coalesced or dropped, and a wait that re-reads its state only when one
/// arrives sleeps through a state that already holds. That is measured rather
/// than hypothetical: across the resident batteries every observed timeout was a
/// wait whose predicate was already true, so the horizon was reporting a lost
/// notification rather than an absent state.
const RECHECK: Duration = Duration::from_millis(50);

/// Block for one signal, giving up after [`RECHECK`] however much horizon is left.
///
/// `remaining` is the caller's whole remaining horizon, and returning early from
/// it is the point: a timeout here is not an answer, it is an invitation to look
/// again. Callers treat it that way, so the horizon is spent by their
/// observations rather than by one long sleep that a dropped notification would
/// make unbounded.
///
/// Passing `remaining` straight to `recv_timeout` instead restores that hang in
/// every caller at once, which is why no caller writes this cap itself.
pub(crate) fn capped_recv<T>(
    receiver: &mpsc::Receiver<T>,
    remaining: Duration,
) -> Result<T, mpsc::RecvTimeoutError> {
    receiver.recv_timeout(remaining.min(RECHECK))
}
