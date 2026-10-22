use rusqlite::{OptionalExtension, params};

use super::commit::{decode_optional_seq, decode_seq};
use super::indexed_read::decode_head;
use super::*;

impl JournalRead<'_> {
    /// One fixed page of live steering in creation order, authored by actors
    /// whose principal currently holds the operator role.
    ///
    /// `start` is either the first page or the continuation returned by the
    /// preceding page. Overflow is always explicit.
    pub fn pending_operator_steering(
        &self,
        start: FactPageStart,
    ) -> Result<SteeringPage, GuardError> {
        self.require_current_steering_index()?;
        let first = steering_page_start(start)?;
        let operators = self.role_holder_spellings(wire::principal::PrincipalRole::Operator)?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT steering.directive, steering.steer_key, steering.lane, \
                 steering.disposition, steering.created_seq, steering.minted_micros, \
                 steering.room, steering.head_seq, steering.body, COUNT(*) OVER () \
                 FROM steering_heads AS steering \
                 JOIN fact_envelopes AS creating ON creating.seq = steering.created_seq \
                 WHERE steering.disposition = 'live' \
                 AND creating.actor IN (SELECT value FROM json_each(?1)) \
                 AND steering.created_seq >= ?2 ORDER BY steering.created_seq LIMIT ?3",
            )
            .map_err(GuardError::Store)?;
        read_metrics::observe_query();
        let rows = statement
            .query_map(
                params![operators, first, steering_query_rows()],
                read_steering_page_row,
            )
            .map_err(GuardError::Store)?;
        self.decode_steering_page(rows)
    }

    /// Current steering standing for one stable directive identity.
    pub fn steering_head(
        &self,
        directive: wire::CanonicalUuid,
    ) -> Result<Option<JournalHead<wire::SteeringHead>>, GuardError> {
        self.steering_row("directive", &directive.to_string())
    }

    /// Current steering standing for one human-sized directive key.
    pub fn steering_key(
        &self,
        key: &str,
    ) -> Result<Option<JournalHead<wire::SteeringHead>>, GuardError> {
        self.steering_row("steer_key", key)
    }

    /// Live stop requests bound to `room`, in creation order. At most `limit`
    /// rows return; a caller that receives `limit` rows cannot know the set is
    /// complete.
    pub fn live_room_stops(
        &self,
        room: &wire::TaskStamp,
        limit: usize,
    ) -> Result<Vec<JournalHead<wire::SteeringHead>>, GuardError> {
        self.require_current_steering_index()?;
        let stop = format!("{}:{room}:%", wire::rooms::StopControl::PREFIX);
        let limit = i64::try_from(limit)
            .map_err(|_| GuardError::Invalid("room steering limit exceeds SQLite".to_owned()))?;
        let mut statement = self
            .connection
            .prepare(
                "SELECT directive, steer_key, lane, disposition, created_seq, minted_micros, \
                 room, head_seq, body FROM steering_heads \
                 WHERE disposition = 'live' \
                 AND json_extract(CAST(body AS TEXT), '$.control') LIKE ?1 \
                 ORDER BY created_seq LIMIT ?2",
            )
            .map_err(GuardError::Store)?;
        read_metrics::observe_query();
        let rows = statement
            .query_map(params![stop, limit], read_steering_columns)
            .map_err(GuardError::Store)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(GuardError::Store)?;
        rows.into_iter()
            .map(|row| {
                read_metrics::observe_row();
                let head = self.decode_steering_row(row)?;
                let bound = head
                    .body
                    .control
                    .as_deref()
                    .and_then(|control| control.parse::<wire::rooms::StopControl>().ok())
                    .is_some_and(|control| control.room == *room);
                if !bound {
                    return Err(GuardError::Invalid(format!(
                        "steering {} indexed into room {room} is not bound to it",
                        head.body.key
                    )));
                }
                Ok(head)
            })
            .collect()
    }

    /// The stop request carrying exactly `control`, whatever its disposition,
    /// absent when none was ever sent.
    pub fn room_stop(
        &self,
        control: &wire::rooms::StopControl,
    ) -> Result<Option<JournalHead<wire::SteeringHead>>, GuardError> {
        self.steering_row(
            "json_extract(CAST(body AS TEXT), '$.control')",
            &control.to_string(),
        )
    }

    /// One fixed page of live steering for one lane or every lane.
    ///
    /// Heads remain in publication order and overflow names the first omitted
    /// creation position for an exact continuation.
    pub fn pending_steering(
        &self,
        lane: Option<wire::CanonicalUuid>,
        start: FactPageStart,
    ) -> Result<SteeringPage, GuardError> {
        self.require_current_steering_index()?;
        let first = steering_page_start(start)?;
        let sql = if lane.is_some() {
            "SELECT directive, steer_key, lane, disposition, created_seq, minted_micros, \
             room, head_seq, body, COUNT(*) OVER () FROM steering_heads \
             WHERE disposition = 'live' AND lane = ?1 AND created_seq >= ?2 \
             ORDER BY created_seq LIMIT ?3"
        } else {
            "SELECT directive, steer_key, lane, disposition, created_seq, minted_micros, \
             room, head_seq, body, COUNT(*) OVER () FROM steering_heads \
             WHERE disposition = 'live' AND created_seq >= ?1 \
             ORDER BY created_seq LIMIT ?2"
        };
        let mut statement = self.connection.prepare(sql).map_err(GuardError::Store)?;
        read_metrics::observe_query();
        if let Some(lane) = lane {
            let rows = statement
                .query_map(
                    params![lane.to_string(), first, steering_query_rows()],
                    read_steering_page_row,
                )
                .map_err(GuardError::Store)?;
            self.decode_steering_page(rows)
        } else {
            let rows = statement
                .query_map(
                    params![first, steering_query_rows()],
                    read_steering_page_row,
                )
                .map_err(GuardError::Store)?;
            self.decode_steering_page(rows)
        }
    }

    /// The `per_lane` most recently retired directives of one lane or every lane.
    ///
    /// Rows are grouped by lane in canonical lane order and, within a lane,
    /// ordered by the retiring fact, newest first. Retirement is the head's
    /// final transition, so the head position is the retiring fact.
    pub fn retired_steering(
        &self,
        lane: Option<wire::CanonicalUuid>,
        per_lane: usize,
    ) -> Result<Vec<RetiredSteering>, GuardError> {
        if per_lane == 0 {
            return Ok(Vec::new());
        }
        self.require_current_steering_index()?;
        let limit = i64::try_from(per_lane).map_err(|_| {
            GuardError::Invalid("retired steering window exceeds SQLite".to_owned())
        })?;
        // Ranking reads only indexed columns; each surviving row then joins
        // back by rowid so bodies outside the window are never loaded.
        let sql = format!(
            "WITH ranked AS (\
                SELECT rowid AS head, ROW_NUMBER() OVER (\
                    PARTITION BY lane ORDER BY head_seq DESC\
                ) AS recency FROM steering_heads \
                WHERE disposition != 'live'{}\
             ) SELECT steering.directive, steering.steer_key, steering.lane, \
                steering.disposition, steering.head_seq, retiring.instant, \
                json_extract(CAST(steering.body AS TEXT), '$.text'), \
                json_extract(CAST(steering.body AS TEXT), '$.attachments'), \
                json_extract(CAST(steering.body AS TEXT), '$.from'), \
                json_extract(CAST(steering.body AS TEXT), '$.fanout'), \
                json_extract(CAST(steering.body AS TEXT), '$.published') \
             FROM ranked \
             JOIN steering_heads AS steering ON steering.rowid = ranked.head \
             JOIN fact_envelopes AS retiring ON retiring.seq = steering.head_seq \
             WHERE ranked.recency <= ?1 ORDER BY steering.lane, steering.head_seq DESC",
            if lane.is_some() { " AND lane = ?2" } else { "" }
        );
        let mut statement = self.connection.prepare(&sql).map_err(GuardError::Store)?;
        read_metrics::observe_query();
        let rows = match lane {
            Some(lane) => statement.query_map(params![limit, lane.to_string()], read_retired_row),
            None => statement.query_map(params![limit], read_retired_row),
        }
        .map_err(GuardError::Store)?;
        let mut retired = Vec::new();
        for row in rows {
            let row = row.map_err(GuardError::Store)?;
            read_metrics::observe_row();
            retired.push(self.decode_retired_row(row)?);
        }
        Ok(retired)
    }

    /// Greatest moment already consumed by the current key mint.
    pub fn steering_minted_micros(&self) -> Result<Option<i64>, GuardError> {
        self.require_current_steering_index()?;
        read_metrics::observe_query();
        let maximum = self
            .connection
            .query_row("SELECT MAX(minted_micros) FROM steering_heads", [], |row| {
                row.get(0)
            })
            .map_err(GuardError::Store)?;
        read_metrics::observe_row();
        Ok(maximum)
    }

    fn steering_row(
        &self,
        column: &'static str,
        key: &str,
    ) -> Result<Option<JournalHead<wire::SteeringHead>>, GuardError> {
        self.require_current_steering_index()?;
        let sql = format!(
            "SELECT directive, steer_key, lane, disposition, created_seq, minted_micros, \
             room, head_seq, body FROM steering_heads WHERE {column} = ?1"
        );
        let row = self
            .connection
            .query_row(&sql, [key], read_steering_columns)
            .optional()
            .map_err(GuardError::Store)?;
        read_metrics::observe_query();
        if row.is_some() {
            read_metrics::observe_row();
        }
        row.map(|row| self.decode_steering_row(row)).transpose()
    }

    pub(super) fn require_current_steering_index(&self) -> Result<(), GuardError> {
        read_metrics::observe_query();
        let indexed = self
            .connection
            .query_row(
                "SELECT indexed_seq FROM steering_index WHERE singleton = 1",
                [],
                |row| row.get::<_, Option<i64>>(0),
            )
            .map_err(GuardError::Store)?;
        read_metrics::observe_row();
        let indexed = decode_optional_seq(indexed).map_err(GuardError::Invalid)?;
        if indexed != self.cursor {
            return Err(GuardError::Invalid(format!(
                "steering index cursor {indexed:?} disagrees with journal cursor {:?}",
                self.cursor
            )));
        }
        Ok(())
    }

    fn decode_steering_row(
        &self,
        row: SteeringColumns,
    ) -> Result<JournalHead<wire::SteeringHead>, GuardError> {
        read_metrics::observe_decode(row.body.len());
        let head = decode_head::<wire::SteeringHead>(row.head_seq, &row.body)?;
        if head.body.directive.to_string() != row.directive
            || head.body.key != row.key
            || head.body.lane.to_string() != row.lane
            || disposition_name(head.body.disposition) != row.disposition
            || i64::try_from(head.body.created.get()).ok() != Some(row.created_seq)
            || head.body.minted_micros != row.minted_micros
            || head.body.room.as_ref().map(ToString::to_string) != row.room
        {
            return Err(GuardError::Invalid(format!(
                "steering head {} indexed columns disagree with its body",
                row.directive
            )));
        }
        if head.body.created > head.position {
            return Err(GuardError::Invalid(format!(
                "steering head {} predates its creating fact",
                row.directive
            )));
        }
        if self.cursor.is_none_or(|cursor| head.position > cursor) {
            return Err(GuardError::Invalid(format!(
                "steering head {} position {} is beyond cursor {:?}",
                row.directive, head.position, self.cursor
            )));
        }
        Ok(head)
    }

    fn decode_steering_page(
        &self,
        rows: impl Iterator<Item = rusqlite::Result<SteeringPageRow>>,
    ) -> Result<SteeringPage, GuardError> {
        let mut heads = Vec::with_capacity(MAX_STEERING_PAGE_ROWS);
        let mut total = None;
        let mut continuation = None;
        let mut observed = 0_u64;
        for row in rows {
            let row = row.map_err(GuardError::Store)?;
            read_metrics::observe_row();
            observed = observed.saturating_add(1);
            let row_total = u64::try_from(row.total).map_err(|_| {
                GuardError::Invalid(format!(
                    "steering page count {} is outside the wire range",
                    row.total
                ))
            })?;
            if total
                .replace(row_total)
                .is_some_and(|total| total != row_total)
            {
                return Err(GuardError::Invalid(
                    "steering page count changed within one snapshot".to_owned(),
                ));
            }
            if heads.len() == MAX_STEERING_PAGE_ROWS {
                continuation =
                    Some(decode_seq(row.columns.created_seq).map_err(GuardError::Invalid)?);
                continue;
            }
            heads.push(self.decode_steering_row(row.columns)?);
        }
        let total = total.unwrap_or(0);
        let expected =
            total.min(u64::try_from(MAX_STEERING_PAGE_ROWS.saturating_add(1)).unwrap_or(u64::MAX));
        if observed != expected {
            return Err(GuardError::Invalid(format!(
                "steering page expected {expected} indexed row(s), found {observed}"
            )));
        }
        let overflow = continuation.map(|continuation| wire::Overflow {
            omitted: total
                .saturating_sub(u64::try_from(MAX_STEERING_PAGE_ROWS).unwrap_or(u64::MAX)),
            continuation,
        });
        Ok(SteeringPage {
            rows: heads,
            overflow,
            horizon: self.cursor,
        })
    }

    fn decode_retired_row(&self, row: RetiredColumns) -> Result<RetiredSteering, GuardError> {
        let invalid = |field: &str, detail: String| {
            GuardError::Invalid(format!(
                "retired steering head {} {field} is invalid: {detail}",
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
            .filter(|disposition| *disposition != wire::Disposition::Live)
            .ok_or_else(|| invalid("disposition", row.disposition.clone()))?;
        let retired = decode_seq(row.head_seq).map_err(GuardError::Invalid)?;
        if self.cursor.is_none_or(|cursor| retired > cursor) {
            return Err(invalid(
                "position",
                format!("{retired} is beyond cursor {:?}", self.cursor),
            ));
        }
        let attachments = row
            .attachments
            .as_deref()
            .map(|attachments| {
                serde_json::from_str(attachments)
                    .map_err(|error| invalid("attachments", error.to_string()))
            })
            .transpose()?
            .unwrap_or_default();
        Ok(RetiredSteering {
            directive: uuid("identity", &row.directive)?,
            fanout: row
                .fanout
                .as_deref()
                .map(|fanout| uuid("fanout", fanout))
                .transpose()?,
            lane: uuid("lane", &row.lane)?,
            from: row
                .from
                .as_deref()
                .map(|from| uuid("sender", from))
                .transpose()?,
            published: instant("publication", &row.published)?,
            retired_at: instant("retirement instant", &row.retired_at)?,
            disposition,
            retired,
            key: row.key,
            text: row.text,
            attachments,
        })
    }
}

fn steering_page_start(start: FactPageStart) -> Result<i64, GuardError> {
    let first = match start {
        FactPageStart::After(None) => 1,
        FactPageStart::After(Some(after)) => after.get().saturating_add(1),
        FactPageStart::At(position) => position.get(),
    };
    i64::try_from(first)
        .map_err(|_| GuardError::Invalid("steering page start exceeds SQLite".to_owned()))
}

fn steering_query_rows() -> i64 {
    i64::try_from(MAX_STEERING_PAGE_ROWS.saturating_add(1))
        .expect("fixed steering page ceiling fits SQLite")
}

struct SteeringPageRow {
    columns: SteeringColumns,
    total: i64,
}

struct SteeringColumns {
    directive: String,
    key: String,
    lane: String,
    disposition: String,
    created_seq: i64,
    minted_micros: Option<i64>,
    room: Option<String>,
    head_seq: i64,
    body: Vec<u8>,
}

fn read_steering_columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<SteeringColumns> {
    Ok(SteeringColumns {
        directive: row.get(0)?,
        key: row.get(1)?,
        lane: row.get(2)?,
        disposition: row.get(3)?,
        created_seq: row.get(4)?,
        minted_micros: row.get(5)?,
        room: row.get(6)?,
        head_seq: row.get(7)?,
        body: row.get(8)?,
    })
}

fn read_steering_page_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SteeringPageRow> {
    Ok(SteeringPageRow {
        columns: read_steering_columns(row)?,
        total: row.get(9)?,
    })
}

struct RetiredColumns {
    directive: String,
    key: String,
    lane: String,
    disposition: String,
    head_seq: i64,
    retired_at: String,
    text: String,
    attachments: Option<String>,
    from: Option<String>,
    fanout: Option<String>,
    published: String,
}

fn read_retired_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RetiredColumns> {
    Ok(RetiredColumns {
        directive: row.get(0)?,
        key: row.get(1)?,
        lane: row.get(2)?,
        disposition: row.get(3)?,
        head_seq: row.get(4)?,
        retired_at: row.get(5)?,
        text: row.get(6)?,
        attachments: row.get(7)?,
        from: row.get(8)?,
        fanout: row.get(9)?,
        published: row.get(10)?,
    })
}

pub(crate) fn disposition_name(disposition: wire::Disposition) -> &'static str {
    match disposition {
        wire::Disposition::Live => "live",
        wire::Disposition::Acked => "acked",
        wire::Disposition::Declined => "declined",
        wire::Disposition::Lapsed => "lapsed",
        wire::Disposition::DeadLettered => "deadlettered",
        wire::Disposition::Canceled => "canceled",
    }
}

pub(super) fn disposition_named(name: &str) -> Option<wire::Disposition> {
    [
        wire::Disposition::Live,
        wire::Disposition::Acked,
        wire::Disposition::Declined,
        wire::Disposition::Lapsed,
        wire::Disposition::DeadLettered,
        wire::Disposition::Canceled,
    ]
    .into_iter()
    .find(|disposition| disposition_name(*disposition) == name)
}

pub(crate) fn canonical_identity(miss: &wire::body::SteerMissed) -> Result<String, GuardError> {
    #[derive(Serialize)]
    struct Identity<'a> {
        key: &'a str,
        lane: wire::CanonicalUuid,
        locus: wire::body::TranscriptLocus,
        miss: wire::body::AcknowledgmentMiss,
    }

    let bytes = wire::encode(&Identity {
        key: &miss.key,
        lane: miss.lane,
        locus: miss.locus,
        miss: miss.miss,
    })
    .map_err(|error| GuardError::Invalid(error.to_string()))?;
    String::from_utf8(bytes).map_err(|error| GuardError::Invalid(error.to_string()))
}
