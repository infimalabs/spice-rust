//! The proposal `design propose` derives from the Design its caller
//! holds, and the resident refusing it once a witnessed head has moved.

use super::contribution::{contribute, contributed, contribution_body};
use super::*;

use crate::board_command::client::{derive_proposal, held_design};
use crate::board_command::tests::room::design_standing::win_named;
use wire::rooms::RoomContributionKind;

/// The proposal the client derives from `board` for the Design it holds.
fn derived(board: &BoardAnswer, output: &TaskHandle, micros: i64) -> DesignPassProposal {
    let design = held_design(board, None).expect("one held Design");
    derive_proposal(
        design,
        wire::rooms::DesignPassBody {
            recommendation: wire::rooms::DesignPassRecommendation::Work,
            governed_inputs: Vec::new(),
            outputs: vec![output.born().expect("output stamp")],
            justification: "the cited contract supports this Design pass".to_owned(),
            references: vec![CandidateReference {
                relation: "contract".parse().expect("relation"),
                source: journal_source(board, &design.statement.body.handle),
            }],
        },
        RoomAuthorityStamp::for_micros(micros).expect("candidate identity"),
    )
    .expect("a held Design derives its proposal")
}

/// `design`'s newest room publication, as the client reads it to assemble.
fn room_head(bus: &Bus, design: &TaskHandle) -> Option<wire::Seq> {
    let room = design.born().expect("design stamp");
    bus.read(move |read| read.room_publication_head(&room))
        .expect("room head reads")
}

/// Issue `proposal` from `seat` with witnesses read from the current board
/// and the room witnessed at `room`.
fn propose(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    design: &TaskHandle,
    output: &TaskHandle,
    proposal: DesignPassProposal,
    room: Option<wire::Seq>,
) -> Outcome<CommandAnswer> {
    let current = board(bus, epoch, Actor::Lane(seat.lane));
    let mut expected = vec![Witness::Proposal {
        proposal: ProposalRef::Candidate {
            candidate: proposal.candidate.clone(),
        },
        head: None,
    }];
    expected.extend(transition_witnesses(at(&current, design)));
    expected.extend(transition_witnesses(at(&current, output)));
    expected.push(Witness::Room {
        room: design.born().expect("design stamp"),
        head: room,
    });
    expected.push(residency(&current, seat));
    issue(
        bus,
        epoch,
        Actor::Lane(seat.lane),
        CommandKind::DesignPropose,
        expected,
        &DesignPropose {
            candidate: proposal,
        },
    )
}

/// Propose `proposal` witnessing the room as read before a late contribution,
/// which refuses as a moved witness, then again witnessing the current room.
fn propose_past_late_contribution(
    bus: &Bus,
    epoch: CanonicalUuid,
    root: &Path,
    seat: &Seat,
    design: &TaskHandle,
    output: &TaskHandle,
    proposal: DesignPassProposal,
) -> Outcome<CommandAnswer> {
    let assembled_at = room_head(bus, design);
    contributed(contribute(
        bus,
        epoch,
        root,
        Actor::Lane(seat.lane),
        contribution_body(design, RoomContributionKind::charge(1), "Late.", Vec::new()),
    ));
    let moved = propose(
        bus,
        epoch,
        seat,
        design,
        output,
        proposal.clone(),
        assembled_at,
    );
    assert!(
        matches!(&moved, Outcome::Refusal(refusal) if refusal.reason() == "board::witness-moved"),
        "{moved:?}"
    );
    propose(
        bus,
        epoch,
        seat,
        design,
        output,
        proposal,
        room_head(bus, design),
    )
}

#[test]
fn a_derived_proposal_witnesses_the_heads_it_read_and_refuses_once_they_move() {
    let (root, bus, epoch, seats) = fixture("preparation-derived-proposal", 1);
    let lane = Actor::Lane(seats[0].lane);
    let design = add_controller(
        &bus,
        epoch,
        "Derived Design",
        wire::TaskIntent::Design,
        None,
    );
    stand_design(&bus, epoch, &design);
    let output = add_plan_output(&bus, epoch, &design, "Derived output");

    // Holding no Design derives nothing, with or without a name.
    let unheld = board(&bus, epoch, lane.clone());
    for named in [None, Some(&design)] {
        assert!(matches!(
            held_design(&unheld, named),
            Err(Outcome::Refusal(refusal)) if refusal.reason() == "preparation::design-unheld"
        ));
    }

    capture_for(&bus, epoch, &seats[0], &design);
    let held = board(&bus, epoch, lane.clone());
    let heads = at(&held, &design);
    let proposal = derived(&held, &output, 1_760_000_000_000_201);
    assert_eq!(proposal.candidate.controller, heads.statement.body.born);
    assert_eq!(proposal.statement, heads.statement.position);
    assert_eq!(
        proposal.claim,
        heads.claim.as_ref().map(|claim| claim.position)
    );
    assert_eq!(
        held_design(&held, Some(&design))
            .expect("the named Design is held")
            .statement
            .position,
        heads.statement.position
    );

    // The operator rewrites the Design: the witnessed statement head moved.
    assert_committed(super::super::edit::correct(
        &bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            description: Some("the question, restated".to_owned()),
            ..super::super::edit::only(&design)
        },
    ));
    assert!(matches!(
        propose(&bus, epoch, &seats[0], &design, &output, proposal, room_head(&bus, &design)),
        Outcome::Refusal(refusal)
            if refusal.reason() == "preparation::proposal-statement-moved"
    ));

    // Derived again from the edited heads, the proposal names the current
    // statement and the same active claim, but that claim has not
    // acknowledged the Design's new generation.
    let edited = board(&bus, epoch, lane.clone());
    let stale = derived(&edited, &output, 1_760_000_000_000_202);
    assert_eq!(stale.statement, at(&edited, &design).statement.position);
    let stale_outcome = propose(
        &bus,
        epoch,
        &seats[0],
        &design,
        &output,
        stale.clone(),
        room_head(&bus, &design),
    );
    assert!(
        matches!(&stale_outcome, Outcome::Refusal(refusal) if refusal.reason() == "preparation::proposal-claim-stale"),
        "{stale_outcome:?}"
    );

    // The refresh moves the claim head: a proposal witnessing the earlier
    // claim no longer names the active one.
    refresh_held(&bus, epoch, &lane, &seats[0], &design);
    let current = board(&bus, epoch, lane.clone());
    assert_ne!(
        stale.claim,
        at(&current, &design)
            .claim
            .as_ref()
            .map(|claim| claim.position)
    );
    assert!(matches!(
        propose(&bus, epoch, &seats[0], &design, &output, stale, room_head(&bus, &design)),
        Outcome::Refusal(refusal) if refusal.reason() == "preparation::proposal-claim-moved"
    ));

    // A contribution landing after the room was read refuses the write until
    // it witnesses the current room; then the pass records and frees the claim.
    let fresh = derived(&current, &output, 1_760_000_000_000_204);
    assert_committed(propose_past_late_contribution(
        &bus,
        epoch,
        root.as_ref(),
        &seats[0],
        &design,
        &output,
        fresh,
    ));
    let after = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&after, &design)
            .claim
            .as_ref()
            .map(|claim| claim.body.disposition),
        Some(ClaimDisposition::Freed)
    );
    assert!(
        at(&after, &design).design_cycle.proposal.is_some(),
        "the pass awaits the operator's decision"
    );
    clean(&root, bus);
}

/// A lane holding no Design is told both claim routes, and following the named
/// one claims a Design still gathering input, whose pass then records.
#[test]
fn an_unheld_gathering_design_is_claimed_by_name_and_its_pass_proposed() {
    let (root, bus, epoch, seats) = fixture("preparation-gathering-repair", 1);
    let lane = Actor::Lane(seats[0].lane);
    let design = add_controller(
        &bus,
        epoch,
        "Gathering Design",
        wire::TaskIntent::Design,
        None,
    );
    stand_design(&bus, epoch, &design);
    let output = add_plan_output(&bus, epoch, &design, "Gathering output");
    let unheld = board(&bus, epoch, lane.clone());
    assert_eq!(
        unheld
            .design_cycles
            .get(&design.born().expect("design stamp")),
        Some(&wire::rooms::DesignCycle::GatheringInput)
    );

    let by_name = format!("`spice task capture {design}`");
    for (named, capture) in [
        (None, "`spice task capture HANDLE`"),
        (Some(&design), by_name.as_str()),
    ] {
        let Err(Outcome::Refusal(refusal)) = held_design(&unheld, named) else {
            panic!("holding no Design derives nothing");
        };
        assert_eq!(refusal.reason(), "preparation::design-unheld");
        for command in ["`spice task next`", capture] {
            assert!(
                refusal.explanation().contains(command),
                "{named:?} remedy names {command}: {}",
                refusal.explanation()
            );
        }
    }

    assert_committed(win_named(&bus, epoch, &seats[0], &design, "capture"));
    let held = board(&bus, epoch, lane);
    cover_questions(&bus, epoch, &seats[0], &design);
    let proposal = derived(&held, &output, 1_760_000_000_000_301);
    assert_committed(propose(
        &bus,
        epoch,
        &seats[0],
        &design,
        &output,
        proposal,
        room_head(&bus, &design),
    ));
    clean(&root, bus);
}

/// The refusal a pass earns while `questions` of its Design stay uncovered.
fn assert_uncovered(outcome: &Outcome<CommandAnswer>, questions: &str, item: &str) {
    let Outcome::Refusal(refusal) = outcome else {
        panic!("an uncovered question refuses the pass: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "preparation::questions-uncovered");
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(|diagnostic| diagnostic.observed()),
        Some(questions)
    );
    assert!(
        refusal.explanation().contains(item),
        "the refusal names the uncovered question: {}",
        refusal.explanation()
    );
}

/// A pass refuses while any current question lacks an item-specific nonzero
/// charge, naming each gap and moving neither claim, flow, nor proposal. A
/// charge of either sign covers its question, and coverage follows the current
/// statement, so a question an edit adds must be weighed before the pass
/// records.
#[test]
fn a_pass_records_only_once_every_current_question_is_covered() {
    let (root, bus, epoch, seats) = fixture("preparation-question-coverage", 1);
    let lane = Actor::Lane(seats[0].lane);
    let design = add_controller(
        &bus,
        epoch,
        "Covered Design",
        wire::TaskIntent::Design,
        None,
    );
    stand_design(&bus, epoch, &design);
    let output = add_plan_output(&bus, epoch, &design, "Covered output");
    capture_for(&bus, epoch, &seats[0], &design);
    let first = at(&board(&bus, epoch, Actor::Operator), &design)
        .statement
        .body
        .acceptance[0]
        .clone();

    let held = board(&bus, epoch, lane.clone());
    let uncovered = propose(
        &bus,
        epoch,
        &seats[0],
        &design,
        &output,
        derived(&held, &output, 1_760_000_000_000_401),
        room_head(&bus, &design),
    );
    assert_uncovered(&uncovered, "1", &first);
    let after = board(&bus, epoch, lane.clone());
    let (before, unmoved) = (at(&held, &design), at(&after, &design));
    assert_eq!(unmoved.claim, before.claim, "the claim stays active");
    assert_eq!(unmoved.workflow, before.workflow, "the flow stays put");
    assert!(unmoved.design_cycle.proposal.is_none(), "nothing publishes");

    // A negative charge covers its question: coverage is not agreement.
    contributed(contribute(
        &bus,
        epoch,
        root.as_ref(),
        lane.clone(),
        contribution_body(
            &design,
            RoomContributionKind::charge(-1),
            "Not needed.",
            Vec::new(),
        ),
    ));
    let added = "Who owns the follow-up?";
    assert_committed(super::super::edit::correct(
        &bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            acceptance: vec![first, added.to_owned()],
            ..super::super::edit::only(&design)
        },
    ));
    refresh_held(&bus, epoch, &lane, &seats[0], &design);
    let edited = board(&bus, epoch, lane.clone());
    let partial = propose(
        &bus,
        epoch,
        &seats[0],
        &design,
        &output,
        derived(&edited, &output, 1_760_000_000_000_402),
        room_head(&bus, &design),
    );
    assert_uncovered(&partial, "2", added);

    cover_questions(&bus, epoch, &seats[0], &design);
    assert_committed(propose(
        &bus,
        epoch,
        &seats[0],
        &design,
        &output,
        derived(&edited, &output, 1_760_000_000_000_403),
        room_head(&bus, &design),
    ));
    assert!(
        at(&board(&bus, epoch, Actor::Operator), &design)
            .design_cycle
            .proposal
            .is_some(),
        "the covered pass records"
    );
    clean(&root, bus);
}
