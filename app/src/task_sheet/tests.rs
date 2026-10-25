use wire::rooms::{
    CandidateRef, CandidateReference, DesignPassProposal, DesignPassRecommendation, RoomSource,
};
use wire::{Seq, SequenceCode};

use crate::work::tests::stamp;

#[test]
fn proposal_records_distinguish_journal_positions_from_authored_numbers() {
    let position = Seq::new(1_000_000).expect("positive journal position");
    let mut proposal = DesignPassProposal {
        candidate: CandidateRef {
            controller: stamp(1),
            candidate: "dVsYdDj7h".parse().expect("fixture candidate"),
        },
        statement: position,
        claim: Some(Seq::new(53).expect("positive claim")),
        recommendation: DesignPassRecommendation::Conclude,
        governed_inputs: Vec::new(),
        outputs: Vec::new(),
        justification: "1000000 observations support 53 conclusions.".to_owned(),
        references: vec![CandidateReference {
            relation: "evidence".parse().expect("fixture relation"),
            source: RoomSource::Journal {
                fact: SequenceCode::new(position.get()),
            },
        }],
    };
    let display: serde_json::Value =
        serde_json::from_str(&super::proposal_record(&proposal)).expect("display JSON");
    assert_eq!(
        display["statement"],
        SequenceCode::new(position.get()).to_string()
    );
    assert_eq!(display["claim"], SequenceCode::new(53).to_string());
    assert_eq!(
        display["justification"],
        "1000000 observations support 53 conclusions."
    );
    let protocol = serde_json::to_value(&proposal).expect("protocol JSON");
    assert_eq!(protocol["statement"], position.get());
    assert_eq!(protocol["claim"], 53);

    proposal.claim = None;
    let display: serde_json::Value =
        serde_json::from_str(&super::proposal_record(&proposal)).expect("display JSON");
    assert!(
        display.get("claim").is_none(),
        "unknown history stays absent"
    );
}
