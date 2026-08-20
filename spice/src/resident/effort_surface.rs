//! Resident-owned claim-to-terminal effort reads.
//! One current replayable fold answers the CLI and the Canvas from the same bytes.

use fold::select_task_effort;
use wire::intent::{CommandAnswer, EffortRead};
use wire::{Outcome, Reply, ReplyPosition};

use super::{read_materializations::CurrentReads, typed_protocol_fault, typed_store_fault};

/// Answer one effort read from the resident's current durable-fold generation.
///
/// The reduction is the same one `spice journal task-effort` prints, so a
/// Canvas row and a terminal row that disagree are a fold defect rather than
/// two independent computations drifting apart. The request path only selects
/// from immutable bytes already derived by the background materializer.
pub(super) fn read(reads: &CurrentReads, read: &EffortRead) -> Outcome<CommandAnswer> {
    let generation = match reads.current() {
        Ok(generation) => generation,
        Err(error) => return typed_store_fault("effort::materialization-unavailable", &error),
    };
    match select_task_effort(generation.effort().clone(), read) {
        Ok(selected) => Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(generation.cursor()),
            body: CommandAnswer::Effort(Box::new(selected)),
        }),
        Err(error) => typed_protocol_fault("effort::selection-failed", &error),
    }
}
