use rusqlite::{OptionalExtension, params};

use super::{MigrationError, OpenError};

const PREDECESSOR_RELEASE: &str = "schema-76";

/// Cross retained opportunistic-Design state into the singleton generation.
pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let mut statement = transaction
        .prepare("SELECT stamp, body FROM task_heads ORDER BY stamp")
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })
        .map_err(OpenError::Sqlite)?;
    let mut current = Vec::new();
    for row in rows {
        let (stamp, body) = row.map_err(OpenError::Sqlite)?;
        let task = wire::decode::<wire::TaskHead>(&body)
            .map_err(|error| retained("task_heads", stamp.clone(), error.to_string()))?;
        if task.intent != Some(wire::TaskIntent::OpportunisticDesign) {
            continue;
        }
        let workflow = transaction
            .query_row(
                "SELECT body FROM workflow_heads WHERE stamp = ?1",
                [&stamp],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(OpenError::Sqlite)?
            .ok_or_else(|| {
                retained(
                    "workflow_heads",
                    stamp.clone(),
                    "opportunistic Design has no workflow head".to_owned(),
                )
            })?;
        let workflow = wire::decode::<wire::WorkflowHead>(&workflow)
            .map_err(|error| retained("workflow_heads", stamp.clone(), error.to_string()))?;
        if workflow.status.is_pending() {
            current.push(task);
        }
    }
    drop(statement);
    match current.as_slice() {
        [] => Ok(()),
        [design] => append_head(transaction, design),
        designs => Err(retained(
            "task_heads",
            designs
                .iter()
                .map(|design| design.born.to_string())
                .collect::<Vec<_>>()
                .join(","),
            "multiple retained open opportunistic Designs cannot name one generation".to_owned(),
        )),
    }
}

fn append_head(
    transaction: &rusqlite::Transaction<'_>,
    design: &wire::TaskHead,
) -> Result<(), OpenError> {
    let cause = design.queue_origin;
    let (instant, origin, root, depth) = transaction
        .query_row(
            "SELECT f.instant, f.origin, t.root, t.depth
             FROM fact_envelopes f
             JOIN thread_index t ON t.seq = f.seq
             WHERE f.seq = ?1",
            [sql(cause)],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(OpenError::Sqlite)?
        .ok_or_else(|| {
            retained(
                "fact_envelopes",
                cause.to_string(),
                "opportunistic Design birth fact is absent".to_owned(),
            )
        })?;
    let at = instant
        .parse::<wire::Instant>()
        .map_err(|error| retained("fact_envelopes", cause.to_string(), error.to_string()))?;
    let origin = origin
        .parse::<wire::Origin>()
        .map_err(|error| retained("fact_envelopes", cause.to_string(), error.to_string()))?;
    let next = transaction
        .query_row(
            "SELECT coalesce(max(seq), 0) + 1 FROM fact_envelopes",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(OpenError::Sqlite)?;
    let sequence = sequence(next)?;
    let advanced = wire::body::OopsGenerationAdvanced {
        revision: 1,
        from: None,
        to: Some(design.born.clone()),
    };
    let fact: wire::AnyFact = wire::decode(
        &wire::encode(&wire::Fact::new(advanced))
            .map_err(|error| retained("fact_envelopes", next.to_string(), error.to_string()))?,
    )
    .map_err(|error| retained("fact_envelopes", next.to_string(), error.to_string()))?;
    let envelope = wire::Envelope {
        v: wire::WireVersion::CURRENT,
        seq: sequence,
        at,
        actor: wire::Actor::Spice,
        origin: origin.clone(),
        cause: Some(cause),
        fact,
    };
    let envelope = wire::encode(&envelope)
        .map_err(|error| retained("fact_envelopes", next.to_string(), error.to_string()))?;
    transaction
        .execute(
            "INSERT INTO fact_envelopes(
                seq, wire_version, instant, actor, origin, cause, kind, envelope
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'oops::generation', ?7)",
            params![
                next,
                wire::WireVersion::CURRENT.0,
                at.to_string(),
                wire::Actor::Spice.to_string(),
                origin.as_str(),
                sql(cause),
                envelope,
            ],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![next, origin.as_str(), sql(cause), root, depth + 1],
        )
        .map_err(OpenError::Sqlite)?;
    let head = wire::OopsGenerationHead {
        revision: 1,
        design: Some(design.born.clone()),
    };
    let head = wire::encode(&head)
        .map_err(|error| retained("oops_generation_head", "1".to_owned(), error.to_string()))?;
    transaction
        .execute(
            "INSERT INTO oops_generation_head(singleton, head_seq, body) VALUES (1, ?1, ?2)",
            params![next, head],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "UPDATE cursor_head SET seq = ?1 WHERE singleton = 1",
            [next],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "UPDATE steering_index SET indexed_seq = ?1 WHERE singleton = 1",
            [next],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

fn sequence(value: i64) -> Result<wire::Seq, OpenError> {
    wire::Seq::new(
        u64::try_from(value)
            .map_err(|error| retained("fact_envelopes", value.to_string(), error.to_string()))?,
    )
    .ok_or_else(|| {
        retained(
            "fact_envelopes",
            value.to_string(),
            "zero sequence".to_owned(),
        )
    })
}

fn sql(sequence: wire::Seq) -> i64 {
    i64::try_from(sequence.get()).expect("a durable sequence fits SQLite")
}

fn retained(table: &'static str, row: String, detail: String) -> OpenError {
    MigrationError::RetainedRow {
        table,
        row,
        release: PREDECESSOR_RELEASE,
        detail,
    }
    .into()
}
