#![cfg(unix)]

//! The shared readiness wait, exercised on each of the ways it ends.
//!
//! Every battery that waits for a spawned child now goes through one helper, so
//! the ways that wait can end are proved here once rather than trusted eight
//! times. They are not interchangeable: a state that already holds returns on
//! the first post-arm read, a state reached later returns on a direct kernel
//! notification, a child that ends first returns on its process signal, and a
//! state that never arrives spends the horizon. A battery that hangs is told
//! apart from one whose child died only because the wait keeps those endings
//! apart.
//!
//! Two of them can be reachable at the same instant, and which one comes back
//! then is an order rather than an ending, so the order is pinned here as well.
//! So is the order the wait rests on before reaching any of them: the observer
//! is armed before the first state read. A child already gone by the time the
//! kernel is asked to watch it is the last thing proved here, because that is
//! an ending the kernel reports as a refusal.

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[path = "support/readiness.rs"]
mod readiness;

use readiness::Readiness;
use readiness::process_exit::ProcessExitObserver;

/// Wide enough that reaching it means no completion signal arrived at all.
///
/// This is deliberately wider than the horizon the batteries allow. A delivery
/// slower than they tolerate is a fact about this machine that this battery
/// should report, with the number it took, rather than one its own bound cuts
/// off: the assertions below fail at [`readiness::ARRIVAL_HORIZON`] and leave
/// the same margin again above it for the wait to keep running into.
const IDLE_HORIZON: Duration = Duration::from_secs(readiness::ARRIVAL_HORIZON.as_secs() * 2);
/// Short enough that a battery waiting on nothing fails promptly.
const SPENT_HORIZON: Duration = Duration::from_millis(500);

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
    /// until [`released_after_arming`] answers it.
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

/// Answer `holds`, releasing the child on the first post-arm state read.
///
/// The observer is already armed when `holds` is first called. Taking the
/// answer before releasing the child puts everything the child does after that
/// read, so a later answer can only follow a real child-exit or kernel
/// notification.
fn released_after_arming<'a>(
    input: &'a RefCell<ChildStdin>,
    asked: &'a AtomicU64,
    holds: impl Fn() -> bool + 'a,
) -> impl Fn() -> bool + 'a {
    move || {
        let answer = holds();
        if asked.fetch_add(1, Ordering::Relaxed) == 0 {
            writeln!(input.borrow_mut()).expect("fixture child takes its release");
        }
        answer
    }
}

/// A child that ends is observed, not left for the horizon to reveal.
///
/// The state never holds, so the wait can only end on the process observer, and
/// the child does not begin ending until the filesystem observer is armed. A
/// wait that missed that completion would take half a minute here.
#[test]
fn a_child_that_ends_first_ends_the_wait_when_it_ends() {
    let mut held = Held::new("exits", "read release; exit 3");
    let asked = AtomicU64::new(0);
    let ready = released_after_arming(&held.input, &asked, || false);

    let started = Instant::now();
    let waited = readiness::wait_until_ready(
        &mut held.child,
        &held.tree,
        IDLE_HORIZON,
        "quiet state",
        ready,
    );
    let elapsed = started.elapsed();

    assert!(matches!(waited, Readiness::Exited), "the child ended first");
    assert!(
        asked.load(Ordering::Relaxed) > 1,
        "the exit was answered without a completion signal"
    );
    assert!(
        elapsed < readiness::ARRIVAL_HORIZON,
        "the exit was waited out rather than observed: {elapsed:?}"
    );
    assert_eq!(
        held.child.wait().expect("fixture child status").code(),
        Some(3),
        "the caller still owns the child and its diagnosis"
    );
}

/// A state reached during the post-arm read needs no later notification.
///
/// The predicate creates and then reads the state itself. This can only happen
/// after the observer is armed, and it proves the one immediate state read
/// closes the setup window without waiting for the notification it just caused.
#[test]
fn a_state_reached_during_the_post_arm_read_needs_no_notification() {
    let mut held = Held::new("holds", "read hold");
    let reached = held.tree.join("reached");
    let asked = AtomicU64::new(0);

    let started = Instant::now();
    let waited = readiness::wait_until_ready(
        &mut held.child,
        &held.tree,
        IDLE_HORIZON,
        "held state",
        || {
            asked.fetch_add(1, Ordering::Relaxed);
            fs::write(&reached, b"ready").expect("post-arm state writes");
            reached.is_file()
        },
    );
    let elapsed = started.elapsed();

    assert!(
        matches!(waited, Readiness::Ready),
        "the state held during the post-arm read"
    );
    assert_eq!(
        asked.load(Ordering::Relaxed),
        1,
        "a notification caused an unnecessary second state read"
    );
    assert!(
        elapsed < readiness::ARRIVAL_HORIZON,
        "the post-arm state read waited for its own notification: {elapsed:?}"
    );
}

/// A state that already holds is answered on the first post-arm read.
///
/// The fixture above creates its state during the post-arm read, so it proves
/// that setup window without proving an already-held state is answered. A
/// predicate that simply holds is read once, and the child is never asked about
/// at all.
#[test]
fn a_state_that_already_holds_is_answered_on_the_first_post_arm_read() {
    let mut held = Held::new("prompt", "read hold");
    let asked = AtomicU64::new(0);

    let started = Instant::now();
    let waited = readiness::wait_until_ready(
        &mut held.child,
        &held.tree,
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
        elapsed < readiness::ARRIVAL_HORIZON,
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
/// Removing the post-arm state read reds fixtures above, but its order against
/// the child check matters only once the child has gone, which is the case this
/// fixture supplies. Swapping those two checks is what it pins.
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
        &held.tree,
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
        &held.tree,
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
        elapsed < readiness::ARRIVAL_HORIZON,
        "an already-exited child spent the readiness horizon: {elapsed:?}"
    );
}

/// A state reached after arming arrives on the kernel notification.
///
/// This is the ending every converted battery actually takes. The child creates
/// the awaited file only once released, which happens after both reads the
/// helper makes on its own, and then goes on blocking: the child is still live,
/// so nothing the wait already knew can account for the answer.
#[test]
fn a_state_reached_after_arming_arrives_on_the_kernel_notification() {
    let mut held = Held::new("created", r#"read release; : > "$1/ready"; read hold"#);
    let awaited = held.tree.join("ready");
    let asked = AtomicU64::new(0);
    let ready = released_after_arming(&held.input, &asked, || awaited.exists());

    let started = Instant::now();
    let waited = readiness::wait_until_ready(
        &mut held.child,
        &held.tree,
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
        asked.load(Ordering::Relaxed) > 1,
        "the state was answered before the child could have created it"
    );
    assert!(
        elapsed < readiness::ARRIVAL_HORIZON,
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

/// The observer is armed before the first state read, not after it.
///
/// The watch root does not exist when the wait is called, and the first state
/// read is what creates it. Arming first therefore fails the way
/// [`a_missing_watch_root_fails_observer_setup_loudly`] fails, while arming
/// after either first-pass read would find the root already there and answer
/// normally. The ending proves the order because it is the one ending the other
/// order cannot produce.
///
/// Proving the same order by its consequence — an arrival that lands after the
/// first read reaching the wait anyway — was measured and rejected. It needs
/// the arrival to be invisible to a late-armed observer, and on this machine a
/// stream armed one second after a change still woke on it twice in fifteen
/// tries, and once in fifteen at two seconds. No pause makes that ending
/// certain, so the order is pinned by one that does not depend on it.
#[test]
#[should_panic(expected = "root-creating state observer arms on")]
fn the_observer_is_armed_before_the_first_state_read() {
    let mut held = Held::new("armed-first", "read hold");
    let root = held.tree.join("root");
    let created = root.clone();

    readiness::wait_until_ready(
        &mut held.child,
        &root,
        IDLE_HORIZON,
        "root-creating state",
        move || {
            fs::create_dir_all(&created).expect("the first state read creates the watch root");
            true
        },
    );
}

/// An exit observer for a process already gone answers instead of refusing.
///
/// The wait reads the child before asking the kernel to watch it, so the
/// shipped order reaches this only when the child ends inside that gap, which
/// no fixture can schedule. What refuses is the registration, and it refuses
/// the same way whatever opened the gap, so it is asked here directly: a reaped
/// pid can no longer be watched, and that refusal is the exit having already
/// happened rather than a failure to observe one.
///
/// This names the tolerant constructor the wait uses, which is the only caller
/// that owns the gap above. Every other caller gets the strict one, pinned by
/// [`an_exit_observer_armed_on_a_gone_process_refuses_by_name`].
#[test]
fn an_exit_observer_for_a_gone_process_answers_rather_than_refusing() {
    let mut held = Held::new("gone", "read release; exit 11");
    writeln!(held.input.borrow_mut()).expect("fixture child takes its release");
    let departed = held.child.id();
    assert_eq!(
        held.child.wait().expect("fixture child status").code(),
        Some(11),
        "the child was not reaped before its pid was observed"
    );

    let (answered, answer) = mpsc::channel();
    thread::spawn(move || {
        ProcessExitObserver::or_ended(departed).wait();
        let _ = answered.send(());
    });

    answer
        .recv_timeout(SPENT_HORIZON)
        .expect("a gone process is answered rather than watched for an exit it already had");
}

/// The strict observer names a process already gone rather than answering.
///
/// The same registration refusal reaches both constructors, so nothing but the
/// constructor chosen decides what it means. A caller that arms an observer on
/// a process it is about to end has already lost the thing it meant to watch,
/// and would otherwise pass having observed nothing: it would signal a pid the
/// kernel no longer knows, wait on an `Ended` it was handed for free, and read
/// whatever the departed process happened to leave. That is a live regression
/// shape, not a hypothetical, which is why the refusal is spelled rather than
/// left to each caller to check.
#[test]
#[should_panic(expected = "had already ended before its exit observer was armed")]
fn an_exit_observer_armed_on_a_gone_process_refuses_by_name() {
    let mut held = Held::new("refused", "read release; exit 12");
    writeln!(held.input.borrow_mut()).expect("fixture child takes its release");
    let departed = held.child.id();
    assert_eq!(
        held.child.wait().expect("fixture child status").code(),
        Some(12),
        "the child was not reaped before its pid was observed"
    );

    ProcessExitObserver::new(departed);
}

/// A state that never arrives fails by name rather than blocking forever.
///
/// The child and its tree outlive the panic this raises, which is why both are
/// ended from a drop: nothing after the call below runs.
#[test]
#[should_panic(expected = "absent state did not arrive within")]
fn a_state_that_never_arrives_fails_with_its_name() {
    let mut held = Held::new("absent", "read hold");

    readiness::wait_until_ready(
        &mut held.child,
        &held.tree,
        SPENT_HORIZON,
        "absent state",
        || false,
    );
}

/// Observer setup failure is reported immediately rather than becoming a hang.
#[test]
#[should_panic(expected = "broken observer observer arms on")]
fn a_missing_watch_root_fails_observer_setup_loudly() {
    let mut held = Held::new("broken-observer", "read hold");
    let missing = held.tree.join("missing");

    readiness::wait_until_ready(
        &mut held.child,
        &missing,
        IDLE_HORIZON,
        "broken observer",
        || false,
    );
}
