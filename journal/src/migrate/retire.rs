//! Rewrite one retired fact row in place as inert
//! [`wire::body::JournalRetained`] evidence.
//!
//! The row keeps its position, attribution, and causal links; only its kind
//! and fact body change, and the retained body preserves the retired fact as
//! opaque JSON. The stored envelope must agree with its indexed columns before
//! anything is rewritten.

use rusqlite::params;

use crate::{MigrationError, OpenError};

/// The columns [`read_rows`] expects, in order.
pub(super) const COLUMNS: &str = "seq, wire_version, instant, actor, origin, cause, envelope, kind";

/// Who retired a row: the release that last owned it and what it was.
#[derive(Clone, Copy)]
pub(super) struct Retirement {
    pub(super) release: &'static str,
    pub(super) what: &'static str,
}

impl Retirement {
    pub(super) fn error(self, seq: i64, detail: String) -> OpenError {
        MigrationError::RetiredFact {
            seq,
            kind: self.what,
            release: self.release,
            detail,
        }
        .into()
    }
}

/// One stored fact row a crossing retires.
pub(super) struct RetiredRow {
    pub(super) seq: i64,
    wire_version: i64,
    instant: String,
    actor: String,
    origin: String,
    cause: Option<i64>,
    pub(super) envelope: Vec<u8>,
    kind: String,
}

/// Every row `select` names; it must select [`COLUMNS`] in order.
pub(super) fn read_rows(
    transaction: &rusqlite::Transaction<'_>,
    select: &str,
) -> Result<Vec<RetiredRow>, OpenError> {
    let mut statement = transaction.prepare(select).map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok(RetiredRow {
                seq: row.get(0)?,
                wire_version: row.get(1)?,
                instant: row.get(2)?,
                actor: row.get(3)?,
                origin: row.get(4)?,
                cause: row.get(5)?,
                envelope: row.get(6)?,
                kind: row.get(7)?,
            })
        })
        .map_err(OpenError::Sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(OpenError::Sqlite)?;
    Ok(rows)
}

/// Replace `row`'s fact with inert evidence naming `retirement.release`.
pub(super) fn retire_row(
    transaction: &rusqlite::Transaction<'_>,
    row: RetiredRow,
    retirement: Retirement,
) -> Result<(), OpenError> {
    let error = |detail: String| retirement.error(row.seq, detail);
    let envelope: wire::Envelope<serde_json::Value> = serde_json::from_slice(&row.envelope)
        .map_err(|cause| error(format!("retired fact envelope does not decode: {cause}")))?;
    let indexed_cause = row.cause.and_then(|cause| u64::try_from(cause).ok());
    if !envelope.v.is_current()
        || Some(envelope.seq.get()) != u64::try_from(row.seq).ok()
        || i64::from(envelope.v.0) != row.wire_version
        || envelope.at.to_string() != row.instant
        || envelope.actor.to_string() != row.actor
        || envelope.origin.as_str() != row.origin
        || envelope.cause.map(wire::Seq::get) != indexed_cause
    {
        return Err(error(
            "retired fact envelope disagrees with its indexed columns".to_owned(),
        ));
    }
    let encoded = wire::encode(&wire::Fact::new(wire::body::JournalRetained {
        release: retirement.release.to_owned(),
        fact: envelope.fact,
    }))
    .map_err(|cause| {
        error(format!(
            "retired fact cannot become inert authority: {cause}"
        ))
    })?;
    let fact: wire::AnyFact = wire::decode(&encoded)
        .map_err(|cause| error(format!("inert replacement does not decode: {cause}")))?;
    let replacement = wire::Envelope {
        v: envelope.v,
        seq: envelope.seq,
        at: envelope.at,
        actor: envelope.actor,
        origin: envelope.origin,
        cause: envelope.cause,
        fact,
    };
    let bytes = wire::encode(&replacement)
        .map_err(|cause| error(format!("inert replacement does not encode: {cause}")))?;
    let changed = transaction
        .execute(
            "UPDATE fact_envelopes SET kind = ?1, envelope = ?2 WHERE seq = ?3 AND kind = ?4",
            params![
                wire::FactKind::JournalRetained.as_str(),
                bytes,
                row.seq,
                row.kind
            ],
        )
        .map_err(OpenError::Sqlite)?;
    if changed != 1 {
        return Err(error(
            "retired fact changed during exclusive migration".to_owned(),
        ));
    }
    Ok(())
}
