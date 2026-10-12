use super::*;

use journal::{FactReadBudget, MAX_RETAINED_FACT_PAGE_BYTES};
use wire::intent::{
    WORKING_GROUP_PAGE_LIMIT_MAX, WorkingGroupAnchor, WorkingGroupChange, WorkingGroupChangeStatus,
    WorkingGroupPage, WorkingGroupPut, WorkingGroupRead, WorkingGroupRemove, WorkingGroupView,
};
use wire::rooms::{WORKING_GROUP_DESIGN_LIMIT, WorkingGroupRemoved, WorkingGroupStored};

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
        name: group.name,
        anchors,
        head: position,
    })
}

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
    let designs = match design_witnesses(bus, put) {
        Ok(designs) => designs,
        Err(outcome) => return outcome,
    };
    let group = put.group.group.clone();
    change_working_group(
        bus,
        command,
        group,
        WorkingGroupChangeStatus::Stored,
        designs,
        |write, cause| {
            write.fact(
                WorkingGroupStored {
                    group: put.group.clone(),
                },
                cause,
            )
        },
        put,
    )
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

pub(super) fn remove_working_group(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    remove: &WorkingGroupRemove,
) -> Outcome<CommandAnswer> {
    let group = remove.group.clone();
    change_working_group(
        bus,
        command,
        group.clone(),
        WorkingGroupChangeStatus::Removed,
        Vec::new(),
        |write, cause| write.fact(WorkingGroupRemoved { group }, cause),
        remove,
    )
}

fn change_working_group<B, F>(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    group: wire::rooms::RoomAuthorityStamp,
    status: WorkingGroupChangeStatus,
    designs: Vec<Witness>,
    append: F,
    body: &B,
) -> Outcome<CommandAnswer>
where
    B: serde::Serialize + Clone,
    F: FnOnce(&mut JournalWrite, FactCause) -> Result<PendingFact, journal::BuildError>,
{
    let at = match current_instant() {
        Ok(at) => at,
        Err(outcome) => return outcome,
    };
    let origin = format!("group:{group}")
        .parse()
        .expect("a non-empty group stamp forms a structural origin");
    let mut write = JournalWrite::new(command.actor().clone(), origin, at, Producer::CommandDoor);
    let mut expected = command.expected().to_vec();
    expected.extend(designs);
    guard(&mut write, expected);
    let previous = command.expected().iter().find_map(|witness| match witness {
        Witness::Group {
            group: witnessed,
            head,
        } if witnessed == &group => *head,
        _ => None,
    });
    let cause = previous.map_or(FactCause::Root, FactCause::Existing);
    if let Err(error) = append(&mut write, cause) {
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
