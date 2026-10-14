use wire::intent::{BoardTask, TextAnswer};
use wire::rooms::{
    CandidateRef, CandidateReference, DecisionOutcome, RetainedCandidate, RoomSource,
};
use wire::{Seq, SequenceCode, Witness};

use super::{DecisionChoice, DecisionComposer, DecisionDraft, DraftNotice, DraftPhase};
use crate::work::tests::{stamp, task};

fn design() -> BoardTask {
    let mut design = task(1, "rooms.design");
    design.statement.body.intent = Some(wire::TaskIntent::Design);
    design.workflow.body.flow = vec!["design".parse().expect("fixture phase")];
    design.workflow.body.phase = "design".parse().expect("fixture phase");
    design
}

fn candidate() -> RetainedCandidate {
    RetainedCandidate {
        candidate: candidate_ref(),
        governed_inputs: vec![stamp(2)],
        outputs: vec![stamp(3), stamp(4)],
        references: vec![CandidateReference {
            relation: "evidence".parse().expect("fixture relation"),
            source: RoomSource::Journal {
                fact: SequenceCode::new(40),
            },
        }],
        evidence: Vec::new(),
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

#[test]
fn a_partial_selection_previews_exact_current_authority() {
    let candidate = candidate();
    let mut draft = DecisionDraft::new(candidate_ref());
    assert_eq!(draft.candidate(), &candidate_ref());
    assert_eq!(draft.choice(), DecisionChoice::Selected);
    draft.toggle_output(stamp(4));
    draft.preview(
        &design(),
        Seq::new(50).expect("fixture position"),
        &candidate,
        SequenceCode::new(2),
        &board(),
    );
    let DraftPhase::Previewing(submission) = draft.phase() else {
        panic!("valid selection previews: {:?}", draft.phase());
    };
    assert_eq!(
        submission.request.decision.outcome,
        DecisionOutcome::Selected {
            candidate: candidate_ref(),
            selected_outputs: vec![stamp(4)],
        }
    );
    assert_eq!(submission.request.decision.decision.controller, stamp(1));
    assert_eq!(
        submission.request.decision.source,
        RoomSource::Journal {
            fact: SequenceCode::new(50)
        }
    );
    assert!(submission.expected.contains(&Witness::Proposal {
        proposal: wire::rooms::ProposalRef::Candidate {
            candidate: candidate_ref()
        },
        head: Some(Seq::new(50).expect("fixture position")),
    }));
    assert!(submission.expected.contains(&Witness::Authorization {
        output: stamp(4),
        head: None,
    }));
    assert!(!submission.expected.iter().any(|witness| {
        matches!(witness, Witness::Authorization { output, .. } if *output == stamp(3))
    }));
}

#[test]
fn return_and_collapse_require_reasons_and_keep_the_design_open_until_authority_moves() {
    let candidate = candidate();
    let mut draft = DecisionDraft::new(candidate_ref());
    draft.set_choice(DecisionChoice::Returned);
    draft.preview(
        &design(),
        Seq::new(50).expect("fixture position"),
        &candidate,
        SequenceCode::new(1),
        &board(),
    );
    assert_eq!(refusal(&draft), "preparation::return-reason-empty");

    draft.set_reason("  Rework the boundary.  ".to_owned());
    draft.preview(
        &design(),
        Seq::new(50).expect("fixture position"),
        &candidate,
        SequenceCode::new(1),
        &board(),
    );
    let DraftPhase::Previewing(returned) = draft.phase() else {
        panic!("return previews");
    };
    assert!(matches!(
        &returned.request.decision.outcome,
        DecisionOutcome::Returned { reason, .. } if reason == "Rework the boundary."
    ));

    draft.set_choice(DecisionChoice::Collapse);
    draft.preview(
        &design(),
        Seq::new(50).expect("fixture position"),
        &candidate,
        SequenceCode::new(2),
        &board(),
    );
    let DraftPhase::Previewing(collapsed) = draft.phase() else {
        panic!("collapse previews");
    };
    assert!(matches!(
        &collapsed.request.decision.outcome,
        DecisionOutcome::Collapse { reason, .. } if reason == "Rework the boundary."
    ));
}

#[test]
fn a_pending_decision_submits_once_and_refusal_keeps_the_exact_draft() {
    let candidate = candidate();
    let mut draft = DecisionDraft::new(candidate_ref());
    draft.toggle_output(stamp(3));
    draft.preview(
        &design(),
        Seq::new(50).expect("fixture position"),
        &candidate,
        SequenceCode::new(1),
        &board(),
    );
    let submission = draft.begin_submit().expect("preview submits");
    assert!(draft.is_pending());
    assert!(draft.begin_submit().is_none());
    draft.toggle_output(stamp(4));
    draft.set_reason("ignored while pending".to_owned());
    draft.revise();
    assert_eq!(draft.phase(), &DraftPhase::Pending(submission.clone()));

    draft.settle(Err("preparation::stale: proposal moved".to_owned()));
    assert_eq!(refusal(&draft), "preparation::stale");
    assert!(draft.selected(&stamp(3)) && !draft.selected(&stamp(4)));
    assert!(draft.reason().is_empty());
    assert!(
        draft.begin_submit().is_none(),
        "refusal requires a fresh preview"
    );

    draft.preview(
        &design(),
        Seq::new(50).expect("fixture position"),
        &candidate,
        SequenceCode::new(1),
        &board(),
    );
    assert_eq!(draft.begin_submit(), Some(submission));
    draft.settle(Ok(TextAnswer {
        text: "selected 1 output".to_owned(),
    }));
    assert_eq!(
        draft.notice(),
        Some(&DraftNotice::Recorded("selected 1 output".to_owned()))
    );
    assert!(
        draft.selected(&stamp(3)),
        "the reply is not optimistic authority"
    );
}

#[test]
fn stale_or_incomplete_render_data_refuses_without_silently_narrowing_the_choice() {
    let candidate = candidate();
    let mut draft = DecisionDraft::new(candidate_ref());
    draft.preview(
        &design(),
        Seq::new(50).expect("fixture position"),
        &candidate,
        SequenceCode::new(1),
        &board(),
    );
    assert_eq!(refusal(&draft), "preparation::selection-empty");

    draft.toggle_output(stamp(4));
    let incomplete = vec![design(), task(2, "rooms.input"), task(3, "rooms.first")];
    draft.preview(
        &design(),
        Seq::new(50).expect("fixture position"),
        &candidate,
        SequenceCode::new(1),
        &incomplete,
    );
    assert_eq!(refusal(&draft), "preparation::output-unavailable");

    let mut concluded = design();
    concluded.workflow.body.status = "completed".to_owned();
    concluded.workflow.body.review = "clean".to_owned();
    concluded.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    draft.preview(
        &concluded,
        Seq::new(50).expect("fixture position"),
        &candidate,
        SequenceCode::new(1),
        &board(),
    );
    assert_eq!(refusal(&draft), "preparation::design-not-open");
}

#[test]
fn a_pending_version_keeps_the_composer_until_its_exact_response_settles() {
    let first = candidate_ref();
    let second = CandidateRef {
        controller: stamp(1),
        candidate: "dVsYdDj7m".parse().expect("second candidate"),
    };
    let candidate = candidate();
    let tasks = board();
    let mut composer = DecisionComposer::default();
    composer.apply(first.clone(), false, |draft| {
        draft.set_choice(DecisionChoice::Returned);
        draft.set_reason("revise the premise".to_owned());
        draft.preview(
            &design(),
            Seq::new(50).expect("fixture position"),
            &candidate,
            SequenceCode::new(1),
            &tasks,
        );
        assert!(draft.begin_submit().is_some());
    });
    assert!(composer.is_pending());

    composer.apply(second.clone(), false, |draft| {
        draft.set_choice(DecisionChoice::Returned);
    });
    assert!(
        composer.draft(&second).is_none(),
        "another version cannot displace a pending decision"
    );
    assert!(
        composer
            .draft(&first)
            .is_some_and(DecisionDraft::is_pending),
        "the submitted version stays visibly pending"
    );

    composer.apply(first.clone(), true, |draft| {
        draft.settle(Err("preparation::stale: proposal moved".to_owned()));
    });
    let first_draft = composer
        .draft(&first)
        .expect("first version remains selected");
    assert_eq!(refusal(first_draft), "preparation::stale");
    assert_eq!(first_draft.choice(), DecisionChoice::Returned);
    assert_eq!(first_draft.reason(), "revise the premise");

    composer.apply(second.clone(), false, |draft| {
        draft.set_choice(DecisionChoice::Returned);
    });
    assert_eq!(
        composer
            .draft(&second)
            .expect("second version edits after settlement")
            .choice(),
        DecisionChoice::Returned
    );
}
