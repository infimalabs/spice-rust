//! Resident-owned claim-to-terminal effort reads.
//! One replayable fold answers the CLI and the Canvas from the same bytes.

use bus::Bus;
use fold::{Store, TaskEffortReducer, select_task_effort, task_effort_answer};
use wire::intent::{CommandAnswer, EffortRead};
use wire::{Outcome, Reply, ReplyPosition};

use super::{RuntimePaths, typed_protocol_fault, typed_store_fault};

/// Answer one effort read from the durable fold rather than a live recount.
///
/// The reduction is the same one `spice journal task-effort` prints, so a
/// Canvas row and a terminal row that disagree are a fold defect rather than
/// two independent computations drifting apart. Nothing here recomputes usage:
/// the resident materializes, selects, and hands the typed answer across.
pub(super) fn read(bus: &Bus, paths: &RuntimePaths, read: &EffortRead) -> Outcome<CommandAnswer> {
    let snapshot = match bus.fact_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return typed_store_fault("effort::facts-unreadable", &error.to_string()),
    };
    let Some(horizon) = snapshot.horizon() else {
        return typed_store_fault(
            "effort::cursor-missing",
            "a running resident has no durable fact horizon",
        );
    };
    let mut store = match Store::open(&paths.folds) {
        Ok(store) => store,
        Err(error) => return typed_store_fault("effort::store-unavailable", &error.to_string()),
    };
    let materialization = match store.read_or_rebuild_snapshot(&snapshot, &TaskEffortReducer) {
        Ok(outcome) => outcome.materialization,
        Err(error) => return typed_store_fault("effort::fold-failed", &error.to_string()),
    };
    let answer = match task_effort_answer(&materialization) {
        Ok(answer) => answer,
        Err(error) => return typed_protocol_fault("effort::payload-invalid", &error),
    };
    let Some(materialized_cursor) = materialization.cursor.position() else {
        return typed_protocol_fault(
            "effort::cursor-missing",
            "task::effort returned an empty cursor for non-empty authority",
        );
    };
    if materialized_cursor != horizon {
        return typed_protocol_fault(
            "effort::snapshot-mismatch",
            &format!(
                "task::effort materialization is at {materialized_cursor}, authority snapshot is at {horizon}"
            ),
        );
    }
    match select_task_effort(answer, read) {
        Ok(selected) => Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(materialized_cursor),
            body: CommandAnswer::Effort(Box::new(selected)),
        }),
        Err(error) => typed_protocol_fault("effort::selection-failed", &error),
    }
}
