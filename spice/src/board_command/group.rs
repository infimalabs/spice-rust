use super::*;

use wire::grouping::{Destination, Group, GroupingError, Partition, Placement, RevisionError};
use wire::intent::{
    DesignStanding, WorkingGroupAnchor, WorkingGroupArrange, WorkingGroupChange,
    WorkingGroupChangeStatus, WorkingGroupFast, WorkingGroupMove, WorkingGroupPage,
    WorkingGroupRead, WorkingGroupView,
};
use wire::rooms::{
    RoomAuthorityStamp, WORKING_GROUP_DESIGN_LIMIT, WorkingGroup, WorkingGroupDisband,
    WorkingGroupMoved, WorkingGroupPartition, WorkingGroupsArranged, working_group_origin,
};

type Designs<'a> = Partition<&'a RoomAuthorityStamp, &'a TaskStamp>;
type DesignError<'a> = GroupingError<&'a RoomAuthorityStamp, &'a TaskStamp>;

pub(super) fn read_working_groups(
    bus: &Bus,
    _request: &WorkingGroupRead,
) -> Outcome<CommandAnswer> {
    // Collapse reasons come from the task snapshot taken after the journal
    // read, so every conclusion that read observed carries its reason.
    let page = bus.read(group_page);
    let snapshot = match bus.task_snapshot() {
        Ok(snapshot) => snapshot,
        Err(error) => return store_fault("group::read-unavailable", &error.to_string()),
    };
    match page {
        Ok(Ok((cursor, mut page))) => {
            for anchor in page.groups.iter_mut().flat_map(|group| &mut group.anchors) {
                if let WorkingGroupAnchor::Available { born, design, .. } = anchor
                    && let DesignStanding::Collapsed { reason } = design
                {
                    *reason = snapshot.facts().collapse_reason(born).map(str::to_owned);
                }
            }
            Outcome::Reply(Reply {
                position: ReplyPosition::Snapshot(cursor),
                body: CommandAnswer::WorkingGroupPage(page),
            })
        }
        Ok(Err(outcome)) => outcome,
        Err(error) => store_fault("group::read-unavailable", &error.to_string()),
    }
}

fn group_page(
    read: &JournalRead<'_>,
) -> Result<Result<(Seq, WorkingGroupPage), Outcome<CommandAnswer>>, GuardError> {
    let Some(cursor) = read.cursor()? else {
        return Ok(Err(refusal(
            "group::journal-empty",
            None,
            "start the resident before reading saved working groups",
        )));
    };
    let partition = current_partition(read)?;
    Ok(Ok((
        cursor,
        WorkingGroupPage {
            revision: partition.revision,
            groups: partition
                .groups
                .into_iter()
                .map(|group| working_group_view(read, group))
                .collect::<Result<_, _>>()?,
        },
    )))
}

fn current_partition(read: &JournalRead<'_>) -> Result<WorkingGroupPartition, GuardError> {
    Ok(read
        .group_partition()?
        .map(journal::JournalHead::into_body)
        .unwrap_or_default())
}

fn working_group_view(
    read: &JournalRead<'_>,
    group: WorkingGroup,
) -> Result<WorkingGroupView, GuardError> {
    let anchors = group
        .anchors
        .iter()
        .map(|born| {
            let task = read.task_head(born)?;
            let workflow = read.workflow_head(born)?;
            Ok(match (task, workflow) {
                (Some(task), Some(workflow)) if !workflow.body().status.is_deleted() => {
                    match DesignStanding::of(task.body().intent, workflow.body(), None) {
                        Some(design) => WorkingGroupAnchor::Available {
                            born: born.clone(),
                            handle: TaskHandle::for_project(&task.body().project, born),
                            design,
                        },
                        None => WorkingGroupAnchor::Unavailable { born: born.clone() },
                    }
                }
                _ => WorkingGroupAnchor::Unavailable { born: born.clone() },
            })
        })
        .collect::<Result<_, GuardError>>()?;
    Ok(WorkingGroupView {
        group: group.group,
        anchors,
    })
}

/// Working groups as a grouping kind: members are Design tasks, a design in
/// no group is simply absent, and a group never splits, so it never rekeys.
struct DesignKind<'a> {
    /// The one member this change verified as a Design, if any.
    admitted: Option<&'a TaskStamp>,
}

impl<'a> wire::grouping::Kind for DesignKind<'a> {
    type Group = &'a RoomAuthorityStamp;
    type Member = &'a TaskStamp;

    fn admits(&self, member: &'a TaskStamp) -> bool {
        self.admitted == Some(member)
    }

    fn ungrouped(&self, _member: &'a TaskStamp) -> Option<&'a RoomAuthorityStamp> {
        None
    }

    fn rekey(
        &self,
        _partition: &Designs<'a>,
        group: &'a RoomAuthorityStamp,
        _leaving: &'a TaskStamp,
    ) -> &'a RoomAuthorityStamp {
        group
    }
}

/// Move one design into a group, founding the group when no group holds that
/// identity, or out of every group; a group its last design leaves ends.
///
/// Acquisition wakes: a deferred open Design moved into a group, including
/// one founding it, has its deferral cleared in the same write, which then
/// commits under the Design's own task origin.
pub(super) fn move_working_group(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &WorkingGroupMove,
) -> Outcome<CommandAnswer> {
    let design = request.design.clone();
    let (current, head, workflow) = match bus.read(move |read| {
        Ok((
            current_partition(read)?,
            read.task_head(&design)?,
            read.workflow_head(&design)?,
        ))
    }) {
        Ok(observed) => observed,
        Err(error) => return store_fault("group::read-unavailable", &error.to_string()),
    };
    let revision = match next_revision(command, current.revision) {
        Ok(revision) => revision,
        Err(outcome) => return outcome,
    };
    let (Some(head), Some(workflow)) = (head, workflow) else {
        return refusal(
            "group::design-missing",
            Some(request.design.to_string()),
            "read the board and retry with a current Design task",
        );
    };
    let task = head.body();
    let (fact, status) = match moved_fact(&current, task, workflow.body(), request, revision) {
        Ok(moved) => moved,
        Err(outcome) => return outcome,
    };
    let woken = (request.to.is_some()
        && workflow.body().deferral.is_some()
        && wire::intent::is_open_design(task.intent, workflow.body()))
    .then(|| wire::WorkflowHead {
        deferral: None,
        ..workflow.body().clone()
    });
    let origin = if woken.is_some() {
        match task_origin(&task.born) {
            Ok(origin) => origin,
            Err(outcome) => return outcome,
        }
    } else {
        working_group_origin()
    };
    // Statement and workflow witnesses keep an intent edit or phase transition
    // from slipping a pre-Design row into a group, and keep a wake from
    // landing over a workflow that moved. Group membership changes
    // organization only; execution authorization is an explicit preparation
    // decision.
    let witnesses = vec![
        Witness::Task {
            task: task.born.clone(),
            head: head.position(),
        },
        Witness::Workflow {
            task: task.born.clone(),
            head: workflow.position(),
            phase: workflow.body().phase.clone(),
        },
    ];
    let born = task.born.clone();
    change_partition(
        bus,
        command,
        PartitionChange {
            origin,
            revision,
            status,
            witnesses,
        },
        |write, _| {
            write
                .fact(fact, FactCause::Root)
                .map_err(|error| store_fault("group::fact-invalid", &error.to_string()))?;
            if let Some(woken) = woken {
                let wake = write
                    .fact(body::TaskWoken { task: born.clone() }, FactCause::Root)
                    .map_err(|error| store_fault("group::wake-invalid", &error.to_string()))?;
                write.workflow_head(born, wake, move |_| woken);
            }
            Ok(())
        },
        request.clone(),
    )
}

/// The move fact and change status `request` makes of `current`, refusing a
/// non-Design, a Design joining a group while neither open nor completed, an
/// unchanged standing, or a full group. A completed Design may join: a group
/// organizes Designs and gates no possession.
fn moved_fact(
    current: &WorkingGroupPartition,
    task: &wire::TaskHead,
    workflow: &wire::WorkflowHead,
    request: &WorkingGroupMove,
    revision: u64,
) -> Result<(WorkingGroupMoved, WorkingGroupChangeStatus), Outcome<CommandAnswer>> {
    let admitted = task.intent.is_some_and(wire::TaskIntent::is_design)
        && (request.to.is_none()
            || workflow.status.is_completed()
            || wire::intent::is_open_design(task.intent, workflow));
    let kind = DesignKind {
        admitted: admitted.then_some(&request.design),
    };
    let destination = request
        .to
        .as_ref()
        .map_or(Destination::Ungrouped, Destination::Group);
    let designs = designs(current)?;
    let (next, moved) = match designs.apply_move(&kind, &request.design, destination) {
        Ok(moved) => moved,
        Err(GroupingError::NotAdmitted(_)) => {
            return Err(refusal(
                "group::design-only",
                Some(TaskHandle::for_project(&task.project, &task.born).to_string()),
                "a working group admits only a Design that is open or completed",
            ));
        }
        Err(GroupingError::Unchanged) => {
            return Err(refusal(
                "group::design-unchanged",
                Some(request.design.to_string()),
                "the design already stands there; drop it on another group or out of its group",
            ));
        }
        Err(other) => return Err(grouping_refusal(other)),
    };
    let ended = moved.from.is_some_and(|from| !next.holds(from));
    let fact = WorkingGroupMoved {
        design: request.design.clone(),
        from: moved.from.cloned(),
        to: moved.to.cloned(),
        partition: stated(&next, revision, current),
    };
    let status = if ended {
        WorkingGroupChangeStatus::Ended
    } else {
        WorkingGroupChangeStatus::Moved
    };
    Ok((fact, status))
}

/// Restate the whole partition: group order, designs per group, and design
/// order. A design never enters or leaves through an arrangement.
pub(super) fn arrange_working_groups(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &WorkingGroupArrange,
) -> Outcome<CommandAnswer> {
    let (current, revision) = match advancing_partition(bus, command) {
        Ok(found) => found,
        Err(outcome) => return outcome,
    };
    let designs = match designs(&current) {
        Ok(designs) => designs,
        Err(outcome) => return outcome,
    };
    let proposed = request
        .groups
        .iter()
        .map(|group| Group {
            group: &group.group,
            members: group.anchors.iter().collect(),
        })
        .collect::<Vec<_>>();
    let arranged = match designs.arrange(&proposed) {
        Ok(arranged) => arranged,
        Err(error) => return arrangement_refusal(error),
    };
    let fact = WorkingGroupsArranged {
        partition: stated(&arranged.partition, revision, &current),
        from: current.clone(),
    };
    restate(
        bus,
        command,
        WorkingGroupChangeStatus::Arranged,
        fact,
        request.clone(),
    )
}

/// Turn one standing group's fast-mode flag on or off: the whole partition
/// restated with only that flag changed. Turning it on also forgets the
/// fast-mode refusals of the lanes it runs fast in the same transaction, so
/// each next launch tries fast mode.
pub(super) fn set_working_group_fast(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &WorkingGroupFast,
) -> Outcome<CommandAnswer> {
    let (current, revision) = match advancing_partition(bus, command) {
        Ok(found) => found,
        Err(outcome) => return outcome,
    };
    let designs = match standing_group(&current, &request.group) {
        Ok(group) => group.anchors.iter().cloned().collect(),
        Err(outcome) => return outcome,
    };
    let mut partition = current.clone();
    partition.revision = revision;
    if !partition.set_fast(&request.group, request.fast_mode) {
        return refusal(
            "group::fast-unchanged",
            Some(request.fast_mode.to_string()),
            "the group's fast mode already stands there",
        );
    }
    let forgotten = if request.fast_mode {
        match bus.read(move |read| {
            let Some(observed) = read.residency_head()? else {
                return Ok(None);
            };
            let lanes = design_claimants(read, &designs)?;
            Ok(observed
                .body()
                .forget_fast_refusals(|row| lanes.contains(&row.lane))
                .map(|next| (observed, next)))
        }) {
            Ok(forgotten) => forgotten,
            Err(error) => return store_fault("group::read-unavailable", &error.to_string()),
        }
    } else {
        None
    };
    let fact = WorkingGroupsArranged {
        from: current,
        partition,
    };
    change_partition(
        bus,
        command,
        PartitionChange {
            origin: working_group_origin(),
            revision,
            status: WorkingGroupChangeStatus::Retuned,
            witnesses: Vec::new(),
        },
        |write, _| {
            let arranged = write
                .fact(fact, FactCause::Root)
                .map_err(|error| store_fault("group::fact-invalid", &error.to_string()))?;
            if let Some((observed, next)) = forgotten {
                crate::resident::guard_residency(write, &observed);
                write.residency_head(arranged, move |_| next);
            }
            Ok(())
        },
        request.clone(),
    )
}

/// End one standing group and leave all of its Designs ungrouped. Grouping
/// gates no possession, so a disband stops no holder.
pub(super) fn disband_working_group(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &WorkingGroupDisband,
) -> Outcome<CommandAnswer> {
    let (current, revision) = match advancing_partition(bus, command) {
        Ok(found) => found,
        Err(outcome) => return outcome,
    };
    if let Err(outcome) = standing_group(&current, &request.group) {
        return outcome;
    }
    let mut partition = current.clone();
    partition.revision = revision;
    partition
        .groups
        .retain(|group| group.group != request.group);
    partition.fast.retain(|group| group != &request.group);
    let fact = WorkingGroupsArranged {
        from: current,
        partition,
    };
    restate(
        bus,
        command,
        WorkingGroupChangeStatus::Disbanded,
        fact,
        request.clone(),
    )
}

/// Lanes a fast-flagged working group runs fast: each holder of an active
/// claim on a task associated with one of its Design anchors.
pub(crate) fn group_fast_lanes(
    read: &JournalRead<'_>,
) -> Result<std::collections::BTreeSet<wire::CanonicalUuid>, GuardError> {
    let partition = current_partition(read)?;
    let designs = partition
        .groups
        .iter()
        .filter(|group| partition.is_fast(&group.group))
        .flat_map(|group| group.anchors.iter().cloned())
        .collect();
    design_claimants(read, &designs)
}

/// Lanes holding an active claim on a task associated with one of `designs`.
fn design_claimants(
    read: &JournalRead<'_>,
    designs: &std::collections::BTreeSet<wire::TaskStamp>,
) -> Result<std::collections::BTreeSet<wire::CanonicalUuid>, GuardError> {
    let mut lanes = std::collections::BTreeSet::new();
    if designs.is_empty() {
        return Ok(lanes);
    }
    for lane in read.actively_claimed_lanes()? {
        let Some(active) = read.active_claim_for_actor(&wire::Actor::Lane(lane))? else {
            continue;
        };
        if designs
            .iter()
            .any(|design| active.statement().body().is_associated_with_design(design))
        {
            lanes.insert(lane);
        }
    }
    Ok(lanes)
}

/// The revision a change observed at its group witness commits, refusing a
/// partition that moved since.
fn next_revision(
    command: &wire::AdmittedCommand,
    current: u64,
) -> Result<u64, Outcome<CommandAnswer>> {
    let observed = command
        .expected()
        .iter()
        .find_map(|witness| match witness {
            Witness::Group { revision } => Some(*revision),
            _ => None,
        })
        .expect("group law requires the partition witness");
    wire::grouping::advance_revision(current, observed).map_err(|error| match error {
        RevisionError::Moved { current } => refusal(
            "group::partition-moved",
            Some(current.to_string()),
            "the working groups changed since they were read; read them and retry",
        ),
        RevisionError::Exhausted => store_fault(
            "group::revision-exhausted",
            "the working-group partition revision cannot advance",
        ),
    })
}

/// The standing partition and the revision a change to it commits.
fn advancing_partition(
    bus: &Bus,
    command: &wire::AdmittedCommand,
) -> Result<(WorkingGroupPartition, u64), Outcome<CommandAnswer>> {
    let current = bus
        .read(current_partition)
        .map_err(|error| store_fault("group::read-unavailable", &error.to_string()))?;
    let revision = next_revision(command, current.revision)?;
    Ok((current, revision))
}

/// The standing group `group` names, refusing one that no longer stands.
fn standing_group<'a>(
    current: &'a WorkingGroupPartition,
    group: &RoomAuthorityStamp,
) -> Result<&'a WorkingGroup, Outcome<CommandAnswer>> {
    current
        .groups
        .iter()
        .find(|standing| standing.group == *group)
        .ok_or_else(|| {
            refusal(
                "group::missing",
                Some(group.to_string()),
                "the working group no longer stands; read the working groups and retry",
            )
        })
}

fn designs(current: &WorkingGroupPartition) -> Result<Designs<'_>, Outcome<CommandAnswer>> {
    Partition::new(current.groups.iter().flat_map(|group| {
        group.anchors.iter().map(|member| Placement {
            member,
            group: &group.group,
        })
    }))
    .map_err(|error| store_fault("group::partition-invalid", &format!("{error:?}")))
}

/// The partition `designs` states at `revision`, carrying each retained
/// group's fast-mode flag from `current`.
fn stated(
    designs: &Designs<'_>,
    revision: u64,
    current: &WorkingGroupPartition,
) -> WorkingGroupPartition {
    let mut partition = WorkingGroupPartition {
        revision,
        groups: designs
            .groups()
            .into_iter()
            .map(|group| WorkingGroup {
                group: group.group.clone(),
                anchors: group.members.into_iter().cloned().collect(),
            })
            .collect(),
        fast: Vec::new(),
    };
    partition.carry_fast(current);
    partition
}

/// The refusal for a grouping change no group command words for itself.
fn grouping_refusal(error: DesignError<'_>) -> Outcome<CommandAnswer> {
    match error {
        GroupingError::MemberLimit { members } => refusal(
            "group::design-limit",
            Some(members.to_string()),
            &format!(
                "a working group holds at most {WORKING_GROUP_DESIGN_LIMIT} designs; move one \
                 out or choose another group"
            ),
        ),
        other => store_fault("group::grouping-invalid", &format!("{other:?}")),
    }
}

fn arrangement_refusal(error: DesignError<'_>) -> Outcome<CommandAnswer> {
    let (reason, observed, detail) = match error {
        GroupingError::ArrangeEmpty => (
            "group::arrangement-empty",
            "0".to_owned(),
            "state every current group; an arrangement never removes a design",
        ),
        GroupingError::UnknownGroup(group) => (
            "group::arrangement-unknown-group",
            group.to_string(),
            "a fresh group must hold one design split from a current group of two or more",
        ),
        GroupingError::DuplicateGroup(group) => (
            "group::arrangement-duplicate-group",
            group.to_string(),
            "state each group once",
        ),
        GroupingError::VacantGroup(group) => (
            "group::arrangement-vacant-group",
            group.to_string(),
            "omit an emptied group rather than arranging it with no design",
        ),
        GroupingError::UnknownMember(design) => (
            "group::arrangement-unknown-design",
            design.to_string(),
            "an arrangement only reorders grouped designs; move a design in to group it",
        ),
        GroupingError::DuplicateMember(design) => (
            "group::arrangement-duplicate-design",
            design.to_string(),
            "hold every design in exactly one group",
        ),
        GroupingError::Incomplete { missing, .. } => (
            "group::arrangement-incomplete",
            missing.to_string(),
            "state every grouped design; move a design out to ungroup it",
        ),
        other => return grouping_refusal(other),
    };
    refusal(reason, Some(observed), detail)
}

/// One partition change's identity: the origin it commits under, the revision
/// and status its receipt answers, and the witnesses guarding it beyond the
/// command's own.
struct PartitionChange {
    origin: wire::Origin,
    revision: u64,
    status: WorkingGroupChangeStatus,
    witnesses: Vec<Witness>,
}

/// Commit one partition change: `stage` appends the change's facts to a write
/// guarded by the command's witnesses and the change's own.
fn change_partition<F, B>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    change: PartitionChange,
    stage: F,
    body: B,
) -> Outcome<CommandAnswer>
where
    F: FnOnce(&mut JournalWrite, Instant) -> Result<(), Outcome<CommandAnswer>>,
    B: serde::Serialize,
{
    let PartitionChange {
        origin,
        revision,
        status,
        witnesses,
    } = change;
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    let mut expected = command.expected().to_vec();
    expected.extend(witnesses);
    guard(&mut write, expected);
    if let Err(outcome) = stage(&mut write, at) {
        return outcome;
    }
    let answer = WorkingGroupChange { revision, status };
    if let Err(error) =
        write.command_result(request(command, body), command.epoch(), answer.clone())
    {
        return store_fault("group::receipt-invalid", &error.to_string());
    }
    committed(bus, write, CommandAnswer::WorkingGroupChange(answer))
}

/// Commit one whole-partition restatement at the revision it restates.
fn restate<B: serde::Serialize>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    status: WorkingGroupChangeStatus,
    fact: WorkingGroupsArranged,
    body: B,
) -> Outcome<CommandAnswer> {
    let revision = fact.partition.revision;
    change_partition(
        bus,
        command,
        PartitionChange {
            origin: working_group_origin(),
            revision,
            status,
            witnesses: Vec::new(),
        },
        |write, _| {
            write
                .fact(fact, FactCause::Root)
                .map(drop)
                .map_err(|error| store_fault("group::fact-invalid", &error.to_string()))
        },
        body,
    )
}
