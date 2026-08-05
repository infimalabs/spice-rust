//! One blocking wait for a spawned child to reach a state a battery needs.
//!
//! Four endings are reachable here, and telling them apart is the point. A
//! state that already holds returns Ready on the first read, before the child
//! is asked about at all. A state reached later returns Ready on the read that
//! finds it. A child that ends without reaching it returns Exited, and the
//! caller owns the diagnosis. A state that never arrives spends the horizon and
//! names itself in the panic.
//!
//! Nothing tells this wait when the state arrives; it looks. A filesystem
//! watcher was the alternative and it was measured out at [`recheck`], which
//! owns the one bound on how long the wait goes without looking.

use std::process::Child;
use std::time::{Duration, Instant};

#[path = "recheck.rs"]
pub(crate) mod recheck;

/// Which ending a readiness wait observed.
pub(super) enum Readiness {
    /// The awaited state holds.
    Ready,
    /// The child ended before the state held; the caller owns the diagnosis.
    Exited,
}

/// Block until `ready` holds, the child exits, or `horizon` is spent.
///
/// `what` names the awaited state in every failure message. Both reads happen
/// before anything is slept on, so a state that already holds is answered
/// without the wait ever pausing.
pub(super) fn wait_until_ready(
    child: &mut Child,
    horizon: Duration,
    what: &str,
    ready: impl Fn() -> bool,
) -> Readiness {
    let deadline = Instant::now() + horizon;
    loop {
        // The state winning over the child is deliberate: one that holds was
        // reached, whatever the child did afterwards, so a battery whose child
        // does its work and exits promptly is told its work is done rather than
        // that its child died.
        if ready() {
            return Readiness::Ready;
        }
        if ended(child, what) {
            return Readiness::Exited;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("{what} did not arrive within {horizon:?}")
        }
        recheck::pause(remaining);
    }
}

/// Whether the observed child has already ended.
fn ended(child: &mut Child, what: &str) -> bool {
    child
        .try_wait()
        .unwrap_or_else(|error| panic!("{what} child status: {error}"))
        .is_some()
}
