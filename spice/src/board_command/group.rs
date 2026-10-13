use super::*;

use journal::{FactReadBudget, MAX_RETAINED_FACT_PAGE_BYTES};
use wire::intent::{
    WORKING_GROUP_PAGE_LIMIT_MAX, WorkingGroupAnchor, WorkingGroupChange, WorkingGroupChangeStatus,
    WorkingGroupPage, WorkingGroupPut, WorkingGroupRead, WorkingGroupView,
};
use wire::rooms::{
    RoomAuthorityStamp, WORKING_GROUP_DESIGN_LIMIT, WorkingGroup, WorkingGroupRemoved,
    WorkingGroupStored, working_group_origin,
};

pub(super) fn read_working_groups(bus: &Bus, request: &WorkingGroupRead) -> Outcome<CommandAnswer> {
    if request.limit == 0 || request.limit > WORKING_GROUP_PAGE_LIMIT_MAX {
        return refusal(
            "group::limit-invalid",
            Some(request.limit.to_string()),
            "use a limit from 1 through 100",
        );
    }
    if request.group.is_some() && request.after.is_some() {
        return refusal(
            "group::read-shape-invalid",
            None,
            "an exact group read cannot also carry a catalog cursor",
        );
    }
    let request = request.clone();
    match bus.read(move |read| group_page(read, &request)) {
        Ok(Ok((cursor, page))) => Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(cursor),
            body: CommandAnswer::WorkingGroupPage(page),
        }),
        Ok(Err(outcome)) => outcome,
        Err(error) => store_fault("group::read-unavailable", &error.to_string()),
    }
}

fn group_page(
    read: &JournalRead<'_>,
    request: &WorkingGroupRead,
) -> Result<Result<(Seq, WorkingGroupPage), Outcome<CommandAnswer>>, GuardError> {
    let Some(cursor) = read.cursor()? else {
        return Ok(Err(refusal(
            "group::journal-empty",
            None,
            "start the resident before reading saved working groups",
        )));
    };
    let heads: Vec<journal::JournalHead<wire::rooms::WorkingGroup>> = if let Some(group) =
        &request.group
    {
        read.group_head(group)?.into_iter().collect()
    } else {
        let budget = FactReadBudget::new(usize::from(request.limit), MAX_RETAINED_FACT_PAGE_BYTES)
            .expect("validated group page bounds form a journal budget");
        let page = read.group_head_page(request.after.as_ref(), budget)?;
        return Ok(Ok((
            cursor,
            WorkingGroupPage {
                groups: page
                    .rows
                    .into_iter()
                    .map(|head| working_group_view(read, head))
                    .collect::<Result<_, _>>()?,
                unavailable: None,
                continuation: page.continuation,
            },
        )));
    };
    let unavailable = request.group.clone().filter(|_| heads.is_empty());
    Ok(Ok((
        cursor,
        WorkingGroupPage {
            groups: heads
                .into_iter()
                .map(|head| working_group_view(read, head))
                .collect::<Result<_, _>>()?,
            unavailable,
            continuation: None,
        },
    )))
}

fn working_group_view(
    read: &JournalRead<'_>,
    head: journal::JournalHead<wire::rooms::WorkingGroup>,
) -> Result<WorkingGroupView, GuardError> {
    let position = head.position();
    let group = head.into_body();
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
                    }
                }
                _ => WorkingGroupAnchor::Unavailable { born: born.clone() },
            })
        })
        .collect::<Result<_, GuardError>>()?;
    Ok(WorkingGroupView {
        group: group.group,
        anchors,
        head: position,
    })
}

/// Store one group's complete design order. Each named design leaves the
/// group that held it, and a group left without a design ends, all in one
/// guarded write; an empty order ends this group.
pub(super) fn put_working_group(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    put: &WorkingGroupPut,
) -> Outcome<CommandAnswer> {
    let designs = put.group.anchors.len();
    if designs > WORKING_GROUP_DESIGN_LIMIT {
        return refusal(
            "group::design-limit",
            Some(designs.to_string()),
            &format!(
                "a working group holds at most {WORKING_GROUP_DESIGN_LIMIT} designs; remove one \
                 or use another group"
            ),
        );
    }
    let group = put.group.group.clone();
    let ending = put.group.anchors.is_empty();
    if ending && witnessed_head(command, &group).is_none() {
        return refusal(
            "group::design-required",
            Some(group.to_string()),
            "a working group exists only while it holds a design; drop a design to start one",
        );
    }
    let mut witnesses = match design_witnesses(bus, put) {
        Ok(designs) => designs,
        Err(outcome) => return outcome,
    };
    let departures = match departures(bus, put) {
        Ok(departures) => departures,
        Err(outcome) => return outcome,
    };
    witnesses.extend(departures.iter().map(|(head, left)| Witness::Group {
        group: left.group.clone(),
        head: Some(*head),
    }));
    let status = if ending {
        WorkingGroupChangeStatus::Removed
    } else {
        WorkingGroupChangeStatus::Stored
    };
    change_working_group(
        bus,
        command,
        group.clone(),
        status,
        witnesses,
        |write| {
            // A group's head may lie under another group's origin once a design
            // moved, so group facts root their own thread; the group witnesses
            // guard every head this write replaces.
            for (_, left) in departures {
                if left.anchors.is_empty() {
                    write.fact(WorkingGroupRemoved { group: left.group }, FactCause::Root)?;
                } else {
                    write.fact(WorkingGroupStored { group: left }, FactCause::Root)?;
                }
            }
            if ending {
                write.fact(WorkingGroupRemoved { group }, FactCause::Root)
            } else {
                write.fact(
                    WorkingGroupStored {
                        group: put.group.clone(),
                    },
                    FactCause::Root,
                )
            }
        },
        put,
    )
}

fn witnessed_head(command: &wire::AdmittedCommand, group: &RoomAuthorityStamp) -> Option<Seq> {
    command.expected().iter().find_map(|witness| match witness {
        Witness::Group {
            group: witnessed,
            head,
        } if witnessed == group => *head,
        _ => None,
    })
}

/// Every other group a named design leaves, at its observed head, with the
/// designs that remain in it.
fn departures(
    bus: &Bus,
    put: &WorkingGroupPut,
) -> Result<Vec<(Seq, WorkingGroup)>, Outcome<CommandAnswer>> {
    let put = put.group.clone();
    bus.read(move |read| {
        let mut departures: Vec<(Seq, WorkingGroup)> = Vec::new();
        for anchor in &put.anchors {
            let Some(held) = read.anchor_group(anchor)? else {
                continue;
            };
            let head = held.position();
            let held = held.into_body();
            if held.group == put.group
                || departures.iter().any(|(_, left)| left.group == held.group)
            {
                continue;
            }
            let remaining = held
                .anchors
                .iter()
                .filter(|design| !put.anchors.contains(design))
                .cloned()
                .collect();
            departures.push((
                head,
                WorkingGroup {
                    group: held.group,
                    anchors: remaining,
                },
            ));
        }
        Ok(departures)
    })
    .map_err(|error| store_fault("group::read-unavailable", &error.to_string()))
}

/// Admit only Design anchors, guarding each statement head so a concurrent
/// intent edit cannot slip a withdrawn Design into the stored group.
///
/// A missing anchor is left to its existence witness.
fn design_witnesses(
    bus: &Bus,
    put: &WorkingGroupPut,
) -> Result<Vec<Witness>, Outcome<CommandAnswer>> {
    let anchors = put.group.anchors.clone();
    let heads = bus
        .read(move |read| {
            anchors
                .iter()
                .filter_map(|anchor| read.task_head(anchor).transpose())
                .collect::<Result<Vec<_>, GuardError>>()
        })
        .map_err(|error| store_fault("group::read-unavailable", &error.to_string()))?;
    heads
        .into_iter()
        .map(|head| {
            let task = head.body();
            if task.intent != Some(wire::TaskIntent::Design) {
                return Err(refusal(
                    "group::design-only",
                    Some(TaskHandle::for_project(&task.project, &task.born).to_string()),
                    "a working group holds only Design tasks; choose a Design or declare this \
                     task's Design intent",
                ));
            }
            Ok(Witness::Task {
                task: task.born.clone(),
                head: head.position(),
            })
        })
        .collect()
}

fn change_working_group<F>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    group: RoomAuthorityStamp,
    status: WorkingGroupChangeStatus,
    witnesses: Vec<Witness>,
    append: F,
    body: &WorkingGroupPut,
) -> Outcome<CommandAnswer>
where
    F: FnOnce(&mut JournalWrite) -> Result<PendingFact, journal::BuildError>,
{
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = working_group_origin(&group);
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    let mut expected = command.expected().to_vec();
    expected.extend(witnesses);
    guard(&mut write, expected);
    if let Err(error) = append(&mut write) {
        return store_fault("group::fact-invalid", &error.to_string());
    }
    let answer = WorkingGroupChange { group, status };
    if let Err(error) = write.command_result(
        request(command, body.clone()),
        command.epoch(),
        answer.clone(),
    ) {
        return store_fault("group::receipt-invalid", &error.to_string());
    }
    committed(bus, write, CommandAnswer::WorkingGroupChange(answer))
}
