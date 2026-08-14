use super::*;

/// Read from a real commit rather than from githooks(5): the transaction this
/// guard inspects and the transaction that holds the shared lock are disjoint.
#[test]
fn only_the_queue_naming_no_branch_is_handed_over_under_the_shared_lock() {
    let fixture = Fixture::history("locked");
    let log = fixture.record_lock_state();
    fixture.stage("four");
    fixture.expect(&["commit", "--quiet", "-m", "four"]);

    let recorded = fs::read_to_string(&log).expect("the hook recorded its states");
    let mut held = 0;
    let mut named_branch_free = 0;
    for line in recorded.lines() {
        let (state, rest) = line.split_once(' ').expect("each record is tagged");
        let (lock, queue) = rest.split_once(' ').expect("each record names the lock");
        if lock == "held" {
            held += 1;
            assert!(
                !queue.contains(LOCAL_PREFIX),
                "a queue naming the branch was handed over under the shared \
                 lock, so the guard's early answer is unsound: {line}"
            );
        } else if queue.contains(LOCAL_PREFIX) && state == PREPARED {
            named_branch_free += 1;
        }
    }
    assert!(
        held > 0,
        "no queue was handed over under the shared lock, so this proves \
         nothing: {recorded}"
    );
    assert!(
        named_branch_free > 0,
        "the branch update was never observed, so this proves nothing: \
         {recorded}"
    );
}

/// Contention that Git's own bounded retry absorbs must leave no trace. This
/// is the first half of the outcome an agent reads: silence means recovered.
#[test]
fn a_commit_that_waited_for_the_shared_lock_reports_nothing() {
    let fixture = Fixture::history("waited");
    fixture.expect(&["config", "core.packedRefsTimeout", "10000"]);
    fixture.stage("four");
    let before = fixture.head();

    let hold = Duration::from_millis(400);
    let holder = fixture.hold_shared_lock(hold);
    let started = Instant::now();
    let output = git(fixture.path(), &["commit", "--quiet", "-m", "four"]).expect("git launches");
    let waited = started.elapsed();
    holder.join().expect("the holder thread finishes");

    assert!(
        waited >= hold,
        "the commit never contended, so this proves nothing"
    );
    assert!(
        output.status.success(),
        "the commit failed: {}",
        combined(&output)
    );
    assert_ne!(
        fixture.head(),
        before,
        "the successful commit advances HEAD"
    );
    assert_eq!(
        fixture.head_subject(),
        "four",
        "the requested commit was recorded"
    );
    let seen = combined(&output);
    assert!(
        !seen.contains("packed-refs.lock"),
        "a commit that recovered on its own must say nothing about the lock: \
         {seen}"
    );
}

/// The companion the clean-output assertion needs. If a commit could be made
/// quiet by muting its streams, the test above would pass on a broken tree.
#[test]
fn a_commit_that_genuinely_fails_still_reports_its_failure() {
    let fixture = Fixture::history("failed");

    let output = git(fixture.path(), &["commit", "-m", "nothing staged"]).expect("git launches");

    assert!(
        !output.status.success(),
        "a commit with nothing staged has to fail for this to prove anything"
    );
    assert!(
        !combined(&output).trim().is_empty(),
        "a failed commit that says nothing reads exactly like a clean one"
    );
}

/// The second half of the outcome an agent reads: a message means a real
/// cleanup was genuinely skipped, and the lock behind it belongs to somebody
/// else. Nothing may remove, truncate or steal it.
#[test]
fn a_lock_another_writer_holds_survives_a_commit_that_gave_up() {
    let fixture = Fixture::history("survives");
    let fixture_root = fixture.path().to_owned();
    fixture.expect(&["config", "core.packedRefsTimeout", "100"]);
    let held = "another writer holds this\n";
    let foreign_lock = ForeignLock::take(fixture.shared_lock(), held);
    let identity = LockIdentity::read(foreign_lock.path());
    fixture.stage("five");
    let before = fixture.head();

    let output = git(fixture.path(), &["commit", "--quiet", "-m", "five"]).expect("git launches");

    assert!(
        output.status.success(),
        "recording the commit never needs the shared lock: {}",
        combined(&output)
    );
    assert_ne!(
        fixture.head(),
        before,
        "the successful commit advances HEAD"
    );
    assert_eq!(
        fixture.head_subject(),
        "five",
        "the requested commit was recorded"
    );
    let seen = combined(&output);
    assert!(
        seen.contains("Unable to create")
            && seen.contains("packed-refs.lock")
            && seen.contains("File exists"),
        "a cleanup that genuinely gave up must say so, or silence stops \
         meaning recovered: {}",
        seen
    );
    assert_eq!(
        LockIdentity::read(foreign_lock.path()),
        identity,
        "the lock another writer holds changed bytes or metadata"
    );
    drop(fixture);
    assert_eq!(
        LockIdentity::read(foreign_lock.path()),
        identity,
        "fixture cleanup touched the lock that the other writer still holds"
    );
    foreign_lock.release();
    fs::remove_dir_all(fixture_root).expect("the released fixture is removed");
}
