#![cfg(unix)]

//! The shared readiness wait, exercised on both of its endings.
//!
//! Every battery that waits for a spawned child now goes through one helper,
//! so the two ways that wait can end are proved here once rather than trusted
//! eight times.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[path = "support/readiness.rs"]
mod readiness;

use readiness::Readiness;

/// Wide enough that reaching it means no signal arrived at all.
const IDLE_HORIZON: Duration = Duration::from_secs(30);
/// Short enough that a battery waiting on nothing fails promptly.
const SPENT_HORIZON: Duration = Duration::from_millis(500);

static NEXT: AtomicU64 = AtomicU64::new(0);

/// One empty directory nothing writes to, so only the child can end a wait.
fn quiet_tree(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "spice-readiness-{label}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).expect("readiness fixture directory");
    path
}

/// The exit is a signal, not something the horizon eventually reveals.
///
/// Nothing writes to the watched tree, so the wait can only end early by
/// observing the process itself. A wait that returned by spending its horizon
/// would take a minute here instead of a second.
#[test]
fn a_child_that_ends_first_ends_the_wait_when_it_ends() {
    let watched = quiet_tree("exits");
    let mut child = Command::new("/bin/sh")
        .args(["-c", "sleep 1; exit 3"])
        .spawn()
        .expect("readiness fixture child starts");

    let started = Instant::now();
    let waited =
        readiness::wait_until_ready(&mut child, &watched, IDLE_HORIZON, "quiet state", || false);
    let elapsed = started.elapsed();

    assert!(matches!(waited, Readiness::Exited), "the child ended first");
    assert!(
        elapsed < IDLE_HORIZON / 2,
        "the exit was waited out rather than observed: {elapsed:?}"
    );
    assert_eq!(
        child.wait().expect("fixture child status").code(),
        Some(3),
        "the caller still owns the child and its diagnosis"
    );
    fs::remove_dir_all(&watched).expect("readiness fixture cleanup");
}

/// A state reached while the wait is arming is still observed.
///
/// The predicate is false when the helper first asks and true ever after, so
/// it holds exactly during the window between arming the watcher and blocking
/// on it. Nothing writes to the watched tree, so no event will ever arrive:
/// this returns only because the helper asks again after arming.
#[test]
fn a_state_reached_while_arming_needs_no_event() {
    let watched = quiet_tree("holds");
    let mut child = Command::new("/bin/sh")
        .args(["-c", "sleep 30"])
        .spawn()
        .expect("readiness fixture child starts");

    let asked = AtomicU64::new(0);
    let started = Instant::now();
    let waited =
        readiness::wait_until_ready(&mut child, &watched, IDLE_HORIZON, "held state", || {
            asked.fetch_add(1, Ordering::Relaxed) > 0
        });
    let elapsed = started.elapsed();

    assert!(
        matches!(waited, Readiness::Ready),
        "the state held once arming finished"
    );
    assert!(
        asked.load(Ordering::Relaxed) > 1,
        "the predicate was never asked again after arming"
    );
    assert!(
        elapsed < IDLE_HORIZON / 2,
        "an already-held state waited for an event: {elapsed:?}"
    );
    child.kill().expect("fixture child stops");
    child.wait().expect("fixture child is reaped");
    fs::remove_dir_all(&watched).expect("readiness fixture cleanup");
}

/// A state that never arrives fails by name rather than blocking forever.
#[test]
#[should_panic(expected = "absent state did not arrive within")]
fn a_state_that_never_arrives_fails_with_its_name() {
    let watched = quiet_tree("absent");
    let mut child = Command::new("/bin/sh")
        .args(["-c", "sleep 5"])
        .spawn()
        .expect("readiness fixture child starts");

    readiness::wait_until_ready(&mut child, &watched, SPENT_HORIZON, "absent state", || {
        false
    });
}
