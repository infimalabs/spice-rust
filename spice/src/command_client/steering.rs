//! Steering-specific client preflight and command mapping.

use super::*;

/// Submit one steering command with the caller's optional activated lane.
pub(crate) fn execute_steering(
    discovery_path: &Path,
    contract: &Contract,
    selected: SelectedCommand<'_>,
    lane_context: Option<CanonicalUuid>,
) -> Outcome<CommandAnswer> {
    match selected {
        SelectedCommand::SteerSend { lane, message } => {
            let inbox = match read_inbox(discovery_path, Connection::Operator, Some(lane)) {
                Ok(inbox) => inbox.answer,
                Err(outcome) => return outcome,
            };
            let Some(row) = inbox.lanes.iter().find(|row| row.lane == lane) else {
                return steering_refusal(
                    "steer::lane-unknown",
                    Some(lane.to_string()),
                    "seat the lane before sending it direction",
                );
            };
            let mapped = MappedCommand::new(
                contract,
                &["steer"],
                CommandKind::SteerSend,
                vec![Witness::Residency {
                    lane,
                    occupant: row.occupant,
                    epoch: inbox.epoch,
                }],
                &wire::intent::SteerSend {
                    fanout: CanonicalUuid::new(uuid::Uuid::new_v4()),
                    lane,
                    message: message.to_owned(),
                    attachments: Vec::new(),
                },
            );
            execute_mapped(discovery_path, contract, Connection::Operator, mapped)
        }
        SelectedCommand::SteerInbox { lane } => {
            let connection = lane_context.map_or(Connection::Operator, Connection::Lane);
            match read_inbox(discovery_path, connection, lane) {
                Ok(inbox) => Outcome::Reply(Reply {
                    position: ReplyPosition::Snapshot(inbox.position),
                    body: CommandAnswer::Inbox(inbox.answer),
                }),
                Err(outcome) => outcome,
            }
        }
        selected => execute(discovery_path, contract, selected),
    }
}

/// Tell another seated lane something, witnessing the seat it lands on.
///
/// The board's trust scope is the residency a lane may read, so the witness
/// comes from the same authority the resident will compare it against.
pub(crate) fn execute_tell(
    repository: &spice::repository::Repository,
    contract: &Contract,
    lane: CanonicalUuid,
    message: &str,
) -> Outcome<CommandAnswer> {
    let board =
        match crate::board_command::current_board(repository, wire::intent::BoardRead::Trust) {
            Ok(board) => board,
            Err(outcome) => return outcome,
        };
    let residency = &board.residency.body;
    let Some(occupant) = residency
        .lanes
        .iter()
        .find(|row| row.lane == lane)
        .and_then(|row| row.occupant)
    else {
        return lane_list_refusal(Some(lane.to_string()));
    };
    execute_typed(
        repository,
        contract,
        CommandKind::SteerTell,
        vec![Witness::Residency {
            lane,
            occupant: Some(occupant),
            epoch: residency.epoch,
        }],
        &wire::intent::SteerTell {
            lane,
            message: message.to_owned(),
        },
    )
}
