use rusqlite::Transaction;

use super::{OpenError, ROOM_AUTHORITY_SCHEMA};

/// Give each retained output one authorization row per distinct decision.
pub(super) fn cross(transaction: &Transaction<'_>, version: u32) -> Result<(), OpenError> {
    if version < ROOM_AUTHORITY_SCHEMA {
        return Ok(());
    }
    transaction
        .execute_batch(
            "DROP INDEX authorization_heads_decision;
             ALTER TABLE authorization_heads RENAME TO authorization_heads_single;
             CREATE TABLE authorization_heads (
                 output TEXT NOT NULL,
                 decision TEXT NOT NULL,
                 head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq),
                 body BLOB NOT NULL,
                 PRIMARY KEY (output, decision)
             ) STRICT, WITHOUT ROWID;
             INSERT INTO authorization_heads(output, decision, head_seq, body)
                 SELECT output, decision, head_seq, body FROM authorization_heads_single;
             DROP TABLE authorization_heads_single;
             CREATE INDEX authorization_heads_decision ON authorization_heads(decision);",
        )
        .map_err(OpenError::Sqlite)
}
