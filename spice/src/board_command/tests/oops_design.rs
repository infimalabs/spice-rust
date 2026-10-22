use super::*;

use wire::rooms::{
    CandidateRef, DecisionOutcome, DecisionRef, OutputDecision, OutputVerdict, PreparationDecide,
    PreparationDecision, ProposalRef, RoomAuthorityStamp, RoomSource,
};

fn task(micros: i64) -> TaskStamp {
    TaskStamp::for_micros(micros).expect("fixture task stamp")
}

fn fact_count(bus: &Bus) -> usize {
    bus.fact_snapshot()
        .expect("fixture facts remain readable")
        .facts()
        .len()
}

fn add_line(
    bus: &Bus,
    epoch: CanonicalUuid,
    design: &TaskStamp,
    report: TaskStamp,
    text: &str,
) -> TaskHandle {
    let current = board(bus, epoch, Actor::Operator);
    let generation = current.oops_generation.as_ref();
    let mut expected = vec![
        Witness::Absence {
            task: report.clone(),
        },
        Witness::OopsGeneration {
            head: generation.map(|head| head.position),
            design: generation.and_then(|head| head.body.design.clone()),
        },
    ];
    if let Some(heads) = current
        .tasks
        .iter()
        .find(|heads| &heads.statement.body.born == design)
    {
        expected.push(statement_witness(heads));
        expected.push(workflow_witness(heads));
    } else {
        expected.push(Witness::Absence {
            task: design.clone(),
        });
    }
    let CommandAnswer::Task(answer) = assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskOopsAdd,
        expected,
        &TaskOopsAdd {
            design: design.clone(),
            title: text.to_owned(),
            origin: Some(fixture_origin()),
            project: Some("board.test".to_owned()),
        },
    )) else {
        panic!("OOPS intake returned another answer shape");
    };
    let view = board(bus, epoch, Actor::Operator);
    let created = at(&view, &answer.task);
    assert_eq!(
        created.presentation_title(),
        text,
        "the report text is the title"
    );
    assert_eq!(
        created.statement.body.description, None,
        "the report text is not repeated as a description"
    );
    assert!(
        created.statement.body.acceptance.is_empty(),
        "intake injects no acceptance"
    );
    answer.task
}

#[test]
fn concurrent_distinct_first_reports_converge_on_one_oops_design() {
    let (root, bus, epoch, _seats) = fixture("oops-design-first-race", 0);
    let first_design = task(1_760_000_000_100_000);
    let second_design = task(1_760_000_000_100_001);
    let first_report = task(1_760_000_000_100_002);
    let second_report = task(1_760_000_000_100_003);
    let expected = |design: TaskStamp, report: TaskStamp| {
        vec![
            Witness::Absence { task: report },
            Witness::Absence { task: design },
            Witness::OopsGeneration {
                head: None,
                design: None,
            },
        ]
    };
    let add = |design: TaskStamp, text: &str| TaskOopsAdd {
        design,
        title: text.to_owned(),
        origin: Some(fixture_origin()),
        project: Some("board.test".to_owned()),
    };
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskOopsAdd,
        expected(first_design.clone(), first_report),
        &add(first_design.clone(), "First concurrent report"),
    ));
    let stale = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskOopsAdd,
        expected(second_design.clone(), second_report.clone()),
        &add(second_design, "Second concurrent report"),
    );
    assert!(
        matches!(&stale, Outcome::Refusal(refusal) if refusal.reason() == "board::witness-moved"),
        "{stale:?}"
    );
    add_line(
        &bus,
        epoch,
        &first_design,
        second_report,
        "Second concurrent report",
    );
    let current = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        current
            .tasks
            .iter()
            .filter(|heads| {
                heads.statement.body.intent == Some(wire::TaskIntent::OpportunisticDesign)
                    && heads.workflow.body.status.is_pending()
            })
            .count(),
        1
    );
    assert_eq!(
        current
            .tasks
            .iter()
            .filter(|heads| {
                heads.statement.body.intent == Some(wire::TaskIntent::Plan)
                    && heads
                        .statement
                        .body
                        .preparation
                        .as_ref()
                        .is_some_and(|preparation| preparation.controller == first_design)
            })
            .count(),
        2
    );
    clean(&root, bus);
}

#[test]
fn unrelated_board_churn_does_not_refuse_oops_intake() {
    let (root, bus, epoch, _seats) = fixture("oops-design-unrelated-churn", 0);
    let design = task(1_760_000_000_150_000);
    let report = task(1_760_000_000_150_001);
    let expected = vec![
        Witness::Absence {
            task: report.clone(),
        },
        Witness::Absence {
            task: design.clone(),
        },
        Witness::OopsGeneration {
            head: None,
            design: None,
        },
    ];
    add(
        &bus,
        epoch,
        Actor::Operator,
        "1B6QBLpz",
        "Unrelated board mutation",
        Some("todo"),
    );
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskOopsAdd,
        expected,
        &TaskOopsAdd {
            design,
            title: "Report after unrelated churn".to_owned(),
            origin: Some(fixture_origin()),
            project: Some("board.test".to_owned()),
        },
    ));
    clean(&root, bus);
}

#[test]
fn oops_decision_refuses_when_a_previewed_line_head_moves() {
    let (root, bus, epoch, _seats) = fixture("oops-design-stale-line", 0);
    let design_stamp = task(1_760_000_000_200_000);
    let line_stamp = task(1_760_000_000_200_001);
    let line = add_line(
        &bus,
        epoch,
        &design_stamp,
        line_stamp.clone(),
        "Friction before revision",
    );
    let design = TaskHandle::for_project(wire::OOPS_DESIGN_PROJECT, &design_stamp);
    let preview = board(&bus, epoch, Actor::Operator);
    let candidate = CandidateRef {
        controller: design_stamp.clone(),
        candidate: RoomAuthorityStamp::for_micros(line_stamp.micros())
            .expect("task birth instant is a room authority instant"),
    };
    let decision = DecisionRef {
        controller: design_stamp.clone(),
        decision: wire::SequenceCode::new(1),
    };
    let expected = vec![
        statement_witness(at(&preview, &design)),
        workflow_witness(at(&preview, &design)),
        claim_witness(at(&preview, &design)),
        statement_witness(at(&preview, &line)),
        workflow_witness(at(&preview, &line)),
        claim_witness(at(&preview, &line)),
        Witness::Authorization {
            output: line_stamp.clone(),
            head: None,
        },
        Witness::Proposal {
            proposal: ProposalRef::Candidate {
                candidate: candidate.clone(),
            },
            head: None,
        },
        Witness::Proposal {
            proposal: ProposalRef::Decision {
                decision: decision.clone(),
            },
            head: None,
        },
    ];
    let source = RoomSource::Journal {
        fact: wire::SequenceCode::new(at(&preview, &line).statement.position.get()),
    };
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(at(&preview, &line))],
        &TaskEdit {
            task: line.clone(),
            title: Some("Friction after revision".to_owned()),
            pitch: None,
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            intent: None,
            priority: None,
            project: None,
            flow: None,
        },
    ));
    let facts = fact_count(&bus);
    let refused = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::PreparationDecide,
        expected,
        &PreparationDecide {
            decision: PreparationDecision {
                decision,
                source,
                outcome: DecisionOutcome::Resolved {
                    candidate,
                    outputs: vec![OutputDecision {
                        output: line_stamp,
                        verdict: OutputVerdict::Accepted,
                        comment: None,
                    }],
                    comment: None,
                },
            },
        },
    );
    assert!(matches!(refused, Outcome::Refusal(_)), "{refused:?}");
    assert_eq!(fact_count(&bus), facts);
    assert!(
        at(&board(&bus, epoch, Actor::Operator), &line)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    clean(&root, bus);
}

fn decide_two_lines(
    bus: &Bus,
    epoch: CanonicalUuid,
    design_stamp: &TaskStamp,
    design: &TaskHandle,
    lines: [&TaskHandle; 2],
    stamps: [TaskStamp; 2],
) -> DecisionRef {
    let [first, second] = lines;
    let [first_stamp, second_stamp] = stamps;
    let current = board(bus, epoch, Actor::Operator);
    let candidate = CandidateRef {
        controller: design_stamp.clone(),
        candidate: RoomAuthorityStamp::for_micros(second_stamp.micros())
            .expect("task birth instant is a room authority instant"),
    };
    let decision = DecisionRef {
        controller: design_stamp.clone(),
        decision: wire::SequenceCode::new(1),
    };
    let mut expected = vec![
        statement_witness(at(&current, design)),
        workflow_witness(at(&current, design)),
        claim_witness(at(&current, design)),
        statement_witness(at(&current, first)),
        claim_witness(at(&current, first)),
        workflow_witness(at(&current, first)),
        statement_witness(at(&current, second)),
        workflow_witness(at(&current, second)),
        Witness::Authorization {
            output: first_stamp.clone(),
            head: None,
        },
        Witness::Proposal {
            proposal: ProposalRef::Candidate {
                candidate: candidate.clone(),
            },
            head: None,
        },
        Witness::Proposal {
            proposal: ProposalRef::Decision {
                decision: decision.clone(),
            },
            head: None,
        },
    ];
    expected.sort();
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::PreparationDecide,
        expected,
        &PreparationDecide {
            decision: PreparationDecision {
                decision: decision.clone(),
                source: RoomSource::Journal {
                    fact: wire::SequenceCode::new(at(&current, second).statement.position.get()),
                },
                outcome: DecisionOutcome::Resolved {
                    candidate,
                    outputs: vec![
                        OutputDecision {
                            output: first_stamp,
                            verdict: OutputVerdict::Accepted,
                            comment: None,
                        },
                        OutputDecision {
                            output: second_stamp,
                            verdict: OutputVerdict::Rejected,
                            comment: None,
                        },
                    ],
                    comment: None,
                },
            },
        },
    ));
    decision
}

fn collapse_and_open_next_generation(bus: &Bus, epoch: CanonicalUuid, design: TaskHandle) {
    let before = board(bus, epoch, Actor::Operator);
    let design_heads = at(&before, &design);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskConclude,
        vec![
            statement_witness(design_heads),
            workflow_witness(design_heads),
            claim_witness(design_heads),
        ],
        &TaskConclude {
            task: design,
            conclusion: body::DesignConclusion::Collapsed {
                reason: "remaining line deferred to a later generation".to_owned(),
            },
        },
    ));

    let next_design = task(1_760_000_000_000_010);
    let next = add_line(
        bus,
        epoch,
        &next_design,
        task(1_760_000_000_000_011),
        "Next-generation friction",
    );
    let final_board = board(bus, epoch, Actor::Operator);
    assert_eq!(
        final_board
            .tasks
            .iter()
            .filter(|heads| {
                heads.statement.body.intent == Some(wire::TaskIntent::OpportunisticDesign)
                    && heads.workflow.body.status.is_pending()
            })
            .count(),
        1
    );
    assert_eq!(
        at(&final_board, &next)
            .statement
            .body
            .preparation
            .as_ref()
            .map(|association| association.parent.clone()),
        Some(next_design)
    );
}

#[test]
fn oops_lines_share_one_design_and_exact_decisions_leave_new_arrivals_for_the_next_candidate() {
    let (root, bus, epoch, _seats) = fixture("oops-design-lines", 0);
    let design_stamp = task(1_760_000_000_000_000);
    let first_stamp = task(1_760_000_000_000_001);
    let second_stamp = task(1_760_000_000_000_002);
    let third_stamp = task(1_760_000_000_000_003);
    let first = add_line(
        &bus,
        epoch,
        &design_stamp,
        first_stamp.clone(),
        "First friction",
    );
    let second = add_line(
        &bus,
        epoch,
        &design_stamp,
        second_stamp.clone(),
        "Second friction",
    );

    let preview = board(&bus, epoch, Actor::Operator);
    let design = TaskHandle::for_project(wire::OOPS_DESIGN_PROJECT, &design_stamp);
    let design_heads = at(&preview, &design);
    assert_eq!(
        design_heads.statement.body.intent,
        Some(wire::TaskIntent::OpportunisticDesign)
    );
    for line in [&first, &second] {
        let heads = at(&preview, line);
        assert_eq!(heads.statement.body.intent, Some(wire::TaskIntent::Plan));
        let association = heads
            .statement
            .body
            .preparation
            .as_ref()
            .expect("OOPS line is controlled");
        assert_eq!(association.parent, design_stamp);
        assert_eq!(association.controller, design_stamp);
        assert_eq!(association.construction, heads.statement.body.born);
    }

    // This line arrives after the operator saw the candidate. Its birth is
    // beyond that candidate's cutoff, so the exact older candidate remains
    // decidable and the new line remains for the next pass.
    let third = add_line(
        &bus,
        epoch,
        &design_stamp,
        third_stamp,
        "Concurrent friction",
    );
    let _decision = decide_two_lines(
        &bus,
        epoch,
        &design_stamp,
        &design,
        [&first, &second],
        [first_stamp, second_stamp],
    );

    let decided = board(&bus, epoch, Actor::Operator);
    assert!(
        !at(&decided, &first)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert!(
        at(&decided, &second)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert!(
        at(&decided, &third)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert!(at(&decided, &design).workflow.body.status.is_pending());

    collapse_and_open_next_generation(&bus, epoch, design);
    clean(&root, bus);
}

/// A crossed journal keeps eight-character millisecond report identities. A
/// candidate whose newest line is retained names that line's moment, and the
/// decision matches each line by its own identity, never a reminted one.
#[test]
fn retained_report_identities_are_decided_without_reminting() {
    let (root, bus, epoch, _seats) = fixture("oops-design-retained", 0);
    let design_stamp = task(1_759_999_999_000_000);
    let current = task(1_759_999_999_999_999);
    let retained: TaskStamp = "1j14rjy8".parse().expect("retained report stamp");
    assert!(retained < current && current.birth_cmp(&retained).is_lt());
    let first = add_line(
        &bus,
        epoch,
        &design_stamp,
        current.clone(),
        "Current friction",
    );
    let second = add_line(
        &bus,
        epoch,
        &design_stamp,
        retained.clone(),
        "Retained friction",
    );
    let later = add_line(
        &bus,
        epoch,
        &design_stamp,
        task(1_760_000_000_000_001),
        "Later friction",
    );
    let design = TaskHandle::for_project(wire::OOPS_DESIGN_PROJECT, &design_stamp);
    let _decision = decide_two_lines(
        &bus,
        epoch,
        &design_stamp,
        &design,
        [&first, &second],
        [current, retained.clone()],
    );

    let decided = board(&bus, epoch, Actor::Operator);
    assert!(
        !at(&decided, &first)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert_eq!(at(&decided, &second).statement.body.born, retained);
    assert!(
        at(&decided, &second)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert!(
        at(&decided, &later)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    clean(&root, bus);
}
