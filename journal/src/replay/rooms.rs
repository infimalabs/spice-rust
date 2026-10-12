//! The room-authority projection is exactly the fold of its facts.

use std::collections::BTreeMap;
use std::fmt::Debug;

use rusqlite::Connection;

use super::{FactRecord, VerifyError, Violation, ViolationClass};
use crate::rooms::{History, Projection};

/// Fold the complete history, naming every fact the fold refuses.
pub(super) fn expected(
    facts: &BTreeMap<u64, FactRecord>,
    violations: &mut Vec<Violation>,
) -> Projection {
    let (projection, refusals) = History::rebuild(facts.iter().filter_map(|(seq, record)| {
        let envelope = record.envelope.as_ref()?;
        Some((
            wire::Seq::new(*seq)?,
            &envelope.actor,
            envelope.at,
            &envelope.fact,
        ))
    }));
    for (seq, refusal) in refusals {
        violations.push(Violation::new(
            ViolationClass::Head,
            format!("room_authority:{seq}"),
            refusal.to_string(),
        ));
    }
    projection
}

pub(super) fn verify(
    connection: &Connection,
    facts: &BTreeMap<u64, FactRecord>,
    violations: &mut Vec<Violation>,
) -> Result<usize, VerifyError> {
    let expected = expected(facts, violations);
    let stored = Projection::load(connection).map_err(VerifyError::Sqlite)?;
    compare(
        "room_publications",
        &expected.publications,
        &stored.publications,
        violations,
    );
    compare("group_heads", &expected.groups, &stored.groups, violations);
    compare(
        "group_anchors",
        &expected.anchors,
        &stored.anchors,
        violations,
    );
    compare(
        "proposal_heads",
        &expected.proposals,
        &stored.proposals,
        violations,
    );
    compare(
        "authorization_heads",
        &expected.authorizations,
        &stored.authorizations,
        violations,
    );
    compare(
        "room_cursors",
        &expected.cursors,
        &stored.cursors,
        violations,
    );
    compare(
        "room_horizons",
        &expected.horizons,
        &stored.horizons,
        violations,
    );
    compare(
        "room_directions",
        &expected.directed,
        &stored.directed,
        violations,
    );
    compare(
        "room_shelvings",
        &expected.shelved,
        &stored.shelved,
        violations,
    );
    compare(
        "room_conversations",
        &expected.conversation.publications,
        &stored.conversation.publications,
        violations,
    );
    compare(
        "room_items",
        &expected.conversation.items,
        &stored.conversation.items,
        violations,
    );
    compare(
        "room_results",
        &expected.conversation.results,
        &stored.conversation.results,
        violations,
    );
    Ok(stored.groups.len()
        + stored.proposals.len()
        + stored.authorizations.len()
        + stored.cursors.len()
        + stored.directed.len()
        + stored.shelved.len()
        + stored.conversation.publications.len()
        + stored.conversation.items.len()
        + stored.conversation.results.len())
}

fn compare<K: Ord + Debug, V: PartialEq>(
    table: &str,
    expected: &BTreeMap<K, V>,
    stored: &BTreeMap<K, V>,
    violations: &mut Vec<Violation>,
) {
    for (key, value) in expected {
        if stored.get(key) != Some(value) {
            violations.push(Violation::new(
                ViolationClass::Head,
                format!("{table}:{key:?}"),
                "projection row is missing or disagrees with its facts",
            ));
        }
    }
    for key in stored.keys().filter(|key| !expected.contains_key(key)) {
        violations.push(Violation::new(
            ViolationClass::Head,
            format!("{table}:{key:?}"),
            "projection row has no deriving fact",
        ));
    }
}
