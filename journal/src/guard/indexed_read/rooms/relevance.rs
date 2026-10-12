//! Bounded reads of shadow relevance observations and their assessments.

use rusqlite::params;
use wire::rooms::{RoomAuthorityStamp, RoomRelevanceAssessment, RoomRelevanceObservation};

use super::{
    CurrentHeadWitness, FactReadBudget, GuardError, JournalHead, JournalRead, decode_head,
    indexed_length, probe_limit, read_metrics,
};

/// One source-backed shadow observation and its optional operator assessment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomRelevanceRecord {
    /// Immutable observation at its admitted position.
    pub observation: JournalHead<RoomRelevanceObservation>,
    /// Attributable assessment, absent until the operator records one.
    pub assessment: Option<JournalHead<RoomRelevanceAssessment>>,
}

/// Bounded aggregate quality and classification counters for one room.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RoomRelevanceMetrics {
    pub observations: usize,
    pub relevant_contributions: usize,
    pub declines: usize,
    pub duplicates: usize,
    pub stale_activity: usize,
    pub missed_opportunities: usize,
    pub non_evidence_signals: usize,
    pub unknown_observations: usize,
    pub progress: usize,
    pub confirmed: usize,
    pub false_positives: usize,
    pub unknown_assessments: usize,
    pub unassessed: usize,
}

/// One identity-ordered bounded page of shadow observations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RoomRelevancePage {
    pub rows: Vec<RoomRelevanceRecord>,
    /// Last returned observation identity when more rows remain.
    pub continuation: Option<RoomAuthorityStamp>,
    /// Aggregate counters for the complete room, independent of page size.
    pub metrics: RoomRelevanceMetrics,
    /// Cursor identity of the projection this page observed.
    pub witness: CurrentHeadWitness,
}

impl JournalRead<'_> {
    /// One bounded identity-ordered page of shadow observations for `room`.
    ///
    /// Metrics cover the complete room at this read transaction's cursor. The
    /// read changes no observation, room, task, delivery, or authorization.
    pub fn room_relevance_page(
        &self,
        room: &wire::TaskStamp,
        after: Option<&RoomAuthorityStamp>,
        budget: FactReadBudget,
    ) -> Result<RoomRelevancePage, GuardError> {
        let after = after.map(ToString::to_string).unwrap_or_default();
        let metrics = self.room_relevance_metrics(room)?;
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT length(CAST(observation.observation AS BLOB)), \
                        observation.observation, observation.head_seq, \
                        observation.through_seq, observation.classification, \
                        observation.progress, length(observation.body), observation.body, \
                        assessment.head_seq, assessment.verdict, \
                        length(assessment.body), assessment.body \
                 FROM room_relevance_observations AS observation \
                 LEFT JOIN room_relevance_assessments AS assessment \
                    ON assessment.observation = observation.observation \
                 WHERE observation.room = ?1 AND observation.observation > ?2 \
                 ORDER BY observation.observation LIMIT ?3",
            )
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params![room.as_str(), after, probe_limit(budget)?])
            .map_err(GuardError::Store)?;
        let mut records = Vec::with_capacity(budget.rows());
        let mut decoded = 0usize;
        let mut continuation = None;
        while let Some(row) = rows.next().map_err(GuardError::Store)? {
            read_metrics::observe_row();
            if records.len() == budget.rows() {
                continuation = records.last().map(|record: &RoomRelevanceRecord| {
                    record.observation.body.observation.clone()
                });
                break;
            }
            let length = relevance_row_length(row)?;
            if !records.is_empty() && decoded.saturating_add(length) > budget.decoded_bytes() {
                continuation = records.last().map(|record: &RoomRelevanceRecord| {
                    record.observation.body.observation.clone()
                });
                break;
            }
            let record = decode_relevance_record(row, room)?;
            read_metrics::observe_decode(length);
            decoded = decoded.saturating_add(length);
            records.push(record);
        }
        Ok(RoomRelevancePage {
            rows: records,
            continuation,
            metrics,
            witness: CurrentHeadWitness {
                cursor: self.cursor,
            },
        })
    }

    /// One shadow observation and its assessment, absent when no observation
    /// carries `observation`.
    pub fn room_relevance_record(
        &self,
        observation: &RoomAuthorityStamp,
    ) -> Result<Option<RoomRelevanceRecord>, GuardError> {
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT length(CAST(observation.observation AS BLOB)), \
                        observation.observation, observation.head_seq, \
                        observation.through_seq, observation.classification, \
                        observation.progress, length(observation.body), observation.body, \
                        assessment.head_seq, assessment.verdict, \
                        length(assessment.body), assessment.body, observation.room \
                 FROM room_relevance_observations AS observation \
                 LEFT JOIN room_relevance_assessments AS assessment \
                    ON assessment.observation = observation.observation \
                 WHERE observation.observation = ?1",
            )
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query([observation.to_string()])
            .map_err(GuardError::Store)?;
        let Some(row) = rows.next().map_err(GuardError::Store)? else {
            return Ok(None);
        };
        read_metrics::observe_row();
        let room = row
            .get::<_, String>(12)
            .map_err(GuardError::Store)?
            .parse::<wire::TaskStamp>()
            .map_err(|error| {
                GuardError::Invalid(format!(
                    "relevance observation {observation} names invalid room: {error}"
                ))
            })?;
        let length = relevance_row_length(row)?;
        let record = decode_relevance_record(row, &room)?;
        read_metrics::observe_decode(length);
        Ok(Some(record))
    }

    /// How many shadow observations of `room` lie strictly after `after` in
    /// identity order, or in all without it.
    pub fn room_relevance_count(
        &self,
        room: &wire::TaskStamp,
        after: Option<&RoomAuthorityStamp>,
    ) -> Result<usize, GuardError> {
        let after = after.map(ToString::to_string).unwrap_or_default();
        read_metrics::observe_query();
        self.connection
            .query_row(
                "SELECT count(*) FROM room_relevance_observations \
                 WHERE room = ?1 AND observation > ?2",
                params![room.as_str(), after],
                |row| metric(row, 0),
            )
            .map_err(GuardError::Store)
    }

    /// Whole-room shadow classification and assessment counters at this read
    /// transaction's cursor.
    pub fn room_relevance_metrics(
        &self,
        room: &wire::TaskStamp,
    ) -> Result<RoomRelevanceMetrics, GuardError> {
        read_metrics::observe_query();
        self.connection
            .query_row(
                "SELECT count(*), \
                        sum(observation.classification = 'relevant-contribution'), \
                        sum(observation.classification = 'declined'), \
                        sum(observation.classification = 'duplicate'), \
                        sum(observation.classification = 'stale-activity'), \
                        sum(observation.classification IN (
                            'agreement', 'message-count', 'silence', 'elapsed-time'
                        )), \
                        sum(observation.classification = 'missed-opportunity'), \
                        sum(observation.classification = 'unknown'), \
                        sum(observation.progress = 'advanced'), \
                        sum(assessment.verdict = 'confirmed'), \
                        sum(assessment.verdict = 'false-positive'), \
                        sum(assessment.verdict = 'unknown'), \
                        sum(assessment.observation IS NULL) \
                 FROM room_relevance_observations AS observation \
                 LEFT JOIN room_relevance_assessments AS assessment \
                    ON assessment.observation = observation.observation \
                 WHERE observation.room = ?1",
                [room.as_str()],
                |row| {
                    Ok(RoomRelevanceMetrics {
                        observations: metric(row, 0)?,
                        relevant_contributions: metric(row, 1)?,
                        declines: metric(row, 2)?,
                        duplicates: metric(row, 3)?,
                        stale_activity: metric(row, 4)?,
                        missed_opportunities: metric(row, 6)?,
                        non_evidence_signals: metric(row, 5)?,
                        unknown_observations: metric(row, 7)?,
                        progress: metric(row, 8)?,
                        confirmed: metric(row, 9)?,
                        false_positives: metric(row, 10)?,
                        unknown_assessments: metric(row, 11)?,
                        unassessed: metric(row, 12)?,
                    })
                },
            )
            .map_err(GuardError::Store)
    }
}

fn relevance_row_length(row: &rusqlite::Row<'_>) -> Result<usize, GuardError> {
    let identity = indexed_length(row, 0, "relevance observation identity")?;
    let body = indexed_length(row, 6, "relevance observation body")?;
    let assessment = row
        .get::<_, Option<i64>>(10)
        .map_err(GuardError::Store)?
        .map(|length| {
            usize::try_from(length).map_err(|_| {
                GuardError::Invalid("relevance assessment has an invalid byte length".to_owned())
            })
        })
        .transpose()?
        .unwrap_or_default();
    Ok(identity.saturating_add(body).saturating_add(assessment))
}

fn decode_relevance_record(
    row: &rusqlite::Row<'_>,
    room: &wire::TaskStamp,
) -> Result<RoomRelevanceRecord, GuardError> {
    let key = row.get::<_, String>(1).map_err(GuardError::Store)?;
    let through = row.get::<_, i64>(3).map_err(GuardError::Store)?;
    let classification = row.get::<_, String>(4).map_err(GuardError::Store)?;
    let progress = row.get::<_, String>(5).map_err(GuardError::Store)?;
    let body = row.get::<_, Vec<u8>>(7).map_err(GuardError::Store)?;
    let position = row.get(2).map_err(GuardError::Store)?;
    let observation = decode_head::<RoomRelevanceObservation>(position, &body)?;
    let stored_through = u64::try_from(through).map_err(|_| {
        GuardError::Invalid(format!(
            "relevance observation {key} has invalid horizon {through}"
        ))
    })?;
    if observation.body.observation.to_string() != key
        || observation.body.room != *room
        || observation.body.through.get() != stored_through
        || observation.body.classification.name() != classification
        || observation.body.progress.name() != progress
    {
        return Err(GuardError::Invalid(format!(
            "relevance observation projection {key} disagrees with its body"
        )));
    }
    let assessment = decode_relevance_assessment(row, &key)?;
    Ok(RoomRelevanceRecord {
        observation,
        assessment,
    })
}

fn decode_relevance_assessment(
    row: &rusqlite::Row<'_>,
    key: &str,
) -> Result<Option<JournalHead<RoomRelevanceAssessment>>, GuardError> {
    let position = row.get::<_, Option<i64>>(8).map_err(GuardError::Store)?;
    let verdict = row.get::<_, Option<String>>(9).map_err(GuardError::Store)?;
    let body = row
        .get::<_, Option<Vec<u8>>>(11)
        .map_err(GuardError::Store)?;
    match (position, verdict, body) {
        (Some(position), Some(verdict), Some(body)) => {
            let assessment = decode_head::<RoomRelevanceAssessment>(position, &body)?;
            if assessment.body.observation.to_string() != key
                || assessment.body.verdict.name() != verdict
            {
                return Err(GuardError::Invalid(format!(
                    "relevance assessment projection {key} disagrees with its body"
                )));
            }
            Ok(Some(assessment))
        }
        (None, None, None) => Ok(None),
        _ => Err(GuardError::Invalid(format!(
            "relevance assessment projection {key} is partial"
        ))),
    }
}

fn metric(row: &rusqlite::Row<'_>, column: usize) -> Result<usize, rusqlite::Error> {
    let value = row.get::<_, Option<i64>>(column)?.unwrap_or_default();
    usize::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}
