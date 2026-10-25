use super::*;

use crate::board_command::client::{PassRecord, assemble, held_design};
use wire::rooms::{
    CandidateRef, CandidateReference, DecisionOutcome, DecisionRef, OutputDecision, OutputVerdict,
    PreparationDecide, PreparationDecision, ProposalRef, RoomAuthorityStamp, RoomSource,
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

/// The operator's decision accepting `reports` as though `design` had
/// composed a candidate from them, named by the newest report's birth.
fn decide_unproposed(
    bus: &Bus,
    epoch: CanonicalUuid,
    design: &TaskStamp,
    reports: [TaskStamp; 2],
    source: RoomSource,
) -> Outcome<CommandAnswer> {
    let candidate = CandidateRef {
        controller: design.clone(),
        candidate: RoomAuthorityStamp::for_micros(reports[1].micros())
            .expect("task birth instant is a room authority instant"),
    };
    let decision = DecisionRef {
        controller: design.clone(),
        decision: wire::SequenceCode::new(1),
    };
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::PreparationDecide,
        vec![
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
        ],
        &PreparationDecide {
            decision: PreparationDecision {
                decision,
                source,
                outcome: DecisionOutcome::Resolved {
                    candidate,
                    outputs: reports
                        .map(|output| OutputDecision {
                            output,
                            verdict: OutputVerdict::Accepted,
                            comment: None,
                        })
                        .to_vec(),
                    comment: None,
                },
            },
        },
    )
}

/// The pass `lane` assembles for the Design it holds, citing its statement.
fn assemble_held(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
) -> Result<wire::rooms::DesignPassBody, Outcome<CommandAnswer>> {
    let held = board(bus, epoch, lane.clone());
    let design = held_design(&held, None).expect("the lane holds the OOPS Design");
    assemble(
        design,
        &held.tasks,
        &PassRecord::default(),
        None,
        vec![CandidateReference {
            relation: "contract".parse().expect("relation"),
            source: RoomSource::Journal {
                fact: wire::SequenceCode::new(design.statement.position.get()),
            },
        }],
        "Both reports share one cause.".to_owned(),
    )
}

/// Reports gather under one Design that composes no candidate of its own:
/// deciding one refuses until a holder proposes, and the holder's pass
/// assembles exactly the reports given acceptance as its outputs.
#[test]
fn oops_reports_wait_for_their_holders_proposal() {
    let (root, bus, epoch, seats) = fixture("oops-design-lines", 1);
    let lane = Actor::Lane(seats[0].lane);
    let design_stamp = task(1_760_000_000_000_000);
    let first_stamp = task(1_760_000_000_000_001);
    let second_stamp = task(1_760_000_000_000_002);
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

    // No proposal exists until a holder writes one, so the operator cannot
    // accept the pending reports directly.
    let facts = fact_count(&bus);
    let source = RoomSource::Journal {
        fact: wire::SequenceCode::new(at(&preview, &second).statement.position.get()),
    };
    let refused = decide_unproposed(
        &bus,
        epoch,
        &design_stamp,
        [first_stamp.clone(), second_stamp.clone()],
        source,
    );
    assert!(
        matches!(&refused, Outcome::Refusal(refusal) if refusal.reason() == "preparation::candidate-missing"),
        "{refused:?}"
    );
    assert_eq!(fact_count(&bus), facts);

    // Its statement text amends like any Design's; its intent does not.
    assert_committed(super::edit::correct(
        &bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            description: Some("Triage these reports as one cluster.".to_owned()),
            ..super::edit::only(&design)
        },
    ));
    let retyped = super::edit::correct(
        &bus,
        epoch,
        Actor::Operator,
        &TaskEdit {
            intent: Some(wire::TaskIntentEdit::Clear {}),
            ..super::edit::only(&design)
        },
    );
    assert!(
        matches!(&retyped, Outcome::Refusal(refusal) if refusal.reason() == "task::oops-design-internal"),
        "{retyped:?}"
    );

    // Moved to an active group, it is claimed and worked like any Design.
    group_design(&bus, epoch, &design);
    super::resolution::claim_task(&bus, epoch, &seats[0], &lane, &design);
    let accept = |report: &TaskHandle| {
        assert_committed(super::edit::correct(
            &bus,
            epoch,
            lane.clone(),
            &TaskEdit {
                acceptance: vec![format!("{report} names its fix.")],
                ..super::edit::only(report)
            },
        ));
    };
    let pass = || assemble_held(&bus, epoch, &lane);
    accept(&first);
    assert!(
        matches!(pass(), Err(Outcome::Refusal(refusal)) if refusal.reason() == "preparation::output-unaccepted"),
        "a report without acceptance is not yet an output"
    );
    accept(&second);
    let body = pass().expect("a triaged pass assembles");
    assert_eq!(body.outputs, vec![first_stamp, second_stamp]);

    super::reclaim::free_active(&bus, epoch, &lane, &seats[0], &design);
    collapse_and_open_next_generation(&bus, epoch, design);
    clean(&root, bus);
}
