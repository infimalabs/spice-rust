//! Retained working-group markers from the former authorization model.
//!
//! Release `4b805a018` owns this unsupported shape. Writable open refuses a
//! journal that retains one before runtime can treat it as authority, and
//! current statement writes cannot add or preserve one.

use rusqlite::{Connection, OptionalExtension};
use wire::TaskHead;

use super::{Refusal, unsound};

/// Refuse a task statement carrying authority owned by the retired model.
pub(crate) fn require_transition(
    connection: &Connection,
    stamp: &str,
    body: &[u8],
) -> Result<(), Refusal> {
    let head = wire::decode::<TaskHead>(body)?;
    let prior_retained = connection
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = ?1",
            [stamp],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?
        .map(|body| wire::decode::<TaskHead>(&body))
        .transpose()?
        .is_some_and(|prior| prior.engagement.is_some());
    if prior_retained || head.engagement.is_some() {
        return unsound(format!(
            "task {stamp} carries a retained group marker owned by release 4b805a018"
        ));
    }
    Ok(())
}
