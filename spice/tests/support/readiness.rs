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
pub(crate) mod process_exit;

use process_exit::ProcessExitObserver;

/// The longest a battery waits for a state only a callback can announce.
///
/// Nothing here decides when the state arrives, so this is sized from what
/// announcing one costs rather than from what publishing one ought to take. It
/// was measured by recording every wait's elapsed time on its way out, across
/// three rounds of the eight batteries that wait, on this eleven-lane machine
/// at load averages between 1.9 and 4.8.
///
/// The awaited state is usually already there: of 177 recorded waits, all but
/// three returned within 3.9 seconds. The three were 7.59, 17.14 and 17.45
/// seconds, and they are the reason this is not a small number. The battery
/// below measures the same delivery in isolation and puts its worst at 21.86
/// seconds, against 0.03 to 0.26 to arm the observer and 0.02 to 0.04 for the
/// process-exit signal in the same fixture. The tail is the filesystem
/// deciding when to speak, not a resident being slow to publish.
///
/// The ten seconds these batteries inherited from the poller they replaced sat
/// inside that tail. At that value the waits recorded 10.002 through 10.007
/// seconds six times in one three-round sweep, which is the ceiling rather
/// than an arrival, and consent_cli, steering, release_publish and resident
/// failed in two rounds of three. The same sweep at sixty seconds was green in
/// three of three and never waited longer than the 17.45 above. This value is
/// twice the slowest arrival ever seen here, sits between those two, and was
/// itself green in three rounds of three, so reaching it means no callback came
/// rather than that a slow one was cut off.
pub(super) const ARRIVAL_HORIZON: Duration = Duration::from_secs(45);

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
    // Reading the child here has a second purpose: one that is already gone is
    // answered without asking the kernel to watch it, which the kernel would
    // refuse. That refusal stays reachable, because the child may end in the
    // gap between this read and that registration, and the observer treats it
    // as the exit it is rather than as a failure.
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
