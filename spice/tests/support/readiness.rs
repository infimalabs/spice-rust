//! One blocking wait for a spawned child to reach a state a battery needs.
//!
//! Four endings are reachable here, and telling them apart is the point. A
//! state that already holds returns Ready on the first post-arm read. A state
//! reached later returns Ready after a direct kernel notification. A child that
//! ends first returns Exited on its process observer, and a state that never
//! arrives spends the horizon and names itself in the panic.
//!
//! The two completion sources feed one channel, so no time-driven recheck is a
//! hidden second mechanism. The kernel observer is armed before state is
//! read, closing the setup race; after that read, only a real kernel notification
//! or child exit can wake the receiver.

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

/// The failure guard around a state only a kernel event can announce.
///
/// This does not pace readiness: inotify or kqueue wakes the blocked waiter as
/// soon as the runtime directory changes. The horizon exists only to make an
/// absent publication or broken kernel notification fail by name.
///
/// The recursive FSEvents observer this replaced was disproved rather than
/// given more time. Under lane load, release_publish reached forty-five seconds
/// with `state_ready=true`, a live child, and zero change signals. The state had
/// already arrived; FSEvents had not announced it. An isolated rerun then
/// completed in 6.63 seconds, confirming that the variable was delivery, not
/// publication. A larger number could not repair that mechanism.
///
/// Ten seconds is therefore a failure budget around a direct event, not an
/// estimate of FSEvents latency. The mechanism battery derives its wider idle
/// horizon from this value so it can prove both prompt notification and loud
/// silence without inventing another bound.
pub(super) const ARRIVAL_HORIZON: Duration = Duration::from_secs(10);

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
    let observer = filesystem_event::DirectObserver::arm(watched, what);

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

    let (sender, receiver) = mpsc::channel::<Signal>();
    wait_for_change(observer, sender.clone());
    let exit = ProcessExitObserver::new(child.id());
    let exits = sender.clone();
    thread::spawn(move || {
        exit.wait();
        let _ = exits.send(Signal::Exited);
    });

    let mut changed_signals = 0_u64;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            timeout(what, horizon, changed_signals, ready(), ended(child, what))
        }
        let next_observer = match receiver.recv_timeout(remaining) {
            Ok(Signal::Changed) => {
                changed_signals += 1;
                // Re-arm before reading the state. If this change was
                // unrelated, a state arriving during the read is either seen
                // by that read or retained by the next kernel observer.
                Some(filesystem_event::DirectObserver::arm(watched, what))
            }
            Ok(Signal::Exited) => None,
            Ok(Signal::ObserverFailed(error)) => {
                panic!("{what} observer failed: {error}")
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                timeout(what, horizon, changed_signals, ready(), ended(child, what))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("{what} completion channel disconnected")
            }
        };
        // If a child publishes the state and exits in one burst, the reached
        // state remains the answer regardless of which signal arrived first.
        if ready() {
            return Readiness::Ready;
        }
        if ended(child, what) {
            return Readiness::Exited;
        }
        if let Some(observer) = next_observer {
            wait_for_change(observer, sender.clone());
        }
    }
}

/// Wait for one direct directory event without making the child observer wait
/// behind it.
fn wait_for_change(observer: filesystem_event::DirectObserver, sender: mpsc::Sender<Signal>) {
    thread::spawn(move || {
        let signal = match observer.wait() {
            Ok(()) => Signal::Changed,
            Err(error) => Signal::ObserverFailed(error.to_string()),
        };
        let _ = sender.send(signal);
    });
}

/// Report the complete terminal state without turning the horizon into a
/// second readiness mechanism.
fn timeout(
    what: &str,
    horizon: Duration,
    changed_signals: u64,
    state_ready: bool,
    child_ended: bool,
) -> ! {
    panic!(
        "{what} did not arrive within {horizon:?}: \
         state_ready={state_ready} child_ended={child_ended} changed_signals={changed_signals}"
    )
}

/// Whether the observed child has already ended.
fn ended(child: &mut Child, what: &str) -> bool {
    child
        .try_wait()
        .unwrap_or_else(|error| panic!("{what} child status: {error}"))
        .is_some()
}
