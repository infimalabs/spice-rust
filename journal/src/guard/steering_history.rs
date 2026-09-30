use rusqlite::{OptionalExtension, params};

use super::commit::decode_seq;
use super::steering_index::disposition_named;
use super::*;

/// One operator-authored directive projected for direction history.
///
/// Carries only indexed standing and small body fields: direction text can
/// embed pasted payloads, so a history read never materializes it for every
/// candidate. [`JournalRead::operator_direction_text`] loads one row's text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorDirection {
    /// Stable directive identity.
    pub directive: wire::CanonicalUuid,
    /// Operator submit action shared by target-specific fanout directives.
    pub fanout: Option<wire::CanonicalUuid>,
    /// Human-sized key retained for the directive's lifetime.
    pub key: String,
    /// Lane the directive addressed.
    pub lane: wire::CanonicalUuid,
    /// Current disposition.
    pub disposition: wire::Disposition,
    /// Fact that created the directive.
    pub created: wire::Seq,
    /// Commit instant of the original publication.
    pub published: wire::Instant,
    /// Retiring fact and its commit instant; absent while live.
    pub retired: Option<(wire::Seq, wire::Instant)>,
    /// Whether the text contains the ASCII-lowercased query; true without one.
    pub matched: bool,
}

/// Every operator-authored directive, newest creation first, at one cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperatorDirections {
    /// Operator-authored directives in descending creation order.
    pub rows: Vec<OperatorDirection>,
    /// Journal cursor the projection was read at.
    pub horizon: Option<wire::Seq>,
}

struct DirectionColumns {
    directive: String,
    key: String,
    lane: String,
    disposition: String,
    created_seq: i64,
    head_seq: i64,
    head_instant: String,
    fanout: Option<String>,
    published: String,
    matched: bool,
}

fn read_direction_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<DirectionColumns> {
    Ok(DirectionColumns {
        directive: row.get(0)?,
        key: row.get(1)?,
        lane: row.get(2)?,
        disposition: row.get(3)?,
        created_seq: row.get(4)?,
        head_seq: row.get(5)?,
        head_instant: row.get(6)?,
        fanout: row.get(7)?,
        published: row.get(8)?,
        matched: row.get(9)?,
    })
}

impl JournalRead<'_> {
    /// Every operator-authored directive, newest creation first.
    ///
    /// Authorship is the creating fact's actor, so peer tells, replies, and
    /// resident-authored directives never appear. `query` must already be
    /// ASCII-lowercased: it is matched against SQLite `lower()` of the text,
    /// which folds ASCII only.
    pub fn operator_directions(
        &self,
        query: Option<&str>,
    ) -> Result<OperatorDirections, GuardError> {
        self.require_current_steering_index()?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT steering.directive, steering.steer_key, steering.lane, \
                 steering.disposition, steering.created_seq, steering.head_seq, head.instant, \
                 json_extract(CAST(steering.body AS TEXT), '$.fanout'), \
                 json_extract(CAST(steering.body AS TEXT), '$.published'), \
                 ?2 IS NULL OR \
                 instr(lower(json_extract(CAST(steering.body AS TEXT), '$.text')), ?2) > 0 \
                 FROM steering_heads AS steering \
                 JOIN fact_envelopes AS creating ON creating.seq = steering.created_seq \
                 JOIN fact_envelopes AS head ON head.seq = steering.head_seq \
                 WHERE creating.actor = ?1 ORDER BY steering.created_seq DESC",
            )
            .map_err(GuardError::Store)?;
        read_metrics::observe_query();
        let rows = statement
            .query_map(
                params![wire::Actor::Operator.to_string(), query],
                read_direction_row,
            )
            .map_err(GuardError::Store)?;
        let mut directions = Vec::new();
        for row in rows {
            let row = row.map_err(GuardError::Store)?;
            read_metrics::observe_row();
            directions.push(self.decode_direction_row(row)?);
        }
        Ok(OperatorDirections {
            rows: directions,
            horizon: self.cursor,
        })
    }

    /// Text of one directive, read by its primary key.
    pub fn operator_direction_text(
        &self,
        directive: wire::CanonicalUuid,
    ) -> Result<Option<String>, GuardError> {
        self.require_current_steering_index()?;
        read_metrics::observe_query();
        let text = self
            .connection
            .query_row(
                "SELECT json_extract(CAST(body AS TEXT), '$.text') FROM steering_heads \
                 WHERE directive = ?1",
                [directive.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(GuardError::Store)?;
        if text.is_some() {
            read_metrics::observe_row();
        }
        Ok(text)
    }

    fn decode_direction_row(&self, row: DirectionColumns) -> Result<OperatorDirection, GuardError> {
        let invalid = |field: &str, detail: String| {
            GuardError::Invalid(format!(
                "operator steering head {} {field} is invalid: {detail}",
                row.directive
            ))
        };
        let uuid = |field: &str, text: &str| {
            text.parse::<wire::CanonicalUuid>()
                .map_err(|error| invalid(field, error.to_string()))
        };
        let instant = |field: &str, text: &str| {
            text.parse::<wire::Instant>()
                .map_err(|error| invalid(field, error.to_string()))
        };
        let disposition = disposition_named(&row.disposition)
            .ok_or_else(|| invalid("disposition", row.disposition.clone()))?;
        let created = decode_seq(row.created_seq).map_err(GuardError::Invalid)?;
        let head = decode_seq(row.head_seq).map_err(GuardError::Invalid)?;
        if created > head || self.cursor.is_none_or(|cursor| head > cursor) {
            return Err(invalid(
                "position",
                format!("created {created}, head {head}, cursor {:?}", self.cursor),
            ));
        }
        let retired = match disposition {
            wire::Disposition::Live => None,
            _ => Some((head, instant("retirement instant", &row.head_instant)?)),
        };
        Ok(OperatorDirection {
            directive: uuid("identity", &row.directive)?,
            fanout: row
                .fanout
                .as_deref()
                .map(|fanout| uuid("fanout", fanout))
                .transpose()?,
            key: row.key,
            lane: uuid("lane", &row.lane)?,
            disposition,
            created,
            published: instant("publication", &row.published)?,
            retired,
            matched: row.matched,
        })
    }
}
