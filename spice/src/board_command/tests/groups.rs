use super::*;

use wire::intent::{
    WorkingGroupAnchor, WorkingGroupChangeStatus, WorkingGroupPage, WorkingGroupRead,
};
use wire::rooms::{
    RoomAuthorityStamp, WORKING_GROUP_DESIGN_LIMIT, WorkingGroup, WorkingGroupArrange,
    WorkingGroupMove,
};

fn group_stamp(offset: i64) -> RoomAuthorityStamp {
    RoomAuthorityStamp::for_micros(1_760_000_000_000_000 + offset).expect("fixture group stamp")
}

fn read_partition(bus: &Bus, epoch: CanonicalUuid) -> WorkingGroupPage {
    let outcome = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupRead,
        Vec::new(),
        &WorkingGroupRead {},
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

/// Every group in partition order with its designs in order.
fn standing(bus: &Bus, epoch: CanonicalUuid) -> Vec<(RoomAuthorityStamp, Vec<TaskStamp>)> {
    read_partition(bus, epoch)
        .groups
        .into_iter()
        .map(|group| {
            let borns = group
                .anchors
                .into_iter()
                .map(|anchor| match anchor {
                    WorkingGroupAnchor::Available { born, .. }
                    | WorkingGroupAnchor::Unavailable { born } => born,
                })
                .collect();
            (group.group, borns)
        })
        .collect()
}

fn move_at(
    bus: &Bus,
    epoch: CanonicalUuid,
    revision: u64,
    design: &TaskStamp,
    to: Option<&RoomAuthorityStamp>,
) -> Outcome<CommandAnswer> {
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupMove,
        vec![
            Witness::Group { revision },
            Witness::Existence {
                task: design.clone(),
            },
        ],
        &WorkingGroupMove {
            design: design.clone(),
            to: to.cloned(),
        },
    )
}

/// Move `design` at the current revision.
fn move_design(
    bus: &Bus,
    epoch: CanonicalUuid,
    design: &TaskStamp,
    to: Option<&RoomAuthorityStamp>,
) -> Outcome<CommandAnswer> {
    let revision = read_partition(bus, epoch).revision;
    move_at(bus, epoch, revision, design, to)
}

/// Arrange the partition at the current revision.
fn arrange(
    bus: &Bus,
    epoch: CanonicalUuid,
    groups: &[(&RoomAuthorityStamp, &[&TaskStamp])],
) -> Outcome<CommandAnswer> {
    let revision = read_partition(bus, epoch).revision;
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupArrange,
        vec![Witness::Group { revision }],
        &WorkingGroupArrange {
            groups: groups
                .iter()
                .map(|(group, anchors)| WorkingGroup {
                    group: (*group).clone(),
                    anchors: anchors.iter().map(|anchor| (*anchor).clone()).collect(),
                })
                .collect(),
        },
    )
}

fn change_status(outcome: Outcome<CommandAnswer>) -> WorkingGroupChangeStatus {
    match outcome {
        Outcome::Reply(Reply {
            body: CommandAnswer::WorkingGroupChange(change),
            ..
        }) => change.status,
        other => panic!("working-group change did not commit: {other:?}"),
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
    let anchors = || {
        read_partition(bus, epoch)
            .groups
            .into_iter()
            .flat_map(|group| group.anchors)
            .collect::<Vec<_>>()
    };
    assert!(anchors().iter().any(|anchor| {
        matches!(
            anchor,
            WorkingGroupAnchor::Available { born, handle }
                if *born == first_born && *handle == renamed
        )
    }));

    delete_task(bus, epoch, second);
    assert!(anchors().iter().any(|anchor| matches!(
        anchor,
        WorkingGroupAnchor::Unavailable { born } if *born == second_born
    )));

    let rooms = [first_born.clone(), second_born.clone()];
    let before = room_heads(bus, &rooms);
    assert_eq!(
        change_status(move_design(bus, epoch, &second_born, None)),
        WorkingGroupChangeStatus::Moved
    );
    assert_eq!(
        change_status(move_design(bus, epoch, &first_born, None)),
        WorkingGroupChangeStatus::Ended
    );
    assert!(standing(bus, epoch).is_empty());
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
    assert_committed(move_design(&bus, epoch, &terminal[0], Some(&group)));
    assert_eq!(open_stamps(&bus), [terminal[0].clone()].into());

    assert_committed(move_design(&bus, epoch, &terminal[1], Some(&group)));
    assert_committed(move_design(&bus, epoch, &terminal[0], None));
    assert_eq!(open_stamps(&bus), [terminal[1].clone()].into());
    clean(&root, bus);
}

#[test]
fn a_design_moves_between_groups_and_a_group_lives_only_while_it_holds_one() {
    let (root, bus, epoch, _) = fixture("working-groups", 0);
    let first = add_design_room(&bus, epoch, stamp("group-first"), "First room");
    let second = add_design_room(&bus, epoch, stamp("group-second"), "Second room");
    let first_born = first.born().expect("first room identity");
    let second_born = second.born().expect("second room identity");
    let primary = group_stamp(111);
    let secondary = group_stamp(222);

    assert!(refused_as(
        move_design(&bus, epoch, &first_born, None),
        "group::design-unchanged",
    ));
    assert_eq!(
        change_status(move_design(&bus, epoch, &first_born, Some(&primary))),
        WorkingGroupChangeStatus::Moved
    );
    assert_committed(move_design(&bus, epoch, &second_born, Some(&primary)));
    let observed = read_partition(&bus, epoch).revision;
    assert_committed(move_design(&bus, epoch, &first_born, Some(&secondary)));
    assert_eq!(
        standing(&bus, epoch),
        vec![
            (secondary.clone(), vec![first_born.clone()]),
            (primary.clone(), vec![second_born.clone()]),
        ],
        "a founded group takes its design's place in the partition order"
    );

    assert!(refused_as(
        move_at(&bus, epoch, observed, &second_born, Some(&secondary)),
        "group::partition-moved",
    ));
    assert_eq!(
        change_status(move_design(&bus, epoch, &first_born, Some(&primary))),
        WorkingGroupChangeStatus::Ended,
        "a group whose last design moved away ends"
    );
    assert_eq!(
        standing(&bus, epoch),
        vec![(
            primary.clone(),
            vec![second_born.clone(), first_born.clone()]
        )]
    );
    assert!(refused_as(
        move_design(&bus, epoch, &first_born, Some(&primary)),
        "group::design-unchanged",
    ));

    assert_eq!(
        change_status(arrange(
            &bus,
            epoch,
            &[(&primary, &[&first_born, &second_born])]
        )),
        WorkingGroupChangeStatus::Arranged
    );
    assert_eq!(
        standing(&bus, epoch),
        vec![(
            primary.clone(),
            vec![first_born.clone(), second_born.clone()]
        )]
    );
    let third = add_design_room(&bus, epoch, stamp("group-third"), "Third room")
        .born()
        .expect("third room identity");
    for (groups, reason) in [
        (
            vec![(&primary, &[&first_born][..])],
            "group::arrangement-incomplete",
        ),
        (
            vec![
                (&primary, &[&first_born, &second_born][..]),
                (&secondary, &[&first_born][..]),
            ],
            "group::arrangement-duplicate-design",
        ),
        (
            vec![(&primary, &[&first_born, &second_born, &third][..])],
            "group::arrangement-unknown-design",
        ),
        (vec![(&primary, &[][..])], "group::arrangement-vacant-group"),
    ] {
        assert!(
            refused_as(arrange(&bus, epoch, &groups), reason),
            "{reason}"
        );
    }
    assert_eq!(
        standing(&bus, epoch),
        vec![(primary, vec![first_born.clone(), second_born.clone()])]
    );

    assert_anchor_resolution_and_group_end(&bus, epoch, &first, &second);
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

    let group = group_stamp(333);
    for design in &designs[..WORKING_GROUP_DESIGN_LIMIT] {
        assert_committed(move_design(&bus, epoch, design, Some(&group)));
    }
    let full = read_partition(&bus, epoch);
    assert!(refused_as(
        move_design(
            &bus,
            epoch,
            &designs[WORKING_GROUP_DESIGN_LIMIT],
            Some(&group)
        ),
        "group::design-limit",
    ));
    assert!(refused_as(
        move_design(&bus, epoch, &plain, Some(&group)),
        "group::design-only",
    ));
    assert!(refused_as(
        move_design(&bus, epoch, &plain, Some(&group_stamp(444))),
        "group::design-only",
    ));
    assert_eq!(read_partition(&bus, epoch), full);
    clean(&root, bus);
}
