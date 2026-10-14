use super::*;

use wire::grouping::{Destination, Group, GroupingError, Partition, Placement, RevisionError};
use wire::intent::{
    DesignStanding, WorkingGroupAnchor, WorkingGroupArrange, WorkingGroupChange,
    WorkingGroupChangeStatus, WorkingGroupMove, WorkingGroupPage, WorkingGroupRead,
    WorkingGroupView,
};
use wire::rooms::{
    RoomAuthorityStamp, WORKING_GROUP_DESIGN_LIMIT, WorkingGroup, WorkingGroupMoved,
    WorkingGroupPartition, WorkingGroupsArranged, working_group_origin,
};

type Designs<'a> = Partition<&'a RoomAuthorityStamp, &'a TaskStamp>;
type DesignError<'a> = GroupingError<&'a RoomAuthorityStamp, &'a TaskStamp>;

pub(super) fn read_working_groups(
    bus: &Bus,
    _request: &WorkingGroupRead,
) -> Outcome<CommandAnswer> {
    // Collapse reasons come from the task snapshot taken after the journal
    // read, so every conclusion that read observed carries its reason.
    let page = bus.read(|read| group_page(read));
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
                (Some(task), Some(workflow))
                    if workflow.body().status != "deleted"
                        && task.body().intent == Some(wire::TaskIntent::Design) =>
                {
                    WorkingGroupAnchor::Available {
                        born: born.clone(),
                        handle: TaskHandle::for_project(&task.body().project, born),
                        design: DesignStanding::of(task.body().intent, workflow.body(), true, None)
                            .expect("a Design anchor has a standing"),
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
/// A Design that leaves every group stands passive. The same write withdraws
/// its room's unanswered coordination and asks the lane holding it to
/// checkpoint and free; the returned lane is the one asked to park.
pub(crate) fn move_working_group(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request: &WorkingGroupMove,
    config: &steer::Config,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> (Outcome<CommandAnswer>, Option<wire::CanonicalUuid>) {
    let design = request.design.clone();
    let (current, head) =
        match bus.read(move |read| Ok((current_partition(read)?, read.task_head(&design)?))) {
            Ok(observed) => observed,
            Err(error) => {
                return (
                    store_fault("group::read-unavailable", &error.to_string()),
                    None,
                );
            }
        };
    let revision = match next_revision(command, current.revision) {
        Ok(revision) => revision,
        Err(outcome) => return (outcome, None),
    };
    let Some(head) = head else {
        let missing = refusal(
            "group::design-missing",
            Some(request.design.to_string()),
            "read the board and retry with a current Design task",
        );
        return (missing, None);
    };
    let task = head.body();
    let (fact, status) = match moved_fact(&current, task, request, revision) {
        Ok(moved) => moved,
        Err(outcome) => return (outcome, None),
    };
    let deactivation = if fact.to.is_none() {
        let handle = TaskHandle::for_project(&task.project, &task.born);
        match Deactivation::read(bus, &handle, projects, allocation) {
            Ok(deactivation) => deactivation,
            Err(outcome) => return (outcome, None),
        }
    } else {
        None
    };
    let origin = match deactivation.as_ref().map(Deactivation::origin).transpose() {
        Ok(origin) => origin.unwrap_or_else(working_group_origin),
        Err(outcome) => return (outcome, None),
    };
    // The statement witness keeps a concurrent intent edit from slipping a
    // withdrawn Design into a group. Group membership changes organization
    // only; execution authorization is an explicit preparation decision.
    let witnesses = vec![Witness::Task {
        task: task.born.clone(),
        head: head.position(),
    }];
    let mut parked = None;
    let outcome = change_partition(
        bus,
        command,
        origin,
        revision,
        status,
        witnesses,
        |write, at| {
            write
                .fact(fact, FactCause::Root)
                .map_err(|error| store_fault("group::fact-invalid", &error.to_string()))?;
            if let Some(deactivation) = deactivation {
                parked = deactivation.stage(bus, command, config, write, at)?;
            }
            Ok(())
        },
        request.clone(),
    );
    let committed = matches!(
        &outcome,
        Outcome::Reply(Reply {
            position: ReplyPosition::Committed(_),
            ..
        })
    );
    (outcome, parked.filter(|_| committed))
}

/// The move fact and change status `request` makes of `current`, refusing a
/// non-Design, an unchanged standing, or a full group.
fn moved_fact(
    current: &WorkingGroupPartition,
    task: &wire::TaskHead,
    request: &WorkingGroupMove,
    revision: u64,
) -> Result<(WorkingGroupMoved, WorkingGroupChangeStatus), Outcome<CommandAnswer>> {
    let kind = DesignKind {
        admitted: (task.intent == Some(wire::TaskIntent::Design)).then_some(&request.design),
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
                "a working group holds only Design tasks; choose a Design or declare this \
                 task's Design intent",
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
        partition: stated(&next, revision),
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
    let current = match bus.read(|read| current_partition(read)) {
        Ok(current) => current,
        Err(error) => return store_fault("group::read-unavailable", &error.to_string()),
    };
    let revision = match next_revision(command, current.revision) {
        Ok(revision) => revision,
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
        partition: stated(&arranged.partition, revision),
        from: current.clone(),
    };
    change_partition(
        bus,
        command,
        working_group_origin(),
        revision,
        WorkingGroupChangeStatus::Arranged,
        Vec::new(),
        |write, _| {
            write
                .fact(fact, FactCause::Root)
                .map(drop)
                .map_err(|error| store_fault("group::fact-invalid", &error.to_string()))
        },
        request.clone(),
    )
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

fn designs(current: &WorkingGroupPartition) -> Result<Designs<'_>, Outcome<CommandAnswer>> {
    Partition::new(current.groups.iter().flat_map(|group| {
        group.anchors.iter().map(|member| Placement {
            member,
            group: &group.group,
        })
    }))
    .map_err(|error| store_fault("group::partition-invalid", &format!("{error:?}")))
}

fn stated(designs: &Designs<'_>, revision: u64) -> WorkingGroupPartition {
    WorkingGroupPartition {
        revision,
        groups: designs
            .groups()
            .into_iter()
            .map(|group| WorkingGroup {
                group: group.group.clone(),
                anchors: group.members.into_iter().cloned().collect(),
            })
            .collect(),
    }
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

/// Commit one partition change under `origin`: `stage` appends the change's
/// facts to a write guarded by the command's witnesses and `witnesses`.
#[allow(
    clippy::too_many_arguments,
    reason = "one partition change owns its origin, witnesses, staging, and receipt"
)]
fn change_partition<F, B>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    origin: wire::Origin,
    revision: u64,
    status: WorkingGroupChangeStatus,
    witnesses: Vec<Witness>,
    stage: F,
    body: B,
) -> Outcome<CommandAnswer>
where
    F: FnOnce(&mut JournalWrite, Instant) -> Result<(), Outcome<CommandAnswer>>,
    B: serde::Serialize,
{
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
