//! Retained preparation gates cross into ordinary dependency edges once.

use super::*;

#[derive(Clone, Copy)]
enum Settlement {
    DesignDone,
    Closed,
    Deleted,
}

fn write() -> crate::JournalWrite {
    crate::JournalWrite::new(
        Actor::Operator,
        "task:preparation-edges".parse().expect("origin"),
        instant(),
        wire::Producer::CommandDoor,
    )
}

fn add(
    journal: &mut Journal,
    offset: i64,
    intent: Option<wire::TaskIntent>,
    preparation: Option<(&TaskStamp, &TaskStamp)>,
) -> TaskStamp {
    let current = if intent.is_some_and(wire::TaskIntent::is_design) {
        phase("design")
    } else {
        phase("todo")
    };
    add_routed(journal, offset, intent, preparation, current)
}

fn add_routed(
    journal: &mut Journal,
    offset: i64,
    intent: Option<wire::TaskIntent>,
    preparation: Option<(&TaskStamp, &TaskStamp)>,
    current: wire::Phase,
) -> TaskStamp {
    let task = TaskStamp::for_micros(80_000 + offset).expect("task stamp");
    let preparation = preparation.map(|(parent, controller)| wire::TaskPreparation {
        parent: parent.clone(),
        controller: controller.clone(),
        construction: task.clone(),
    });
    let flow = vec![current.clone()];
    let mut write = write();
    let added = write
        .fact(
            wire::body::TaskAdded {
                preparation: preparation.clone(),
                intent,
                task: task.clone(),
                title: format!("Preparation edge crossing {offset}"),
                pitch: "Each edge must cross exactly once.".to_owned(),
                description: None,
                acceptance: vec!["crosses exactly once".to_owned()],
                requirements: wire::EnvironmentRequirements::default(),
                project: "preparation.edges".to_owned(),
                filing: wire::TaskFiling::Filed,
                priority: None,
                taste: None,
                document_slug: None,
                taste_proposal: None,
                depends: Vec::new(),
                flow: flow.clone(),
                phase: current.clone(),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::OperatorActor,
                ),
                revision: 1,
            },
            crate::FactCause::Root,
        )
        .expect("task fact builds");
    let born = task.clone();
    write.task_head(added, move |revision| wire::TaskHead {
        preparation,
        intent,
        born,
        queue_origin: revision,
        title: format!("Preparation edge crossing {offset}"),
        pitch: "Each edge must cross exactly once.".to_owned(),
        description: None,
        acceptance: vec!["crosses exactly once".to_owned()],
        requirements: wire::EnvironmentRequirements::default(),
        project: "preparation.edges".to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
        taste_proposal: None,
        dependencies: Vec::new(),
        provenance: wire::TaskProvenance::Parentless,
        creation: wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::OperatorActor,
        ),
        revision,
    });
    let workflow_task = task.clone();
    write.workflow_head(workflow_task, added, move |_| wire::WorkflowHead {
        flow,
        phase: current,
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    });
    journal.apply(write).expect("task commits");
    task
}
fn depend(journal: &mut Journal, task: &TaskStamp, after: &TaskStamp) {
    let mut statement = journal
        .read(|read| read.task_head(task))
        .expect("statement reads")
        .expect("statement exists")
        .into_body();
    let mut write = write();
    let fact = write
        .fact(
            wire::body::TaskEdited {
                task: task.clone(),
                revision: u32::try_from(statement.revision.get() + 1).expect("revision fits"),
                dependencies: Some(vec![after.clone()]),
                intent: None,
            },
            crate::FactCause::Root,
        )
        .expect("dependency fact builds");
    statement.dependencies = vec![after.clone()];
    write.task_head(fact, move |revision| wire::TaskHead {
        revision,
        ..statement
    });
    journal.apply(write).expect("dependency commits");
}

fn settle(journal: &mut Journal, task: &TaskStamp, settlement: Settlement) {
    let mut workflow = journal
        .read(|read| read.workflow_head(task))
        .expect("workflow reads")
        .expect("workflow exists")
        .into_body();
    let mut write = write();
    let fact = match settlement {
        Settlement::DesignDone => write.fact(
            wire::body::TaskConcluded {
                task: task.clone(),
                conclusion: wire::body::DesignConclusion::Done {},
            },
            crate::FactCause::Root,
        ),
        Settlement::Closed => write.fact(
            wire::body::TaskClosed {
                task: task.clone(),
                validation: "retained closure".to_owned(),
                phase: workflow.phase.clone(),
                claimed: None,
            },
            crate::FactCause::Root,
        ),
        Settlement::Deleted => write.fact(
            wire::body::TaskDeleted {
                task: task.clone(),
                reason: "retained deletion".to_owned(),
                claimed: None,
            },
            crate::FactCause::Root,
        ),
    }
    .expect("settlement fact builds");
    workflow.status = match settlement {
        Settlement::DesignDone => wire::WorkflowStatus::reviewed(instant()),
        Settlement::Closed => wire::WorkflowStatus::closed(instant()),
        Settlement::Deleted => wire::WorkflowStatus::deleted(instant()),
    };
    workflow.generation += 1;
    write.workflow_head(task.clone(), fact, move |_| workflow);
    journal.apply(write).expect("settlement commits");
}

fn claim(journal: &mut Journal, task: &TaskStamp) {
    let lane = uuid("018f82d4-6f59-7d06-92e3-bce9131163d1");
    let occupant = uuid("018f82d4-6f59-7d06-92e3-bce9131163d2");
    let epoch = uuid("018f82d4-6f59-7d06-92e3-bce9131163d3");
    let actor = Actor::Operator;
    let cell = wire::ClaimCell {
        project: "preparation.edges".to_owned(),
        phase: phase("gather"),
    };
    let mut write = write();
    let won = write
        .fact(
            wire::body::ClaimWon {
                task: task.clone(),
                actor: actor.clone(),
                lane,
                occupant,
                epoch,
                cell: cell.clone(),
                until: instant(),
                git: None,
                recovery: None,
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            crate::FactCause::Root,
        )
        .expect("claim fact builds");
    write.claim_head(task.clone(), won, move |fact| wire::ClaimHead {
        actor,
        fact,
        claimed: fact,
        cell,
        lane,
        occupant,
        epoch,
        recovery: None,
        disposition: wire::ClaimDisposition::Active,
        acknowledged: 0,
    });
    journal.apply(write).expect("claim commits");
}

fn select_in_retained_statement(journal: &Journal, output: &TaskStamp, controller: &TaskStamp) {
    let authorization = serde_json::to_string(&wire::rooms::DecisionRef {
        controller: controller.clone(),
        decision: wire::SequenceCode::new(1),
    })
    .expect("authorization encodes");
    journal
        .connection
        .execute(
            "UPDATE task_heads SET body = CAST(json_set(CAST(body AS TEXT), '$.authorization', json(?1)) AS BLOB) WHERE stamp = ?2",
            rusqlite::params![authorization, output.as_str()],
        )
        .expect("retained authorization is injected");
}

fn statement(journal: &Journal, task: &TaskStamp) -> wire::TaskHead {
    journal
        .read(|read| read.task_head(task))
        .expect("statement reads")
        .expect("statement exists")
        .into_body()
}

fn workflow(journal: &Journal, task: &TaskStamp) -> wire::WorkflowHead {
    journal
        .read(|read| read.workflow_head(task))
        .expect("workflow reads")
        .expect("workflow exists")
        .into_body()
}

fn retain_crossed_output(
    journal: &mut Journal,
    controller: &TaskStamp,
    output: &TaskStamp,
) -> wire::rooms::CandidateRef {
    let candidate = wire::rooms::CandidateRef {
        controller: controller.clone(),
        candidate: wire::rooms::RoomAuthorityStamp::for_micros(1_760_000_000_000_000)
            .expect("candidate identity"),
    };
    let mut write = write();
    write
        .fact(
            wire::rooms::DesignProposed {
                candidate: wire::rooms::DesignPassProposal {
                    candidate: candidate.clone(),
                    statement: statement(journal, controller).revision,
                    claim: None,
                    recommendation: wire::rooms::DesignPassRecommendation::Work,
                    governed_inputs: Vec::new(),
                    outputs: vec![output.clone()],
                    justification: "the retained gate still controls this output".to_owned(),
                    references: vec![wire::rooms::CandidateReference {
                        relation: "contract".parse().expect("relation"),
                        source: wire::rooms::RoomSource::Journal {
                            fact: wire::SequenceCode::new(1),
                        },
                    }],
                },
            },
            crate::FactCause::Root,
        )
        .expect("proposal builds");
    journal.apply(write).expect("crossed output is proposed");
    journal
        .verify()
        .expect("crossed output proposal replays from its retained gate");
    candidate
}

fn reject_crossed_output(
    journal: &mut Journal,
    candidate: wire::rooms::CandidateRef,
    output: &TaskStamp,
) {
    let mut write = write();
    write
        .fact(
            wire::rooms::PreparationDecided {
                decision: wire::rooms::PreparationDecision {
                    decision: wire::rooms::DecisionRef {
                        controller: candidate.controller.clone(),
                        decision: wire::SequenceCode::new(1),
                    },
                    source: wire::rooms::RoomSource::Journal {
                        fact: wire::SequenceCode::new(1),
                    },
                    outcome: wire::rooms::DecisionOutcome::Resolved {
                        candidate,
                        outputs: vec![wire::rooms::OutputDecision {
                            output: output.clone(),
                            verdict: wire::rooms::OutputVerdict::Rejected,
                            comment: None,
                        }],
                        comment: None,
                    },
                },
            },
            crate::FactCause::Root,
        )
        .expect("decision builds");
    journal.apply(write).expect("crossed output is decided");
    journal
        .verify()
        .expect("crossed output decision replays from its retained gate");
}

#[test]
fn schema_seventy_seven_crosses_live_gates_and_closes_unreleasable_branches() {
    let scratch = Scratch::new();
    let path = scratch.path().join("preparation-edges.sqlite3");
    let mut journal = Journal::open(&path).expect("journal creates");

    let open_design = add(&mut journal, 1, Some(wire::TaskIntent::Design), None);
    let open_output = add(&mut journal, 2, None, Some((&open_design, &open_design)));
    let selected_output = add(&mut journal, 3, None, Some((&open_design, &open_design)));
    let gather = add_routed(
        &mut journal,
        15,
        None,
        Some((&open_design, &open_design)),
        phase("gather"),
    );

    let done_design = add(&mut journal, 4, Some(wire::TaskIntent::Design), None);
    let done_output = add(&mut journal, 5, None, Some((&done_design, &done_design)));
    settle(&mut journal, &done_design, Settlement::DesignDone);

    let deleted_design = add(&mut journal, 6, Some(wire::TaskIntent::Design), None);
    let deleted_output = add(
        &mut journal,
        7,
        None,
        Some((&deleted_design, &deleted_design)),
    );
    let deleted_descendant = add(
        &mut journal,
        8,
        None,
        Some((&deleted_output, &deleted_design)),
    );
    depend(&mut journal, &deleted_descendant, &deleted_output);
    settle(&mut journal, &deleted_design, Settlement::Deleted);
    let open_plan = add(&mut journal, 9, Some(wire::TaskIntent::Plan), None);
    depend(&mut journal, &open_output, &open_plan);
    let open_plan_output = add(&mut journal, 10, None, Some((&open_plan, &open_plan)));
    let open_subplan = add(
        &mut journal,
        11,
        Some(wire::TaskIntent::Plan),
        Some((&open_plan, &open_plan)),
    );

    let closed_plan = add(&mut journal, 12, Some(wire::TaskIntent::Plan), None);
    let closed_plan_output = add(&mut journal, 13, None, Some((&closed_plan, &closed_plan)));
    let closed_plan_descendant = add(
        &mut journal,
        14,
        None,
        Some((&closed_plan_output, &closed_plan)),
    );
    depend(&mut journal, &closed_plan_descendant, &closed_plan_output);
    settle(&mut journal, &closed_plan, Settlement::Closed);

    select_in_retained_statement(&journal, &selected_output, &open_design);
    restore_predecessor(&journal, 77);
    drop(journal);

    let mut journal = Journal::open(&path).expect("schema-77 preparation gates cross");
    assert_eq!(
        journal.migration().map(|migration| migration.from()),
        Some(77)
    );
    assert!(
        statement(&journal, &open_output)
            .dependencies
            .contains(&open_design)
    );
    assert!(
        !statement(&journal, &selected_output)
            .dependencies
            .contains(&open_design)
    );
    assert!(
        statement(&journal, &open_plan_output)
            .dependencies
            .contains(&open_plan)
    );
    assert!(
        statement(&journal, &open_subplan)
            .dependencies
            .contains(&open_plan)
    );
    let crossed_gather = statement(&journal, &gather);
    assert!(
        crossed_gather.dependencies.contains(&open_design),
        "every direct output waits on its open Design"
    );
    assert!(workflow(&journal, &gather).status.is_pending());
    let candidate = retain_crossed_output(&mut journal, &open_design, &open_output);
    reject_crossed_output(&mut journal, candidate, &open_output);

    for task in [
        done_output,
        deleted_output,
        deleted_descendant,
        closed_plan_output,
        closed_plan_descendant,
    ] {
        assert!(
            matches!(
                workflow(&journal, &task).status,
                wire::WorkflowStatus::Settled {
                    conclusion: wire::WorkflowConclusion::Negative(wire::WorkflowNegative::Closed),
                    ..
                }
            ),
            "{task} closes"
        );
    }
    journal
        .verify()
        .expect("crossed preparation authority verifies");
}

#[test]
fn schema_eighty_four_gates_live_gather_routes_and_closes_the_rest() {
    let scratch = Scratch::new();
    let path = scratch.path().join("retired-gather.sqlite3");
    let mut journal = Journal::open(&path).expect("journal creates");

    let open_design = add(&mut journal, 20, Some(wire::TaskIntent::Design), None);
    let held = add_routed(
        &mut journal,
        21,
        None,
        Some((&open_design, &open_design)),
        phase("gather"),
    );
    claim(&mut journal, &held);

    let settled = add_routed(
        &mut journal,
        22,
        None,
        Some((&open_design, &open_design)),
        phase("gather"),
    );
    settle(&mut journal, &settled, Settlement::Closed);

    let concluded_design = add(&mut journal, 23, Some(wire::TaskIntent::Design), None);
    let concluded = add_routed(
        &mut journal,
        24,
        None,
        Some((&concluded_design, &concluded_design)),
        phase("gather"),
    );
    settle(&mut journal, &concluded_design, Settlement::DesignDone);

    let absent_design = add(&mut journal, 25, Some(wire::TaskIntent::Design), None);
    let absent = add_routed(
        &mut journal,
        26,
        None,
        Some((&absent_design, &absent_design)),
        phase("gather"),
    );
    journal
        .connection
        .execute(
            "DELETE FROM workflow_heads WHERE stamp = ?1",
            [absent_design.as_str()],
        )
        .expect("remove retained Design workflow");

    restore_predecessor(&journal, 84);
    drop(journal);

    let journal = Journal::open(&path).expect("schema-84 gather routes cross");
    assert_eq!(
        journal.migration().map(|migration| migration.from()),
        Some(84)
    );
    assert!(
        statement(&journal, &held)
            .dependencies
            .contains(&open_design),
        "a live route becomes an ordinary gated output"
    );
    assert!(workflow(&journal, &held).status.is_pending());
    assert!(
        journal
            .read(|read| read.claim_head(&held))
            .expect("claim reads")
            .expect("claim remains")
            .into_body()
            .is_active(),
        "the live output keeps its holder"
    );
    assert!(
        !workflow(&journal, &settled).status.is_pending(),
        "settled history remains readable"
    );
    for task in [concluded, absent] {
        assert!(
            matches!(
                workflow(&journal, &task).status,
                wire::WorkflowStatus::Settled {
                    conclusion: wire::WorkflowConclusion::Negative(wire::WorkflowNegative::Closed),
                    ..
                }
            ),
            "{task} closes"
        );
    }
    journal.verify().expect("retired gather crossing verifies");
}
