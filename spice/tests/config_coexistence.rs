//! Who may add a root table to the tracked `spice.toml`, and what that costs.
//!
//! One file has two strict readers. This port loads it as its tracked layer and
//! refuses anything outside its release registry; the Python predecessor loads
//! the same bytes as its `repository` layer from its own checkout, and refuses
//! the first key outside `spice/config/schema.py`. Neither reader knows
//! the other's schema, and neither is going away -- the two systems coexist
//! permanently, so this is a standing hazard rather than a migration step.
//!
//! **The predecessor owns the root schema.** This port may author a root table
//! into tracked `spice.toml` only where the predecessor already declares one.
//! The tie breaks that way because the two refusals are not symmetric:
//!
//! - The predecessor validates this file on *every* agent tool call, through
//!   the shared hook. A root it does not declare stops every lane's shell at
//!   once, and the message names a file this port owns, so the lane that reads
//!   the failure is generally not the lane that caused it.
//! - This port's refusal (`IssueKind::Unknown`, "path is not present in the
//!   release configuration registry") is raised while a port command resolves
//!   configuration -- in that command, in front of whoever ran it.
//!
//! The cost is real and is pinned below, not hypothetical. This registry admits
//! ten root tables into the tracked layer that the predecessor refuses, and
//! they are legitimate port configuration that simply may not live in this
//! file. It belongs in the repository-local layer instead, which the
//! predecessor does not read. One of the ten is `lock` against the
//! predecessor's `locks`: a single character, and the refusal is fleet-wide.
//!
//! Re-derive both sides (read-only, neither writes the shared file):
//!
//! ```text
//! cargo test -p spice --test config_coexistence
//! python3 -c "from spice.config.schema import CONFIG_SCHEMA; print(sorted(CONFIG_SCHEMA.children))"
//! ```

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use spice::config::descriptors;
use wire::ConfigSource;

/// Root tables `spice/config/schema.py` declares, as `CONFIG_SCHEMA.children`.
///
/// Authoring outside this list is what the decision above forbids. The list is
/// transcribed rather than read, because the predecessor is a separate checkout
/// that may be absent: a test that skipped itself when it went missing would be
/// a check that stops checking exactly when the two systems drift apart.
const PREDECESSOR_ROOTS: &[&str] = &[
    "agent", "commands", "judge", "locks", "maxim", "maxims", "policy", "rtk", "say", "serve",
    "tasks", "taste", "wrappers",
];

/// Roots this registry admits into the tracked layer that the predecessor
/// refuses -- the standing hazard list, pinned so it cannot grow unnoticed.
const REFUSED_BY_PREDECESSOR: &[&str] = &[
    "board", "canvas", "consent", "gate", "lane", "listen", "lock", "proof", "steer", "vendor",
];

/// The shared file this decision governs.
fn tracked_configuration() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("spice.toml")
}

/// Root tables a document authors that the predecessor does not declare.
///
/// This is the whole check. It runs against a document rather than a path so a
/// violation can be proven caught while it is still a candidate edit -- the
/// point is to answer before the bytes reach the file, because once they do,
/// every lane's next shell command is already refused.
fn roots_the_predecessor_refuses(document: &str) -> Vec<String> {
    let parsed: toml::Table = document
        .parse()
        .expect("tracked configuration parses as TOML");
    parsed
        .keys()
        .filter(|root| !PREDECESSOR_ROOTS.contains(&root.as_str()))
        .cloned()
        .collect()
}

/// The shared file authors nothing the predecessor would refuse.
#[test]
fn tracked_configuration_authors_no_root_the_predecessor_refuses() {
    let path = tracked_configuration();
    let document = fs::read_to_string(&path).expect("read the tracked configuration");

    assert_eq!(
        roots_the_predecessor_refuses(&document),
        Vec::<String>::new(),
        "{} authors a root table the predecessor's validator refuses, which stops \
         every lane's shell through the shared hook",
        path.display(),
    );
}

/// Planting one of the ten reproduces the refusal; removing it passes.
///
/// The planted root is not a typo. It is a root this registry genuinely admits
/// into the tracked layer, so what fails here is coexistence alone, and the
/// check is shown able to fail on precisely the edit it exists to stop.
#[test]
fn a_planted_port_root_is_named_and_the_same_document_passes_without_it() {
    let clean =
        fs::read_to_string(tracked_configuration()).expect("read the tracked configuration");
    let planted = format!("{clean}\n[gate]\n");

    assert_eq!(
        roots_the_predecessor_refuses(&planted),
        vec!["gate".to_string()],
        "planting a port-only root was not caught, or was not named",
    );
    assert!(
        REFUSED_BY_PREDECESSOR.contains(&"gate"),
        "the planted root is one this registry admits, not an invented key",
    );
    assert_eq!(
        roots_the_predecessor_refuses(&clean),
        Vec::<String>::new(),
        "the same check must pass on the same document with the planted root removed",
    );
}

/// The hazard list is exactly what the registry admits and the predecessor
/// refuses.
///
/// Admitting a new root into the tracked layer fails here until it is either
/// declared by the predecessor or added above deliberately, so the cost of the
/// decision is restated by the registry itself rather than by this transcript.
#[test]
fn the_registry_roots_the_predecessor_refuses_are_the_pinned_hazard_list() {
    let admitted: BTreeSet<&str> = descriptors()
        .iter()
        .filter(|descriptor| descriptor.layers().contains(&ConfigSource::Tracked))
        .filter_map(|descriptor| descriptor.template().as_str().split('.').next())
        .collect();

    let refused: Vec<&str> = admitted
        .iter()
        .copied()
        .filter(|root| !PREDECESSOR_ROOTS.contains(root))
        .collect();

    assert_eq!(
        refused, REFUSED_BY_PREDECESSOR,
        "the tracked-layer roots the predecessor refuses moved; the shared file's \
         hazard list is stale",
    );
    assert!(
        admitted.iter().any(|root| PREDECESSOR_ROOTS.contains(root)),
        "no tracked root is shared, so the two schemas no longer overlap at all",
    );
}
