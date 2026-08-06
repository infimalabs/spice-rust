//! What the shared event wait promises its callers about ordering.

use super::*;

/// The observer arms before the trigger runs.
///
/// Five call sites rest on that order, including the ones that pass an empty
/// trigger and rely entirely on the read immediately after arming. Proving it
/// by its consequence would need an effect a late-armed observer cannot see,
/// and no pause makes that certain: a stream armed a second after a change
/// still woke on it here. So the order is pinned by an ending the other order
/// cannot reach.
///
/// Nothing below the watched path exists when the wait is called, and the
/// trigger creates the whole chain. A watched path that is not a directory
/// resolves to the directory containing it, which callers awaiting a file that
/// does not exist yet depend on, so the missing directory has to be the
/// resolved one: reading it is the first thing arming does, and it refuses at
/// once. Arming after the trigger would find that directory there, arm
/// normally, and answer on the first read.
#[test]
#[should_panic(expected = "effect directory reads")]
fn the_event_observer_arms_before_the_trigger_runs() {
    let fixture = Fixture::new("arming-order");
    let watched = fixture.root.join("absent/trigger-created");
    let created = watched.clone();

    wait_for_event(
        &watched,
        move || fs::create_dir_all(&created).expect("the trigger creates the watched chain"),
        || Some(()),
    );
}

/// A directory snapshot is only a hint about incidental entries.
///
/// SQLite journal files are short-lived enough to be listed and classified as
/// files, then disappear before the direct kernel observer opens them. The
/// directory observer already covers their deletion and replacement, so that
/// one absence must not prevent the remaining snapshot entries from arming.
#[test]
fn a_listed_effect_entry_may_vanish_before_its_observer_arms() {
    let fixture = Fixture::new("vanished-effect-entry");
    let effects = fixture.root.join("effects");
    let stable = effects.join("stable");
    let vanished = effects.join("spicejournal.sqlite3-journal");
    fs::create_dir(&effects).expect("effect directory creates");
    fs::write(&stable, "stable").expect("stable effect creates");
    fs::write(&vanished, "transient").expect("transient effect creates");

    let mut unlinked = false;
    let observers = effect_observers_before_arm(&effects, |watched| {
        if watched == vanished {
            fs::remove_file(watched).expect("listed effect entry unlinks before arm");
            unlinked = true;
        }
    });

    assert!(unlinked, "the fixture reaches the list-to-arm race");
    assert_eq!(
        observers.len(),
        2,
        "the root and still-existing entry remain observed"
    );
}

/// Only entries discovered incidentally through a directory snapshot are
/// tolerant. A caller that names an absent watch target still gets the
/// observation failure immediately and with the target role in its message.
#[test]
#[should_panic(expected = "explicit effect observer arms on")]
fn directly_arming_an_absent_effect_path_still_fails_loudly() {
    let fixture = Fixture::new("absent-explicit-effect");
    let absent = fixture.root.join("absent-explicit-effect");

    readiness::filesystem_event::DirectObserver::arm(&absent, "explicit effect");
}
