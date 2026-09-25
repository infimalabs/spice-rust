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
        SelectedCommand::SteerAck { key, disposition } => {
            let Some(lane) = lane_context else {
                return environment_fault(
                    "steer::lane-context-absent",
                    &format!(
                        "ack requires the {} identity of an activated Rust lane shell",
                        steer::hook::LANE
                    ),
                );
            };
            let inbox = match read_inbox(discovery_path, Connection::Lane(lane), None) {
                Ok(inbox) => inbox.answer,
                Err(outcome) => return outcome,
            };
            let directive = acknowledgment_directive(&inbox, key);
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

pub(super) fn acknowledgment_directive(
    inbox: &InboxAnswer,
    key: &steer::Key,
) -> Option<CanonicalUuid> {
    inbox.lanes.iter().find_map(|row| {
        row.pending
            .iter()
            .find(|pending| pending.key == key.as_str())
            .map(|pending| pending.directive)
    })
}
