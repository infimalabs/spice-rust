//! Room authority as one SQLite transaction stores it.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use wire::{AnyFact, CanonicalUuid, Seq, TaskPreparation, TaskStamp, TranscriptSourceHead};

use super::{
    Authority, AuthorizationRow, Change, CoordinationRecord, Direction, Projection, ProposalRow,
    Refusal, conversation, relevance, unsound,
};

/// Stored authority as one commit transaction sees it, overlaid with the
/// tasks, transcript generations, and directions earlier facts of the same
/// write introduce.
pub(crate) struct Stored<'a> {
    connection: &'a Connection,
    tasks: BTreeMap<String, Option<TaskPreparation>>,
    transcripts: BTreeMap<String, TranscriptSourceHead>,
    directions: BTreeMap<String, Direction>,
    operator_directions: BTreeMap<String, CanonicalUuid>,
}

impl<'a> Stored<'a> {
    pub(crate) fn new(connection: &'a Connection) -> Self {
        Self {
            connection,
            tasks: BTreeMap::new(),
            transcripts: BTreeMap::new(),
            directions: BTreeMap::new(),
            operator_directions: BTreeMap::new(),
        }
    }

    fn exists(&self, statement: &str, key: impl rusqlite::Params) -> Result<bool, Refusal> {
        Ok(self
            .connection
            .query_row(statement, key, |_| Ok(()))
            .optional()?
            .is_some())
    }
}

impl Authority for Stored<'_> {
    fn task(&self, task: &TaskStamp) -> Result<Option<Option<TaskPreparation>>, Refusal> {
        if let Some(preparation) = self.tasks.get(task.as_str()) {
            return Ok(Some(preparation.clone()));
        }
        let body = self
            .connection
            .query_row(
                "SELECT body FROM task_heads WHERE stamp = ?1",
                [task.as_str()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        body.map(|body| Ok(wire::decode::<wire::TaskHead>(&body)?.preparation))
            .transpose()
    }

    fn set_task(&mut self, task: &TaskStamp, preparation: Option<TaskPreparation>) {
        self.tasks.insert(task.as_str().to_owned(), preparation);
    }

    fn account(&mut self, head: &TranscriptSourceHead) {
        self.transcripts
            .insert(head.key().into_string(), head.clone());
    }

    fn transcript(
        &self,
        head: &TranscriptSourceHead,
    ) -> Result<Option<TranscriptSourceHead>, Refusal> {
        let key = head.key().into_string();
        if let Some(current) = self.transcripts.get(&key) {
            return Ok(Some(current.clone()));
        }
        let body = self
            .connection
            .query_row(
                "SELECT body FROM transcript_source_heads WHERE stream = ?1",
                [&key],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        let current = body
            .map(|body| wire::decode::<TranscriptSourceHead>(&body))
            .transpose()?;
        if current
            .as_ref()
            .is_some_and(|current| current.key().as_str() != key)
        {
            return unsound(format!(
                "transcript source head indexed as {key:?} carries another stream"
            ));
        }
        Ok(current)
    }

    fn published(&self, room: &str, publication: &str) -> Result<bool, Refusal> {
        self.exists(
            "SELECT 1 FROM room_publications WHERE room = ?1 AND publication = ?2",
            params![room, publication],
        )
    }

    fn publication_at(&self, seq: u64) -> Result<bool, Refusal> {
        self.exists("SELECT 1 FROM room_publications WHERE seq = ?1", [sql(seq)])
    }

    fn cursor(&self, lane: &str) -> Result<Option<Vec<u8>>, Refusal> {
        Ok(self
            .connection
            .query_row(
                "SELECT body FROM room_cursors WHERE lane = ?1",
                [lane],
                |row| row.get(0),
            )
            .optional()?)
    }

    fn horizon(&self, lane: &str) -> Result<bool, Refusal> {
        self.exists("SELECT 1 FROM room_horizons WHERE lane = ?1", [lane])
    }

    fn group_partition(&self) -> Result<Option<Vec<u8>>, Refusal> {
        Ok(self
            .connection
            .query_row(
                "SELECT body FROM group_partition WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    fn proposal(&self, subject: &str) -> Result<Option<Vec<u8>>, Refusal> {
        Ok(self
            .connection
            .query_row(
                "SELECT body FROM proposal_heads WHERE subject = ?1",
                [subject],
                |row| row.get(0),
            )
            .optional()?)
    }

    fn proposal_author(&self, subject: &str) -> Result<Option<wire::Actor>, Refusal> {
        let actor = self
            .connection
            .query_row(
                "SELECT fact.actor FROM proposal_heads AS proposal \
                 JOIN fact_envelopes AS fact ON fact.seq = proposal.head_seq \
                 WHERE proposal.subject = ?1",
                [subject],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        actor
            .map(|actor| {
                actor.parse().map_err(|_| {
                    Refusal::Unsound(format!("proposal {subject} author {actor} is not an actor"))
                })
            })
            .transpose()
    }

    fn candidate_decided(&self, candidate: &wire::rooms::CandidateRef) -> Result<bool, Refusal> {
        self.exists(
            "SELECT 1 FROM proposal_heads \
             WHERE controller = ?1 \
             AND json_extract(CAST(body AS TEXT), '$.outcome.candidate.candidate') = ?2 \
             LIMIT 1",
            params![candidate.controller.as_str(), candidate.candidate.as_str()],
        )
    }

    fn output_authorized(&self, output: &str) -> Result<bool, Refusal> {
        self.exists(
            "SELECT 1 FROM authorization_heads WHERE output = ?1",
            [output],
        )
    }

    fn decision_authorized(&self, decision: &str) -> Result<bool, Refusal> {
        self.exists(
            "SELECT 1 FROM authorization_heads WHERE decision = ?1 LIMIT 1",
            [decision],
        )
    }

    fn direction(&self, key: &str) -> Result<Option<Direction>, Refusal> {
        if let Some(direction) = self.directions.get(key) {
            return Ok(Some(direction.clone()));
        }
        let row = self
            .connection
            .query_row(
                "SELECT steering.head_seq, steering.body, fact.actor FROM steering_heads AS steering \
                 JOIN fact_envelopes AS fact ON fact.seq = steering.created_seq \
                 WHERE steering.steer_key = ?1",
                [key],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((head_seq, body, creator)) = row else {
            return Ok(None);
        };
        let head = wire::decode::<wire::SteeringHead>(&body)?;
        Ok(Direction::stored(&head, &creator, unsql(head_seq)))
    }

    fn set_direction(&mut self, key: &str, direction: Direction) {
        self.directions.insert(key.to_owned(), direction);
    }

    fn operator_direction(&self, key: &str) -> Result<Option<CanonicalUuid>, Refusal> {
        if let Some(lane) = self.operator_directions.get(key) {
            return Ok(Some(*lane));
        }
        let row = self
            .connection
            .query_row(
                "SELECT steering.body, fact.actor FROM steering_heads AS steering \
                 JOIN fact_envelopes AS fact ON fact.seq = steering.created_seq \
                 WHERE steering.steer_key = ?1",
                [key],
                |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((body, creator)) = row else {
            return Ok(None);
        };
        let head = wire::decode::<wire::SteeringHead>(&body)?;
        Ok(
            (creator == wire::Actor::Operator.to_string() && head.fanout.is_some())
                .then_some(head.lane),
        )
    }

    fn set_operator_direction(&mut self, key: &str, lane: CanonicalUuid) {
        self.operator_directions.insert(key.to_owned(), lane);
    }

    fn coordination(
        &self,
        record: CoordinationRecord,
        room: &str,
    ) -> Result<Option<(u64, Vec<u8>)>, Refusal> {
        Ok(self
            .connection
            .query_row(
                &format!(
                    "SELECT head_seq, body FROM {} WHERE room = ?1",
                    record.table()
                ),
                [room],
                |row| Ok((unsql(row.get::<_, i64>(0)?), row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()?)
    }

    fn conversation(&self, publication: &str) -> Result<Option<Vec<u8>>, Refusal> {
        conversation::stored_publication(self.connection, publication)
    }

    fn room_item(&self, item: &str) -> Result<Option<conversation::ItemRow>, Refusal> {
        conversation::stored_item(self.connection, item)
    }

    fn question_result(&self, result: &str) -> Result<Option<conversation::ResultRow>, Refusal> {
        conversation::stored_result(self.connection, result)
    }

    fn evidence(&self, task: &TaskStamp) -> Result<Vec<wire::rooms::QuestionResultRef>, Refusal> {
        let mut statement = self
            .connection
            .prepare("SELECT result FROM task_evidence WHERE task = ?1 ORDER BY result")?;
        let keys = statement
            .query_map([task.as_str()], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        super::evidence::parse(keys)
    }

    fn pending_results(&self, requester: &str) -> Result<usize, Refusal> {
        conversation::stored_pending_results(self.connection, requester)
    }
    fn relevance_observation(
        &self,
        observation: &str,
    ) -> Result<Option<relevance::ObservationRow>, Refusal> {
        relevance::stored_observation(self.connection, observation)
    }

    fn relevance_assessment(
        &self,
        observation: &str,
    ) -> Result<Option<relevance::AssessmentRow>, Refusal> {
        relevance::stored_assessment(self.connection, observation)
    }

    fn change(&mut self, change: Change) -> Result<(), Refusal> {
        apply(self.connection, change)?;
        Ok(())
    }
}

fn apply(connection: &Connection, change: Change) -> Result<(), rusqlite::Error> {
    match change {
        Change::Publication {
            room,
            publication,
            seq,
        } => {
            connection.execute(
                "INSERT INTO room_publications(room, publication, seq) VALUES (?1, ?2, ?3)",
                params![room, publication, sql(seq)],
            )?;
        }
        Change::GroupPartition { seq, body } => put_partition(connection, seq, &body)?,
        Change::Proposal { subject, row } => insert_proposal(connection, &subject, &row)?,
        Change::Authorization { outputs, row } => {
            for output in outputs {
                insert_authorization(connection, &output, &row)?;
            }
        }
        Change::Cursor { lane, seq, body } => {
            connection.execute(
                "INSERT INTO room_cursors(lane, head_seq, body) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(lane) DO UPDATE SET head_seq = excluded.head_seq, body = excluded.body",
                params![lane, sql(seq), body],
            )?;
        }
        Change::Horizon { lane, seq, body } => {
            connection.execute(
                "INSERT INTO room_horizons(lane, head_seq, body) VALUES (?1, ?2, ?3)",
                params![lane, sql(seq), body],
            )?;
        }
        Change::Coordination {
            record,
            room,
            seq,
            body,
        } => {
            connection.execute(
                &format!(
                    "INSERT INTO {}(room, head_seq, body) VALUES (?1, ?2, ?3) \
                     ON CONFLICT(room) DO UPDATE SET head_seq = excluded.head_seq, body = excluded.body",
                    record.table()
                ),
                params![room, sql(seq), body],
            )?;
        }
        Change::Conversation(edit) => conversation::apply_sql(connection, edit)?,
        Change::Evidence {
            task,
            released,
            required,
        } => super::evidence::apply_sql(
            connection,
            &task,
            released.as_deref(),
            required
                .as_ref()
                .map(|(result, seq)| (result.as_str(), *seq)),
        )?,
        Change::Relevance(edit) => relevance::apply_sql(connection, edit)?,
    }
    Ok(())
}

fn put_partition(connection: &Connection, seq: u64, body: &[u8]) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO group_partition(singleton, head_seq, body) VALUES (1, ?1, ?2) \
         ON CONFLICT(singleton) DO UPDATE SET head_seq = excluded.head_seq, body = excluded.body",
        params![sql(seq), body],
    )?;
    Ok(())
}

fn insert_proposal(
    connection: &Connection,
    subject: &str,
    row: &ProposalRow,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO proposal_heads(subject, controller, head_seq, body) VALUES (?1, ?2, ?3, ?4)",
        params![subject, row.controller, sql(row.seq), row.body],
    )?;
    Ok(())
}

fn insert_authorization(
    connection: &Connection,
    output: &str,
    row: &AuthorizationRow,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO authorization_heads(output, decision, head_seq, body) \
         VALUES (?1, ?2, ?3, ?4)",
        params![output, row.decision, sql(row.seq), row.body],
    )?;
    Ok(())
}

impl Projection {
    /// Read every stored projection row.
    pub(crate) fn load(connection: &Connection) -> Result<Self, rusqlite::Error> {
        Ok(Self {
            publications: table(
                connection,
                "SELECT room, publication, seq FROM room_publications",
                |row| Ok(((row.get(0)?, row.get(1)?), unsql(row.get(2)?))),
            )?,
            partition: connection
                .query_row(
                    "SELECT head_seq, body FROM group_partition WHERE singleton = 1",
                    [],
                    |row| Ok((unsql(row.get(0)?), row.get(1)?)),
                )
                .optional()?,
            proposals: table(
                connection,
                "SELECT subject, controller, head_seq, body FROM proposal_heads",
                |row| {
                    Ok((
                        row.get(0)?,
                        ProposalRow {
                            controller: row.get(1)?,
                            seq: unsql(row.get(2)?),
                            body: row.get(3)?,
                        },
                    ))
                },
            )?,
            authorizations: table(
                connection,
                "SELECT output, decision, head_seq, body FROM authorization_heads",
                |row| {
                    Ok((
                        row.get(0)?,
                        AuthorizationRow {
                            decision: row.get(1)?,
                            seq: unsql(row.get(2)?),
                            body: row.get(3)?,
                        },
                    ))
                },
            )?,
            cursors: heads(connection, "SELECT lane, head_seq, body FROM room_cursors")?,
            horizons: heads(connection, "SELECT lane, head_seq, body FROM room_horizons")?,
            directed: heads(
                connection,
                "SELECT room, head_seq, body FROM room_directions",
            )?,
            shelved: heads(
                connection,
                "SELECT room, head_seq, body FROM room_shelvings",
            )?,
            conversation: conversation::Heads::load(connection)?,
            evidence: table(
                connection,
                "SELECT task, result, head_seq FROM task_evidence",
                |row| Ok(((row.get(0)?, row.get(1)?), unsql(row.get(2)?))),
            )?,
            relevance: relevance::Heads::load(connection)?,
        })
    }
}

fn table<K: Ord, V>(
    connection: &Connection,
    query: &str,
    decode: impl FnMut(&rusqlite::Row<'_>) -> Result<(K, V), rusqlite::Error>,
) -> Result<BTreeMap<K, V>, rusqlite::Error> {
    let mut statement = connection.prepare(query)?;
    statement.query_map([], decode)?.collect()
}

/// Read one keyed head table whose rows are `(key, head_seq, body)`.
fn heads(
    connection: &Connection,
    query: &str,
) -> Result<BTreeMap<String, (u64, Vec<u8>)>, rusqlite::Error> {
    table(connection, query, |row| {
        Ok((row.get(0)?, (unsql(row.get(1)?), row.get(2)?)))
    })
}

/// Replace every stored projection row with `projection`.
pub(crate) fn write_projection(
    connection: &Connection,
    projection: &Projection,
) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "DELETE FROM room_publications; DELETE FROM group_partition; \
         DELETE FROM proposal_heads; DELETE FROM authorization_heads; DELETE FROM room_cursors; \
         DELETE FROM room_horizons; DELETE FROM room_directions; DELETE FROM room_shelvings; \
         DELETE FROM task_evidence;",
    )?;
    for ((room, publication), seq) in &projection.publications {
        connection.execute(
            "INSERT INTO room_publications(room, publication, seq) VALUES (?1, ?2, ?3)",
            params![room, publication, sql(*seq)],
        )?;
    }
    if let Some((seq, body)) = &projection.partition {
        put_partition(connection, *seq, body)?;
    }
    for (subject, row) in &projection.proposals {
        insert_proposal(connection, subject, row)?;
    }
    for (output, row) in &projection.authorizations {
        insert_authorization(connection, output, row)?;
    }
    for (lane, (seq, body)) in &projection.cursors {
        connection.execute(
            "INSERT INTO room_cursors(lane, head_seq, body) VALUES (?1, ?2, ?3)",
            params![lane, sql(*seq), body],
        )?;
    }
    for (lane, (seq, body)) in &projection.horizons {
        connection.execute(
            "INSERT INTO room_horizons(lane, head_seq, body) VALUES (?1, ?2, ?3)",
            params![lane, sql(*seq), body],
        )?;
    }
    for (record, heads) in [
        (CoordinationRecord::Directed, &projection.directed),
        (CoordinationRecord::Shelved, &projection.shelved),
    ] {
        for (room, (seq, body)) in heads {
            connection.execute(
                &format!(
                    "INSERT INTO {}(room, head_seq, body) VALUES (?1, ?2, ?3)",
                    record.table()
                ),
                params![room, sql(*seq), body],
            )?;
        }
    }
    for ((task, result), seq) in &projection.evidence {
        super::evidence::insert(connection, task, result, *seq)?;
    }
    projection.conversation.write(connection)?;
    projection.relevance.write(connection)
}

/// Execution authorization is its controller's conclusion, so each controller
/// it names must still be open as stored before this write's heads move; an
/// already concluded controller can never authorize afterward. A room
/// contribution likewise enters only a stored, open Design.
pub(crate) fn require_open_controllers<'a>(
    connection: &Connection,
    facts: impl IntoIterator<Item = &'a AnyFact>,
) -> Result<(), Refusal> {
    for fact in facts {
        if let Some(wire::rooms::ExecutionAuthorized { authorization }) = fact.read() {
            let controller = &authorization.candidate.controller;
            let workflow = stored::<wire::WorkflowHead>(connection, WORKFLOW, controller)?;
            if !workflow.is_some_and(|(_, head)| head.completion.is_none()) {
                return unsound(format!(
                    "controller {controller} already concluded and cannot authorize afterward"
                ));
            }
        } else if let Some(contributed) = fact.read::<wire::rooms::RoomContributed>() {
            let room = &contributed.room;
            let statement = stored::<wire::TaskHead>(connection, STATEMENT, room)?;
            let workflow = stored::<wire::WorkflowHead>(connection, WORKFLOW, room)?;
            let design =
                statement.is_some_and(|(_, head)| head.intent == Some(wire::TaskIntent::Design));
            let open = workflow
                .is_some_and(|(_, head)| head.status == "pending" && head.completion.is_none());
            if !(design && open) {
                return unsound(format!("contribution in {room} names no open Design"));
            }
        }
    }
    Ok(())
}

/// Every output control and execution authorization in this write left the
/// task heads it names carrying exactly that authority at its fact, and no
/// statement this write moved disagrees with the authorization projection.
///
/// Control writes the output's preparation. Authorization writes each
/// selected output's statement authorization and leaves its controlling
/// Design open, so a selection can never conclude the Design, and no edit can
/// grant, change, or drop an authorization.
pub(crate) fn settle_controls<'a>(
    connection: &Connection,
    facts: impl IntoIterator<Item = (Seq, &'a AnyFact)>,
) -> Result<(), Refusal> {
    let mut span: Option<(Seq, Seq)> = None;
    for (seq, fact) in facts {
        span = Some(span.map_or((seq, seq), |(first, _)| (first, seq)));
        if let Some(control) = fact.read::<wire::rooms::OutputControlled>() {
            let head = stored::<wire::TaskHead>(connection, STATEMENT, &control.output)?;
            if !head.is_some_and(|(_, head)| {
                head.revision == seq && head.preparation.as_ref() == Some(&control.preparation)
            }) {
                return unsound(format!(
                    "output {} control at {seq} has no task head carrying its preparation",
                    control.output
                ));
            }
        } else if let Some(wire::rooms::ExecutionAuthorized { authorization }) = fact.read() {
            for output in &authorization.selected_outputs {
                let head = stored::<wire::TaskHead>(connection, STATEMENT, output)?;
                if !head.is_some_and(|(_, head)| {
                    head.revision == seq
                        && head.authorization.as_ref() == Some(&authorization.decision)
                }) {
                    return unsound(format!(
                        "selected output {output} authorization at {seq} has no task head \
                         carrying its decision"
                    ));
                }
            }
            let controller = &authorization.candidate.controller;
            let workflow = stored::<wire::WorkflowHead>(connection, WORKFLOW, controller)?;
            if !workflow
                .is_some_and(|(_, head)| head.status == "pending" && head.completion.is_none())
            {
                return unsound(format!(
                    "authorization at {seq} must leave controller {controller} open"
                ));
            }
        }
    }
    match span {
        Some((first, last)) => settle_statements(connection, first, last),
        None => Ok(()),
    }
}

/// Each statement written in `first..=last` carries exactly the decision its
/// output's authorization head names, or none when it has no such head, and
/// exactly the evidence explicit evidence changes left it requiring.
fn settle_statements(connection: &Connection, first: Seq, last: Seq) -> Result<(), Refusal> {
    let mut required = connection.prepare("SELECT result FROM task_evidence WHERE task = ?1")?;
    let mut statement = connection.prepare(
        "SELECT statement.stamp, statement.body, authorization.decision \
         FROM task_heads AS statement \
         LEFT JOIN authorization_heads AS authorization ON authorization.output = statement.stamp \
         WHERE statement.head_seq BETWEEN ?1 AND ?2",
    )?;
    let mut rows = statement.query(params![sql(first.get()), sql(last.get())])?;
    while let Some(row) = rows.next()? {
        let stamp: String = row.get(0)?;
        let head: wire::TaskHead = wire::decode(&row.get::<_, Vec<u8>>(1)?)?;
        let authorized: Option<String> = row.get(2)?;
        let carried = head
            .authorization
            .map(|decision| super::subject(&wire::rooms::ProposalRef::Decision { decision }))
            .transpose()?;
        if carried != authorized {
            return unsound(format!(
                "task {stamp} statement authorization disagrees with its execution authorization"
            ));
        }
        let projected = required
            .query_map([&stamp], |row| row.get::<_, String>(0))?
            .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
        let carried = head
            .evidence
            .iter()
            .map(ToString::to_string)
            .collect::<std::collections::BTreeSet<_>>();
        if carried.len() != head.evidence.len() || carried != projected {
            return unsound(format!(
                "task {stamp} statement evidence disagrees with its explicit evidence changes"
            ));
        }
    }
    Ok(())
}

const STATEMENT: &str = "task_heads";
const WORKFLOW: &str = "workflow_heads";

/// One stored task head and the position it was written at.
fn stored<T: serde::de::DeserializeOwned>(
    connection: &Connection,
    table: &'static str,
    task: &TaskStamp,
) -> Result<Option<(Seq, T)>, Refusal> {
    let row = connection
        .query_row(
            &format!("SELECT head_seq, body FROM {table} WHERE stamp = ?1"),
            [task.as_str()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?;
    let Some((head_seq, body)) = row else {
        return Ok(None);
    };
    let Some(head_seq) = u64::try_from(head_seq).ok().and_then(Seq::new) else {
        return unsound(format!("{table} row {task} has no valid head position"));
    };
    Ok(Some((head_seq, wire::decode(&body)?)))
}

pub(super) fn sql(seq: u64) -> i64 {
    i64::try_from(seq).expect("journal positions fit SQLite")
}

pub(super) fn unsql(seq: i64) -> u64 {
    u64::try_from(seq).unwrap_or_default()
}
