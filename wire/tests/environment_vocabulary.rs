//! The tracked environment vocabulary against the rows Doctor reports.
//!
//! Two authorities govern one list. `[policy] env_names` in the workspace
//! document is accounted by the gate's environment-name ledger, which walks the
//! tracked tree and refuses a commit for every referenced name the list omits
//! and every listed name the tree no longer references. [`SpiceVariable`] is the
//! typed vocabulary Doctor reports rows for, and its doc says it is every
//! ambient variable owned by Spice.
//!
//! Those two memberships agree only while nothing foreign is referenced. The
//! moment the tree references a name Spice does not own, the ledger requires it
//! in the list and an equality against the enum forbids it, and neither can be
//! satisfied without moving the other. The foreign class is therefore named
//! here, once, rather than resolved by widening either authority.
//!
//! This law lives in an integration test rather than beside the enum because
//! `wire/src` is staged into the committed application image: editing a source
//! file there, even to add a test, drifts `dist/stage-sources.txt` and only the
//! sealed build image may restage it. `wire/tests` is not staged.

use serde::Deserialize;
use wire::SpiceVariable;

/// The workspace document, decoded for the one table this law reads.
#[derive(Deserialize)]
struct WorkspacePolicy {
    policy: EnvironmentPolicy,
}

#[derive(Deserialize)]
struct EnvironmentPolicy {
    env_names: Vec<String>,
}

/// Tracked names the vocabulary accounts for and Spice does not own.
///
/// The installed pre-commit hook unsets Git's local environment before it execs
/// verify, and the hooks fixture proves it by exporting exactly these three and
/// asserting the child sees none of them. That reference is what obliges the
/// ledger to account them. They are named here rather than added to
/// [`SpiceVariable`], because Doctor reports the variables Spice owns and a row
/// for one of these would claim ownership Spice does not have.
const FOREIGN_TRACKED_NAMES: &[&str] = &[
    "GIT_DIR",        // env-policy: allow
    "GIT_INDEX_FILE", // env-policy: allow
    "GIT_WORK_TREE",  // env-policy: allow
];

/// The tracked vocabulary is Doctor's rows plus the names Spice does not own.
///
/// Stated as two total directions rather than one containment. Removing the
/// foreign names must leave Doctor's vocabulary exactly, in report order, so a
/// list that dropped a row, added an unowned name without declaring it, or
/// reordered two still fails here. Every foreign name must then be accounted, so
/// deleting one from the list fails in this test rather than only in the ledger
/// that refuses the commit, which is a far later and much less legible door.
#[test]
fn the_tracked_vocabulary_is_doctor_rows_plus_the_named_foreign_class() {
    let tracked: WorkspacePolicy =
        toml::from_str(include_str!("../../spice.toml")).expect("tracked workspace policy decodes");
    let reported: Vec<_> = SpiceVariable::ALL
        .iter()
        .map(|variable| variable.as_str())
        .collect();

    let owned: Vec<_> = tracked
        .policy
        .env_names
        .iter()
        .map(String::as_str)
        .filter(|name| !FOREIGN_TRACKED_NAMES.contains(name))
        .collect();
    assert_eq!(
        owned, reported,
        "the tracked vocabulary, less the foreign names, is what Doctor reports"
    );

    for name in FOREIGN_TRACKED_NAMES {
        assert!(
            tracked
                .policy
                .env_names
                .iter()
                .any(|accounted| accounted == name),
            "the tracked vocabulary accounts {name}, which the tree references",
        );
    }
}
