//! A lane's leave from one room's derived background.

use super::*;

use spice::rooms::AttentionReason;
use wire::intent::{RoomLeave, RoomLeaveAnswer, RoomListeningDuty};
use wire::rooms::RoomLeft;

use super::room::mint_publication;

/// Record that the commanding lane leaves `leave.room`, once per leave in
/// force, and name every native duty that still keeps it listening. Leaving
/// frees no claim, settles no delivery, and answers no question.
pub(crate) fn room_leave(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    leave: &RoomLeave,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let Actor::Lane(lane) = command.actor().clone() else {
        return refusal(
            "room::leave-lane-only",
            None,
            "only a lane listens to a room; the operator has nothing to leave",
        );
    };
    let clause = leave.clause.trim();
    if clause.is_empty() {
        return refusal(
            "room::leave-clause-empty",
            Some(leave.room.to_string()),
            "say why the lane leaves with --clause TEXT",
        );
    }
    let snapshot = match validated_working_snapshot(
        bus,
        command.actor().clone(),
        std::slice::from_ref(&leave.room),
        projects,
        allocation,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let Some(anchor) = task_entry(&snapshot.entries, &leave.room) else {
        return refusal(
            "room::anchor-unknown",
            Some(leave.room.to_string()),
            "read the board and name one current task handle",
        );
    };
    let room = anchor.born().clone();
    let now = jiff::Timestamp::now();
    let at = match Instant::new(now) {
        Ok(at) => at,
        Err(error) => return environment_fault("room::clock-invalid", &error.to_string()),
    };
    let probe = (room.clone(), Actor::Lane(lane));
    let latest = match bus.read(move |read| read.latest_room_publication(&probe.0, &probe.1)) {
        Ok(latest) => latest,
        Err(error) => return store_fault("room::publications-unreadable", &error.to_string()),
    };
    let attention = match project_attention(
        bus,
        &snapshot.entries,
        &snapshot.answer.residency.body,
        &room,
        latest.as_slice(),
    ) {
        Ok(attention) => attention,
        Err(outcome) => return outcome,
    };
    let row = attention.lanes.iter().find(|row| row.lane == lane);
    let kept = row
        .map(|row| kept_duties(&row.attention, &snapshot.entries))
        .unwrap_or_default();
    if let Some(publication) = row.and_then(leave_in_force) {
        return Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(snapshot.cursor),
            body: CommandAnswer::RoomLeave(RoomLeaveAnswer {
                room: anchor.handle().clone(),
                publication,
                recorded: false,
                kept,
            }),
        });
    }
    let publication = match mint_publication(bus, &room, now.as_microsecond()) {
        Ok(publication) => publication,
        Err(outcome) => return outcome,
    };
    let origin = match task_origin(&room) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let answer = RoomLeaveAnswer {
        room: anchor.handle().clone(),
        publication: publication.clone(),
        recorded: true,
        kept,
    };
    let mut write = JournalWrite::new(Actor::Lane(lane), origin, at, Producer::CommandDoor);
    guard(&mut write, command.expected().to_vec());
    if let Err(error) = write.fact(
        RoomLeft {
            room,
            publication,
            lane,
            clause: clause.to_owned(),
        },
        FactCause::Root,
    ) {
        return store_fault("room::leave-build", &error.to_string());
    }
    if let Err(error) = write.command_result(
        request(command, leave.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("room::leave-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::RoomLeave(answer))
}

/// The lane's leave still in force: one no later Design request has restored
/// listening since.
fn leave_in_force(
    row: &spice::rooms::RoomLaneAttention,
) -> Option<wire::rooms::RoomAuthorityStamp> {
    let restored = row
        .attention
        .iter()
        .any(|reason| matches!(reason, AttentionReason::Asked { .. }));
    row.left.clone().filter(|_| !restored)
}

/// The native duties among `reasons`, each named as the lane acts on it.
fn kept_duties(reasons: &[AttentionReason], entries: &[Entry]) -> Vec<RoomListeningDuty> {
    let mut kept = reasons
        .iter()
        .filter_map(|reason| match reason {
            AttentionReason::Work { task } | AttentionReason::Responsibility { task } => entries
                .iter()
                .find(|entry| entry.born() == task)
                .map(|entry| RoomListeningDuty::Work {
                    task: entry.handle().clone(),
                }),
            AttentionReason::Coordination { key } => {
                Some(RoomListeningDuty::Coordination { key: key.clone() })
            }
            AttentionReason::Stop { key } => Some(RoomListeningDuty::Stop { key: key.clone() }),
            AttentionReason::Asked { .. } => None,
        })
        .collect::<Vec<_>>();
    kept.sort();
    kept.dedup();
    kept
}
