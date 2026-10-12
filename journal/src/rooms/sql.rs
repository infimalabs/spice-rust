//! Room authority as one SQLite transaction stores it.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, params};
use wire::{AnyFact, Seq, TaskPreparation, TaskStamp, TranscriptSourceHead};

use super::{
    Authority, AuthorizationRow, Change, CoordinationRecord, Direction, Projection, ProposalRow,
    Refusal, unsound,
};

/// Stored authority as one commit transaction sees it, overlaid with the
/// tasks, transcript generations, and coordination directions earlier facts
/// of the same write introduce.
pub(crate) struct Stored<'a> {
    connection: &'a Connection,
    tasks: BTreeMap<String, Option<TaskPreparation>>,
    transcripts: BTreeMap<String, TranscriptSourceHead>,
    directions: BTreeMap<String, Direction>,
}

impl<'a> Stored<'a> {
    pub(crate) fn new(connection: &'a Connection) -> Self {
        Self {
            connection,
            tasks: BTreeMap::new(),
            transcripts: BTreeMap::new(),
            directions: BTreeMap::new(),
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

    fn grouped(&self, group: &str) -> Result<bool, Refusal> {
        self.exists(
            "SELECT 1 FROM group_heads WHERE working_group = ?1",
            [group],
        )
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
        Change::GroupStored {
            group,
            seq,
            body,
            anchors,
        } => {
            connection.execute(
                "INSERT INTO group_heads(working_group, head_seq, body) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(working_group) DO UPDATE SET \
                    head_seq = excluded.head_seq, body = excluded.body",
                params![group, sql(seq), body],
            )?;
            connection.execute(
                "DELETE FROM group_anchors WHERE working_group = ?1",
                [&group],
            )?;
            for (ordinal, anchor) in (0_i64..).zip(anchors) {
                insert_anchor(connection, &anchor, &group, ordinal)?;
            }
        }
        Change::GroupRemoved { group } => {
            connection.execute(
                "DELETE FROM group_anchors WHERE working_group = ?1",
                [&group],
            )?;
            connection.execute("DELETE FROM group_heads WHERE working_group = ?1", [&group])?;
        }
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
    }
    Ok(())
}

fn insert_anchor(
    connection: &Connection,
    anchor: &str,
    group: &str,
    ordinal: i64,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO group_anchors(anchor, working_group, ordinal) VALUES (?1, ?2, ?3)",
        params![anchor, group, ordinal],
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
            groups: heads(
                connection,
                "SELECT working_group, head_seq, body FROM group_heads",
            )?,
            anchors: table(
                connection,
                "SELECT anchor, working_group, ordinal FROM group_anchors",
                |row| Ok(((row.get(0)?, row.get(1)?), row.get(2)?)),
            )?,
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
            directed: heads(
                connection,
                "SELECT room, head_seq, body FROM room_directions",
            )?,
            shelved: heads(
                connection,
                "SELECT room, head_seq, body FROM room_shelvings",
            )?,
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
        "DELETE FROM room_publications; DELETE FROM group_anchors; DELETE FROM group_heads; \
         DELETE FROM proposal_heads; DELETE FROM authorization_heads; DELETE FROM room_cursors; \
         DELETE FROM room_directions; DELETE FROM room_shelvings;",
    )?;
    for ((room, publication), seq) in &projection.publications {
        connection.execute(
            "INSERT INTO room_publications(room, publication, seq) VALUES (?1, ?2, ?3)",
            params![room, publication, sql(*seq)],
        )?;
    }
    for (group, (seq, body)) in &projection.groups {
        connection.execute(
            "INSERT INTO group_heads(working_group, head_seq, body) VALUES (?1, ?2, ?3)",
            params![group, sql(*seq), body],
        )?;
    }
    for ((anchor, group), ordinal) in &projection.anchors {
        insert_anchor(connection, anchor, group, *ordinal)?;
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
    Ok(())
}

/// Execution authorization is its controller's conclusion, so each controller
/// it names must still be open as stored before this write's heads move; an
/// already concluded controller can never authorize afterward.
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
        }
    }
    Ok(())
}

/// Every output control and execution authorization in this write left the
/// task heads it names carrying exactly that authority at its fact, and no
/// statement this write moved disagrees with the authorization projection.
///
/// Control writes the output's preparation. Authorization is its controller's
/// positive completion and each selected output's statement authorization at
/// once, so neither can land without the other, and no edit can grant,
/// change, or drop an authorization.
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
                .is_some_and(|(head_seq, head)| head_seq == seq && head.concluded_positively())
            {
                return unsound(format!(
                    "authorization at {seq} is not controller {controller}'s positive completion"
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
/// output's authorization head names, or none when it has no such head.
fn settle_statements(connection: &Connection, first: Seq, last: Seq) -> Result<(), Refusal> {
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

fn sql(seq: u64) -> i64 {
    i64::try_from(seq).expect("journal positions fit SQLite")
}

fn unsql(seq: i64) -> u64 {
    u64::try_from(seq).unwrap_or_default()
}
