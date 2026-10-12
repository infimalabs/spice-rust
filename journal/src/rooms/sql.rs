//! Room authority as one SQLite transaction stores it.

use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, OptionalExtension, params};
use wire::{AnyFact, Seq, TaskPreparation, TaskStamp, TranscriptSourceHead};

use super::{Authority, AuthorizationRow, Change, Projection, ProposalRow, Refusal, unsound};

/// Stored authority as one commit transaction sees it, overlaid with the
/// tasks and transcript generations earlier facts of the same write introduce.
pub(crate) struct Stored<'a> {
    connection: &'a Connection,
    tasks: BTreeMap<String, Option<TaskPreparation>>,
    transcripts: BTreeSet<(String, String)>,
}

impl<'a> Stored<'a> {
    pub(crate) fn new(connection: &'a Connection) -> Self {
        Self {
            connection,
            tasks: BTreeMap::new(),
            transcripts: BTreeSet::new(),
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
            .insert((head.key().into_string(), head.source.to_string()));
    }

    fn accounted(&self, head: &TranscriptSourceHead) -> Result<bool, Refusal> {
        let key = head.key().into_string();
        if self
            .transcripts
            .contains(&(key.clone(), head.source.to_string()))
        {
            return Ok(true);
        }
        let body = self
            .connection
            .query_row(
                "SELECT body FROM transcript_source_heads WHERE stream = ?1",
                [&key],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        Ok(match body {
            Some(body) => wire::decode::<TranscriptSourceHead>(&body)?.source == head.source,
            None => false,
        })
    }

    fn published(&self, room: &str, publication: &str) -> Result<bool, Refusal> {
        self.exists(
            "SELECT 1 FROM room_publications WHERE room = ?1 AND publication = ?2",
            params![room, publication],
        )
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
            groups: table(
                connection,
                "SELECT working_group, head_seq, body FROM group_heads",
                |row| Ok((row.get(0)?, (unsql(row.get(1)?), row.get(2)?))),
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

/// Replace every stored projection row with `projection`.
pub(crate) fn write_projection(
    connection: &Connection,
    projection: &Projection,
) -> Result<(), rusqlite::Error> {
    connection.execute_batch(
        "DELETE FROM room_publications; DELETE FROM group_anchors; DELETE FROM group_heads; \
         DELETE FROM proposal_heads; DELETE FROM authorization_heads;",
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
    Ok(())
}

/// Every output control in this write left its task head carrying exactly the
/// admitted preparation at the controlling fact.
pub(crate) fn settle_controls<'a>(
    connection: &Connection,
    facts: impl IntoIterator<Item = (Seq, &'a AnyFact)>,
) -> Result<(), Refusal> {
    for (seq, fact) in facts {
        let Some(control) = fact.read::<wire::rooms::OutputControlled>() else {
            continue;
        };
        let body = connection
            .query_row(
                "SELECT body FROM task_heads WHERE stamp = ?1",
                [control.output.as_str()],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        let head = body
            .map(|body| wire::decode::<wire::TaskHead>(&body))
            .transpose()?;
        if !head.is_some_and(|head| {
            head.revision == seq && head.preparation.as_ref() == Some(&control.preparation)
        }) {
            return unsound(format!(
                "output {} control at {seq} has no task head carrying its preparation",
                control.output
            ));
        }
    }
    Ok(())
}

fn sql(seq: u64) -> i64 {
    i64::try_from(seq).expect("journal positions fit SQLite")
}

fn unsql(seq: i64) -> u64 {
    u64::try_from(seq).unwrap_or_default()
}
