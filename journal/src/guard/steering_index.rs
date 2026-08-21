use rusqlite::OptionalExtension;

use super::commit::decode_optional_seq;
use super::indexed_read::decode_head;
use super::*;

impl JournalRead<'_> {
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

    /// Live steering heads for one lane, or every lane, in publication order.
    pub fn pending_steering(
        &self,
        lane: Option<wire::CanonicalUuid>,
    ) -> Result<Vec<JournalHead<wire::SteeringHead>>, GuardError> {
        self.require_current_steering_index()?;
        let sql = if lane.is_some() {
            "SELECT directive, steer_key, lane, disposition, created_seq, minted_millis, \
             head_seq, body FROM steering_heads WHERE disposition = 'live' AND lane = ?1 \
             ORDER BY created_seq"
        } else {
            "SELECT directive, steer_key, lane, disposition, created_seq, minted_millis, \
             head_seq, body FROM steering_heads WHERE disposition = 'live' ORDER BY created_seq"
        };
        let mut statement = self.connection.prepare(sql).map_err(GuardError::Store)?;
        read_metrics::observe_query();
        let mut heads = Vec::new();
        if let Some(lane) = lane {
            let rows = statement
                .query_map([lane.to_string()], read_steering_columns)
                .map_err(GuardError::Store)?;
            for row in rows {
                let row = row.map_err(GuardError::Store)?;
                read_metrics::observe_row();
                heads.push(self.decode_steering_row(row)?);
            }
        } else {
            let rows = statement
                .query_map([], read_steering_columns)
                .map_err(GuardError::Store)?;
            for row in rows {
                let row = row.map_err(GuardError::Store)?;
                read_metrics::observe_row();
                heads.push(self.decode_steering_row(row)?);
            }
        }
        Ok(heads)
    }

    /// Greatest moment already consumed by the current key mint.
    pub fn steering_minted_millis(&self) -> Result<Option<i64>, GuardError> {
        self.require_current_steering_index()?;
        read_metrics::observe_query();
        let maximum = self
            .connection
            .query_row("SELECT MAX(minted_millis) FROM steering_heads", [], |row| {
                row.get(0)
            })
            .map_err(GuardError::Store)?;
        read_metrics::observe_row();
        Ok(maximum)
    }

    /// Whether this exact missed-recognition body was already recorded.
    pub fn steering_miss_recorded(
        &self,
        miss: &wire::body::SteerMissed,
    ) -> Result<bool, GuardError> {
        self.require_current_steering_index()?;
        let identity = canonical_identity(miss)?;
        read_metrics::observe_query();
        let recorded = self
            .connection
            .query_row(
                "SELECT 1 FROM steering_misses WHERE identity = ?1",
                [identity],
                |_| Ok(()),
            )
            .optional()
            .map_err(GuardError::Store)?;
        if recorded.is_some() {
            read_metrics::observe_row();
        }
        Ok(recorded.is_some())
    }

    fn steering_row(
        &self,
        column: &'static str,
        key: &str,
    ) -> Result<Option<JournalHead<wire::SteeringHead>>, GuardError> {
        self.require_current_steering_index()?;
        let sql = format!(
            "SELECT directive, steer_key, lane, disposition, created_seq, minted_millis, \
             head_seq, body FROM steering_heads WHERE {column} = ?1"
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

    fn require_current_steering_index(&self) -> Result<(), GuardError> {
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
            || head.body.minted_millis != row.minted_millis
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
}

struct SteeringColumns {
    directive: String,
    key: String,
    lane: String,
    disposition: String,
    created_seq: i64,
    minted_millis: Option<i64>,
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
        minted_millis: row.get(5)?,
        head_seq: row.get(6)?,
        body: row.get(7)?,
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
