use super::*;

use wire::intent::{WorkingGroupAnchor, WorkingGroupPage, WorkingGroupRead, WorkingGroupView};
use wire::rooms::{RoomAuthorityStamp, WorkingGroup, WorkingGroupPut, WorkingGroupRemove};

fn group_stamp(offset: i64) -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(1_760_000_000_000_000 + offset).expect("fixture group stamp")
}

fn read_groups(
    bus: &Bus,
    epoch: CanonicalUuid,
    group: Option<RoomAuthorityStamp>,
    after: Option<RoomAuthorityStamp>,
    limit: u16,
) -> WorkingGroupPage {
    let outcome = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupRead,
        Vec::new(),
        &WorkingGroupRead {
            group,
            after,
            limit,
        },
    );
    let Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(_),
        body: CommandAnswer::WorkingGroupPage(page),
    }) = outcome
    else {
        panic!("working-group read did not return a page: {outcome:?}");
    };
    page
}

fn read_group(bus: &Bus, epoch: CanonicalUuid, group: &RoomAuthorityStamp) -> WorkingGroupView {
    read_groups(bus, epoch, Some(group.clone()), None, 1)
        .groups
        .into_iter()
        .next()
        .expect("fixture group exists")
}

fn put_group(
    bus: &Bus,
    epoch: CanonicalUuid,
    group: RoomAuthorityStamp,
    head: Option<Seq>,
    name: &str,
    anchors: Vec<TaskStamp>,
) -> Outcome<CommandAnswer> {
    let mut expected = vec![Witness::Group {
        group: group.clone(),
        head,
    }];
    expected.extend(
        anchors
            .iter()
            .cloned()
            .map(|task| Witness::Existence { task }),
    );
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupPut,
        expected,
        &WorkingGroupPut {
            group: WorkingGroup {
                group,
                name: name.to_owned(),
                anchors,
            },
        },
    )
}

fn delete_task(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) {
    let visible = board(bus, epoch, Actor::Operator);
    let heads = at(&visible, task);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDelete,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskDelete {
            task: task.clone(),
            reason: "fixture room unavailable".to_owned(),
            force_claimed: false,
        },
    ));
}

fn assert_anchor_resolution_and_group_removal(
    bus: &Bus,
    epoch: CanonicalUuid,
    first: &TaskHandle,
    second: &TaskHandle,
    first_born: &TaskStamp,
    second_born: &TaskStamp,
    primary: &RoomAuthorityStamp,
    secondary: &RoomAuthorityStamp,
) {
    assert_committed(correct(
        bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            task: first.clone(),
            title: None,
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            intent: None,
            priority: None,
            project: Some("board.renamed".to_owned()),
        },
    ));
    let renamed = TaskHandle::for_project("board.renamed", first_born);
    for group in [primary, secondary] {
        let view = read_group(bus, epoch, group);
        assert!(view.anchors.iter().any(|anchor| {
            matches!(
                anchor,
                WorkingGroupAnchor::Available { born, handle }
                    if born == first_born && handle == &renamed
            )
        }));
    }

    delete_task(bus, epoch, second);
    let unavailable = read_group(bus, epoch, primary);
    assert!(matches!(
        &unavailable.anchors[0],
        WorkingGroupAnchor::Unavailable { born } if born == second_born
    ));

    let before = bus
        .read({
            let first_born = first_born.clone();
            let second_born = second_born.clone();
            move |read| {
                Ok((
                    read.task_head(&first_born)?.map(|head| head.position()),
                    read.workflow_head(&first_born)?.map(|head| head.position()),
                    read.task_head(&second_born)?.map(|head| head.position()),
                    read.workflow_head(&second_born)?
                        .map(|head| head.position()),
                ))
            }
        })
        .expect("room heads read before group removal");
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupRemove,
        vec![Witness::Group {
            group: primary.clone(),
            head: Some(unavailable.head),
        }],
        &WorkingGroupRemove {
            group: primary.clone(),
        },
    ));
    let removed = read_groups(bus, epoch, Some(primary.clone()), None, 1);
    assert!(removed.groups.is_empty());
    assert_eq!(removed.unavailable, Some(primary.clone()));
    assert_eq!(read_group(bus, epoch, secondary).group, *secondary);
    let first_born = first_born.clone();
    let second_born = second_born.clone();
    let after = bus
        .read(move |read| {
            Ok((
                read.task_head(&first_born)?.map(|head| head.position()),
                read.workflow_head(&first_born)?.map(|head| head.position()),
                read.task_head(&second_born)?.map(|head| head.position()),
                read.workflow_head(&second_born)?
                    .map(|head| head.position()),
            ))
        })
        .expect("room heads read after group removal");
    assert_eq!(after, before, "group removal changes no task or room head");
}

#[test]
fn open_board_retains_only_terminal_tasks_anchored_by_current_groups() {
    let (root, bus, epoch, _seats) = fixture("grouped-terminal-open", 0);
    let terminal = super::read_scope::append_terminal_tasks(&bus, 2)
        .into_iter()
        .collect::<Vec<_>>();
    let open_stamps = |bus: &Bus| {
        let Outcome::Reply(Reply {
            body: CommandAnswer::Board(answer),
            ..
        }) = issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::BoardRead,
            Vec::new(),
            &BoardRead::Open,
        )
        else {
            panic!("open board fixture answers");
        };
        answer
            .tasks
            .iter()
            .map(|task| task.statement.body.born.clone())
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert!(open_stamps(&bus).is_empty());

    let group = group_stamp(9);
    assert_committed(put_group(
        &bus,
        epoch,
        group.clone(),
        None,
        "Settled rooms",
        vec![terminal[0].clone()],
    ));
    assert_eq!(open_stamps(&bus), [terminal[0].clone()].into());

    let head = read_group(&bus, epoch, &group).head;
    assert_committed(put_group(
        &bus,
        epoch,
        group,
        Some(head),
        "Settled rooms",
        vec![terminal[1].clone()],
    ));
    assert_eq!(open_stamps(&bus), [terminal[1].clone()].into());
    clean(&root, bus);
}

#[test]
fn working_groups_are_guarded_saved_organization_over_current_task_identity() {
    let (root, bus, epoch, _) = fixture("working-groups", 0);
    let first = add_born(&bus, epoch, stamp("group-first"), "First room");
    let second = add_born(&bus, epoch, stamp("group-second"), "Second room");
    let first_born = first.born().expect("first room identity");
    let second_born = second.born().expect("second room identity");
    let primary = group_stamp(111);
    let secondary = group_stamp(222);

    assert_committed(put_group(
        &bus,
        epoch,
        primary.clone(),
        None,
        "Planning",
        vec![first_born.clone(), second_born.clone()],
    ));
    assert_committed(put_group(
        &bus,
        epoch,
        secondary.clone(),
        None,
        "Shared room",
        vec![first_born.clone()],
    ));

    let first_page = read_groups(&bus, epoch, None, None, 1);
    assert_eq!(first_page.groups.len(), 1);
    assert_eq!(first_page.continuation, Some(primary.clone()));
    let second_page = read_groups(&bus, epoch, None, first_page.continuation, 1);
    assert_eq!(second_page.groups[0].group, secondary);
    assert_eq!(second_page.continuation, None);

    let original = read_group(&bus, epoch, &primary);
    assert_committed(put_group(
        &bus,
        epoch,
        primary.clone(),
        Some(original.head),
        "Execution order",
        vec![second_born.clone(), first_born.clone()],
    ));
    let reordered = read_group(&bus, epoch, &primary);
    assert_eq!(reordered.name, "Execution order");
    assert_eq!(
        reordered
            .anchors
            .iter()
            .map(|anchor| match anchor {
                WorkingGroupAnchor::Available { born, .. }
                | WorkingGroupAnchor::Unavailable { born } => born.clone(),
            })
            .collect::<Vec<_>>(),
        vec![second_born.clone(), first_born.clone()]
    );

    let stale = put_group(
        &bus,
        epoch,
        primary.clone(),
        Some(original.head),
        "Stale replacement",
        vec![first_born.clone()],
    );
    assert!(matches!(
        stale,
        Outcome::Refusal(refusal) if refusal.reason() == "board::witness-moved"
    ));

    assert_anchor_resolution_and_group_removal(
        &bus,
        epoch,
        &first,
        &second,
        &first_born,
        &second_born,
        &primary,
        &secondary,
    );
    clean(&root, bus);
}
