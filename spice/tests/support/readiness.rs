//! One blocking wait for a spawned child to reach a state a battery needs.
//!
//! Four endings are reachable here, and telling them apart is the point. A
//! state that already holds returns Ready on the first post-arm read. A state
//! reached later returns Ready after a filesystem callback. A child that ends
//! first returns Exited on its process observer, and a state that never arrives
//! spends the horizon and names itself in the panic.
//!
//! The two completion sources feed one channel, so no time-driven recheck is a
//! hidden second mechanism. The filesystem observer is armed before state is
//! read, closing the setup race; after that read, only a real callback or child
//! exit can wake the receiver.

use std::path::Path;
use std::process::Child;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[path = "filesystem_event.rs"]
pub(crate) mod filesystem_event;
#[path = "process_exit.rs"]
mod process_exit;

use process_exit::ProcessExitObserver;

/// Which ending a readiness wait observed.
pub(super) enum Readiness {
    /// The awaited state holds.
    Ready,
    /// The child ended before the state held; the caller owns the diagnosis.
    Exited,
}

/// One explicit completion that can wake a readiness wait.
enum Signal {
    /// The watched tree changed, so the state may now hold.
    Changed,
    /// The observed child exited.
    Exited,
    /// The native observer reported that it can no longer deliver changes.
    ObserverFailed(String),
}

/// Block until `ready` holds, the child exits, or `horizon` is spent.
///
/// `watched` is the tree whose changes can make `ready` hold, and `what` names
/// the awaited state in every failure. The observer is armed before the first
/// state read, so an already-held state and an arrival during setup are both
/// answered without a missed-event window.
pub(super) fn wait_until_ready(
    child: &mut Child,
    watched: &Path,
    horizon: Duration,
    what: &str,
    ready: impl Fn() -> bool,
) -> Readiness {
    let deadline = Instant::now() + horizon;
    let (sender, receiver) = mpsc::channel::<Signal>();
    let changes = sender.clone();
    let _observer = filesystem_event::arm(watched, what, move |event| {
        let signal = match event {
            Ok(_) => Signal::Changed,
            Err(error) => Signal::ObserverFailed(error.to_string()),
        };
        let _ = changes.send(signal);
    });

    // The state winning over the child is deliberate: one that holds was
    // reached, whatever the child did afterwards. This is also the immediate
    // post-arm read that closes the observer setup race.
    if ready() {
        return Readiness::Ready;
    }
    if ended(child, what) {
        return Readiness::Exited;
    }

    let exit = ProcessExitObserver::new(child.id());
    thread::spawn(move || {
        exit.wait();
        let _ = sender.send(Signal::Exited);
    });

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            panic!("{what} did not arrive within {horizon:?}")
        }
        match receiver.recv_timeout(remaining) {
            Ok(Signal::Changed | Signal::Exited) => {}
            Ok(Signal::ObserverFailed(error)) => {
                panic!("{what} observer failed: {error}")
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("{what} did not arrive within {horizon:?}")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("{what} completion channel disconnected")
            }
        }
        // If a child publishes the state and exits in one burst, the reached
        // state remains the answer regardless of which callback arrived first.
        if ready() {
            return Readiness::Ready;
        }
        if ended(child, what) {
            return Readiness::Exited;
        }
    }
}

/// Whether the observed child has already ended.
fn ended(child: &mut Child, what: &str) -> bool {
    child
        .try_wait()
        .unwrap_or_else(|error| panic!("{what} child status: {error}"))
        .is_some()
}
