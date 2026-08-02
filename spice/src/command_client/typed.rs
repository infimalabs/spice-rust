//! Already-typed board and task family command mapping.
//!
//! This seam keeps family mapping growth out of the transport client while
//! preserving its one authenticated request path.

use spice::repository::Repository;

use super::*;

/// Submit one already-typed family request through the sole resident client.
pub(crate) fn execute_typed<B: Serialize>(
    repository: &Repository,
    contract: &Contract,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: &B,
) -> Outcome<CommandAnswer> {
    let Some(mapped_kind) = kind_for(contract.path) else {
        return protocol_fault(
            "command::mapping-unsupported",
            "selected CLI contract has no resident command mapping",
        );
    };
    if mapped_kind != kind {
        return protocol_fault(
            "command::mapping-mismatch",
            "selected CLI contract does not own the requested wire kind",
        );
    }
    let mapped = match MappedCommand::new(contract, contract.path, kind, expected, body) {
        Ok(mapped) => mapped,
        Err(outcome) => return outcome,
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => return crate::state_failure(&error).outcome(),
    };
    let connection = match ambient_lane_identity() {
        Ok(Some(lane)) => Connection::Lane(lane),
        Ok(None) => Connection::Operator,
        Err(outcome) => return outcome,
    };
    let discovery_path = selected_discovery_path(state.canvas(), connection);
    let discovery = match DiscoveryRead::open(&discovery_path) {
        Ok(discovery) => discovery,
        Err(outcome) => return outcome,
    };
    request(contract, mapped, discovery)
}

pub(super) fn kind_for(path: &[&str]) -> Option<CommandKind> {
    Some(match path {
        ["board"] => CommandKind::BoardRead,
        ["board", "graph"] => CommandKind::BoardGraph,
        ["board", "handout"] => CommandKind::BoardHandout,
        ["task", "next"] => CommandKind::ClaimWin,
        ["task", "capture"] => CommandKind::ClaimWin,
        ["task", "add"] => CommandKind::TaskAdd,
        ["task", "oops"] => CommandKind::TaskSystemAdd,
        ["taste", "add"] | ["taste", "drop"] => CommandKind::TaskAdd,
        ["task", "done"] => CommandKind::TaskDone,
        ["task", "review"] => CommandKind::TaskReview,
        ["task", "note"] => CommandKind::TaskNote,
        ["task", "defer"] => CommandKind::TaskDefer,
        ["task", "wake"] => CommandKind::TaskWake,
        ["task", "free"] => CommandKind::ClaimFree,
        ["task", "depends"] => CommandKind::TaskDepends,
        ["doc"] => CommandKind::DocApply,
        ["doc", "ledger"] => CommandKind::DocLedger,
        _ => return None,
    })
}
