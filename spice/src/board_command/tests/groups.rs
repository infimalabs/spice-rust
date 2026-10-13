use super::*;

use wire::intent::{
    WorkingGroupAnchor, WorkingGroupChangeStatus, WorkingGroupPage, WorkingGroupRead,
    WorkingGroupView,
};
use wire::rooms::{RoomAuthorityStamp, WORKING_GROUP_DESIGN_LIMIT, WorkingGroup, WorkingGroupPut};

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

fn group_absent(bus: &Bus, epoch: CanonicalUuid, group: &RoomAuthorityStamp) -> bool {
    let page = read_groups(bus, epoch, Some(group.clone()), None, 1);
    page.groups.is_empty() && page.unavailable.as_ref() == Some(group)
}

fn borns(view: &WorkingGroupView) -> Vec<TaskStamp> {
    view.anchors
        .iter()
        .map(|anchor| match anchor {
            WorkingGroupAnchor::Available { born, .. }
            | WorkingGroupAnchor::Unavailable { born } => born.clone(),
        })
        .collect()
}

fn put_group(
    bus: &Bus,
    epoch: CanonicalUuid,
    group: RoomAuthorityStamp,
    head: Option<Seq>,
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
            group: WorkingGroup { group, anchors },
        },
    )
}

fn change_status(outcome: Outcome<CommandAnswer>) -> WorkingGroupChangeStatus {
    match outcome {
        Outcome::Reply(Reply {
            body: CommandAnswer::WorkingGroupChange(change),
            ..
        }) => change.status,
        other => panic!("working-group put did not commit: {other:?}"),
    }
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

fn room_heads(bus: &Bus, rooms: &[TaskStamp]) -> Vec<(Option<Seq>, Option<Seq>)> {
    let rooms = rooms.to_vec();
    bus.read(move |read| {
        rooms
            .iter()
            .map(|room| {
                Ok((
                    read.task_head(room)?.map(|head| head.position()),
                    read.workflow_head(room)?.map(|head| head.position()),
                ))
            })
            .collect()
    })
    .expect("room heads read")
}

fn assert_anchor_resolution_and_group_end(
    bus: &Bus,
    epoch: CanonicalUuid,
    first: &TaskHandle,
    second: &TaskHandle,
    group: &RoomAuthorityStamp,
) {
    let first_born = first.born().expect("first room identity");
    let second_born = second.born().expect("second room identity");
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
            evidence: None,
            priority: None,
            project: Some("board.renamed".to_owned()),
        },
    ));
    let renamed = TaskHandle::for_project("board.renamed", &first_born);
    assert!(read_group(bus, epoch, group).anchors.iter().any(|anchor| {
        matches!(
            anchor,
            WorkingGroupAnchor::Available { born, handle }
                if *born == first_born && *handle == renamed
        )
    }));

    delete_task(bus, epoch, second);
    let unavailable = read_group(bus, epoch, group);
    assert!(matches!(
        &unavailable.anchors[0],
        WorkingGroupAnchor::Unavailable { born } if *born == second_born
    ));

    let rooms = [first_born, second_born];
    let before = room_heads(bus, &rooms);
    assert_eq!(
        change_status(put_group(
            bus,
            epoch,
            group.clone(),
            Some(unavailable.head),
            Vec::new(),
        )),
        WorkingGroupChangeStatus::Removed
    );
    assert!(group_absent(bus, epoch, group));
    assert_eq!(
        room_heads(bus, &rooms),
        before,
        "a group's end changes no task or room head"
    );
}

#[test]
fn open_board_retains_only_terminal_tasks_anchored_by_current_groups() {
    let (root, bus, epoch, _seats) = fixture("grouped-terminal-open", 0);
    let terminal = super::read_scope::append_terminal_designs(&bus, 2)
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
        vec![terminal[0].clone()],
    ));
    assert_eq!(open_stamps(&bus), [terminal[0].clone()].into());

    let head = read_group(&bus, epoch, &group).head;
    assert_committed(put_group(
        &bus,
        epoch,
        group,
        Some(head),
        vec![terminal[1].clone()],
    ));
    assert_eq!(open_stamps(&bus), [terminal[1].clone()].into());
    clean(&root, bus);
}

#[test]
fn a_design_belongs_to_one_group_and_a_group_lives_only_while_it_holds_one() {
    let (root, bus, epoch, _) = fixture("working-groups", 0);
    let first = add_design_room(&bus, epoch, stamp("group-first"), "First room");
    let second = add_design_room(&bus, epoch, stamp("group-second"), "Second room");
    let first_born = first.born().expect("first room identity");
    let second_born = second.born().expect("second room identity");
    let primary = group_stamp(111);
    let secondary = group_stamp(222);

    assert!(refused_as(
        put_group(&bus, epoch, primary.clone(), None, Vec::new()),
        "group::design-required",
    ));
    assert!(group_absent(&bus, epoch, &primary));

    assert_committed(put_group(
        &bus,
        epoch,
        primary.clone(),
        None,
        vec![first_born.clone(), second_born.clone()],
    ));
    let original = read_group(&bus, epoch, &primary);
    assert_committed(put_group(
        &bus,
        epoch,
        secondary.clone(),
        None,
        vec![first_born.clone()],
    ));
    assert_eq!(
        borns(&read_group(&bus, epoch, &primary)),
        vec![second_born.clone()]
    );
    assert_eq!(
        borns(&read_group(&bus, epoch, &secondary)),
        vec![first_born.clone()]
    );

    let first_page = read_groups(&bus, epoch, None, None, 1);
    assert_eq!(first_page.groups.len(), 1);
    assert_eq!(first_page.continuation, Some(primary.clone()));
    let second_page = read_groups(&bus, epoch, None, first_page.continuation, 1);
    assert_eq!(second_page.groups[0].group, secondary);
    assert_eq!(second_page.continuation, None);

    assert!(refused_as(
        put_group(
            &bus,
            epoch,
            primary.clone(),
            Some(original.head),
            vec![first_born.clone(), second_born.clone()],
        ),
        "board::witness-moved",
    ));

    let current = read_group(&bus, epoch, &primary);
    assert_eq!(
        change_status(put_group(
            &bus,
            epoch,
            primary.clone(),
            Some(current.head),
            vec![second_born.clone(), first_born.clone()],
        )),
        WorkingGroupChangeStatus::Stored
    );
    assert_eq!(
        borns(&read_group(&bus, epoch, &primary)),
        vec![second_born.clone(), first_born.clone()]
    );
    assert!(
        group_absent(&bus, epoch, &secondary),
        "a group whose last design moved away ends"
    );

    assert_anchor_resolution_and_group_end(&bus, epoch, &first, &second, &primary);
    clean(&root, bus);
}

fn add_design_room(bus: &Bus, epoch: CanonicalUuid, born: TaskStamp, title: &str) -> TaskHandle {
    let task = TaskHandle::for_project("board.identity", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born.clone() }],
        &TaskAdd {
            preparation: None,
            intent: Some(wire::TaskIntent::Design),
            title: title.to_owned(),
            project: Some("board.identity".to_owned()),
            description: None,
            acceptance: vec!["The design records its disposition".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some("fixture:identity".parse().expect("fixture origin")),
            flow: Some("design,review".to_owned()),
            depends: Vec::new(),
            claim: false,
        },
    ));
    task
}

fn refused_as(outcome: Outcome<CommandAnswer>, reason: &str) -> bool {
    matches!(outcome, Outcome::Refusal(refusal) if refusal.reason() == reason)
}

#[test]
fn working_groups_admit_at_most_six_designs_and_nothing_but_designs() {
    let (root, bus, epoch, _) = fixture("group-design-limit", 0);
    let designs = (0..=WORKING_GROUP_DESIGN_LIMIT)
        .map(|index| {
            add_design_room(&bus, epoch, stamp(&format!("design-{index}")), "Design")
                .born()
                .expect("design identity")
        })
        .collect::<Vec<_>>();
    let plain = add_born(&bus, epoch, stamp("plain-room"), "Plain task")
        .born()
        .expect("plain identity");
    let six = &designs[..WORKING_GROUP_DESIGN_LIMIT];

    let group = group_stamp(333);
    assert_committed(put_group(&bus, epoch, group.clone(), None, six.to_vec()));
    let full = read_group(&bus, epoch, &group);
    assert!(refused_as(
        put_group(&bus, epoch, group.clone(), Some(full.head), designs.clone()),
        "group::design-limit",
    ));
    let mut swapped = designs[..WORKING_GROUP_DESIGN_LIMIT - 1].to_vec();
    swapped.push(plain.clone());
    assert!(refused_as(
        put_group(&bus, epoch, group.clone(), Some(full.head), swapped),
        "group::design-only",
    ));
    assert_eq!(read_group(&bus, epoch, &group), full);
    assert!(refused_as(
        put_group(&bus, epoch, group_stamp(444), None, designs.clone()),
        "group::design-limit",
    ));
    assert!(refused_as(
        put_group(&bus, epoch, group_stamp(555), None, vec![plain]),
        "group::design-only",
    ));
    assert_eq!(read_group(&bus, epoch, &group), full);
    clean(&root, bus);
}
