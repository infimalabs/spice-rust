//! A Design pass proposal resting on acceptance lent by completed linked work,
//! witnessed from the dependency-scoped read the propose client makes.

use super::*;

/// An open Design that states no acceptance of its own.
fn add_unaccepted_design(bus: &Bus, epoch: CanonicalUuid, name: &str) -> TaskHandle {
    let born = stamp(name);
    let design = TaskHandle::for_project("board.test", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            preparation: None,
            intent: Some(wire::TaskIntent::Design),
            title: name.to_owned(),
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: Some("design".to_owned()),
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    ));
    stand_design(bus, epoch, &design);
    design
}

/// Complete `task`'s held single phase from `seat`.
fn finish(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, task: &TaskHandle) {
    let held = board(bus, epoch, Actor::Lane(seat.lane));
    assert_committed(issue(
        bus,
        epoch,
        Actor::Lane(seat.lane),
        CommandKind::TaskDone,
        progress_witnesses(&held, at(&held, task), seat.lane, seat.occupant, false)
            .expect("held work has progress witnesses"),
        &TaskDone {
            task: task.clone(),
            validation: "the accepted behavior is verified".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    ));
}

/// Propose concluding `design` from `seat`, witnessed as the propose client
/// witnesses it: the Design's own heads from the lane's board and, unless
/// `borrowed` is false, the lenders its dependency-scoped read projects.
fn propose_conclusion(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    design: &TaskHandle,
    micros: i64,
    borrowed: bool,
) -> Outcome<CommandAnswer> {
    let lane = Actor::Lane(seat.lane);
    let held = board(bus, epoch, lane.clone());
    let heads = at(&held, design);
    let born = design.born().expect("design stamp");
    let candidate = CandidateRef {
        controller: born.clone(),
        candidate: RoomAuthorityStamp::for_micros(micros).expect("candidate identity"),
    };
    let mut expected = transition_witnesses(heads);
    if borrowed {
        let scope = match issue(
            bus,
            epoch,
            lane.clone(),
            CommandKind::BoardRead,
            Vec::new(),
            &BoardRead::Tasks {
                tasks: vec![born.clone()],
                dependencies: true,
                claims: false,
                history: false,
            },
        ) {
            Outcome::Reply(Reply {
                body: CommandAnswer::Board(answer),
                ..
            }) => *answer,
            outcome => panic!("scoped Design read failed: {outcome:?}"),
        };
        acceptance_witnesses(&scope, at(&scope, design), &mut expected)
            .expect("the scoped read carries every linked task");
    }
    expected.push(Witness::Proposal {
        proposal: ProposalRef::Candidate {
            candidate: candidate.clone(),
        },
        head: None,
    });
    let room = born.clone();
    expected.push(Witness::Room {
        room: born,
        head: bus
            .read(move |read| read.room_publication_head(&room))
            .expect("room head reads"),
    });
    expected.push(residency(&held, seat));
    issue(
        bus,
        epoch,
        lane,
        CommandKind::DesignPropose,
        expected,
        &DesignPropose {
            candidate: DesignPassProposal {
                candidate,
                statement: heads.statement.position,
                claim: heads.claim.as_ref().map(|claim| claim.position),
                recommendation: wire::rooms::DesignPassRecommendation::Conclude,
                governed_inputs: Vec::new(),
                outputs: Vec::new(),
                justification: "the linked work settles this Design".to_owned(),
                references: vec![CandidateReference {
                    relation: "contract".parse().expect("relation"),
                    source: journal_source(&held, design),
                }],
            },
        },
    )
}

#[test]
fn a_design_proposes_on_acceptance_from_completed_work_linked_either_way() {
    let (root, bus, epoch, seats) = fixture("design-linked-acceptance", 2);
    let worker = Actor::Lane(seats[1].lane);
    let forward = add_unaccepted_design(&bus, epoch, "Forward Design");
    let reverse = add_unaccepted_design(&bus, epoch, "Reverse Design");

    // A completed prerequisite carrying acceptance lends it to the Design.
    let prerequisite = add_project_with_acceptance(
        &bus,
        epoch,
        "LINKED-prereq",
        "Accepted prerequisite",
        "board.test",
        Some("todo"),
        &["the prerequisite behavior is verified"],
    );
    claim_task(&bus, epoch, &seats[1], &worker, &prerequisite);
    finish(&bus, epoch, &seats[1], &prerequisite);
    connect_child(&bus, epoch, &forward, &prerequisite);

    // So does completed work that came to depend on the Design.
    let dependent = add_project_with_acceptance(
        &bus,
        epoch,
        "LINKED-depend",
        "Accepted dependent",
        "board.test",
        Some("todo"),
        &["the dependent behavior is verified"],
    );
    claim_task(&bus, epoch, &seats[1], &worker, &dependent);
    connect_child(&bus, epoch, &dependent, &reverse);
    finish(&bus, epoch, &seats[1], &dependent);
    let settled = board(&bus, epoch, Actor::Operator);
    for lender in [&prerequisite, &dependent] {
        assert!(at(&settled, lender).workflow.body.status.is_completed());
    }

    capture_for(&bus, epoch, &seats[0], &forward);
    let unwitnessed = propose_conclusion(
        &bus,
        epoch,
        &seats[0],
        &forward,
        1_760_000_000_000_301,
        false,
    );
    assert!(
        matches!(&unwitnessed, Outcome::Refusal(refusal) if refusal.reason() == "task::acceptance-witness-missing"),
        "{unwitnessed:?}"
    );
    assert_committed(propose_conclusion(
        &bus,
        epoch,
        &seats[0],
        &forward,
        1_760_000_000_000_302,
        true,
    ));

    capture_for(&bus, epoch, &seats[0], &reverse);
    assert_committed(propose_conclusion(
        &bus,
        epoch,
        &seats[0],
        &reverse,
        1_760_000_000_000_303,
        true,
    ));
    let proposed = board(&bus, epoch, Actor::Operator);
    for design in [&forward, &reverse] {
        assert!(at(&proposed, design).design_cycle.proposal.is_some());
    }
    clean(&root, bus);
}

#[test]
fn a_design_without_any_acceptance_cannot_propose() {
    let (root, bus, epoch, seats) = fixture("design-missing-acceptance", 1);
    let design = add_unaccepted_design(&bus, epoch, "Unaccepted Design");
    capture_for(&bus, epoch, &seats[0], &design);
    let refused = propose_conclusion(&bus, epoch, &seats[0], &design, 1_760_000_000_000_311, true);
    assert!(
        matches!(&refused, Outcome::Refusal(refusal) if refusal.reason() == "task::acceptance-missing"),
        "{refused:?}"
    );
    let view = board(&bus, epoch, Actor::Operator);
    let heads = at(&view, &design);
    assert!(heads.design_cycle.proposal.is_none());
    assert!(heads.claim.as_ref().is_some_and(|claim| {
        claim.body.disposition == ClaimDisposition::Active
            && claim.body.actor == Actor::Lane(seats[0].lane)
    }));
    clean(&root, bus);
}
