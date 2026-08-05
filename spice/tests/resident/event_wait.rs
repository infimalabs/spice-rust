//! What the shared event wait promises its callers about ordering.

use super::*;

/// The observer arms before the trigger runs.
///
/// Five call sites rest on that order, including the ones that pass an empty
/// trigger and rely entirely on the read immediately after arming. Proving it
/// by its consequence would need an effect a late-armed observer cannot see,
/// and no pause makes that certain: a stream armed a second after a change
/// still woke on it here. So the order is pinned by an ending the other order
/// cannot reach. The watched directory does not exist when the wait is called
/// and the trigger creates it, so arming first fails on the missing path.
/// Arming after the trigger would find the directory already there, arm
/// normally, and answer on the first read.
#[test]
#[should_panic(expected = "effect observer arms on")]
fn the_event_observer_arms_before_the_trigger_runs() {
    let fixture = Fixture::new("arming-order");
    let watched = fixture.root.join("trigger-created");
    let created = watched.clone();

    wait_for_event(
        &watched,
        move || fs::create_dir_all(&created).expect("the trigger creates the watched directory"),
        || Some(()),
    );
}
