#![cfg(unix)]

//! The shared readiness wait, exercised on each of the ways it ends.
//!
//! Every battery that waits for a spawned child now goes through one helper, so
//! the ways that wait can end are proved here once rather than trusted eight
//! times. They are not interchangeable: a state that already holds returns on
//! the first read, a state reached later returns on the read that finds it, a
//! child that ends without reaching it returns Exited, and a state that never
//! arrives spends the horizon. A battery that hangs is told apart from one
//! whose child died only because the wait keeps those endings apart.
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

/// Wide enough that reaching it means the wait stopped reading its state.
const IDLE_HORIZON: Duration = Duration::from_secs(30);
/// Short enough that a battery waiting on nothing fails promptly.
const SPENT_HORIZON: Duration = Duration::from_millis(500);
/// Long enough to span several rechecks, so waiting it out is a real risk.
///
/// Nothing announces the state this bounds, so a wait that read it once and
/// then stopped would spend the whole horizon. The delay has to outlast enough
/// rechecks that returning inside the horizon means the wait kept reading.
const UNREPORTED_DELAY: Duration = Duration::from_millis(300);

static NEXT: AtomicU64 = AtomicU64::new(0);

/// A fixture child, the tree it writes into, and the handle that frees it.
///
/// The child blocks on its own input rather than on a timer, so nothing here is
/// decided by how long a sleep outlasts a wait: the test says when the child
/// moves. One fixture below panics by design, and a panic skips whatever
/// follows the call that raised it, so ending the child and the directory
/// belongs in a drop rather than at the end of a test body.
struct Held {
    child: Child,
    tree: PathBuf,
    input: RefCell<ChildStdin>,
}

impl Held {
    /// A child running `script` in a tree only that child ever writes to.
    ///
    /// `script` receives the tree as `$1` and is expected to block on a `read`
    /// until [`released_on_the_second_read`] answers it.
    fn new(label: &str, script: &str) -> Self {
        let tree = std::env::temp_dir().join(format!(
            "spice-readiness-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&tree).expect("readiness fixture directory");
        let mut child = Command::new("/bin/sh")
            .args(["-c", script, "sh"])
            .arg(&tree)
            .stdin(Stdio::piped())
            .spawn()
            .expect("readiness fixture child starts");
        let input = RefCell::new(child.stdin.take().expect("fixture child reads its release"));
        Self { child, tree, input }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        // A fixture that reached its own end has already reaped the child it
        // waited on, and a drop that panics while a panic is unwinding aborts
        // the process, so each ending is attempted and its failure discarded.
        let _killed = self.child.kill();
        let _reaped = self.child.wait();
        let _removed = fs::remove_dir_all(&self.tree);
    }
}

/// Answer `holds`, releasing the child once the wait has read its state twice.
///
/// Nothing else of the test's runs between those two reads. Releasing on the
/// second one puts everything the child then does strictly after both of them,
/// so neither can have seen it and a wait that returns at all returned on a
/// read it took later. The answer is taken before the release for the same
/// reason: reading it afterwards would race the child that the release just
/// started.
fn released_on_the_second_read<'a>(
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

/// A child that ends is observed, not left for the horizon to reveal.
///
/// The state never holds, so the wait can only end by looking at the process
/// itself, and the child does not begin ending until the wait has already read
/// twice. A wait that stopped looking would take half a minute here instead of
/// a moment.
#[test]
fn a_child_that_ends_first_ends_the_wait_when_it_ends() {
    let mut held = Held::new("exits", "read release; exit 3");
    let asked = AtomicU64::new(0);
    let ready = released_on_the_second_read(&held.input, &asked, || false);

    let started = Instant::now();
    let waited = readiness::wait_until_ready(&mut held.child, IDLE_HORIZON, "quiet state", ready);
    let elapsed = started.elapsed();

    assert!(matches!(waited, Readiness::Exited), "the child ended first");
    assert!(
        asked.load(Ordering::Relaxed) > 2,
        "the exit was answered without reading the state again"
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

/// A state reached between two reads is still observed.
///
/// The predicate is false when the wait first asks and true ever after, so it
/// comes to hold in the gap between two reads. Nothing exists to announce that
/// arrival to anyone: this returns only because the wait reads again.
#[test]
fn a_state_reached_between_two_reads_needs_no_signal() {
    let mut held = Held::new("holds", "read hold");
    let asked = AtomicU64::new(0);

    let started = Instant::now();
    let waited = readiness::wait_until_ready(&mut held.child, IDLE_HORIZON, "held state", || {
        asked.fetch_add(1, Ordering::Relaxed) > 0
    });
    let elapsed = started.elapsed();

    assert!(
        matches!(waited, Readiness::Ready),
        "the state held by the second read"
    );
    assert_eq!(
        asked.load(Ordering::Relaxed),
        2,
        "the predicate was never read a second time"
    );
    assert!(
        elapsed < IDLE_HORIZON / 2,
        "the state was waited out rather than read again: {elapsed:?}"
    );
}

/// A state that already holds is answered on the first read.
///
/// This is the first ending the module names, and only its between-reads
/// variant was covered: the predicate above is false on the first ask and true
/// ever after, so it proves the second read happens without ever proving the
/// first one answers. A predicate that simply holds is answered once, and the
/// child is never asked about at all.
#[test]
fn a_state_that_already_holds_is_answered_on_the_first_read() {
    let mut held = Held::new("prompt", "read hold");
    let asked = AtomicU64::new(0);

    let started = Instant::now();
    let waited = readiness::wait_until_ready(&mut held.child, IDLE_HORIZON, "prompt state", || {
        asked.fetch_add(1, Ordering::Relaxed);
        true
    });
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
/// A deletion cannot reach this claim. Removing either read reds a fixture
/// above, and the order between them decides nothing while the child is still
/// live; it decides only once the child has gone, which is the case this
/// fixture is. Swapping the two reads is what it pins.
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

    let waited =
        readiness::wait_until_ready(&mut held.child, IDLE_HORIZON, "outranked state", || {
            asked.fetch_add(1, Ordering::Relaxed);
            true
        });

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

/// A child that has already exited is answered on the first pass.
///
/// The child is reaped before the wait starts and the state never holds, so
/// reading the child is the only valid ending. Reaching the horizon instead
/// would wait out a state that nothing is left to bring about, and asking the
/// predicate again could not change the answer.
#[test]
fn a_child_that_already_exited_is_answered_on_the_first_pass() {
    let mut held = Held::new("already-exited", "read release; exit 7");
    writeln!(held.input.borrow_mut()).expect("fixture child takes its release");
    assert_eq!(
        held.child.wait().expect("fixture child status").code(),
        Some(7),
        "the child had not already exited when the wait began"
    );
    let asked = AtomicU64::new(0);

    let started = Instant::now();
    let waited = readiness::wait_until_ready(
        &mut held.child,
        IDLE_HORIZON,
        "already-exited child",
        || {
            asked.fetch_add(1, Ordering::Relaxed);
            false
        },
    );
    let elapsed = started.elapsed();

    assert!(
        matches!(waited, Readiness::Exited),
        "the child had already exited"
    );
    assert_eq!(
        asked.load(Ordering::Relaxed),
        1,
        "the never-held state was read again past the exited child"
    );
    assert!(
        elapsed < IDLE_HORIZON / 2,
        "an already-exited child spent the readiness horizon: {elapsed:?}"
    );
}

/// A state reached after the wait begins is observed without the child ending.
///
/// This is the ending every converted battery actually takes. The child creates
/// the awaited file only once released, which happens after both reads the
/// helper makes on its own, and then goes on blocking: the child is still live,
/// so nothing the wait already knew can account for the answer.
#[test]
fn a_state_reached_after_the_wait_begins_is_observed_before_the_horizon() {
    let mut held = Held::new("created", r#"read release; : > "$1/ready"; read hold"#);
    let awaited = held.tree.join("ready");
    let asked = AtomicU64::new(0);
    let ready = released_on_the_second_read(&held.input, &asked, || awaited.exists());

    let started = Instant::now();
    let waited = readiness::wait_until_ready(&mut held.child, IDLE_HORIZON, "created state", ready);
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
        "the creation was waited out rather than read: {elapsed:?}"
    );
    assert!(
        held.child
            .try_wait()
            .expect("fixture child status")
            .is_none(),
        "the child outlived the state, so its exit is not what ended the wait"
    );
}

/// A state whose arrival nothing reports is still observed.
///
/// Nothing writes to the tree here and the child never exits, so there is no
/// change any watcher could have delivered and no exit to observe. The state
/// turns on by itself, well after the wait has started and with nothing to
/// announce it, which is what a coalesced or dropped filesystem notification
/// looks like from inside a wait. A wait that read its state only when
/// something told it to would spend the entire horizon here and then name a
/// state that had held nearly all of it.
#[test]
fn a_state_no_signal_reports_is_still_observed() {
    let mut held = Held::new("unreported", "read hold");
    let armed = Instant::now();

    let waited =
        readiness::wait_until_ready(&mut held.child, IDLE_HORIZON, "unreported state", || {
            armed.elapsed() >= UNREPORTED_DELAY
        });
    let elapsed = armed.elapsed();

    assert!(
        matches!(waited, Readiness::Ready),
        "the unreported state was observed"
    );
    assert!(
        elapsed >= UNREPORTED_DELAY,
        "the wait returned before the state could hold: {elapsed:?}"
    );
    assert!(
        elapsed < IDLE_HORIZON / 10,
        "the unreported state was waited out rather than rechecked: {elapsed:?}"
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
/// The child and its tree outlive the panic this raises, which is why both are
/// ended from a drop: nothing after the call below runs.
#[test]
#[should_panic(expected = "absent state did not arrive within")]
fn a_state_that_never_arrives_fails_with_its_name() {
    let mut held = Held::new("absent", "read hold");

    readiness::wait_until_ready(&mut held.child, SPENT_HORIZON, "absent state", || false);
}
