//! One blocking wait for a spawned child to reach a state a battery needs.
//!
//! Four endings are reachable here, and telling them apart is the point. A
//! state that already holds, or that comes to hold while the wait is arming,
//! returns Ready on an evaluation with no signal involved. A state reached
//! after that returns Ready on a filesystem notification, or on the recheck
//! beneath it when that notification is lost. A child that ends first returns
//! Exited on the process-exit observer. A state that never arrives spends the
//! horizon and names itself in the panic.
//!
//! The two signals arrive on one channel, so the wait blocks on whichever comes
//! first. It does not trust them to be complete: a dropped or coalesced
//! filesystem event would otherwise hold the wait until the horizon over a
//! state that already held, so the block is capped at [`RECHECK`] and the
//! horizon is spent by the state reads rather than by one long sleep.

use std::path::Path;
use std::process::Child;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

const WATCH_SCAN_INTERVAL: Duration = Duration::from_millis(100);

#[path = "process_exit.rs"]
mod process_exit;

use process_exit::ProcessExitObserver;

/// How long the wait will sit on a silent channel before reading state again.
///
/// The two signals remain the fast path and answer in microseconds; this is a
/// floor beneath them, not a replacement for them. A filesystem notification
/// can be coalesced or dropped, and a wait that re-reads its state only when
/// one arrives sleeps through a state that already holds. That is not
/// hypothetical here: measured across the resident batteries, every observed
/// timeout was a wait whose predicate was already true, so the horizon was
/// reporting a lost notification rather than an absent state. Deleting this to
/// make the wait purely signal-driven restores that hang.
pub(super) const RECHECK: Duration = Duration::from_millis(50);

/// Which ending a readiness wait observed.
pub(super) enum Readiness {
    /// The awaited state holds.
    Ready,
    /// The child ended before the state held; the caller owns the diagnosis.
    Exited,
}

/// One signal that can end a readiness wait.
enum Signal {
    /// The watched tree changed, so the state may now hold.
    Changed,
    /// The observed child exited.
    Exited,
    /// The watcher itself stopped being able to report.
    WatchFailed(String),
}

/// Block until `ready` holds, the child exits, or `horizon` is spent.
///
/// `watched` is the tree whose changes can make `ready` hold, and `what` names
/// the awaited state in every failure message. The predicate is evaluated once
/// after both signals are armed, so a state reached in that window is still
/// observed rather than waited out.
pub(super) fn wait_until_ready(
    child: &mut Child,
    watched: &Path,
    horizon: Duration,
    what: &str,
    ready: impl Fn() -> bool,
) -> Readiness {
    // The state winning over the child is deliberate: one that already holds
    // was reached, whatever the child did afterwards. Answering a reaped child
    // here also keeps the exit observer, which requires a live pid, from being
    // armed against a pid that is gone or has already been reused.
    if ready() {
        return Readiness::Ready;
    }
    if ended(child, what) {
        return Readiness::Exited;
    }
    let (sender, receiver) = mpsc::channel::<Signal>();
    let changes = sender.clone();
    let mut watcher = notify::PollWatcher::new(
        move |event: notify::Result<notify::Event>| {
            let _ = changes.send(match event {
                Ok(_) => Signal::Changed,
                Err(error) => Signal::WatchFailed(error.to_string()),
            });
        },
        notify::Config::default().with_poll_interval(WATCH_SCAN_INTERVAL),
    )
    .unwrap_or_else(|error| panic!("{what} watcher starts: {error}"));
    watcher
        .watch(watched, RecursiveMode::Recursive)
        .unwrap_or_else(|error| panic!("{what} watcher arms before the state can hold: {error}"));

    let observer = ProcessExitObserver::new(child.id());
    thread::spawn(move || {
        observer.wait();
        let _ = sender.send(Signal::Exited);
    });

    let deadline = Instant::now() + horizon;
    loop {
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
        match receiver.recv_timeout(remaining.min(RECHECK)) {
            Ok(Signal::Changed | Signal::Exited) => {}
            Ok(Signal::WatchFailed(error)) => panic!("{what} watcher failed: {error}"),
            // A quiet interval is not an answer, so the horizon is spent only
            // by the check at the top of the loop.
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                panic!("{what} watcher disconnected")
            }
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
