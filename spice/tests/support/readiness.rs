//! One blocking wait for a spawned child to reach a state a battery needs.
//!
//! Two real signals end this wait. The filesystem notification says the state
//! a battery is waiting for may now hold, and the process-exit observer says
//! the child that owed it is gone. Both arrive on one channel, so the wait
//! blocks on whichever comes first and carries no timer of its own beyond the
//! horizon that bounds the whole wait.

use std::path::Path;
use std::process::Child;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};

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
    // was reached, whatever the child did afterwards. Answering a child that
    // is already gone before arming also keeps the exit observer, which names
    // a live process, from being armed against one that is not.
    if ready() {
        return Readiness::Ready;
    }
    if ended(child, what) {
        return Readiness::Exited;
    }
    let (sender, receiver) = mpsc::channel::<Signal>();
    let changes = sender.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let _ = changes.send(match event {
            Ok(_) => Signal::Changed,
            Err(error) => Signal::WatchFailed(error.to_string()),
        });
    })
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
        match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Signal::Changed | Signal::Exited) => {}
            Ok(Signal::WatchFailed(error)) => panic!("{what} watcher failed: {error}"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("{what} did not arrive within {horizon:?}")
            }
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
