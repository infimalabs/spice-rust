//! The one write that changes which model a seated lane's next launch runs.
//!
//! A seat carries a model spelling from the moment it is added, and every
//! launch reads that spelling rather than re-deriving one, so a seat added
//! with a model keeps it across relaunch, recovery, and succession. That is
//! what makes the seat the only place a model can be changed for good: the
//! vendor configuration is consulted only when the seat names nothing.
//!
//! [`retune`] therefore states the seat outright. A named spelling pins it; an
//! absent one returns the seat to the vendor's configured model. Neither form
//! touches the occupant: a process already running keeps the model it was
//! launched with, because the model reaches a vendor as argv at spawn and no
//! later. The change is read back on the seat until the lane relaunches.

use super::*;

/// Commit one seated lane's next-launch model.
pub(in crate::resident) fn retune(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    command: &wire::AdmittedCommand,
    request: &LaneRetune,
) -> Outcome<CommandAnswer> {
    if model_malformed(request.model.as_ref()) {
        return lane_refusal(
            "lane::model-invalid",
            Some("one nonblank model spelling without surrounding whitespace".to_owned()),
            request.model.clone(),
            "trim the model spelling or clear it to follow the vendor configuration",
        );
    }
    let observed = match current_residency(bus, epoch) {
        Ok(head) => head,
        Err(outcome) => return typed_outcome(outcome),
    };
    let topology_revision = match next_topology_revision(observed.body(), request.topology_revision)
    {
        Ok(revision) => revision,
        Err(outcome) => return outcome,
    };
    let mut next = observed.body().clone();
    next.topology_revision = topology_revision;
    let Some(row) = next.lanes.iter_mut().find(|row| row.lane == request.lane) else {
        return missing_lane(request.lane);
    };
    let from = std::mem::replace(&mut row.model, request.model.clone());
    let answer = match answer(&next, None, seats, paths) {
        Ok(answer) => answer,
        Err(outcome) => return typed_outcome(outcome),
    };

    let mut write = match write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    ) {
        Ok(write) => write,
        Err(outcome) => return typed_outcome(outcome),
    };
    guard_residency(&mut write, &observed);
    let fact = match write.fact(
        wire::body::LaneRetuned {
            lane: request.lane,
            from,
            model: request.model.clone(),
        },
        FactCause::Existing(observed.position()),
    ) {
        Ok(fact) => fact,
        Err(error) => return typed_store_fault("lane::fact-invalid", &error.to_string()),
    };
    write.residency_head(fact, move |_| next);
    if let Err(error) = write.command_result(
        Command {
            v: wire::WireVersion::CURRENT,
            id: command.id(),
            epoch: command.epoch(),
            kind: command.kind(),
            expected: command.expected().to_vec(),
            body: request.clone(),
        },
        epoch,
        answer.clone(),
    ) {
        return typed_store_fault("lane::receipt-invalid", &error.to_string());
    }
    match commit(bus, write) {
        Ok(committed) => Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: seq(committed.first()),
                last: seq(committed.last()),
            }),
            body: CommandAnswer::Lane(Box::new(answer)),
        }),
        Err(outcome) => typed_outcome(outcome),
    }
}
