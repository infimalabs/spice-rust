//! Durable release-note authority derived from task lifecycle facts.

use std::collections::BTreeMap;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::guard::{GuardError, JournalRead};
use crate::migrate::{MigrationError, OpenError};

/// Release that last wrote task Git boundaries without this projection.
pub const PRE_PROJECTION_RELEASE: &str = "db7c4cb9c";

pub(crate) const SCHEMA_SQL: &str = r#"
CREATE TABLE release_landings (
    seq INTEGER PRIMARY KEY REFERENCES fact_envelopes(seq) ON DELETE RESTRICT,
    commit_id TEXT NOT NULL,
    task TEXT NOT NULL REFERENCES task_heads(stamp) ON DELETE RESTRICT,
    project TEXT NOT NULL,
    title TEXT NOT NULL,
    contributes INTEGER NOT NULL CHECK (contributes IN (0, 1)),
    verdict TEXT CHECK (verdict IN ('clean', 'finding'))
) STRICT;
CREATE UNIQUE INDEX release_landings_contribution
    ON release_landings(commit_id) WHERE contributes = 1;
CREATE INDEX release_landings_commit_seq ON release_landings(commit_id, seq);
CREATE INDEX release_landings_task_seq ON release_landings(task, seq);
CREATE TABLE release_corrections (
    correction TEXT PRIMARY KEY REFERENCES task_heads(stamp) ON DELETE RESTRICT,
    parent TEXT NOT NULL REFERENCES task_heads(stamp) ON DELETE RESTRICT,
    head_seq INTEGER NOT NULL REFERENCES fact_envelopes(seq) ON DELETE RESTRICT,
    CHECK (correction <> parent)
) STRICT;
"#;

/// One task lifecycle boundary tied to an integrated repository commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseLanding {
    pub seq: wire::Seq,
    pub commit: String,
    pub task: wire::TaskStamp,
    pub project: String,
    pub title: String,
    pub contributes: bool,
    pub verdict: Option<wire::body::Verdict>,
}

/// One review finding's typed correction relation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseCorrection {
    pub correction: wire::TaskStamp,
    pub parent: wire::TaskStamp,
    pub head: wire::Seq,
}

/// Complete current projection used by release-note folding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseNoteAuthority {
    pub landings: Vec<ReleaseLanding>,
    pub corrections: Vec<ReleaseCorrection>,
}

pub(crate) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    let authority = derive(transaction)?;
    transaction
        .execute_batch(SCHEMA_SQL)
        .map_err(OpenError::Sqlite)?;
    for landing in authority.landings {
        insert_landing(transaction, &landing).map_err(OpenError::Sqlite)?;
    }
    for correction in authority.corrections {
        insert_correction(transaction, &correction).map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

/// Keep the projection current inside the fact transaction.
pub(crate) fn apply<'a>(
    transaction: &Transaction<'_>,
    facts: impl IntoIterator<Item = &'a wire::AnyFact>,
    assigned: &[wire::Seq],
) -> Result<(), ProjectionWriteError> {
    for (fact, seq) in facts.into_iter().zip(assigned.iter().copied()) {
        if let Some(reviewed) = fact.read::<wire::body::TaskReviewed>() {
            record_corrections(transaction, seq, &reviewed)?;
        }
        if let Some((task, git, verdict)) = terminal(fact) {
            let Some(git) = git else { continue };
            let (project, title) = task_metadata(transaction, &task)
                .map_err(ProjectionWriteError::Sqlite)?
                .ok_or_else(|| {
                    ProjectionWriteError::Refused(format!(
                        "release landing fact {seq} names task {task} without statement authority"
                    ))
                })?;
            let landing = ReleaseLanding {
                seq,
                commit: git.integrated,
                task,
                project,
                title,
                contributes: git.local_commits > 0,
                verdict,
            };
            ensure_contribution_available(transaction, &landing)?;
            insert_landing(transaction, &landing).map_err(ProjectionWriteError::Sqlite)?;
        }
        if let Some(task) = statement_task(fact) {
            refresh_metadata(transaction, &task)?;
        }
    }
    Ok(())
}

impl JournalRead<'_> {
    /// Read the typed landing and correction authority for release-note folding.
    pub fn release_note_authority(&self) -> Result<ReleaseNoteAuthority, GuardError> {
        stored(self.connection()).map_err(|error| match error {
            ProjectionReadError::Sqlite(error) => GuardError::Store(error),
            ProjectionReadError::Invalid(detail) => GuardError::Invalid(detail),
        })
    }
}

#[derive(Debug)]
pub(crate) enum ProjectionWriteError {
    Sqlite(rusqlite::Error),
    Refused(String),
}

fn derive(connection: &Connection) -> Result<ReleaseNoteAuthority, OpenError> {
    let mut statement = connection
        .prepare(
            "SELECT seq, kind, envelope FROM fact_envelopes \
             WHERE kind IN ('task::done', 'task::reviewed') ORDER BY seq",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(OpenError::Sqlite)?;
    let mut landings = Vec::new();
    let mut corrections = BTreeMap::<wire::TaskStamp, ReleaseCorrection>::new();
    let mut contributions = BTreeMap::<String, wire::Seq>::new();
    for row in rows {
        let (raw_seq, kind, bytes) = row.map_err(OpenError::Sqlite)?;
        let envelope = crate::read::decode_fact(raw_seq, &kind, &bytes).map_err(|error| {
            retained(
                raw_seq.to_string(),
                format!("terminal fact does not decode: {error}"),
            )
        })?;
        if let Some(reviewed) = envelope.fact.read::<wire::body::TaskReviewed>() {
            derive_corrections(envelope.seq, &reviewed, &mut corrections)?;
        }
        let Some((task, git, verdict)) = terminal(&envelope.fact) else {
            continue;
        };
        let Some(git) = git else { continue };
        let (project, title) = task_metadata(connection, &task)
            .map_err(OpenError::Sqlite)?
            .ok_or_else(|| {
                retained(
                    raw_seq.to_string(),
                    format!("terminal fact names task {task} without statement authority"),
                )
            })?;
        if git.local_commits > 0
            && let Some(prior) = contributions.insert(git.integrated.clone(), envelope.seq)
        {
            return Err(retained(
                raw_seq.to_string(),
                format!(
                    "integrated commit {} is attributed by both landing facts {prior} and {}",
                    git.integrated, envelope.seq
                ),
            ));
        }
        landings.push(ReleaseLanding {
            seq: envelope.seq,
            commit: git.integrated,
            task,
            project,
            title,
            contributes: git.local_commits > 0,
            verdict,
        });
    }
    Ok(ReleaseNoteAuthority {
        landings,
        corrections: corrections.into_values().collect(),
    })
}

fn derive_corrections(
    seq: wire::Seq,
    reviewed: &wire::body::TaskReviewed,
    corrections: &mut BTreeMap<wire::TaskStamp, ReleaseCorrection>,
) -> Result<(), OpenError> {
    for correction in &reviewed.followups {
        if correction == &reviewed.task {
            return Err(retained(
                seq.to_string(),
                format!("task {} names itself as a review correction", reviewed.task),
            ));
        }
        if let Some(prior) = corrections.get(correction)
            && prior.parent != reviewed.task
        {
            return Err(retained(
                seq.to_string(),
                format!(
                    "correction task {correction} is attributed to both {} and {}",
                    prior.parent, reviewed.task
                ),
            ));
        }
        corrections.insert(
            correction.clone(),
            ReleaseCorrection {
                correction: correction.clone(),
                parent: reviewed.task.clone(),
                head: seq,
            },
        );
    }
    Ok(())
}

fn record_corrections(
    transaction: &Transaction<'_>,
    seq: wire::Seq,
    reviewed: &wire::body::TaskReviewed,
) -> Result<(), ProjectionWriteError> {
    for correction in &reviewed.followups {
        if correction == &reviewed.task {
            return Err(ProjectionWriteError::Refused(format!(
                "task {} cannot name itself as a release-note correction",
                reviewed.task
            )));
        }
        let prior = transaction
            .query_row(
                "SELECT parent FROM release_corrections WHERE correction = ?1",
                [correction.as_str()],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(ProjectionWriteError::Sqlite)?;
        if let Some(prior) = prior
            && prior != reviewed.task.as_str()
        {
            return Err(ProjectionWriteError::Refused(format!(
                "correction task {correction} already belongs to release-note parent {prior}"
            )));
        }
        let row = ReleaseCorrection {
            correction: correction.clone(),
            parent: reviewed.task.clone(),
            head: seq,
        };
        transaction
            .execute(
                "INSERT INTO release_corrections(correction, parent, head_seq) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(correction) DO UPDATE SET head_seq = excluded.head_seq",
                params![
                    row.correction.as_str(),
                    row.parent.as_str(),
                    sql_seq(row.head)
                ],
            )
            .map_err(ProjectionWriteError::Sqlite)?;
    }
    Ok(())
}

fn terminal(
    fact: &wire::AnyFact,
) -> Option<(
    wire::TaskStamp,
    Option<wire::body::GitBoundary>,
    Option<wire::body::Verdict>,
)> {
    if let Some(done) = fact.read::<wire::body::TaskDone>() {
        return Some((done.task, done.git, None));
    }
    fact.read::<wire::body::TaskReviewed>()
        .map(|reviewed| (reviewed.task, reviewed.git, Some(reviewed.verdict)))
}

fn statement_task(fact: &wire::AnyFact) -> Option<wire::TaskStamp> {
    fact.read::<wire::body::TaskEdited>()
        .map(|body| body.task)
        .or_else(|| fact.read::<wire::body::TaskFiled>().map(|body| body.task))
}

fn task_metadata(
    connection: &Connection,
    task: &wire::TaskStamp,
) -> Result<Option<(String, String)>, rusqlite::Error> {
    connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = ?1",
            [task.as_str()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .map(|bytes| {
            wire::decode::<wire::TaskHead>(&bytes)
                .map(|head| (head.project, head.title))
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        bytes.len(),
                        rusqlite::types::Type::Blob,
                        Box::new(error),
                    )
                })
        })
        .transpose()
}

fn refresh_metadata(
    transaction: &Transaction<'_>,
    task: &wire::TaskStamp,
) -> Result<(), ProjectionWriteError> {
    let Some((project, title)) =
        task_metadata(transaction, task).map_err(ProjectionWriteError::Sqlite)?
    else {
        return Err(ProjectionWriteError::Refused(format!(
            "task {task} changed without statement authority"
        )));
    };
    transaction
        .execute(
            "UPDATE release_landings SET project = ?1, title = ?2 WHERE task = ?3",
            params![project, title, task.as_str()],
        )
        .map_err(ProjectionWriteError::Sqlite)?;
    Ok(())
}

fn ensure_contribution_available(
    transaction: &Transaction<'_>,
    landing: &ReleaseLanding,
) -> Result<(), ProjectionWriteError> {
    if !landing.contributes {
        return Ok(());
    }
    let prior = transaction
        .query_row(
            "SELECT seq FROM release_landings WHERE commit_id = ?1 AND contributes = 1",
            [&landing.commit],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(ProjectionWriteError::Sqlite)?;
    if let Some(prior) = prior {
        return Err(ProjectionWriteError::Refused(format!(
            "integrated commit {} already belongs to release landing fact {prior}",
            landing.commit
        )));
    }
    Ok(())
}

fn insert_landing(
    connection: &Connection,
    landing: &ReleaseLanding,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO release_landings(seq, commit_id, task, project, title, contributes, verdict) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            sql_seq(landing.seq),
            landing.commit,
            landing.task.as_str(),
            landing.project,
            landing.title,
            landing.contributes,
            landing.verdict.map(verdict_name),
        ],
    )?;
    Ok(())
}

fn insert_correction(
    connection: &Connection,
    correction: &ReleaseCorrection,
) -> Result<(), rusqlite::Error> {
    connection.execute(
        "INSERT INTO release_corrections(correction, parent, head_seq) VALUES (?1, ?2, ?3)",
        params![
            correction.correction.as_str(),
            correction.parent.as_str(),
            sql_seq(correction.head),
        ],
    )?;
    Ok(())
}

fn stored(connection: &Connection) -> Result<ReleaseNoteAuthority, ProjectionReadError> {
    let mut landing_statement = connection
        .prepare(
            "SELECT seq, commit_id, task, project, title, contributes, verdict \
             FROM release_landings ORDER BY seq",
        )
        .map_err(ProjectionReadError::Sqlite)?;
    let landings = landing_statement
        .query_map([], |row| {
            let raw_seq = row.get::<_, i64>(0)?;
            let raw_task = row.get::<_, String>(2)?;
            let verdict = row.get::<_, Option<String>>(6)?;
            Ok((
                raw_seq,
                row.get::<_, String>(1)?,
                raw_task,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, bool>(5)?,
                verdict,
            ))
        })
        .map_err(ProjectionReadError::Sqlite)?
        .map(|row| {
            let (raw_seq, commit, raw_task, project, title, contributes, verdict) =
                row.map_err(ProjectionReadError::Sqlite)?;
            Ok(ReleaseLanding {
                seq: parse_seq(raw_seq)?,
                commit,
                task: raw_task.parse().map_err(|error| {
                    ProjectionReadError::Invalid(format!(
                        "release landing task {raw_task:?} is invalid: {error}"
                    ))
                })?,
                project,
                title,
                contributes,
                verdict: verdict.map(parse_verdict).transpose()?,
            })
        })
        .collect::<Result<Vec<_>, ProjectionReadError>>()?;

    let mut correction_statement = connection
        .prepare(
            "SELECT correction, parent, head_seq FROM release_corrections ORDER BY head_seq, correction",
        )
        .map_err(ProjectionReadError::Sqlite)?;
    let corrections = correction_statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(ProjectionReadError::Sqlite)?
        .map(|row| {
            let (correction, parent, head) = row.map_err(ProjectionReadError::Sqlite)?;
            Ok(ReleaseCorrection {
                correction: correction.parse().map_err(|error| {
                    ProjectionReadError::Invalid(format!(
                        "release correction task {correction:?} is invalid: {error}"
                    ))
                })?,
                parent: parent.parse().map_err(|error| {
                    ProjectionReadError::Invalid(format!(
                        "release correction parent {parent:?} is invalid: {error}"
                    ))
                })?,
                head: parse_seq(head)?,
            })
        })
        .collect::<Result<Vec<_>, ProjectionReadError>>()?;
    Ok(ReleaseNoteAuthority {
        landings,
        corrections,
    })
}

#[derive(Debug)]
enum ProjectionReadError {
    Sqlite(rusqlite::Error),
    Invalid(String),
}

fn parse_seq(raw: i64) -> Result<wire::Seq, ProjectionReadError> {
    u64::try_from(raw)
        .ok()
        .and_then(wire::Seq::new)
        .ok_or_else(|| {
            ProjectionReadError::Invalid(format!("release projection position {raw} is invalid"))
        })
}

fn parse_verdict(raw: String) -> Result<wire::body::Verdict, ProjectionReadError> {
    match raw.as_str() {
        "clean" => Ok(wire::body::Verdict::Clean),
        "finding" => Ok(wire::body::Verdict::Finding),
        _ => Err(ProjectionReadError::Invalid(format!(
            "release landing verdict {raw:?} is invalid"
        ))),
    }
}

const fn verdict_name(verdict: wire::body::Verdict) -> &'static str {
    match verdict {
        wire::body::Verdict::Clean => "clean",
        wire::body::Verdict::Finding => "finding",
    }
}

fn sql_seq(seq: wire::Seq) -> i64 {
    i64::try_from(seq.get()).expect("journal positions fit SQLite integers")
}

fn retained(row: String, detail: String) -> OpenError {
    MigrationError::RetainedRow {
        table: "fact_envelopes",
        row,
        release: PRE_PROJECTION_RELEASE,
        detail,
    }
    .into()
}
