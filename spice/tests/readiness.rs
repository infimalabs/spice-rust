#![cfg(unix)]

//! The shared readiness wait, exercised on each of the four ways it ends.
//!
//! Every battery that waits for a spawned child now goes through one helper, so
//! the ways that wait can end are proved here once rather than trusted eight
//! times. They are not interchangeable: a state that already holds returns
//! without any signal, a state reached later returns on a filesystem event, a
//! child that ends first returns on the exit observer, and a state that never
//! arrives spends the horizon. A battery that hangs is told apart from one whose
//! child died only because the wait keeps those endings apart.
//!
//! Two of them can be reachable at the same instant, and which one comes back
//! then is an order rather than an ending, so the order is pinned here as well.

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
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

/// A fixture child, the empty tree it is watched in, and the handle that frees it.
///
/// The child blocks on its own input rather than on a timer, so nothing here is
/// decided by how long a sleep outlasts a wait: the test says when the child
/// moves. One fixture below panics by design, and a panic skips whatever
/// follows the call that raised it, so ending the child and the directory
/// belongs in a drop rather than at the end of a test body.
struct Held {
    child: Child,
    watched: PathBuf,
    input: RefCell<ChildStdin>,
}

impl Held {
    /// A child running `script` in a tree only that child ever writes to.
    ///
    /// `script` receives the tree as `$1` and is expected to block on a `read`
    /// until [`released_after_arming`] answers it.
    fn new(label: &str, script: &str) -> Self {
        let watched = std::env::temp_dir().join(format!(
            "spice-readiness-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&watched).expect("readiness fixture directory");
        let mut child = Command::new("/bin/sh")
            .args(["-c", script, "sh"])
            .arg(&watched)
            .stdin(Stdio::piped())
            .spawn()
            .expect("readiness fixture child starts");
        let input = RefCell::new(child.stdin.take().expect("fixture child reads its release"));
        Self {
            child,
            watched,
            input,
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // A fixture that reached its own end has already reaped the child it
        // waited on, and a drop that panics while a panic is unwinding aborts
        // the process, so each ending is attempted and its failure discarded.
        let _killed = self.child.kill();
        let _reaped = self.child.wait();
        let _removed = fs::remove_dir_all(&self.watched);
    }
}

/// Answer `holds`, releasing the child once the wait has armed its signals.
///
/// The helper asks once before arming and again immediately after, and nothing
/// else of the test's runs in between. Releasing on that second ask puts
/// everything the child then does strictly after both evaluations, so neither
/// of them can have seen it and a wait that returns at all returned on a
/// signal. The answer is taken before the release for the same reason: reading
/// it afterwards would race the child that the release just started.
fn released_after_arming<'a>(
    input: &'a RefCell<ChildStdin>,
    asked: &'a AtomicU64,
    holds: impl Fn() -> bool + 'a,
) -> impl Fn() -> bool + 'a {
    move || {
        let answer = holds();
        if asked.fetch_add(1, Ordering::Relaxed) == 1 {
            writeln!(input.borrow_mut()).expect("fixture child takes its release");
        }
        answer
    }
}

/// The exit is a signal, not something the horizon eventually reveals.
///
/// Nothing writes to the watched tree, so the wait can only end by observing
/// the process itself, and the child does not begin ending until the wait is
/// already armed. A wait that returned by spending its horizon would take half
/// a minute here instead of a moment.
#[test]
fn a_child_that_ends_first_ends_the_wait_when_it_ends() {
    let mut held = Held::new("exits", "read release; exit 3");
    let asked = AtomicU64::new(0);
    let ready = released_after_arming(&held.input, &asked, || false);

    let started = Instant::now();
    let waited = readiness::wait_until_ready(
        &mut held.child,
        &held.watched,
        IDLE_HORIZON,
        "quiet state",
        ready,
    );
    let elapsed = started.elapsed();

    assert!(matches!(waited, Readiness::Exited), "the child ended first");
    assert!(
        asked.load(Ordering::Relaxed) > 2,
        "the exit was seen by an evaluation rather than by the observer"
    );
    assert!(
        elapsed < IDLE_HORIZON / 2,
        "the exit was waited out rather than observed: {elapsed:?}"
    );
    assert_eq!(
        held.child.wait().expect("fixture child status").code(),
        Some(3),
        "the caller still owns the child and its diagnosis"
    );
}

/// A state reached while the wait is arming is still observed.
///
/// The predicate is false when the helper first asks and true ever after, so it
/// holds exactly during the window between arming the watcher and blocking on
/// it. Nothing writes to the watched tree, so no event will ever arrive: this
/// returns only because the helper asks again after arming.
#[test]
fn a_state_reached_while_arming_needs_no_event() {
    let mut held = Held::new("holds", "read hold");
    let asked = AtomicU64::new(0);

    let started = Instant::now();
    let waited = readiness::wait_until_ready(
        &mut held.child,
        &held.watched,
        IDLE_HORIZON,
        "held state",
        || asked.fetch_add(1, Ordering::Relaxed) > 0,
    );
    let elapsed = started.elapsed();

    assert!(
        matches!(waited, Readiness::Ready),
        "the state held once arming finished"
    );
    assert_eq!(
        asked.load(Ordering::Relaxed),
        2,
        "the predicate was never asked again after arming"
    );
    assert!(
        elapsed < IDLE_HORIZON / 2,
        "an already-held state waited for an event: {elapsed:?}"
    );
}

/// A state that already holds is answered before anything is armed.
///
/// This is the first ending the module names, and only its arming-window
/// variant was covered: the predicate above is false on the first ask and true
/// ever after, so it proves the second evaluation happens without ever proving
/// the first one answers. A predicate that simply holds is answered once, and
/// the child is never asked about at all.
#[test]
fn a_state_that_already_holds_is_answered_before_anything_is_armed() {
    let mut held = Held::new("prompt", "read hold");
    let asked = AtomicU64::new(0);

    let started = Instant::now();
    let waited = readiness::wait_until_ready(
        &mut held.child,
        &held.watched,
        IDLE_HORIZON,
        "prompt state",
        || {
            asked.fetch_add(1, Ordering::Relaxed);
            true
        },
    );
    let elapsed = started.elapsed();

    assert!(matches!(waited, Readiness::Ready), "the state already held");
    assert_eq!(
        asked.load(Ordering::Relaxed),
        1,
        "a state that already holds was asked about more than once"
    );
    assert!(
        elapsed < IDLE_HORIZON / 2,
        "an already-held state waited for something: {elapsed:?}"
    );
    assert!(
        held.child
            .try_wait()
            .expect("fixture child status")
            .is_none(),
        "the child was still live, so nothing about it can have answered"
    );
}

/// A state that already holds outranks a child that has already exited.
///
/// Both endings are available at the same instant here, so which one comes back
/// is decided by nothing but the order of the two guards. That order is the
/// claim the helper documents: a state that holds was reached, whatever the
/// child did afterwards, so a battery whose child does its work and exits
/// promptly is told its work is done rather than that its child died.
///
/// A deletion cannot reach this claim. Removing the pre-arm evaluation leaves
/// the fixture above green, because a state that holds is still answered at the
/// top of the loop; it changes the answer only once the child has gone, which
/// is the case this fixture is. Swapping the two guards is what it pins.
#[test]
fn a_state_that_already_holds_outranks_a_child_that_has_already_exited() {
    let mut held = Held::new("outranked", "read release; exit 5");
    writeln!(held.input.borrow_mut()).expect("fixture child takes its release");
    assert_eq!(
        held.child.wait().expect("fixture child status").code(),
        Some(5),
        "the child had not already exited when the wait began"
    );
    let asked = AtomicU64::new(0);

    let waited = readiness::wait_until_ready(
        &mut held.child,
        &held.watched,
        IDLE_HORIZON,
        "outranked state",
        || {
            asked.fetch_add(1, Ordering::Relaxed);
            true
        },
    );

    assert!(
        matches!(waited, Readiness::Ready),
        "a child that had already exited outranked a state that already held"
    );
    assert_eq!(
        asked.load(Ordering::Relaxed),
        1,
        "the state was not the first thing the wait asked about"
    );
}

/// A state reached after arming is delivered by the watcher.
///
/// This is the ending every converted battery actually takes, and the only one
/// that cannot be reached without a filesystem notification. The child creates
/// the awaited file only once released, which happens after both evaluations
/// the helper makes on its own, and then goes on blocking: the exit signal is
/// still owed, so the watcher is the sole thing that can wake the wait.
#[test]
fn a_state_reached_after_arming_arrives_on_the_watcher() {
    let mut held = Held::new("created", r#"read release; : > "$1/ready"; read hold"#);
    let awaited = held.watched.join("ready");
    let asked = AtomicU64::new(0);
    let ready = released_after_arming(&held.input, &asked, || awaited.exists());

    let started = Instant::now();
    let waited = readiness::wait_until_ready(
        &mut held.child,
        &held.watched,
        IDLE_HORIZON,
        "created state",
        ready,
    );
    let elapsed = started.elapsed();

    assert!(
        matches!(waited, Readiness::Ready),
        "the created state was reached"
    );
    assert!(
        asked.load(Ordering::Relaxed) > 2,
        "the state was answered before the child could have created it"
    );
    assert!(
        elapsed < IDLE_HORIZON / 2,
        "the creation was waited out rather than delivered: {elapsed:?}"
    );
    assert!(
        held.child
            .try_wait()
            .expect("fixture child status")
            .is_none(),
        "the child outlived the state, so its exit is not what ended the wait"
    );
}

/// A state that never arrives fails by name rather than blocking forever.
///
/// The child and the watched tree outlive the panic this raises, which is why
/// both are ended from a drop: nothing after the call below runs.
#[test]
#[should_panic(expected = "absent state did not arrive within")]
fn a_state_that_never_arrives_fails_with_its_name() {
    let mut held = Held::new("absent", "read hold");

    readiness::wait_until_ready(
        &mut held.child,
        &held.watched,
        SPENT_HORIZON,
        "absent state",
        || false,
    );
}
