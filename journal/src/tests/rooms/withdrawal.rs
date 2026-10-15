//! The author's withdrawal of one undecided candidate, as the fold admits it.

use wire::rooms::{CandidateWithdrawal, CandidateWithdrawn};

use super::*;

fn withdrawn(preparation: &Preparation, micros: i64) -> CandidateWithdrawn {
    CandidateWithdrawn {
        withdrawal: CandidateWithdrawal {
            candidate: preparation.candidate(micros),
            reason: "cites another Design's facts".to_owned(),
        },
    }
}

fn returned(preparation: &Preparation, micros: i64) -> DecisionOutcome {
    DecisionOutcome::Returned {
        candidate: preparation.candidate(micros),
        reason: "another pass".to_owned(),
    }
}

#[test]
fn only_the_author_withdraws_an_undecided_candidate_and_decision_and_withdrawal_exclude() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let author = uuid("019fe57c-a453-7ec1-a400-26e6901aaeb1");
    let other = uuid("019fe57c-a453-7ec1-a400-26e6901aaeb2");
    for micros in [1, 2] {
        commit_as(
            &mut journal,
            Actor::Lane(author),
            preparation.retained(micros, vec![preparation.output.clone()]),
        )
        .expect("the lane retains a candidate");
    }
    let retained = journal
        .read(|read| {
            read.proposal_head(&ProposalRef::Candidate {
                candidate: preparation.candidate(1),
            })
        })
        .expect("candidate reads")
        .expect("candidate retained");

    for writer in [Actor::Lane(other), Actor::Operator] {
        let result = commit_as(&mut journal, writer, withdrawn(&preparation, 1));
        refused(&journal, result);
    }
    commit_as(
        &mut journal,
        Actor::Lane(author),
        withdrawn(&preparation, 1),
    )
    .expect("the author withdraws its undecided candidate");
    let again = commit_as(
        &mut journal,
        Actor::Lane(author),
        withdrawn(&preparation, 1),
    );
    refused(&journal, again);
    let decided = commit(
        &mut journal,
        preparation.decision(1, returned(&preparation, 1)),
    );
    refused(&journal, decided);
    let still = journal
        .read(|read| {
            read.proposal_head(&ProposalRef::Candidate {
                candidate: preparation.candidate(1),
            })
        })
        .expect("candidate reads")
        .expect("withdrawal keeps the candidate");
    assert_eq!(still, retained, "withdrawal never rewrites the candidate");

    commit(
        &mut journal,
        preparation.decision(1, returned(&preparation, 2)),
    )
    .expect("the operator decides the corrected candidate");
    let late = commit_as(
        &mut journal,
        Actor::Lane(author),
        withdrawn(&preparation, 2),
    );
    refused(&journal, late);

    let expected = projection(&journal);
    journal.verify().expect("withdrawal history verifies");
    journal.reindex().expect("withdrawal history rebuilds");
    assert_eq!(projection(&journal), expected);
}
