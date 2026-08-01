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
            let Some(occupant) = row.occupant else {
                return steering_refusal(
                    "steer::lane-vacant",
                    Some(lane.to_string()),
                    "start an occupant in the lane before sending it direction",
                );
            };
            let mapped = MappedCommand::new(
                contract,
                &["steer"],
                CommandKind::SteerSend,
                vec![Witness::Residency {
                    lane,
                    occupant,
                    epoch: inbox.epoch,
                }],
                &wire::intent::SteerSend {
                    lane,
                    message: message.to_owned(),
                },
            );
            execute_mapped(discovery_path, contract, Connection::Operator, mapped)
        }
        SelectedCommand::SteerAck { key, disposition } => {
            let Some(lane) = lane_context else {
                return environment_fault(
                    "steer::lane-context-absent",
                    "ack requires the SPICE_LANE identity of an activated lane shell",
                );
            };
            let inbox = match read_inbox(discovery_path, Connection::Lane(lane), None) {
                Ok(inbox) => inbox.answer,
                Err(outcome) => return outcome,
            };
            let directive = inbox
                .lanes
                .iter()
                .flat_map(|row| &row.pending)
                .find(|pending| pending.key == key.as_str())
                .map(|pending| pending.directive);
            let expected = directive
                .map(|directive| Witness::Directive {
                    steer: directive,
                    disposition: Disposition::Live,
                })
                .into_iter()
                .collect();
            let mapped = MappedCommand::new(
                contract,
                &["ack"],
                CommandKind::SteerDispose,
                expected,
                &wire::intent::SteerDispose {
                    token: key.to_string(),
                    disposition,
                },
            );
            execute_mapped(discovery_path, contract, Connection::Lane(lane), mapped)
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
