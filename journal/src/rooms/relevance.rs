//! Source-backed shadow observations that carry no room or task authority.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use wire::rooms::{
    ROOM_RELEVANCE_SOURCE_LIMIT, ROOM_RELEVANCE_TEXT_LIMIT, RoomProgressStanding,
    RoomRelevanceAssessed, RoomRelevanceClass, RoomRelevanceObserved, RoomSource, RoomSourceView,
};
use wire::{Actor, AnyFact, Seq};

use super::{Authority, Change, Refusal, require_source, require_task, unsound};

/// Relevance projection tables, created with the current schema.
pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE room_relevance_observations (
    observation TEXT PRIMARY KEY,
    room TEXT NOT NULL,
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    through_seq INTEGER NOT NULL CHECK (through_seq >= 0 AND through_seq < head_seq),
    classification TEXT NOT NULL CHECK (classification IN (
        'relevant-contribution', 'declined', 'duplicate', 'stale-activity',
        'missed-opportunity', 'agreement', 'message-count', 'silence',
        'elapsed-time', 'unknown'
    )),
    progress TEXT NOT NULL CHECK (progress IN ('advanced', 'no-progress', 'unknown')),
    body BLOB NOT NULL
) STRICT;
CREATE INDEX room_relevance_observations_room_seq
    ON room_relevance_observations(room, head_seq);
CREATE TABLE room_relevance_assessments (
    observation TEXT PRIMARY KEY REFERENCES room_relevance_observations(observation),
    head_seq INTEGER NOT NULL UNIQUE REFERENCES fact_envelopes(seq),
    verdict TEXT NOT NULL CHECK (verdict IN ('confirmed', 'false-positive', 'unknown')),
    body BLOB NOT NULL
) STRICT;
"#;

/// One stored observation row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ObservationRow {
    pub(crate) room: String,
    pub(crate) seq: u64,
    pub(crate) through: u64,
    pub(crate) classification: &'static str,
    pub(crate) progress: &'static str,
    pub(crate) body: Vec<u8>,
}

/// One stored operator assessment row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AssessmentRow {
    pub(crate) seq: u64,
    pub(crate) verdict: &'static str,
    pub(crate) body: Vec<u8>,
}

/// Complete relevance projection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Heads {
    pub(crate) observations: BTreeMap<String, ObservationRow>,
    pub(crate) assessments: BTreeMap<String, AssessmentRow>,
}

/// One relevance projection change.
pub(crate) enum Edit {
    Observed {
        observation: String,
        row: ObservationRow,
    },
    Assessed {
        observation: String,
        row: AssessmentRow,
    },
}

impl Heads {
    pub(crate) fn apply(&mut self, edit: Edit) {
        match edit {
            Edit::Observed { observation, row } => {
                self.observations.insert(observation, row);
            }
            Edit::Assessed { observation, row } => {
                self.assessments.insert(observation, row);
            }
        }
    }

    pub(crate) fn load(connection: &Connection) -> Result<Self, rusqlite::Error> {
        let mut observations = connection.prepare(
            "SELECT observation, room, head_seq, through_seq, classification, progress, body \
             FROM room_relevance_observations",
        )?;
        let observations = observations
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    ObservationRow {
                        room: row.get(1)?,
                        seq: super::sql::unsql(row.get(2)?),
                        through: super::sql::unsql(row.get(3)?),
                        classification: stored_name(row.get(4)?),
                        progress: stored_name(row.get(5)?),
                        body: row.get(6)?,
                    },
                ))
            })?
            .collect::<Result<_, _>>()?;
        let mut assessments = connection.prepare(
            "SELECT observation, head_seq, verdict, body FROM room_relevance_assessments",
        )?;
        let assessments = assessments
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    AssessmentRow {
                        seq: super::sql::unsql(row.get(1)?),
                        verdict: stored_name(row.get(2)?),
                        body: row.get(3)?,
                    },
                ))
            })?
            .collect::<Result<_, _>>()?;
        Ok(Self {
            observations,
            assessments,
        })
    }

    pub(crate) fn write(&self, connection: &Connection) -> Result<(), rusqlite::Error> {
        connection.execute_batch(
            "DELETE FROM room_relevance_assessments; DELETE FROM room_relevance_observations;",
        )?;
        for (observation, row) in &self.observations {
            insert_observation(connection, observation, row)?;
        }
        for (observation, row) in &self.assessments {
            insert_assessment(connection, observation, row)?;
        }
        Ok(())
    }
}

pub(crate) fn stored_observation(
    connection: &Connection,
    observation: &str,
) -> Result<Option<ObservationRow>, Refusal> {
    connection
        .query_row(
            "SELECT room, head_seq, through_seq, classification, progress, body \
             FROM room_relevance_observations WHERE observation = ?1",
            [observation],
            |row| {
                Ok(ObservationRow {
                    room: row.get(0)?,
                    seq: super::sql::unsql(row.get(1)?),
                    through: super::sql::unsql(row.get(2)?),
                    classification: stored_name(row.get(3)?),
                    progress: stored_name(row.get(4)?),
                    body: row.get(5)?,
                })
            },
        )
        .optional()
        .map_err(Refusal::Store)
}

pub(crate) fn stored_assessment(
    connection: &Connection,
    observation: &str,
) -> Result<Option<AssessmentRow>, Refusal> {
    connection
        .query_row(
            "SELECT head_seq, verdict, body FROM room_relevance_assessments \
             WHERE observation = ?1",
            [observation],
            |row| {
                Ok(AssessmentRow {
                    seq: super::sql::unsql(row.get(0)?),
                    verdict: stored_name(row.get(1)?),
                    body: row.get(2)?,
                })
            },
        )
        .optional()
        .map_err(Refusal::Store)
}

pub(crate) fn apply_sql(connection: &Connection, edit: Edit) -> Result<(), rusqlite::Error> {
    match edit {
        Edit::Observed { observation, row } => insert_observation(connection, &observation, &row),
        Edit::Assessed { observation, row } => insert_assessment(connection, &observation, &row),
    }
}

fn insert_observation(
    connection: &Connection,
    observation: &str,
    row: &ObservationRow,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO room_relevance_observations(\
             observation, room, head_seq, through_seq, classification, progress, body\
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            observation,
            row.room,
            super::sql::sql(row.seq),
            super::sql::sql(row.through),
            row.classification,
            row.progress,
            row.body,
        ],
    )?;
    Ok(())
}

fn insert_assessment(
    connection: &Connection,
    observation: &str,
    row: &AssessmentRow,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO room_relevance_assessments(observation, head_seq, verdict, body) \
         VALUES (?1, ?2, ?3, ?4)",
        params![observation, super::sql::sql(row.seq), row.verdict, row.body],
    )?;
    Ok(())
}

/// Admit one relevance observation or assessment, or `None` for another fact.
pub(crate) fn fold(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    fact: &AnyFact,
) -> Option<Result<(), Refusal>> {
    if let Some(body) = fact.read::<RoomRelevanceObserved>() {
        return Some(observe(authority, seq, actor, body));
    }
    fact.read::<RoomRelevanceAssessed>()
        .map(|body| assess(authority, seq, actor, body))
}

fn observe(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    body: RoomRelevanceObserved,
) -> Result<(), Refusal> {
    if *actor != Actor::Spice {
        return unsound(format!(
            "shadow relevance observation is written by {actor}, not spice"
        ));
    }
    let observation = body.observation;
    require_task(authority, &observation.room, "relevance observation room")?;
    if observation.through.get() >= seq.get() {
        return unsound(format!(
            "relevance observation {} horizon {} is not earlier than {seq}",
            observation.observation, observation.through
        ));
    }
    if observation.sources.is_empty() || observation.sources.len() > ROOM_RELEVANCE_SOURCE_LIMIT {
        return unsound(format!(
            "relevance observation {} has {} sources outside 1..={ROOM_RELEVANCE_SOURCE_LIMIT}",
            observation.observation,
            observation.sources.len()
        ));
    }
    for (index, view) in observation.sources.iter().enumerate() {
        let source = view_source(view);
        if observation.sources[..index]
            .iter()
            .any(|previous| view_source(previous) == source)
        {
            return unsound(format!(
                "relevance observation {} repeats source {index}",
                observation.observation
            ));
        }
        if let RoomSource::Journal { fact } = source
            && fact.get() > observation.through.get()
        {
            return unsound(format!(
                "relevance observation {} source {fact} exceeds horizon {}",
                observation.observation, observation.through
            ));
        }
        if matches!(view, RoomSourceView::Available { .. }) {
            require_source(authority, seq, source)?;
        }
    }
    let expected = match observation.classification {
        RoomRelevanceClass::RelevantContribution => RoomProgressStanding::Advanced,
        RoomRelevanceClass::Declined
        | RoomRelevanceClass::Duplicate
        | RoomRelevanceClass::StaleActivity
        | RoomRelevanceClass::MissedOpportunity
        | RoomRelevanceClass::Agreement
        | RoomRelevanceClass::MessageCount
        | RoomRelevanceClass::Silence
        | RoomRelevanceClass::ElapsedTime => RoomProgressStanding::NoProgress,
        RoomRelevanceClass::Unknown => RoomProgressStanding::Unknown,
    };
    if observation.progress != expected {
        return unsound(format!(
            "relevance observation {} class {:?} requires progress {:?}, not {:?}",
            observation.observation, observation.classification, expected, observation.progress
        ));
    }
    require_text(
        &observation.explanation,
        "explanation",
        &observation.observation.to_string(),
        false,
    )?;
    require_text(
        &observation.exceptions,
        "exceptions",
        &observation.observation.to_string(),
        true,
    )?;
    require_text(
        &observation.uncertainty,
        "uncertainty",
        &observation.observation.to_string(),
        true,
    )?;
    let key = observation.observation.to_string();
    if authority.relevance_observation(&key)?.is_some() {
        return unsound(format!("relevance observation {key} already exists"));
    }
    let row = ObservationRow {
        room: observation.room.to_string(),
        seq: seq.get(),
        through: observation.through.get(),
        body: wire::encode(&observation)?,
        classification: observation.classification.name(),
        progress: observation.progress.name(),
    };
    authority.change(Change::Relevance(Edit::Observed {
        observation: key,
        row,
    }))
}

fn assess(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    body: RoomRelevanceAssessed,
) -> Result<(), Refusal> {
    if *actor != Actor::Operator {
        return unsound(format!(
            "shadow relevance assessment is written by {actor}, not the operator"
        ));
    }
    let assessment = body.assessment;
    let key = assessment.observation.to_string();
    if authority.relevance_observation(&key)?.is_none() {
        return unsound(format!(
            "relevance assessment names absent observation {key}"
        ));
    }
    if authority.relevance_assessment(&key)?.is_some() {
        return unsound(format!("relevance observation {key} is already assessed"));
    }
    require_text(&assessment.explanation, "assessment", &key, false)?;
    let row = AssessmentRow {
        seq: seq.get(),
        verdict: assessment.verdict.name(),
        body: wire::encode(&assessment)?,
    };
    authority.change(Change::Relevance(Edit::Assessed {
        observation: key,
        row,
    }))
}

fn stored_name(name: String) -> &'static str {
    match name.as_str() {
        "relevant-contribution" => "relevant-contribution",
        "declined" => "declined",
        "duplicate" => "duplicate",
        "stale-activity" => "stale-activity",
        "missed-opportunity" => "missed-opportunity",
        "agreement" => "agreement",
        "message-count" => "message-count",
        "silence" => "silence",
        "elapsed-time" => "elapsed-time",
        "advanced" => "advanced",
        "no-progress" => "no-progress",
        "confirmed" => "confirmed",
        "false-positive" => "false-positive",
        "unknown" => "unknown",
        _ => "",
    }
}

fn view_source(view: &RoomSourceView) -> &RoomSource {
    match view {
        RoomSourceView::Available { source } | RoomSourceView::Unavailable { source, .. } => source,
    }
}

fn require_text(text: &str, field: &str, observation: &str, empty: bool) -> Result<(), Refusal> {
    if (!empty && text.trim().is_empty()) || text.len() > ROOM_RELEVANCE_TEXT_LIMIT {
        return unsound(format!(
            "relevance observation {observation} {field} is outside {}..={ROOM_RELEVANCE_TEXT_LIMIT} bytes",
            usize::from(!empty)
        ));
    }
    Ok(())
}
