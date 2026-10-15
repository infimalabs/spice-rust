//! Rule-driven room resumption: the resident resuming a ruled room on new
//! input, as the principal whose direction set the rule.

use super::*;

/// One room's automated resume: the target lane when it committed, `None`
/// when a refusal or guard left the room unchanged.
pub(crate) type RuleResume = (
    wire::TaskStamp,
    Result<Option<CanonicalUuid>, Outcome<CommandAnswer>>,
);

/// Issue every automated resume the journal makes ready.
///
/// An operator Start or Resume may carry a bounded resume rule. While a
/// room's latest coordination record carries that rule with budget left,
/// nothing has put the room aside since, its target acknowledged the request,
/// and the anchor is pending and unclaimed, the first room publication by
/// another party that the target has not spoken after is new input: the room
/// is resumed through the operator's own staging, guards, and refusals,
/// citing that input. Silence, repeated agreement, a decline, a lapse, a
/// cancellation, an unseated target, a settled or claimed anchor, and a spent
/// budget each leave the room unchanged, so an unchanged cycle stops.
pub(crate) fn room_resumes_by_rule(
    bus: &Bus,
    config: &steer::Config,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<Vec<RuleResume>, Outcome<CommandAnswer>> {
    Ok(ready_resumes(bus)?
        .into_iter()
        .map(|ready| {
            let room = ready.directed.body().room.clone();
            let resumed = room_resume_by_rule(bus, ready, config, projects, allocation);
            (room, resumed)
        })
        .collect())
}

/// One ruled room ready to resume.
struct Ready {
    /// The room's latest direction, carrying the rule.
    directed: JournalHead<RoomCoordinationStarted>,
    /// The unanswered input that triggers the resumption.
    evidence: RoomAuthorityStamp,
    /// The actor that authored the direction, whom the resumption acts for.
    author: Actor,
}

/// Ruled rooms with budget left whose acknowledged request has unanswered
/// input from another party, each with the input that triggers it.
fn ready_resumes(bus: &Bus) -> Result<Vec<Ready>, Outcome<CommandAnswer>> {
    bus.read(|read| {
        let mut ready = Vec::new();
        for directed in read.ruled_room_directions()? {
            let started = directed.body();
            let spent = started
                .automated
                .as_ref()
                .map_or(0, |automated| automated.step);
            if started.rule.as_ref().is_none_or(|rule| spent >= rule.limit) {
                continue;
            }
            let room = &started.room;
            if read
                .room_shelving(room)?
                .is_some_and(|shelved| shelved.position() > directed.position())
            {
                continue;
            }
            let Some(request) = read.steering_key(&started.key)? else {
                continue;
            };
            if request.body().disposition != wire::Disposition::Acked {
                continue;
            }
            let pending = read.workflow_head(room)?.is_some_and(|workflow| {
                workflow.body().status == "pending" && workflow.body().completion.is_none()
            });
            let claimed = read
                .claim_head(room)?
                .is_some_and(|claim| claim.body().disposition == ClaimDisposition::Active);
            if !pending || claimed {
                continue;
            }
            if let Some((_, evidence)) = read.room_unanswered_input(
                room,
                request.position(),
                &Actor::Lane(request.body().lane),
            )? {
                let author = direction_author(read, directed.position())?;
                ready.push(Ready {
                    directed,
                    evidence,
                    author,
                });
            }
        }
        Ok(ready)
    })
    .map_err(|error| store_fault("room::automation-unreadable", &error.to_string()))
}

/// The actor of the `room::directed` fact at `position`: an operator's
/// direction, or an earlier resumption acting for one.
fn direction_author(
    read: &journal::JournalRead<'_>,
    position: wire::Seq,
) -> Result<Actor, journal::GuardError> {
    read.fact_page(
        journal::FactPageStart::At(position),
        position,
        journal::FactReadBudget::RETAINED,
    )?
    .rows
    .into_iter()
    .next()
    .map(|fact| fact.actor)
    .ok_or_else(|| journal::GuardError::Invalid(format!("room direction {position} is absent")))
}

/// Resume the room `ready` names under the resume rule its direction
/// carries, citing its input, through the staging and guards an operator
/// Resume uses, as the direction's author.
fn room_resume_by_rule(
    bus: &Bus,
    ready: Ready,
    config: &steer::Config,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<Option<CanonicalUuid>, Outcome<CommandAnswer>> {
    let Ready {
        directed,
        evidence,
        author,
    } = ready;
    let started = directed.body();
    let Some(rule) = started.rule.clone() else {
        return Ok(None);
    };
    let automated = RoomAutomatedResume {
        rule: started
            .automated
            .as_ref()
            .map_or_else(|| started.publication.clone(), |prior| prior.rule.clone()),
        step: started.automated.as_ref().map_or(0, |prior| prior.step) + 1,
        evidence,
    };
    if automated.step > rule.limit {
        return Ok(None);
    }
    let snapshot = validated_working_snapshot_for_tasks(
        bus,
        author.clone(),
        std::slice::from_ref(&started.room),
        projects,
        allocation,
    )?;
    let Some(anchor) = snapshot
        .entries
        .iter()
        .find(|entry| entry.born() == &started.room)
        .cloned()
    else {
        return Ok(None);
    };
    let heads = task_heads(&snapshot.answer, anchor.handle())?.clone();
    let coordination = Coordination::read(bus, &started.room)?;
    if coordination.directed.as_ref().map(JournalHead::position) != Some(directed.position()) {
        return Ok(None);
    }
    let request = wire::intent::RoomCoordinate {
        room: anchor.handle().clone(),
        target: started.target,
        action: wire::intent::RoomCoordinateAction::Resume,
        criterion: rule.criterion.clone(),
        contribution: rule.contribution.clone(),
        stop: rule.stop.clone(),
        auto_resume: Some(rule.limit),
    };
    // One rule step owns one directive identity, so a repeated evaluation of
    // the same step is refused as already issued.
    let identity =
        blake3::hash(format!("spice:room-resume:{}:{}", automated.rule, automated.step).as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&identity.as_bytes()[..16]);
    let issue = Issue {
        author,
        directive: CanonicalUuid::new(uuid::Uuid::from_bytes(bytes)),
        rule: Some(rule),
        automated: Some(automated),
    };
    let (write, _, lane) = match stage(
        bus,
        config,
        &snapshot,
        &anchor,
        &heads,
        &coordination,
        &request,
        issue,
    ) {
        Ok(staged) => staged,
        Err(Outcome::Refusal(_)) => return Ok(None),
        Err(outcome) => return Err(outcome),
    };
    match bus
        .commit(write)
        .map_err(|error| store_fault("room::coordination-commit", &error.to_string()))?
    {
        CommitOutcome::Committed(_) => Ok(Some(lane)),
        CommitOutcome::Refused(_) => Ok(None),
        CommitOutcome::Fault(fault) => {
            Err(store_fault("room::coordination-commit", &fault.to_string()))
        }
    }
}
