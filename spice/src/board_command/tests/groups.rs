use super::*;

use wire::intent::{
    WorkingGroupAnchor, WorkingGroupChangeStatus, WorkingGroupPage, WorkingGroupRead,
};
use wire::rooms::{
    RoomAuthorityStamp, WORKING_GROUP_DESIGN_LIMIT, WorkingGroup, WorkingGroupArrange,
    WorkingGroupDisband, WorkingGroupFast, WorkingGroupMove, WorkingGroupStance,
    WorkingGroupStanceSet,
};

use super::preparation::{
    add_controller, add_plan_output, authorize_test_output, conclude, done_conclusion,
    record_test_coverage, retain_test_candidate,
};
use crate::board_command::group_fast_lanes;

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
                    stance: wire::rooms::WorkingGroupStance::Explore,
                })
                .collect(),
        },
    )
}

fn disband_at(
    bus: &Bus,
    epoch: CanonicalUuid,
    revision: u64,
    group: &RoomAuthorityStamp,
) -> Outcome<CommandAnswer> {
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupDisband,
        vec![Witness::Group { revision }],
        &WorkingGroupDisband {
            group: group.clone(),
        },
    )
}

fn disband(bus: &Bus, epoch: CanonicalUuid, group: &RoomAuthorityStamp) -> Outcome<CommandAnswer> {
    disband_at(bus, epoch, read_partition(bus, epoch).revision, group)
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
            pitch: None,
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            intent: None,
            priority: None,
            project: Some("board.renamed".to_owned()),
            flow: None,
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
            WorkingGroupAnchor::Available { born, handle, .. }
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
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.identity".to_owned()),
            description: None,
            acceptance: vec!["The design records its disposition".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: Some("design".to_owned()),
            depends: Vec::new(),
            claim: false,
            draft: false,
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

#[test]
fn a_completed_design_joins_a_group_while_a_deleted_one_stays_out() {
    let (root, bus, epoch, _) = fixture("group-completed-design", 0);
    let completed = add_design_room(&bus, epoch, stamp("completed-design"), "Completed");
    let deleted = add_design_room(&bus, epoch, stamp("deleted-design"), "Deleted");
    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &completed,
        done_conclusion(),
    ));
    delete_task(&bus, epoch, &deleted);
    let completed = completed.born().expect("completed identity");
    let deleted = deleted.born().expect("deleted identity");

    let group = group_stamp(555);
    assert_committed(move_design(&bus, epoch, &completed, Some(&group)));
    assert!(refused_as(
        move_design(&bus, epoch, &deleted, Some(&group)),
        "group::design-only",
    ));
    assert_eq!(standing(&bus, epoch), vec![(group, vec![completed])]);
    clean(&root, bus);
}

#[test]
fn disbanding_a_group_leaves_every_design_ungrouped_under_the_partition_guard() {
    let (root, bus, epoch, _) = fixture("group-disband", 0);
    let first = add_design_room(&bus, epoch, stamp("disband-first"), "First room");
    let second = add_design_room(&bus, epoch, stamp("disband-second"), "Second room");
    let first_born = first.born().expect("first room identity");
    let second_born = second.born().expect("second room identity");
    let group = group_stamp(449);
    assert_committed(move_design(&bus, epoch, &first_born, Some(&group)));
    assert_committed(move_design(&bus, epoch, &second_born, Some(&group)));
    assert_committed(set_stance(&bus, epoch, &group, WorkingGroupStance::Explore));
    assert_committed(set_fast(&bus, epoch, &group, true));
    let observed = read_partition(&bus, epoch).revision;
    let before_room_heads = room_heads(&bus, &[first_born.clone(), second_born.clone()]);

    assert_eq!(
        change_status(disband(&bus, epoch, &group)),
        WorkingGroupChangeStatus::Disbanded
    );
    assert!(standing(&bus, epoch).is_empty());
    let visible = board(&bus, epoch, Actor::Operator);
    for design in [&first, &second] {
        let row = at(&visible, design);
        assert!(!row.grouped);
        assert!(!row.design_active);
    }
    assert_eq!(
        room_heads(&bus, &[first_born, second_born]),
        before_room_heads,
        "disbanding unheld Designs changes no room authority"
    );
    assert!(refused_as(disband(&bus, epoch, &group), "group::missing"));
    assert!(refused_as(
        disband_at(&bus, epoch, observed, &group),
        "group::partition-moved"
    ));
    clean(&root, bus);
}

fn set_stance_at(
    bus: &Bus,
    epoch: CanonicalUuid,
    revision: u64,
    group: &RoomAuthorityStamp,
    stance: WorkingGroupStance,
) -> Outcome<CommandAnswer> {
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupStance,
        vec![Witness::Group { revision }],
        &WorkingGroupStanceSet {
            group: group.clone(),
            stance,
        },
    )
}

fn set_stance(
    bus: &Bus,
    epoch: CanonicalUuid,
    group: &RoomAuthorityStamp,
    stance: WorkingGroupStance,
) -> Outcome<CommandAnswer> {
    set_stance_at(
        bus,
        epoch,
        read_partition(bus, epoch).revision,
        group,
        stance,
    )
}

#[test]
fn a_group_stance_controls_design_activity_under_the_partition_guard() {
    let (root, bus, epoch, _) = fixture("group-stance", 1);
    let design = add_controller(
        &bus,
        epoch,
        "Stanced design",
        wire::TaskIntent::Design,
        None,
    );
    let born = design.born().expect("design identity");
    let group = group_stamp(450);
    assert_committed(move_design(&bus, epoch, &born, Some(&group)));

    let page = read_partition(&bus, epoch);
    assert_eq!(page.groups[0].stance, WorkingGroupStance::Passive);
    let visible = board(&bus, epoch, Actor::Operator);
    let held = at(&visible, &design);
    assert!(held.grouped);
    assert!(!held.design_active);

    let founded_revision = page.revision;
    assert_eq!(
        change_status(set_stance(&bus, epoch, &group, WorkingGroupStance::Explore)),
        WorkingGroupChangeStatus::Stanced
    );
    assert!(at(&board(&bus, epoch, Actor::Operator), &design).design_active);
    assert_committed(set_fast(&bus, epoch, &group, true));
    assert_committed(set_stance(
        &bus,
        epoch,
        &group,
        WorkingGroupStance::Converge,
    ));
    let partition = bus
        .read(|read| read.group_partition())
        .expect("partition reads")
        .expect("partition stands")
        .into_body();
    assert_eq!(partition.groups[0].stance, WorkingGroupStance::Converge);
    assert!(partition.is_fast(&group));
    assert!(at(&board(&bus, epoch, Actor::Operator), &design).design_active);

    assert!(refused_as(
        set_stance(&bus, epoch, &group, WorkingGroupStance::Converge),
        "group::stance-unchanged"
    ));
    assert!(refused_as(
        set_stance(&bus, epoch, &group_stamp(451), WorkingGroupStance::Passive),
        "group::missing"
    ));
    assert!(refused_as(
        set_stance_at(
            &bus,
            epoch,
            founded_revision,
            &group,
            WorkingGroupStance::Passive
        ),
        "group::partition-moved"
    ));
    clean(&root, bus);
}

fn set_fast(
    bus: &Bus,
    epoch: CanonicalUuid,
    group: &RoomAuthorityStamp,
    fast_mode: bool,
) -> Outcome<CommandAnswer> {
    let revision = read_partition(bus, epoch).revision;
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupFast,
        vec![Witness::Group { revision }],
        &WorkingGroupFast {
            group: group.clone(),
            fast_mode,
        },
    )
}

/// Record that every seat's vendor refused fast mode for the model it runs.
fn refuse_fast(bus: &Bus, epoch: CanonicalUuid) {
    let observed = bus
        .task_snapshot()
        .expect("fixture residency snapshot reads")
        .residency()
        .expect("fixture residency exists")
        .clone();
    let mut refused = observed.body().clone();
    for row in &mut refused.lanes {
        let occupant = row.occupant.expect("fixture seat is occupied");
        let mut launch = wire::body::LaneLaunchIdentity::new("mockgw/gpt-5.4", "high");
        launch.fast_refusal = Some("priority refused".to_owned());
        row.session = Some(wire::LaneSession {
            lane: row.lane,
            occupant,
            worktree: row.worktree.clone(),
            vendor: row.vendor.clone(),
            launch,
            epoch,
            stream: "stdout".to_owned(),
            identity: format!("session-{}", row.lane),
            locus: wire::TapeLocus {
                member: 1,
                offset: 0,
                length: 1,
            },
        });
    }
    let first = &refused.lanes[0];
    let retuned = wire::body::LaneRuntimeRetuned {
        lane: first.lane,
        occupant: first.occupant.expect("fixture seat is occupied"),
        model: None,
        effort: None,
        fast_mode: Some(false),
        fast_refusal: Some("priority refused".to_owned()),
    };
    let origin = format!("resident:{epoch}")
        .parse()
        .expect("resident origin");
    let at = Instant::new(jiff::Timestamp::now()).expect("fixture instant");
    let mut write = JournalWrite::new(Actor::Spice, origin, at, Producer::ResidentRecognition);
    let retuned = write
        .fact(retuned, FactCause::Existing(observed.position()))
        .expect("fixture refusal builds");
    write.residency_head(retuned, move |_| refused);
    assert!(matches!(
        bus.commit(write).expect("fixture refusal reaches writer"),
        CommitOutcome::Committed(_)
    ));
}

fn refusing(bus: &Bus) -> std::collections::BTreeSet<CanonicalUuid> {
    bus.residency_head()
        .expect("residency reads")
        .expect("residency exists")
        .body()
        .lanes
        .iter()
        .filter(|row| row.refuses_fast_mode())
        .map(|row| row.lane)
        .collect()
}

#[test]
fn a_fast_flagged_group_runs_claimants_of_its_design_associated_work() {
    let (root, bus, epoch, seats) = fixture("group-fast", 4);
    let design = |name| add_controller(&bus, epoch, name, wire::TaskIntent::Design, None);
    let controller = design("Fast controller");
    group_design(&bus, epoch, &controller);
    claim_task(
        &bus,
        epoch,
        &seats[0],
        &Actor::Lane(seats[0].lane),
        &controller,
    );
    let plan = add_plan_output(&bus, epoch, &controller, "Fast selected plan");
    let spare = add_plan_output(&bus, epoch, &controller, "Fast spare plan");
    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats[..2], &controller, &plan, &spare);
    record_test_coverage(&bus, epoch, &seats[..2], &controller, &candidate, &source);
    authorize_test_output(&bus, epoch, &controller, &plan, &spare, candidate);
    let (flagged, unflagged) = (design("Fast design"), design("Slow design"));
    let (fast, slow) = (group_stamp(500), group_stamp(600));
    for (task, group) in [(&controller, &fast), (&flagged, &fast), (&unflagged, &slow)] {
        let born = task.born().expect("design identity");
        assert_committed(move_design(&bus, epoch, &born, Some(group)));
    }
    assert_committed(set_stance(&bus, epoch, &fast, WorkingGroupStance::Explore));
    assert_committed(set_stance(&bus, epoch, &slow, WorkingGroupStance::Explore));
    let current = board(&bus, epoch, Actor::Operator);
    for task in [&flagged, &unflagged] {
        assert_committed(nudge_design(&bus, epoch, Actor::Operator, &current, task));
    }
    assert!(refused_as(
        set_fast(&bus, epoch, &group_stamp(700), true),
        "group::missing"
    ));
    assert_committed(set_fast(&bus, epoch, &fast, true));
    let fast_lanes = || bus.read(group_fast_lanes).expect("group fast lanes read");
    assert!(
        fast_lanes().is_empty(),
        "a flag runs no lane without a claim"
    );

    capture_for(&bus, epoch, &seats[0], &flagged);
    capture_for(&bus, epoch, &seats[1], &unflagged);
    capture_for(&bus, epoch, &seats[2], &plan);
    assert_eq!(
        fast_lanes(),
        [seats[0].lane, seats[2].lane].into(),
        "the flagged Design and its selected Plan run fast while another group's claimant and an idle lane do not"
    );

    refuse_fast(&bus, epoch);
    assert_committed(set_fast(&bus, epoch, &fast, false));
    assert!(
        fast_lanes().is_empty(),
        "lowering the flag releases its claimant"
    );
    let every = seats.iter().map(|seat| seat.lane).collect();
    assert_eq!(
        refusing(&bus),
        every,
        "lowering the flag keeps every refusal"
    );
    assert!(refused_as(
        set_fast(&bus, epoch, &fast, false),
        "group::fast-unchanged"
    ));
    assert_committed(set_fast(&bus, epoch, &fast, true));
    assert_eq!(
        refusing(&bus),
        [seats[1].lane, seats[3].lane].into(),
        "raising the flag retries fast mode on its claimants alone"
    );
    clean(&root, bus);
}
