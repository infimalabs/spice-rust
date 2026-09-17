//! Durable agent-renewal intent and resident-owned handoff reconciliation.

use super::*;

pub(super) const HANDOFF_CONTROL: &str = "lane-renewal-handoff";
pub(super) const CONTEXT_CONTROL: &str = "lane-renewal-context";

const HANDOFF_TEXT: &str = concat!(
    "Agent renewal requested. Complete or hand off the current phase cleanly, ",
    "post a concise handoff, do not claim additional work, and finish this agent turn."
);

pub(in crate::resident) fn set(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &Seats,
    steering: &steer::Config,
    command: &wire::AdmittedCommand,
    request: &wire::intent::LaneRenewal,
) -> Outcome<CommandAnswer> {
    let (observed, row, current) = match admit_set(bus, epoch, request) {
        Ok(admitted) => admitted,
        Err(outcome) => return outcome,
    };
    let transition = match resolve_set(observed.body(), &row, seats, current, request) {
        Ok(transition) => transition,
        Err(outcome) => return typed_outcome(outcome),
    };
    commit_set(
        bus, paths, epoch, steering, command, request, observed, transition,
    )
}

type AdmittedSet = (
    journal::JournalHead<wire::ResidencyHead>,
    wire::LaneResidency,
    Option<wire::LaneRenewalRecord>,
);

fn admit_set(
    bus: &Bus,
    epoch: CanonicalUuid,
    request: &wire::intent::LaneRenewal,
) -> Result<AdmittedSet, Outcome<CommandAnswer>> {
    let observed = current_residency(bus, epoch).map_err(typed_outcome)?;
    if request.topology_revision != observed.body().topology_revision {
        return Err(lane_refusal(
            "lane::renewal-topology-moved",
            Some(observed.body().topology_revision.to_string()),
            Some(request.topology_revision.to_string()),
            "read lane state and retry against the current topology revision",
        ));
    }
    let Some(row) = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == request.lane)
        .cloned()
    else {
        return Err(missing_lane(request.lane));
    };
    if row.team != request.team {
        return Err(lane_refusal(
            "lane::renewal-team-moved",
            Some(row.team.to_string()),
            Some(request.team.to_string()),
            "read lane state and retry with the member's current team",
        ));
    }
    let current = renewal_for(observed.body(), request.lane)
        .map_err(typed_outcome)?
        .cloned();
    Ok((observed, row, current))
}

struct SetTransition {
    record: Option<wire::LaneRenewalRecord>,
    cancellation: Option<(CanonicalUuid, String)>,
    needs_handoff: bool,
}

fn resolve_set(
    head: &wire::ResidencyHead,
    row: &wire::LaneResidency,
    seats: &Seats,
    current: Option<wire::LaneRenewalRecord>,
    request: &wire::intent::LaneRenewal,
) -> CommandResult<SetTransition> {
    let cancellation = current.as_ref().and_then(|record| {
        matches!(record.state, wire::LaneRenewalState::Requested { .. })
            .then(|| (handoff_directive(record), handoff_control(record)))
    });
    let record = if request.requested {
        resolve_request(head, row, seats, current, request)?
    } else {
        resolve_cancel(current)?
    };
    let needs_handoff = record
        .as_ref()
        .is_some_and(|record| matches!(record.state, wire::LaneRenewalState::Requested { .. }));
    Ok(SetTransition {
        record,
        cancellation,
        needs_handoff,
    })
}

fn resolve_request(
    head: &wire::ResidencyHead,
    row: &wire::LaneResidency,
    seats: &Seats,
    current: Option<wire::LaneRenewalRecord>,
    request: &wire::intent::LaneRenewal,
) -> CommandResult<Option<wire::LaneRenewalRecord>> {
    match current {
        None
        | Some(wire::LaneRenewalRecord {
            state: wire::LaneRenewalState::SuccessorStarted { .. },
            ..
        }) => requested_record(head, row, seats).map(Some),
        Some(wire::LaneRenewalRecord {
            state:
                wire::LaneRenewalState::LaunchRefused {
                    predecessor,
                    team_slot,
                    ..
                },
            ..
        }) => Ok(Some(wire::LaneRenewalRecord {
            team: request.team,
            lane: request.lane,
            revision: request.topology_revision,
            state: wire::LaneRenewalState::HandoffPending {
                predecessor,
                team_slot,
            },
        })),
        Some(record) => Err(unit_outcome(lane_refusal(
            "lane::renewal-already-active",
            Some("no current renewal or one launch refusal".to_owned()),
            Some(state_name(&record.state).to_owned()),
            "wait for the current handoff or repair a refused successor launch",
        ))),
    }
}

fn resolve_cancel(
    current: Option<wire::LaneRenewalRecord>,
) -> CommandResult<Option<wire::LaneRenewalRecord>> {
    match current {
        Some(wire::LaneRenewalRecord {
            state: wire::LaneRenewalState::Requested { .. },
            ..
        }) => Ok(None),
        Some(record) => Err(unit_outcome(lane_refusal(
            "lane::renewal-cancel-late",
            Some("requested".to_owned()),
            Some(state_name(&record.state).to_owned()),
            "renewal can be canceled only before the predecessor reaches handoff",
        ))),
        None => Err(unit_outcome(lane_refusal(
            "lane::renewal-not-requested",
            Some("one requested renewal".to_owned()),
            Some("absent".to_owned()),
            "request renewal before attempting to clear it",
        ))),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one renewal commit carries the complete admitted command boundary"
)]
fn commit_set(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    steering: &steer::Config,
    command: &wire::AdmittedCommand,
    request: &wire::intent::LaneRenewal,
    observed: journal::JournalHead<wire::ResidencyHead>,
    transition: SetTransition,
) -> Outcome<CommandAnswer> {
    let mut next = observed.body().clone();
    replace_renewal(&mut next, request.lane, transition.record.clone());
    let mut proposal = match write(
        command.actor().clone(),
        epoch,
        Producer::ResidentRecognition,
    ) {
        Ok(proposal) => proposal,
        Err(outcome) => return typed_outcome(outcome),
    };
    guard_residency(&mut proposal, &observed);
    let changed = wire::body::LaneRenewalChanged {
        team: request.team,
        lane: request.lane,
        renewal: transition.record.clone(),
    };
    let head = match proposal.fact(changed, FactCause::Existing(observed.position())) {
        Ok(head) => head,
        Err(error) => {
            return typed_store_fault("lane::renewal-fact-invalid", &error.to_string());
        }
    };
    proposal.residency_head(head, move |_| next);
    let answer = wire::intent::LaneRenewalAnswer {
        team: request.team,
        lane: request.lane,
        topology_revision: request.topology_revision,
        renewal: transition.record.clone(),
    };
    if let Err(error) = proposal.command_result(
        Command {
            v: wire::WireVersion::CURRENT,
            id: command.id(),
            epoch: command.epoch(),
            kind: command.kind(),
            expected: command.expected().to_vec(),
            body: *request,
        },
        epoch,
        answer.clone(),
    ) {
        return typed_store_fault("lane::renewal-receipt-invalid", &error.to_string());
    }
    let committed = match commit(bus, proposal) {
        Ok(committed) => committed,
        Err(outcome) => return typed_outcome(outcome),
    };
    finish_set(
        bus,
        paths,
        epoch,
        steering,
        request.lane,
        transition,
        committed,
        answer,
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "the post-commit phase carries the committed renewal boundary"
)]
fn finish_set(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    steering: &steer::Config,
    lane: CanonicalUuid,
    transition: SetTransition,
    committed: bus::JournalCommit,
    answer: wire::intent::LaneRenewalAnswer,
) -> Outcome<CommandAnswer> {
    let reconciled = if transition.needs_handoff {
        ensure_handoff(bus, paths, epoch, steering, lane)
    } else if let Some((directive, control)) = transition.cancellation.as_ref() {
        cancel_handoff(bus, paths, epoch, *directive, lane, control)
    } else {
        Ok(())
    };
    if let Err(outcome) = reconciled {
        return typed_outcome(outcome);
    }
    let residency = match current_residency(bus, epoch) {
        Ok(residency) => residency,
        Err(outcome) => return typed_outcome(outcome),
    };
    if let Err(outcome) = super::super::steering::publish_inbox(bus, paths, residency.body(), lane)
    {
        return typed_outcome(outcome);
    }
    Outcome::Reply(Reply {
        position: ReplyPosition::Committed(SeqRange {
            first: seq(committed.first()),
            last: seq(committed.last()),
        }),
        body: CommandAnswer::LaneRenewal(Box::new(answer)),
    })
}

fn requested_record(
    head: &wire::ResidencyHead,
    row: &wire::LaneResidency,
    seats: &Seats,
) -> CommandResult<wire::LaneRenewalRecord> {
    let occupant = row.occupant.ok_or_else(|| {
        unit_outcome(lane_refusal(
            "lane::renewal-vacant",
            Some(OCCUPIED.to_owned()),
            Some(row.disposition.clone()),
            "wake the lane before requesting agent renewal",
        ))
    })?;
    if !seats
        .occupant(row.lane)
        .is_some_and(|seated| seated.occupant() == occupant)
    {
        return Err(store_fault(
            "lane::renewal-occupant-unowned",
            "the durable renewal predecessor is not supervised by this resident",
        ));
    }
    let session = row.session.as_ref().ok_or_else(|| {
        store_fault(
            "lane::renewal-session-missing",
            "the durable renewal predecessor has no resumable session identity",
        )
    })?;
    if session.occupant != occupant || session.lane != row.lane {
        return Err(store_fault(
            "lane::renewal-session-moved",
            "the durable session does not identify the current lane occupant",
        ));
    }
    let team_slot = head
        .lanes
        .iter()
        .filter(|candidate| candidate.team == row.team)
        .position(|candidate| candidate.lane == row.lane)
        .and_then(|slot| u64::try_from(slot).ok())
        .ok_or_else(|| {
            store_fault(
                "lane::renewal-slot-missing",
                "the current lane has no position inside its durable lineage",
            )
        })?;
    Ok(wire::LaneRenewalRecord {
        team: row.team,
        lane: row.lane,
        revision: head.topology_revision,
        state: wire::LaneRenewalState::Requested {
            predecessor: identity(occupant, session),
            team_slot,
        },
    })
}

fn identity(occupant: CanonicalUuid, session: &wire::LaneSession) -> wire::RenewalIdentity {
    wire::RenewalIdentity {
        occupant,
        session: session.identity.clone(),
        thread: session.identity.clone(),
    }
}

pub(super) fn requested_exit(
    head: &wire::ResidencyHead,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> bool {
    renewal_for(head, lane)
        .ok()
        .flatten()
        .is_some_and(|record| {
            matches!(
                &record.state,
                wire::LaneRenewalState::Requested { predecessor, .. }
                    if predecessor.occupant == occupant
                        || head.lanes.iter().find(|row| row.lane == lane).is_some_and(|row| {
                            row.occupant == Some(occupant)
                                && row.session.as_ref().is_some_and(|session| {
                                    session.occupant == occupant
                                        && session.identity == predecessor.session
                                })
                        })
            )
        })
}

pub(super) fn advance_exit(
    proposal: &mut JournalWrite,
    next: &mut wire::ResidencyHead,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    cause: PendingFact,
) -> CommandResult<PendingFact> {
    let Some(current) = renewal_for(next, lane)?.cloned() else {
        return Ok(cause);
    };
    let wire::LaneRenewalState::Requested {
        predecessor,
        team_slot,
    } = current.state
    else {
        return Ok(cause);
    };
    let resumed_predecessor = next
        .lanes
        .iter()
        .find(|row| row.lane == lane)
        .and_then(|row| row.session.as_ref())
        .is_some_and(|session| {
            session.occupant == occupant && session.identity == predecessor.session
        });
    if predecessor.occupant != occupant && !resumed_predecessor {
        return Ok(cause);
    }
    let renewal = wire::LaneRenewalRecord {
        team: current.team,
        lane,
        revision: next.topology_revision,
        state: wire::LaneRenewalState::HandoffPending {
            predecessor,
            team_slot,
        },
    };
    replace_renewal(next, lane, Some(renewal.clone()));
    proposal
        .fact(
            wire::body::LaneRenewalChanged {
                team: renewal.team,
                lane,
                renewal: Some(renewal),
            },
            FactCause::Pending(cause),
        )
        .map_err(|error| store_fault("lane::renewal-fact-invalid", &error.to_string()))
}

pub(super) fn pending(
    head: &wire::ResidencyHead,
) -> CommandResult<Option<wire::LaneRenewalRecord>> {
    Ok(head
        .renewals
        .iter()
        .find(|record| matches!(record.state, wire::LaneRenewalState::HandoffPending { .. }))
        .cloned())
}

pub(in crate::resident) fn reconcile(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    seats: &mut Seats,
    runtime: &Runtime<'_>,
    config: &super::super::SurfaceConfig,
) -> CommandResult<bool> {
    let observed = current_residency(bus, epoch)?;
    if let Some(requested) = observed
        .body()
        .renewals
        .iter()
        .find(|record| matches!(record.state, wire::LaneRenewalState::Requested { .. }))
    {
        ensure_handoff(bus, paths, epoch, &config.steering, requested.lane)?;
    }
    let Some(renewal) = pending(observed.body())? else {
        return Ok(false);
    };
    let row = observed
        .body()
        .lanes
        .iter()
        .find(|row| row.lane == renewal.lane)
        .ok_or_else(|| {
            store_fault(
                "lane::renewal-member-missing",
                "a handoff-pending renewal names no current lane member",
            )
        })?;
    if row.team != renewal.team {
        return Err(store_fault(
            "lane::renewal-team-moved",
            "a handoff-pending renewal no longer belongs to its recorded team",
        ));
    }
    if row.occupant.is_some() || seats.occupied(row.lane) {
        return Err(store_fault(
            "lane::renewal-capacity-occupied",
            "a handoff-pending renewal still has an occupied lane",
        ));
    }
    cancel_handoff(
        bus,
        paths,
        epoch,
        handoff_directive(&renewal),
        renewal.lane,
        &handoff_control(&renewal),
    )?;
    ensure_context(bus, paths, epoch, &config.steering, &renewal)?;
    super::super::consent_surface::commit_lapse(bus, runtime.reads, renewal.lane)?;
    let occupant = new_uuid();
    match super::automatic::wake_renewal(bus, paths, epoch, seats, runtime, &renewal, occupant)? {
        AutomaticWake::Started => Ok(true),
        AutomaticWake::Unstarted(failure) => {
            mark_launch_refused(bus, epoch, &renewal, &failure)?;
            Ok(true)
        }
    }
}

pub(super) fn ensure_context(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    steering: &steer::Config,
    renewal: &wire::LaneRenewalRecord,
) -> CommandResult<()> {
    let control = context_control(renewal);
    let directive = context_directive(renewal);
    if super::super::steering::control_standing(bus, directive, renewal.lane, &control)?
        == Some(wire::Disposition::Live)
    {
        return Ok(());
    }
    let wire::LaneRenewalState::HandoffPending { predecessor, .. } = &renewal.state else {
        return Err(store_fault(
            "lane::renewal-state-moved",
            "rehydration context requires a handoff-pending renewal",
        ));
    };
    let observed = current_residency(bus, epoch)?;
    if renewal_for(observed.body(), renewal.lane)? != Some(renewal) {
        return Err(Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "lane::renewal-state-moved".to_owned(),
                Some("the selected handoff-pending renewal".to_owned()),
                None,
                None,
            )
            .with_detail("reconcile the current renewal state before launching a successor"),
        )));
    }
    let text = format!(
        "Renewal successor context: previous agent thread {}. Resolve its transcript with `spice session thread {}`; then run `spice session briefing PATH` and `spice session sweep PATH --count 4` using the returned transcript path before continuing.",
        predecessor.thread, predecessor.thread,
    );
    let (mut proposal, published) = super::super::steering::internal_write(epoch)?;
    guard_residency(&mut proposal, &observed);
    super::super::steering::append_internal_directive(
        bus,
        &mut proposal,
        epoch,
        super::super::steering::InternalDirective {
            directive,
            lane: renewal.lane,
            text,
            control,
            lifetime: steering.expiry().get(),
            cause: FactCause::Existing(observed.position()),
            published,
        },
    )?;
    commit(bus, proposal)?;
    let current = current_residency(bus, epoch)?;
    super::super::steering::publish_inbox(bus, paths, current.body(), renewal.lane)
}

fn ensure_handoff(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    steering: &steer::Config,
    lane: CanonicalUuid,
) -> CommandResult<()> {
    let observed = current_residency(bus, epoch)?;
    let Some(renewal) = renewal_for(observed.body(), lane)? else {
        return Ok(());
    };
    if !matches!(renewal.state, wire::LaneRenewalState::Requested { .. }) {
        return Ok(());
    }
    let control = handoff_control(renewal);
    let directive = handoff_directive(renewal);
    if super::super::steering::control_standing(bus, directive, lane, &control)?.is_some() {
        return Ok(());
    }
    let (mut proposal, published) = super::super::steering::internal_write(epoch)?;
    guard_residency(&mut proposal, &observed);
    super::super::steering::append_internal_directive(
        bus,
        &mut proposal,
        epoch,
        super::super::steering::InternalDirective {
            directive,
            lane,
            text: HANDOFF_TEXT.to_owned(),
            control,
            lifetime: steering.expiry().get(),
            cause: FactCause::Existing(observed.position()),
            published,
        },
    )?;
    commit(bus, proposal)?;
    let current = current_residency(bus, epoch)?;
    super::super::steering::publish_inbox(bus, paths, current.body(), lane)
}

fn cancel_handoff(
    bus: &Bus,
    paths: &RuntimePaths,
    epoch: CanonicalUuid,
    directive: CanonicalUuid,
    lane: CanonicalUuid,
    control: &str,
) -> CommandResult<()> {
    let (mut proposal, published) = super::super::steering::internal_write(epoch)?;
    let Some(_canceled) = super::super::steering::append_control_cancellation(
        bus,
        &mut proposal,
        directive,
        lane,
        control,
        published,
    )?
    else {
        return Ok(());
    };
    commit(bus, proposal)?;
    let current = current_residency(bus, epoch)?;
    super::super::steering::publish_inbox(bus, paths, current.body(), lane)
}

pub(super) fn mark_launch_refused(
    bus: &Bus,
    epoch: CanonicalUuid,
    renewal: &wire::LaneRenewalRecord,
    failure: &Outcome<()>,
) -> CommandResult<()> {
    let wire::LaneRenewalState::HandoffPending {
        predecessor,
        team_slot,
    } = &renewal.state
    else {
        return Ok(());
    };
    let observed = current_residency(bus, epoch)?;
    if renewal_for(observed.body(), renewal.lane)? != Some(renewal) {
        return Ok(());
    }
    let reason = match failure {
        Outcome::Refusal(refusal) => format!("{}: {refusal:?}", refusal.reason()),
        Outcome::Fault(fault) => format!("{}: {}", fault.reason, fault.detail),
        Outcome::Reply(_) => unreachable!("a successful launch is not a refusal"),
    };
    let refused = wire::LaneRenewalRecord {
        team: renewal.team,
        lane: renewal.lane,
        revision: observed.body().topology_revision,
        state: wire::LaneRenewalState::LaunchRefused {
            predecessor: predecessor.clone(),
            team_slot: *team_slot,
            reason,
        },
    };
    let mut next = observed.body().clone();
    replace_renewal(&mut next, renewal.lane, Some(refused.clone()));
    let mut proposal = write(Actor::Spice, epoch, Producer::ResidentRecognition)?;
    guard_residency(&mut proposal, &observed);
    let fact = proposal
        .fact(
            wire::body::LaneRenewalChanged {
                team: renewal.team,
                lane: renewal.lane,
                renewal: Some(refused),
            },
            FactCause::Existing(observed.position()),
        )
        .map_err(|error| store_fault("lane::renewal-fact-invalid", &error.to_string()))?;
    proposal.residency_head(fact, move |_| next);
    commit(bus, proposal).map(|_| ())
}

pub(super) fn successor_record(
    renewal: &wire::LaneRenewalRecord,
    session: &wire::LaneSession,
    revision: u64,
) -> CommandResult<wire::LaneRenewalRecord> {
    let wire::LaneRenewalState::HandoffPending {
        predecessor,
        team_slot,
    } = &renewal.state
    else {
        return Err(store_fault(
            "lane::renewal-state-moved",
            "a successor can start only from handoff-pending state",
        ));
    };
    Ok(wire::LaneRenewalRecord {
        team: renewal.team,
        lane: renewal.lane,
        revision,
        state: wire::LaneRenewalState::SuccessorStarted {
            predecessor: predecessor.clone(),
            successor: identity(session.occupant, session),
            team_slot: *team_slot,
        },
    })
}

pub(super) fn replace_renewal(
    head: &mut wire::ResidencyHead,
    lane: CanonicalUuid,
    renewal: Option<wire::LaneRenewalRecord>,
) {
    head.renewals.retain(|record| record.lane != lane);
    if let Some(renewal) = renewal {
        head.renewals.push(renewal);
        head.renewals
            .sort_by_key(|record| (record.team, record.lane));
    }
}

fn renewal_for(
    head: &wire::ResidencyHead,
    lane: CanonicalUuid,
) -> CommandResult<Option<&wire::LaneRenewalRecord>> {
    let mut matching = head.renewals.iter().filter(|record| record.lane == lane);
    let selected = matching.next();
    if matching.next().is_some() {
        return Err(store_fault(
            "lane::renewal-ambiguous",
            "one lane has more than one durable renewal state",
        ));
    }
    Ok(selected)
}

fn state_name(state: &wire::LaneRenewalState) -> &'static str {
    match state {
        wire::LaneRenewalState::Requested { .. } => "requested",
        wire::LaneRenewalState::HandoffPending { .. } => "handoff-pending",
        wire::LaneRenewalState::SuccessorStarted { .. } => "successor-started",
        wire::LaneRenewalState::LaunchRefused { .. } => "launch-refused",
    }
}

fn handoff_control(renewal: &wire::LaneRenewalRecord) -> String {
    format!("{HANDOFF_CONTROL}:{}", predecessor(&renewal.state).occupant)
}

fn context_control(renewal: &wire::LaneRenewalRecord) -> String {
    format!("{CONTEXT_CONTROL}:{}", predecessor(&renewal.state).occupant)
}

fn handoff_directive(renewal: &wire::LaneRenewalRecord) -> CanonicalUuid {
    control_identity("handoff", predecessor(&renewal.state).occupant)
}

fn context_directive(renewal: &wire::LaneRenewalRecord) -> CanonicalUuid {
    control_identity("context", predecessor(&renewal.state).occupant)
}

fn control_identity(kind: &str, occupant: CanonicalUuid) -> CanonicalUuid {
    let identity = format!("spice:lane-renewal:{kind}:{occupant}");
    let digest = blake3::hash(identity.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    CanonicalUuid::new(uuid::Uuid::from_bytes(bytes))
}

fn predecessor(state: &wire::LaneRenewalState) -> &wire::RenewalIdentity {
    match state {
        wire::LaneRenewalState::Requested { predecessor, .. }
        | wire::LaneRenewalState::HandoffPending { predecessor, .. }
        | wire::LaneRenewalState::SuccessorStarted { predecessor, .. }
        | wire::LaneRenewalState::LaunchRefused { predecessor, .. } => predecessor,
    }
}

fn unit_outcome(outcome: Outcome<CommandAnswer>) -> Outcome<()> {
    match outcome {
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
        Outcome::Reply(_) => unreachable!("a renewal refusal returned a command reply"),
    }
}

#[cfg(all(test, unix))]
mod tests;
