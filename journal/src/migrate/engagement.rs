//! Retained Designs a working group anchored before engagement authorized
//! their outputs.
//!
//! A predecessor's operator working-group save is the activation the current
//! contract reads as engagement, but that release recorded no grant on the
//! Design's controlled outputs. Each such output therefore crosses carrying the
//! anchoring group's current head as its engagement, exactly as a current save
//! would have granted it. A group head that is no operator `group::stored`
//! anchoring the controller has no current reading and refuses the crossing.

use rusqlite::{Connection, OptionalExtension, params};

use super::{MigrationError, OpenError};

/// Last release whose build anchored a Design in a working group without
/// authorizing its controlled outputs.
const PRE_ENGAGEMENT_RELEASE: &str = "298de2d58";

/// Engage every unauthorized controlled output whose controller a retained
/// working group anchors, through that group's head.
pub(super) fn cross(connection: &Connection) -> Result<(), OpenError> {
    let rows = {
        let mut statement = connection
            .prepare(
                "SELECT task.stamp, task.body, grouped.head_seq FROM task_heads AS task \
                 JOIN group_anchors AS anchor ON anchor.anchor = \
                     json_extract(CAST(task.body AS TEXT), '$.preparation.controller') \
                 JOIN group_heads AS grouped ON grouped.working_group = anchor.working_group \
                 WHERE json_type(CAST(task.body AS TEXT), '$.authorization') IS NULL \
                   AND json_type(CAST(task.body AS TEXT), '$.engagement') IS NULL \
                 ORDER BY task.stamp",
            )
            .map_err(OpenError::Sqlite)?;
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (stamp, body, group_seq) in rows {
        let mut head = wire::decode::<wire::TaskHead>(&body)
            .map_err(|error| retained(&stamp, format!("the statement does not decode: {error}")))?;
        let Some(controller) = crate::rooms::gated_controller(&head).cloned() else {
            continue;
        };
        let grant = u64::try_from(group_seq).ok().and_then(wire::Seq::new);
        let envelope = connection
            .query_row(
                "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
                [group_seq],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(OpenError::Sqlite)?
            .and_then(|bytes| wire::decode::<wire::Envelope<wire::AnyFact>>(&bytes).ok());
        let engaging = envelope.is_some_and(|envelope| {
            crate::rooms::engages(&envelope.actor, &envelope.fact, &controller)
        });
        let (Some(grant), true) = (grant, engaging) else {
            return Err(retained(
                &stamp,
                format!(
                    "its controller {controller} is anchored by working-group head {group_seq}, \
                     which is no operator working-group save anchoring it"
                ),
            ));
        };
        head.engagement = Some(grant);
        let bytes = wire::encode(&head).map_err(|error| {
            retained(
                &stamp,
                format!("the engaged statement cannot encode: {error}"),
            )
        })?;
        let changed = connection
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE stamp = ?2 AND body = ?3",
                params![bytes, stamp, body],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(retained(
                &stamp,
                "the statement changed while its exclusive migration held the authority".to_owned(),
            ));
        }
    }
    Ok(())
}

fn retained(stamp: &str, detail: String) -> OpenError {
    MigrationError::RetainedRow {
        table: "task_heads",
        row: stamp.to_owned(),
        release: PRE_ENGAGEMENT_RELEASE,
        detail,
    }
    .into()
}
