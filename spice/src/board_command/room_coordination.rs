use super::*;

use journal::JournalHead;
use wire::{
    CanonicalUuid, OccupantAddress,
    rooms::{
        CoordinationControl, ROOM_RESUME_LIMIT, RoomAuthorityStamp, RoomAutomatedResume,
        RoomCoordinationStarted, RoomPutAside as RoomPutAsideFact, RoomResumeRule, RoomStopRequest,
    },
};

pub(crate) fn coordination_lane(bus: &Bus, target: OccupantAddress) -> Option<CanonicalUuid> {
    let snapshot = bus.task_snapshot().ok()?;
    let residency = snapshot.residency()?;
    snapshot
        .facts()
        .seat_addresses(&residency.body().lanes)
        .into_iter()
        .find_map(|(lane, address)| (address.address() == Some(target)).then_some(lane))
}

use super::room::mint_publication;

mod automation;
pub(crate) use automation::room_resumes_by_rule;

/// A room's latest coordination records, read beside the board snapshot.
struct Coordination {
    directed: Option<JournalHead<RoomCoordinationStarted>>,
    shelved: Option<JournalHead<RoomPutAsideFact>>,
}

impl Coordination {
    fn read(bus: &Bus, room: &wire::TaskStamp) -> Result<Self, Outcome<CommandAnswer>> {
        let room = room.clone();
        bus.read(move |read| {
            Ok(Self {
                directed: read.room_direction(&room)?,
                shelved: read.room_shelving(&room)?,
            })
        })
        .map_err(|error| store_fault("room::coordination-unreadable", &error.to_string()))
    }

    /// Admit `write` only while the room's coordination records are the ones
    /// this command read, so concurrent coordination commands serialize and
    /// at most one coordination request is live.
    fn guard(&self, room: &wire::TaskStamp, write: &mut JournalWrite) {
        let room = room.clone();
        let directed = self.directed.as_ref().map(JournalHead::position);
        let shelved = self.shelved.as_ref().map(JournalHead::position);
        write.guard(journal::JournalGuard::new(move |read| {
            if read
                .room_direction(&room)?
                .as_ref()
                .map(JournalHead::position)
                == directed
                && read
                    .room_shelving(&room)?
                    .as_ref()
                    .map(JournalHead::position)
                    == shelved
            {
                Ok(())
            } else {
                Err(journal::GuardError::refused("room::coordination-moved"))
            }
        }));
    }
}

const REQUEST_FIELD_BYTES: usize = 2_048;

pub(crate) fn room_coordination_read(
    bus: &Bus,
    request: &wire::intent::RoomCoordinationRead,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let (snapshot, anchor, heads, coordination) =
        match context(bus, &request.room, projects, allocation) {
            Ok(context) => context,
            Err(outcome) => return outcome,
        };
    let mut view = match project(bus, anchor.handle(), &heads, &coordination, false) {
        Ok(view) => view,
        Err(outcome) => return outcome,
    };
    // The working snapshot holds the whole live board, so this is the same
    // resolution an Open read judges every Design by.
    let availability = if anchor.claim().is_some_and(wire::ClaimHead::is_active) {
        wire::rooms::DesignAvailability::Claimed
    } else if view.design == Some(wire::intent::DesignStanding::Active) {
        wire::rooms::DesignAvailability::Unclaimed
    } else {
        wire::rooms::DesignAvailability::Passive
    };
    view.cycle =
        board::design_cycle(&anchor, &board::resolved_tasks(&snapshot.entries)).map(|cycle| {
            wire::rooms::DesignCyclePresentation::new(
                cycle,
                heads.design_cycle,
                anchor.handle(),
                availability,
            )
        });
    Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(snapshot.cursor),
        body: CommandAnswer::RoomCoordination(view),
    })
}

pub(crate) fn room_coordinate(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &wire::intent::RoomCoordinate,
    config: &steer::Config,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    room_coordinate_with(bus, command, request, config, projects, allocation, || {})
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn room_coordinate_with(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &wire::intent::RoomCoordinate,
    config: &steer::Config,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
    before_commit: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    let rule = match admit_request(request) {
        Ok(rule) => rule,
        Err(outcome) => return outcome,
    };
    let (snapshot, anchor, heads, coordination) =
        match context(bus, &request.room, projects, allocation) {
            Ok(context) => context,
            Err(outcome) => return outcome,
        };
    let issue = Issue {
        author: command.actor().clone(),
        directive: command.id(),
        rule,
        automated: None,
    };
    let (mut write, view, _) = match stage(
        bus,
        config,
        &snapshot,
        &anchor,
        &heads,
        &coordination,
        request,
        issue,
    ) {
        Ok(staged) => staged,
        Err(outcome) => return outcome,
    };
    guard(&mut write, command.expected().to_vec());
    commit_coordinate(bus, command, request, write, view, before_commit)
}

/// Who issues one coordination request and what its record carries.
struct Issue {
    /// The principal the direction is written as.
    author: Actor,
    directive: CanonicalUuid,
    rule: Option<RoomResumeRule>,
    automated: Option<RoomAutomatedResume>,
}

/// Stage one Start or Resume: refuse what the room or target cannot take,
/// then write the direction and its `room::directed` record under the guards
/// that keep the room, its records, and the target's claim slot as read.
#[allow(
    clippy::too_many_arguments,
    reason = "staging reads the board, anchor, and coordination its two issuers resolve"
)]
fn stage(
    bus: &Bus,
    config: &steer::Config,
    snapshot: &ValidatedSnapshot,
    anchor: &Entry,
    heads: &wire::intent::BoardTask,
    coordination: &Coordination,
    request: &wire::intent::RoomCoordinate,
    issue: Issue,
) -> Result<
    (
        JournalWrite,
        wire::intent::RoomCoordinationView,
        CanonicalUuid,
    ),
    Outcome<CommandAnswer>,
> {
    if super::room::anchor_settlement(&heads.workflow.body).is_some() {
        return Err(refusal(
            "room::anchor-settled",
            Some(anchor.handle().to_string()),
            "coordinate only an unresolved anchor task",
        ));
    }
    if board::passive_design(anchor) {
        return Err(refusal(
            "room::design-passive",
            Some(anchor.handle().to_string()),
            "a Design outside every working group is passive; move it into a working group before coordinating lanes onto it",
        ));
    }
    let current = project(bus, anchor.handle(), heads, coordination, false)?;
    if !current.requests.is_empty() {
        return Err(refusal(
            "room::coordination-live",
            Some(current.requests[0].key.clone()),
            "retire or put aside the unanswered request before issuing fresh scope",
        ));
    }
    if let Some(stop) = current.stop {
        return Err(refusal(
            "room::stop-pending",
            Some(stop.key),
            "wait for the holder's native completion or free before restarting coordination",
        ));
    }
    let lane = admit_target(snapshot, heads, request.target)?;
    let now = jiff::Timestamp::now();
    let at = Instant::new(now)
        .map_err(|error| environment_fault("room::clock-invalid", &error.to_string()))?;
    let publication = mint_publication(bus, anchor.born(), now.as_microsecond())?;
    let room = anchor.born().clone();
    let origin = task_origin(&room)?;
    let message = direction(
        anchor.handle(),
        heads,
        lane,
        request,
        issue.automated.as_ref(),
    );
    let control = CoordinationControl::Coordinate(room.clone()).to_string();
    let mut write = JournalWrite::new(issue.author, origin.clone(), at, Producer::CommandDoor);
    coordination.guard(&room, &mut write);
    guard_target_claim(&mut write, lane, &room);
    let (key, _) = crate::resident::append_operator_directive(
        bus,
        origin,
        config,
        &mut write,
        crate::resident::OperatorDirective {
            directive: issue.directive,
            fanout: None,
            lane,
            text: &message,
            control: Some(&control),
            room: None,
        },
        at,
        FactCause::Root,
    )?;
    let started = RoomCoordinationStarted {
        room,
        publication,
        key: key.clone(),
        target: request.target,
        rule: issue.rule,
        automated: issue.automated,
    };
    let rule = budget(&started);
    write
        .fact(started, FactCause::Root)
        .map_err(|error| store_fault("room::coordination-build", &error.to_string()))?;
    crate::resident::steering_envelope_admits(&write)?;
    let view = wire::intent::RoomCoordinationView {
        room: anchor.handle().clone(),
        requests: vec![wire::intent::RoomCoordinationRequest {
            key,
            target: request.target,
        }],
        stop: None,
        rule,
        design: heads.design_standing(),
        cycle: None,
        hide: false,
    };
    Ok((write, view, lane))
}

/// The resume budget one record carries: its rule's limit and the steps
/// spent through this record.
fn budget(started: &RoomCoordinationStarted) -> Option<wire::intent::RoomResumeBudget> {
    started
        .rule
        .as_ref()
        .map(|rule| wire::intent::RoomResumeBudget {
            limit: rule.limit,
            spent: started
                .automated
                .as_ref()
                .map_or(0, |automated| automated.step),
        })
}

fn commit_coordinate(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &wire::intent::RoomCoordinate,
    mut write: JournalWrite,
    view: wire::intent::RoomCoordinationView,
    before_commit: impl FnOnce(),
) -> Outcome<CommandAnswer> {
    if let Err(error) = write.command_result(
        request_command(command, request.clone()),
        command.epoch(),
        view.clone(),
    ) {
        return store_fault("room::coordination-receipt", &error.to_string());
    }
    before_commit();
    committed_preserving_guard_refusal(
        bus,
        write,
        CommandAnswer::RoomCoordination(view),
        "room::lane-busy",
    )
}

pub(crate) fn room_put_aside(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &wire::intent::RoomPutAside,
    config: &steer::Config,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let (snapshot, anchor, heads, coordination) =
        match context(bus, &request.room, projects, allocation) {
            Ok(context) => context,
            Err(outcome) => return outcome,
        };
    let current = match project(bus, anchor.handle(), &heads, &coordination, false) {
        Ok(view) => view,
        Err(outcome) => return outcome,
    };
    if quiescent(&current, &heads) {
        return Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(snapshot.cursor),
            body: CommandAnswer::RoomCoordination(wire::intent::RoomCoordinationView {
                hide: true,
                ..current
            }),
        });
    }
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let room = anchor.born().clone();
    let origin = match task_origin(&room) {
        Ok(origin) => origin,
        Err(outcome) => return outcome,
    };
    let existing_stop = current.stop.clone();
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    coordination.guard(&room, &mut write);
    let withdrawn = match stage_withdrawal(
        bus,
        command,
        config,
        Standing {
            snapshot: &snapshot,
            anchor: &anchor,
            heads: &heads,
            current: &current,
        },
        origin,
        &mut write,
        at,
        Withdrawal::PutAside,
        None,
    ) {
        Ok(Some(withdrawn)) => withdrawn,
        Ok(None) => {
            return refusal(
                "room::already-quiescent",
                Some(anchor.handle().to_string()),
                "the room has no unanswered coordination or active anchor claim to put aside",
            );
        }
        Err(outcome) => return outcome,
    };
    let visible_stop = withdrawn
        .stop
        .map(|stop| wire::intent::RoomCoordinationStop {
            key: stop.key,
            target: stop.target,
            claim: stop.claim,
        })
        .or(existing_stop);
    let view = wire::intent::RoomCoordinationView {
        room: anchor.handle().clone(),
        requests: Vec::new(),
        stop: visible_stop,
        rule: None,
        design: heads.design_standing(),
        cycle: None,
        hide: true,
    };
    if let Err(error) = write.command_result(
        request_command(command, request.clone()),
        command.epoch(),
        view.clone(),
    ) {
        return store_fault("room::put-aside-receipt", &error.to_string());
    }
    committed(bus, write, CommandAnswer::RoomCoordination(view))
}

/// Why a room's unanswered coordination and its anchor's holder are withdrawn.
#[derive(Clone, Copy)]
enum Withdrawal {
    /// The operator put the room aside.
    PutAside,
    /// The anchor Design left every working group and stands passive.
    Deactivation,
}

impl Withdrawal {
    fn message(self, anchor: &TaskHandle) -> String {
        match self {
            Self::PutAside => format!(
                "Put aside task room {anchor}. Checkpoint the anchor's current work and run `spice task free {anchor}` at the next safe boundary. Do not release child-plan claims or unrelated steering/ASK duties."
            ),
            Self::Deactivation => format!(
                "Design {anchor} became passive under its working-group stance. Checkpoint its \
                 current work and run `spice task free {anchor}` at the next safe boundary; free \
                 parks unpublished commits and dirty work under the task, and your claim stays \
                 authoritative until then. Do not release child-plan claims or unrelated \
                 steering/ASK duties."
            ),
        }
    }
}

/// Whether the room has nothing a withdrawal could take: no unanswered
/// coordination, and either a standing stop or no lane holding the anchor.
fn quiescent(
    current: &wire::intent::RoomCoordinationView,
    heads: &wire::intent::BoardTask,
) -> bool {
    current.requests.is_empty() && (current.stop.is_some() || active_lane_claim(heads).is_none())
}

/// One anchor's board row and coordination state as a withdrawal read them.
#[derive(Clone, Copy)]
struct Standing<'a> {
    snapshot: &'a ValidatedSnapshot,
    anchor: &'a Entry,
    heads: &'a wire::intent::BoardTask,
    current: &'a wire::intent::RoomCoordinationView,
}

/// What one withdrawal staged beyond its cancellations.
struct Withdrawn {
    stop: Option<RoomStopRequest>,
}

/// Withdraw the room's unanswered coordination and, unless a stop already
/// stands, ask the lane holding the anchor to checkpoint and free, recording
/// both as one `room::shelved` record in `write`. `None` when the room has
/// nothing to withdraw.
#[allow(
    clippy::too_many_arguments,
    reason = "a withdrawal stages into the write and origin its issuing command owns"
)]
fn stage_withdrawal(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    config: &steer::Config,
    standing: Standing<'_>,
    origin: wire::Origin,
    write: &mut JournalWrite,
    at: Instant,
    why: Withdrawal,
    directive: Option<(CanonicalUuid, CanonicalUuid)>,
) -> Result<Option<Withdrawn>, Outcome<CommandAnswer>> {
    let current = standing.current;
    if quiescent(current, standing.heads) {
        return Ok(None);
    }
    let room = standing.anchor.born().clone();
    let publication = mint_publication(bus, &room, jiff::Timestamp::now().as_microsecond())?;
    let canceled = cancel_requests(bus, write, at, current.requests.clone())?;
    let stop = if current.stop.is_some() {
        None
    } else {
        stage_stop_request(
            bus, standing, command, config, origin, write, at, why, directive,
        )?
    };
    if canceled.is_empty() && stop.is_none() {
        return Ok(None);
    }
    write
        .fact(
            RoomPutAsideFact {
                room,
                publication,
                canceled,
                stop: stop.clone(),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("room::put-aside-build", &error.to_string()))?;
    crate::resident::steering_envelope_admits(write)?;
    Ok(Some(Withdrawn { stop }))
}

/// A Design's room as read before or after a partition change makes the Design
/// passive. The change withdraws the room's coordination and asks the lane
/// holding the Design to park, either in the move write or in a following
/// task-origin write.
pub(super) struct Deactivation {
    snapshot: ValidatedSnapshot,
    anchor: Entry,
    heads: wire::intent::BoardTask,
    coordination: Coordination,
    current: wire::intent::RoomCoordinationView,
}

impl Deactivation {
    /// Read `design`'s room, or `None` when the board no longer shows it.
    pub(super) fn read(
        bus: &Bus,
        design: &TaskHandle,
        projects: &ProjectPolicy,
        allocation: &board::NextConfig,
    ) -> Result<Option<Self>, Outcome<CommandAnswer>> {
        let snapshot = validated_working_snapshot(
            bus,
            Actor::Operator,
            std::slice::from_ref(design),
            projects,
            allocation,
        )?;
        let Some(anchor) = task_entry(&snapshot.entries, design).cloned() else {
            return Ok(None);
        };
        let heads = task_heads(&snapshot.answer, design)?.clone();
        let coordination = Coordination::read(bus, anchor.born())?;
        let current = project(bus, anchor.handle(), &heads, &coordination, false)?;
        if current.stop.is_none()
            && let Some((lane, claim)) = active_lane_claim(&heads)
        {
            holder_address(&snapshot, lane, claim.body.occupant)?;
        }
        Ok(Some(Self {
            snapshot,
            anchor,
            heads,
            coordination,
            current,
        }))
    }

    /// Whether the room owes nothing: no live coordination to withdraw and no
    /// holder without a standing stop for its current claim win.
    pub(super) fn is_quiescent(&self) -> bool {
        quiescent(&self.current, &self.heads)
    }

    /// The origin the partition change commits under: the partition's own,
    /// unless it withdraws something from the Design's room, whose steering
    /// and claim its withdrawal cites and whose origin a cause cannot leave.
    pub(super) fn origin(&self) -> Result<wire::Origin, Outcome<CommandAnswer>> {
        if quiescent(&self.current, &self.heads) {
            Ok(wire::rooms::working_group_origin())
        } else {
            task_origin(self.anchor.born())
        }
    }

    /// Stage the withdrawal into the partition change's `write`, guarding the
    /// claim and coordination records it read, so a claim won or coordination
    /// issued meanwhile refuses the change. Returns the lane asked to park.
    pub(super) fn stage(
        self,
        bus: &Bus,
        command: &wire::AdmittedCommand,
        config: &steer::Config,
        write: &mut JournalWrite,
        at: Instant,
        directive: Option<(CanonicalUuid, CanonicalUuid)>,
    ) -> Result<Option<CanonicalUuid>, Outcome<CommandAnswer>> {
        let room = self.anchor.born().clone();
        self.coordination.guard(&room, write);
        guard(
            write,
            vec![Witness::Claim {
                task: room,
                head: self.heads.claim.as_ref().map(|claim| claim.position),
            }],
        );
        let origin = self.origin()?;
        let withdrawn = stage_withdrawal(
            bus,
            command,
            config,
            Standing {
                snapshot: &self.snapshot,
                anchor: &self.anchor,
                heads: &self.heads,
                current: &self.current,
            },
            origin,
            write,
            at,
            Withdrawal::Deactivation,
            directive,
        )?;
        Ok(withdrawn
            .and_then(|withdrawn| withdrawn.stop)
            .and(active_lane_claim(&self.heads).map(|(lane, _)| lane)))
    }

    /// Commit this room's deactivation under its own task origin after the
    /// shared partition has made it passive. A guard refusal means the room
    /// moved since it was read, so the caller reads it again.
    pub(super) fn commit(
        self,
        bus: &Bus,
        command: &wire::AdmittedCommand,
        config: &steer::Config,
        directive: (CanonicalUuid, CanonicalUuid),
    ) -> Result<Shelving, Outcome<CommandAnswer>> {
        let origin = self.origin()?;
        let at = current_instant()?;
        let mut write =
            JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
        let lane = self.stage(bus, command, config, &mut write, at, Some(directive))?;
        match bus
            .commit(write)
            .map_err(|error| store_fault("room::deactivation-commit", &error.to_string()))?
        {
            CommitOutcome::Committed(commit) => Ok(Shelving::Committed {
                range: SeqRange {
                    first: seq(commit.first()),
                    last: seq(commit.last()),
                },
                lane,
            }),
            CommitOutcome::Refused(_) => Ok(Shelving::Moved),
            CommitOutcome::Fault(fault) => {
                Err(store_fault("room::deactivation-fault", &fault.to_string()))
            }
        }
    }
}

/// What one task-origin deactivation write did.
pub(super) enum Shelving {
    /// Its withdrawal committed over `range`, asking `lane` to park.
    Committed {
        range: SeqRange,
        lane: Option<CanonicalUuid>,
    },
    /// The room moved since it was read; read it again.
    Moved,
}

fn cancel_requests(
    bus: &Bus,
    write: &mut JournalWrite,
    at: Instant,
    requests: Vec<wire::intent::RoomCoordinationRequest>,
) -> Result<Vec<String>, Outcome<CommandAnswer>> {
    let mut canceled = Vec::with_capacity(requests.len());
    for request in requests {
        let head = match steering_key(bus, &request.key)? {
            Some(head) if head.body().disposition == wire::Disposition::Live => head,
            _ => continue,
        };
        canceled.push(crate::resident::append_cancellation(write, &head, at)?);
    }
    Ok(canceled)
}

#[allow(
    clippy::too_many_arguments,
    reason = "a stop request stages into the write and origin its withdrawal owns"
)]
fn stage_stop_request(
    bus: &Bus,
    standing: Standing<'_>,
    command: &wire::AdmittedCommand,
    config: &steer::Config,
    origin: wire::Origin,
    write: &mut JournalWrite,
    at: Instant,
    why: Withdrawal,
    directive: Option<(CanonicalUuid, CanonicalUuid)>,
) -> Result<Option<RoomStopRequest>, Outcome<CommandAnswer>> {
    let Some((lane, claim)) = active_lane_claim(standing.heads) else {
        return Ok(None);
    };
    let target = match why {
        Withdrawal::PutAside => target_for_lane(standing.snapshot, lane)?,
        Withdrawal::Deactivation => holder_address(standing.snapshot, lane, claim.body.occupant)?,
    };
    let claim_code = wire::SequenceCode::new(claim.body.claimed.get());
    let control = CoordinationControl::Stop {
        room: standing.anchor.born().clone(),
        claim: claim_code,
    }
    .to_string();
    let message = why.message(standing.anchor.handle());
    let (directive, fanout) = directive.map_or((command.id(), None), |(directive, fanout)| {
        (directive, Some(fanout))
    });
    let (key, _) = crate::resident::append_operator_directive(
        bus,
        origin,
        config,
        write,
        crate::resident::OperatorDirective {
            directive,
            fanout,
            lane,
            text: &message,
            control: Some(&control),
            room: None,
        },
        at,
        FactCause::Existing(claim.body.claimed),
    )?;
    Ok(Some(RoomStopRequest {
        key,
        target,
        claim: claim_code,
    }))
}

/// The address that reaches the lane holding a deactivated Design: its seated
/// occupant's, or, while the lane is vacant, the birth address of the occupant
/// that won the claim, under which steering queues for the lane's next seat.
fn holder_address(
    snapshot: &ValidatedSnapshot,
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
) -> Result<OccupantAddress, Outcome<CommandAnswer>> {
    snapshot
        .answer
        .addresses
        .get(&lane)
        .and_then(wire::SeatAddress::address)
        .or_else(|| snapshot.facts.occupant_address(lane, occupant).ok())
        .ok_or_else(|| {
            refusal(
                "room::holder-address-unavailable",
                None,
                "the current anchor holder has no proved base-52 occupant address",
            )
        })
}

fn context(
    bus: &Bus,
    room: &TaskHandle,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Result<
    (
        ValidatedSnapshot,
        Entry,
        wire::intent::BoardTask,
        Coordination,
    ),
    Outcome<CommandAnswer>,
> {
    let snapshot = validated_working_snapshot(
        bus,
        Actor::Operator,
        std::slice::from_ref(room),
        projects,
        allocation,
    )?;
    let anchor = task_entry(&snapshot.entries, room)
        .cloned()
        .ok_or_else(|| {
            refusal(
                "room::anchor-unknown",
                Some(room.to_string()),
                "read the board and name one current task handle",
            )
        })?;
    let heads = task_heads(&snapshot.answer, room)?.clone();
    let coordination = Coordination::read(bus, anchor.born())?;
    Ok((snapshot, anchor, heads, coordination))
}

/// Admit the request's scope and the resume rule it authorizes, if any.
fn admit_request(
    request: &wire::intent::RoomCoordinate,
) -> Result<Option<RoomResumeRule>, Outcome<CommandAnswer>> {
    for (name, value) in [
        ("criterion", &request.criterion),
        ("contribution", &request.contribution),
        ("stop", &request.stop),
    ] {
        if value.trim().is_empty() {
            return Err(refusal(
                "room::coordination-scope-empty",
                Some(name.to_owned()),
                "name the unresolved criterion, bounded contribution, and stop condition",
            ));
        }
        if value.len() > REQUEST_FIELD_BYTES {
            return Err(refusal(
                "room::coordination-scope-too-large",
                Some(format!("{name}={} bytes", value.len())),
                "keep each scoped request field within 2048 UTF-8 bytes",
            ));
        }
    }
    let Some(limit) = request.auto_resume else {
        return Ok(None);
    };
    if !(1..=ROOM_RESUME_LIMIT).contains(&limit) {
        return Err(refusal(
            "room::auto-resume-out-of-range",
            Some(limit.to_string()),
            "authorize from 1 through 8 automated resumes",
        ));
    }
    Ok(Some(RoomResumeRule {
        limit,
        criterion: request.criterion.clone(),
        contribution: request.contribution.clone(),
        stop: request.stop.clone(),
    }))
}

/// Resolve `target` to the lane and occupant of the occupied seat it names.
pub(super) fn coordinate_target(
    board: &BoardAnswer,
    target: OccupantAddress,
) -> Result<(CanonicalUuid, CanonicalUuid), Outcome<CommandAnswer>> {
    board
        .residency
        .body
        .occupied_seat(&board.addresses, target)
        .map_err(|seat| {
            refusal(
                seat.reason(),
                Some(seat.observed(target)),
                &seat.explanation(target),
            )
        })
}

fn admit_target(
    snapshot: &ValidatedSnapshot,
    anchor: &wire::intent::BoardTask,
    target: OccupantAddress,
) -> Result<CanonicalUuid, Outcome<CommandAnswer>> {
    let (lane, _) = coordinate_target(&snapshot.answer, target)?;
    if let Some(claim) = anchor.claim.as_ref()
        && claim.body.disposition == ClaimDisposition::Active
    {
        return match claim.body.actor {
            Actor::Lane(holder) if holder == lane => Ok(lane),
            _ => Err(refusal(
                "room::anchor-held-elsewhere",
                Some(anchor.statement.body.handle.to_string()),
                "resume an active anchor only through its current holder",
            )),
        };
    }
    if snapshot.entries.iter().any(|entry| {
        entry.claim().is_some_and(|claim| {
            claim.actor == Actor::Lane(lane) && claim.disposition == ClaimDisposition::Active
        })
    }) {
        return Err(refusal(
            "room::lane-busy",
            Some(target.to_string()),
            "choose an idle occupant or wait for its current native claim disposition",
        ));
    }
    Ok(lane)
}

/// Recheck the target's one active-claim slot in the same transaction that
/// publishes the coordination direction. Holding this room's anchor is the
/// one admitted active claim; every other claim makes the lane busy.
fn guard_target_claim(write: &mut JournalWrite, lane: CanonicalUuid, room: &wire::TaskStamp) {
    let actor = Actor::Lane(lane);
    let room = room.clone();
    write.guard(JournalGuard::new(move |read| {
        match read.active_claim_for_actor(&actor)? {
            None => Ok(()),
            Some(active) if active.statement().body().born == room => Ok(()),
            Some(_) => Err(GuardError::refused("room::lane-busy")),
        }
    }));
}

fn target_for_lane(
    snapshot: &ValidatedSnapshot,
    lane: CanonicalUuid,
) -> Result<OccupantAddress, Outcome<CommandAnswer>> {
    snapshot
        .answer
        .addresses
        .get(&lane)
        .and_then(wire::SeatAddress::address)
        .ok_or_else(|| {
            refusal(
                "room::holder-address-unavailable",
                None,
                "the current anchor holder has no proved base-52 occupant address",
            )
        })
}

fn direction(
    handle: &TaskHandle,
    heads: &wire::intent::BoardTask,
    lane: CanonicalUuid,
    request: &wire::intent::RoomCoordinate,
    automated: Option<&RoomAutomatedResume>,
) -> String {
    let action = match request.action {
        wire::intent::RoomCoordinateAction::Start => "Start",
        wire::intent::RoomCoordinateAction::Resume => "Resume",
    };
    let capture = if heads.claim.as_ref().is_some_and(|claim| {
        claim.body.disposition == ClaimDisposition::Active && claim.body.actor == Actor::Lane(lane)
    }) {
        "Continue under your current native claim.".to_owned()
    } else {
        format!(
            "Run `spice task capture {handle}` to acquire this anchor through its ordinary native claim path; any refusal leaves this request pending without possession."
        )
    };
    let input = automated.map_or_else(String::new, |automated| {
        format!(
            "\nNew input: room publication {} arrived after your last contribution; this is automated resume {} of {} under operator rule {}.",
            automated.evidence,
            automated.step,
            request.auto_resume.unwrap_or(automated.step),
            automated.rule,
        )
    });
    format!(
        "{action} task room {handle}. Before acting, re-read its current task and cited authority; push back or redirect if the ground changed.\nUnresolved criterion: {}\nBounded contribution: {}\nStop condition: {}{input}\n{capture}",
        request.criterion, request.contribution, request.stop,
    )
}

/// The room's coordination state: the latest request while its steering is
/// still live, and the latest stop while the claim win it names stays active.
/// Starting refuses while a request is live and every coordination write
/// guards the records it read, so no earlier request can still be live.
fn project(
    bus: &Bus,
    handle: &TaskHandle,
    heads: &wire::intent::BoardTask,
    coordination: &Coordination,
    hide: bool,
) -> Result<wire::intent::RoomCoordinationView, Outcome<CommandAnswer>> {
    let mut requests = Vec::new();
    if let Some(directed) = &coordination.directed {
        let started = directed.body();
        let Some(head) = steering_key(bus, &started.key)? else {
            return Err(store_fault(
                "room::coordination-steering-missing",
                &format!("room marker names absent steering key {}", started.key),
            ));
        };
        if head.body().disposition == wire::Disposition::Live {
            requests.push(wire::intent::RoomCoordinationRequest {
                key: started.key.clone(),
                target: started.target,
            });
        }
    }
    let active = active_lane_claim(heads);
    let stop = coordination
        .shelved
        .as_ref()
        .and_then(|shelved| shelved.body().stop.clone())
        .filter(|stop| {
            active.is_some_and(|(_, claim)| claim.body.claimed.get() == stop.claim.get())
        })
        .map(|stop| wire::intent::RoomCoordinationStop {
            key: stop.key,
            target: stop.target,
            claim: stop.claim,
        });
    let rule = coordination
        .directed
        .as_ref()
        .filter(|directed| {
            coordination
                .shelved
                .as_ref()
                .is_none_or(|shelved| shelved.position() < directed.position())
        })
        .and_then(|directed| budget(directed.body()));
    Ok(wire::intent::RoomCoordinationView {
        room: handle.clone(),
        requests,
        stop,
        rule,
        design: heads.design_standing(),
        cycle: None,
        hide,
    })
}

fn steering_key(
    bus: &Bus,
    key: &str,
) -> Result<Option<journal::JournalHead<wire::SteeringHead>>, Outcome<CommandAnswer>> {
    let key = key.to_owned();
    bus.read(move |read| read.steering_key(&key))
        .map_err(|error| store_fault("room::coordination-unreadable", &error.to_string()))
}

fn active_lane_claim(
    heads: &wire::intent::BoardTask,
) -> Option<(CanonicalUuid, &wire::intent::Positioned<wire::ClaimHead>)> {
    let claim = heads
        .claim
        .as_ref()
        .filter(|claim| claim.body.disposition == ClaimDisposition::Active)?;
    let Actor::Lane(lane) = claim.body.actor else {
        return None;
    };
    Some((lane, claim))
}

fn request_command<B: serde::Serialize + Clone>(
    command: &wire::AdmittedCommand,
    body: B,
) -> wire::Command<B> {
    request(command, body)
}
