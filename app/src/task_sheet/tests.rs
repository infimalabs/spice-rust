use wire::rooms::{
    CandidateRef, CandidateReference, DesignPassProposal, DesignPassRecommendation, RoomSource,
};
use wire::{Seq, SequenceCode};

use super::{ProposalLine, proposal_lines, sheet_blocks};
use crate::work::tests::{stamp, task};

#[test]
fn sheet_blocks_read_description_acceptance_history_with_their_counts() {
    let mut row = task(1, "spice.app");
    row.statement.body.description =
        Some("Need.\n\nMechanism line one.\nMechanism line two.\n".to_owned());
    row.statement.body.acceptance = vec!["One".to_owned(), "Two".to_owned(), "Three".to_owned()];
    assert_eq!(
        sheet_blocks(&row.statement.body, 12).map(|block| (block.label, block.count)),
        [("Description", 3), ("Acceptance", 3), ("History", 12)]
    );
}

#[test]
fn a_design_counts_questions_and_an_absent_description_counts_zero() {
    let mut row = task(2, "rooms.design");
    row.statement.body.intent = Some(wire::TaskIntent::Design);
    row.statement.body.description = None;
    row.statement.body.acceptance = vec!["Which store?".to_owned(), "Who owns it?".to_owned()];
    assert_eq!(
        sheet_blocks(&row.statement.body, 0).map(|block| (block.label, block.count)),
        [("Description", 0), ("Questions", 2), ("History", 0)]
    );
}

#[test]
fn proposal_lines_run_outputs_inputs_reference_count_then_justification() {
    let reference = |fact| CandidateReference {
        relation: "evidence".parse().expect("fixture relation"),
        source: RoomSource::Journal {
            fact: SequenceCode::new(fact),
        },
    };
    let candidate = DesignPassProposal {
        candidate: CandidateRef {
            controller: stamp(1),
            candidate: "dVsYdDj7h".parse().expect("fixture candidate"),
        },
        statement: Seq::new(10).expect("fixture statement"),
        claim: None,
        recommendation: DesignPassRecommendation::Work,
        governed_inputs: vec![stamp(2)],
        outputs: vec![stamp(3), stamp(4)],
        references: vec![reference(40), reference(41)],
        justification: "the evidence supports this pass".to_owned(),
    };
    let (two, three, four) = (stamp(2), stamp(3), stamp(4));
    assert_eq!(
        proposal_lines(&candidate),
        vec![
            ProposalLine::Output(&three),
            ProposalLine::Output(&four),
            ProposalLine::Input(&two),
            ProposalLine::References(2),
            ProposalLine::Justification("the evidence supports this pass"),
        ]
    );
}
