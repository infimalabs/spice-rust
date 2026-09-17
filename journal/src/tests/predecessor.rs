//! Turn a current authority into an authentic predecessor.
//!
//! Migration tests begin from a real current database and lower its schema
//! identity; these helpers restore the shapes and spellings each older schema
//! actually wrote, so the step under test runs against what a release stored.

use super::*;

/// Remove indexes introduced after schema 6 when constructing an older authority.
pub(super) fn drop_steering_index(connection: &Connection) {
    connection
        .execute_batch(
            "DROP TABLE IF EXISTS claim_index;
             DROP TABLE steering_index;
             DROP TABLE steering_misses;
             DROP TABLE steering_heads;",
        )
        .expect("remove post-schema-6 indexes from older fixture");
}

/// How every schema before 26 spelled the lane group in a JSON carrier.
///
/// Only `from_team` has two ancestors: the arranged member's `from_lineage_id`
/// and the configured team's optional `from_lineage`; fixtures plant only the
/// former. The renewal slot is not here because it is per era: `team_slot`
/// before 17 and `org_slot` from 17 to 25, so each fixture spells its own.
const LINEAGE_SPELLINGS: [(&[u8], &[u8]); 6] = [
    (br#""team":"#, br#""lineage_id":"#),
    (br#""from_team":"#, br#""from_lineage_id":"#),
    (br#""teams":"#, br#""lineages":"#),
    (br#""team_ids":"#, br#""lineage_ids":"#),
    (br#""creator_team":"#, br#""creator_lineage":"#),
    (br#""kind":"team::"#, br#""kind":"lineage::"#),
];

/// `bytes` as the schemas before 26 spelled them.
pub(super) fn lineage_spelling(bytes: &[u8]) -> Vec<u8> {
    LINEAGE_SPELLINGS
        .iter()
        .fold(bytes.to_vec(), |bytes, (team, lineage)| {
            replace_all(&bytes, team, lineage)
        })
}

/// `bytes` as schema 26 spells them, the renewal slot included.
pub(super) fn team_spelling(bytes: &[u8]) -> Vec<u8> {
    let bytes = replace_all(bytes, br#""org_slot":"#, br#""team_slot":"#);
    LINEAGE_SPELLINGS
        .iter()
        .fold(bytes, |bytes, (team, lineage)| {
            replace_all(&bytes, lineage, team)
        })
}

/// A kind column as the schemas before 26 spelled it.
fn lineage_kind(kind: &str) -> String {
    kind.strip_prefix("team::")
        .map_or_else(|| kind.to_owned(), |rest| format!("lineage::{rest}"))
}

/// Respell every carrier and kind column as the schemas before 26 spelled
/// them, so a fixture built from current wire is an authentic predecessor.
pub(super) fn spell_lineage(connection: &Connection) {
    respell(connection, lineage_spelling, lineage_kind);
}

/// Respell the renewal slot as schemas 17 through 25 spelled it.
pub(super) fn spell_slots_as_seventeen(connection: &Connection) {
    respell(
        connection,
        |bytes| replace_all(bytes, br#""team_slot":"#, br#""org_slot":"#),
        str::to_owned,
    );
}

/// Rewrite every JSON carrier through `carrier` and every kind column through
/// `kind`, across the tables the schema-26 step walks.
fn respell(
    connection: &Connection,
    carrier: impl Fn(&[u8]) -> Vec<u8>,
    kind: impl Fn(&str) -> String,
) {
    for (table, column) in [
        ("fact_envelopes", "kind"),
        ("fact_envelopes", "envelope"),
        ("residency_head", "body"),
        ("task_heads", "body"),
        ("command_receipts", "command_kind"),
        ("command_receipts", "request"),
        ("command_receipts", "evaluated_witnesses"),
        ("command_receipts", "response"),
    ] {
        let mut statement = connection
            .prepare(&format!("SELECT rowid, {column} FROM {table}"))
            .expect("prepare a column to respell");
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, rusqlite::types::Value>(0)?,
                    row.get::<_, rusqlite::types::Value>(1)?,
                ))
            })
            .expect("read a column to respell")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect a column to respell");
        drop(statement);
        for (rowid, value) in rows {
            let respelled = match value {
                rusqlite::types::Value::Text(name) => rusqlite::types::Value::Text(kind(&name)),
                rusqlite::types::Value::Blob(bytes) => {
                    rusqlite::types::Value::Blob(carrier(&bytes))
                }
                other => other,
            };
            connection
                .execute(
                    &format!("UPDATE {table} SET {column} = ?1 WHERE rowid = ?2"),
                    params![respelled, rowid],
                )
                .expect("store a respelled column");
        }
    }
}

pub(super) fn replace_all(bytes: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while let Some(relative) = bytes[cursor..]
        .windows(from.len())
        .position(|window| window == from)
    {
        let start = cursor + relative;
        output.extend_from_slice(&bytes[cursor..start]);
        output.extend_from_slice(to);
        cursor = start + from.len();
    }
    output.extend_from_slice(&bytes[cursor..]);
    output
}

/// The two schema identities an authority stores: the journal row and the
/// SQLite pragma.
pub(super) fn version_records(connection: &Connection) -> (u32, u32) {
    (
        connection
            .query_row(
                "SELECT version FROM journal_schema WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .expect("read schema row version"),
        connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read schema pragma version"),
    )
}

pub(super) fn admit_empty_steering_fold(journal: &mut Journal) {
    let snapshot = journal.fact_snapshot().expect("migration snapshot");
    assert!(
        snapshot.facts().iter().all(|envelope| {
            crate::guard::steering_fact_identity(&envelope.fact).is_none()
                && envelope.fact.kind() != FactKind::SteerMissed
        }),
        "only a fixture with no steering history may publish an empty fold"
    );
    journal
        .rebuild_steering_index(snapshot.horizon(), &[], &[])
        .expect("publish migrated empty steering fold");
}
