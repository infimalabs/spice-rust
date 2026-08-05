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
