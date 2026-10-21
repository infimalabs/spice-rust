//! The proposal `preparation propose` derives from the Design its caller
//! holds, and the resident refusing it once a witnessed head has moved.

use super::*;

use crate::board_command::client::{derive_proposal, held_design};

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

/// Issue `proposal` from `seat` with witnesses read from the current board.
fn propose(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    design: &TaskHandle,
    output: &TaskHandle,
    proposal: DesignPassProposal,
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
    group_design(&bus, epoch, &design);
    let output = add_plan_output(&bus, epoch, &design, "Derived output");

    // Holding no Design derives nothing, with or without a name.
    let unheld = board(&bus, epoch, lane.clone());
    for named in [None, Some(&design)] {
        assert!(matches!(
            held_design(&unheld, named),
            Err(Outcome::Refusal(refusal)) if refusal.reason() == "preparation::design-unheld"
        ));
    }

    claim_task(&bus, epoch, &seats[0], &lane, &design);
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
        propose(&bus, epoch, &seats[0], &design, &output, proposal),
        Outcome::Refusal(refusal)
            if refusal.reason() == "preparation::proposal-statement-moved"
    ));

    // Derived again from the edited heads, the proposal names the current
    // statement and the same active claim, but that claim has not
    // acknowledged the Design's new generation.
    let edited = board(&bus, epoch, lane.clone());
    let stale = derived(&edited, &output, 1_760_000_000_000_202);
    assert_eq!(stale.statement, at(&edited, &design).statement.position);
    let stale_outcome = propose(&bus, epoch, &seats[0], &design, &output, stale.clone());
    assert!(
        matches!(&stale_outcome, Outcome::Refusal(refusal) if refusal.reason() == "preparation::proposal-claim-stale"),
        "{stale_outcome:?}"
    );

    // The reclaim moves the claim head: a proposal witnessing the earlier
    // claim no longer names the active one.
    reclaim_held(&bus, epoch, &lane, &seats[0], &design);
    let current = board(&bus, epoch, lane.clone());
    assert_ne!(
        stale.claim,
        at(&current, &design)
            .claim
            .as_ref()
            .map(|claim| claim.position)
    );
    assert!(matches!(
        propose(&bus, epoch, &seats[0], &design, &output, stale),
        Outcome::Refusal(refusal) if refusal.reason() == "preparation::proposal-claim-moved"
    ));

    // Derived from the current heads, the pass records and releases the claim.
    let fresh = derived(&current, &output, 1_760_000_000_000_204);
    assert_committed(propose(&bus, epoch, &seats[0], &design, &output, fresh));
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
