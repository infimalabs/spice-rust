use wire::intent::{BoardTask, TextAnswer};
use wire::rooms::{
    CandidateRef, CandidateReference, DecisionOutcome, DesignPassProposal,
    DesignPassRecommendation, OutputDecision, OutputVerdict, RoomSource,
};
use wire::{Seq, SequenceCode, Witness};

use super::{DecisionComposer, DecisionDraft, DraftNotice, DraftPhase};
use crate::work::tests::{stamp, task};

fn design() -> BoardTask {
    let mut design = task(1, "rooms.design");
    design.statement.body.intent = Some(wire::TaskIntent::Design);
    design.workflow.body.flow = vec!["design".parse().expect("fixture phase")];
    design.workflow.body.phase = "design".parse().expect("fixture phase");
    design
}

fn candidate() -> DesignPassProposal {
    DesignPassProposal {
        candidate: candidate_ref(),
        statement: Seq::new(10).expect("fixture statement"),
        claim: Some(Seq::new(11).expect("fixture claim")),
        recommendation: DesignPassRecommendation::Work,
        governed_inputs: vec![stamp(2)],
        outputs: vec![stamp(3), stamp(4)],
        references: vec![CandidateReference {
            relation: "evidence".parse().expect("fixture relation"),
            source: RoomSource::Journal {
                fact: SequenceCode::new(40),
            },
        }],
        justification: "the evidence supports this pass".to_owned(),
    }
}

fn candidate_ref() -> CandidateRef {
    CandidateRef {
        controller: stamp(1),
        candidate: "dVsYdDj7h".parse().expect("fixture candidate"),
    }
}

fn refusal(draft: &DecisionDraft) -> &str {
    match draft.notice() {
        Some(DraftNotice::Refused(reason)) => reason.split(": ").next().unwrap_or(reason),
        other => panic!("expected refusal, got {other:?}"),
    }
}

fn board() -> Vec<BoardTask> {
    vec![
        design(),
        task(2, "rooms.input"),
        task(3, "rooms.first"),
        task(4, "rooms.second"),
    ]
}

fn preview(draft: &mut DecisionDraft, candidate: &DesignPassProposal, sequence: u64) {
    draft.preview(
        &design(),
        Seq::new(50).expect("fixture position"),
        candidate,
        SequenceCode::new(sequence),
        &board(),
    );
}

#[test]
fn mixed_verdicts_preview_every_comment_and_authorize_only_accepted_outputs() {
    let candidate = candidate();
    let mut draft = DecisionDraft::new(candidate_ref());
    draft.set_output_verdict(stamp(3), OutputVerdict::Accepted);
    draft.set_output_comment(stamp(3), "  acceptance explained  ".to_owned());
    draft.set_output_verdict(stamp(4), OutputVerdict::Rejected);
    draft.set_output_comment(stamp(4), "  revise this boundary  ".to_owned());
    draft.set_comment("  overall context  ".to_owned());
    preview(&mut draft, &candidate, 2);

    let DraftPhase::Previewing(submission) = draft.phase() else {
        panic!("complete decision previews: {:?}", draft.phase());
    };
    assert_eq!(
        submission.request.decision.outcome,
        DecisionOutcome::Resolved {
            candidate: candidate_ref(),
            outputs: vec![
                OutputDecision {
                    output: stamp(3),
                    verdict: OutputVerdict::Accepted,
                    comment: Some("acceptance explained".to_owned()),
                },
                OutputDecision {
                    output: stamp(4),
                    verdict: OutputVerdict::Rejected,
                    comment: Some("revise this boundary".to_owned()),
                },
            ],
            comment: Some("overall context".to_owned()),
        }
    );
    assert!(submission.expected.contains(&Witness::Authorization {
        output: stamp(3),
        head: None,
    }));
    assert!(!submission.expected.iter().any(
        |witness| matches!(witness, Witness::Authorization { output, .. } if *output == stamp(4))
    ));
}

#[test]
fn an_undecided_output_refuses_preview_until_every_output_is_marked() {
    let candidate = candidate();
    let mut draft = DecisionDraft::new(candidate_ref());
    for output in &candidate.outputs {
        assert_eq!(draft.verdict(output), None);
    }
    draft.set_output_verdict(stamp(3), OutputVerdict::Accepted);
    preview(&mut draft, &candidate, 1);
    assert!(matches!(draft.phase(), DraftPhase::Editing));
    assert_eq!(refusal(&draft), "preparation::decision-undecided-output");
    assert!(
        matches!(draft.notice(), Some(DraftNotice::Refused(reason)) if reason.contains(&stamp(4).to_string())),
        "the refusal names the undecided output"
    );

    draft.set_every_output_verdict(candidate.outputs.iter().cloned(), OutputVerdict::Rejected);
    assert_eq!(draft.verdict(&stamp(3)), Some(OutputVerdict::Rejected));
    assert_eq!(draft.verdict(&stamp(4)), Some(OutputVerdict::Rejected));
    preview(&mut draft, &candidate, 1);
    let DraftPhase::Previewing(submission) = draft.phase() else {
        panic!("a fully rejected decision previews: {:?}", draft.notice());
    };
    assert!(
        !submission
            .expected
            .iter()
            .any(|witness| matches!(witness, Witness::Authorization { .. }))
    );
    assert!(matches!(
        &submission.request.decision.outcome,
        DecisionOutcome::Resolved { outputs, .. }
            if outputs.iter().all(|output| output.verdict == OutputVerdict::Rejected)
    ));

    draft.set_every_output_verdict(candidate.outputs.iter().cloned(), OutputVerdict::Accepted);
    preview(&mut draft, &candidate, 1);
    let DraftPhase::Previewing(submission) = draft.phase() else {
        panic!("a fully accepted decision previews: {:?}", draft.notice());
    };
    assert!(matches!(
        &submission.request.decision.outcome,
        DecisionOutcome::Resolved { outputs, .. }
            if outputs.iter().all(|output| output.verdict == OutputVerdict::Accepted)
    ));
}

#[test]
fn outputless_recommendations_accept_without_fabricating_output_authority() {
    for recommendation in [
        DesignPassRecommendation::Conclude,
        DesignPassRecommendation::Collapse,
    ] {
        let mut candidate = candidate();
        candidate.recommendation = recommendation;
        candidate.governed_inputs.clear();
        candidate.outputs.clear();
        let mut draft = DecisionDraft::new(candidate_ref());
        preview(&mut draft, &candidate, 1);
        let DraftPhase::Previewing(submission) = draft.phase() else {
            panic!("outputless acceptance previews");
        };
        assert_eq!(
            submission.request.decision.outcome,
            DecisionOutcome::Accepted {
                candidate: candidate_ref(),
            }
        );
        assert!(
            !submission
                .expected
                .iter()
                .any(|witness| matches!(witness, Witness::Authorization { .. }))
        );
    }
}

#[test]
fn entered_feedback_returns_and_a_pending_decision_submits_once() {
    let candidate = candidate();
    let mut draft = DecisionDraft::new(candidate_ref());
    draft.set_every_output_verdict(candidate.outputs.iter().cloned(), OutputVerdict::Accepted);
    draft.set_feedback("   ".to_owned());
    preview(&mut draft, &candidate, 1);
    let DraftPhase::Previewing(submission) = draft.phase() else {
        panic!(
            "blank feedback leaves the verdicts deciding: {:?}",
            draft.notice()
        );
    };
    assert!(matches!(
        submission.request.decision.outcome,
        DecisionOutcome::Resolved { .. }
    ));

    draft.set_feedback("  premise failed  ".to_owned());
    preview(&mut draft, &candidate, 1);
    let DraftPhase::Previewing(submission) = draft.phase() else {
        panic!("entered feedback previews a return: {:?}", draft.notice());
    };
    assert_eq!(
        submission.request.decision.outcome,
        DecisionOutcome::Returned {
            candidate: candidate_ref(),
            reason: "premise failed".to_owned(),
        }
    );
    let expected = draft.begin_submit().expect("preview submits");
    assert!(draft.begin_submit().is_none());
    draft.set_feedback("ignored while pending".to_owned());
    assert_eq!(draft.phase(), &DraftPhase::Pending(expected.clone()));

    draft.settle(Err("preparation::stale: proposal moved".to_owned()));
    assert_eq!(refusal(&draft), "preparation::stale");
    assert_eq!(draft.feedback(), "  premise failed  ");
    preview(&mut draft, &candidate, 1);
    assert_eq!(draft.begin_submit(), Some(expected));
    draft.settle(Ok(TextAnswer {
        text: "decision recorded".to_owned(),
    }));
    assert_eq!(
        draft.notice(),
        Some(&DraftNotice::Recorded("decision recorded".to_owned()))
    );
}

#[test]
fn composer_retains_versions_and_one_exact_pending_owner() {
    let first = candidate_ref();
    let second = CandidateRef {
        controller: stamp(1),
        candidate: "dVsYdDj7m".parse().expect("second candidate"),
    };
    let candidate = candidate();
    let mut composer = DecisionComposer::default();
    composer.apply(first.clone(), false, |draft| {
        draft.set_feedback("revise the premise".to_owned());
        preview(draft, &candidate, 1);
        assert!(draft.begin_submit().is_some());
    });
    assert!(composer.is_pending());

    composer.apply(second.clone(), false, |_| {
        panic!("another version cannot displace a pending decision");
    });
    assert!(composer.draft(&second).is_none());
    composer.apply(first.clone(), true, |draft| {
        draft.settle(Err("preparation::stale: proposal moved".to_owned()));
    });
    assert_eq!(
        composer.draft(&first).map(DecisionDraft::feedback),
        Some("revise the premise")
    );

    composer.apply(second.clone(), false, |draft| {
        draft.set_feedback("second premise".to_owned());
    });
    assert_eq!(
        composer.draft(&second).map(DecisionDraft::feedback),
        Some("second premise")
    );
}
