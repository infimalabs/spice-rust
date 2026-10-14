//! Preparation decisions, coverage authority, and execution authorization.

use super::*;

#[test]
fn authorization_matches_one_exact_selected_candidate() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    adopt(
        &mut journal,
        &preparation.spare,
        prepared_by(&preparation.controller),
        prepared_by(&preparation.controller),
    )
    .expect("spare output adopted");
    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("candidate retained");
    commit(&mut journal, preparation.coverage(1, 1)).expect("coverage");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Returned {
                candidate: preparation.candidate(1),
                reason: "revise the candidate".to_owned(),
            },
        ),
    )
    .expect("returned decision");
    let input = vec![preparation.input.clone()];
    let output = vec![preparation.output.clone()];
    let result = authorize(
        &mut journal,
        preparation.authorization(1, 1, input.clone(), output.clone()),
    );
    refused(&journal, result);

    commit(
        &mut journal,
        preparation.decision(
            2,
            DecisionOutcome::Selected {
                candidate: preparation.candidate(1),
                selected_outputs: output.clone(),
            },
        ),
    )
    .expect("selected decision");
    for inexact in [
        preparation.authorization(2, 1, input.clone(), vec![preparation.spare.clone()]),
        preparation.authorization(2, 1, Vec::new(), output.clone()),
    ] {
        let result = authorize(&mut journal, inexact);
        refused(&journal, result);
    }
    let mut covered = preparation.authorization(2, 1, input.clone(), output.clone());
    covered.authorization.coverage.push(CoverageRef {
        candidate: preparation.candidate(1),
        coverage: SequenceCode::new(1),
    });
    let result = authorize(&mut journal, covered);
    refused(&journal, result);
    authorize(
        &mut journal,
        preparation.authorization(2, 1, input.clone(), output.clone()),
    )
    .expect("exact selection authorized");

    // A sealed selection admits no second authorization of the same output.
    commit(
        &mut journal,
        preparation.decision(
            3,
            DecisionOutcome::Selected {
                candidate: preparation.candidate(1),
                selected_outputs: output.clone(),
            },
        ),
    )
    .expect("second selection");
    let result = authorize(
        &mut journal,
        preparation.authorization(3, 1, input.clone(), output.clone()),
    );
    refused(&journal, result);
    let result = authorize(&mut journal, preparation.authorization(2, 1, input, output));
    refused(&journal, result);
    journal.verify().expect("exact authority verifies");
}

#[test]
fn a_collapse_request_authorizes_nothing() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("candidate retained");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Collapse {
                candidate: preparation.candidate(1),
                reason: "the premise no longer holds".to_owned(),
            },
        ),
    )
    .expect("collapse request records");
    let result = authorize(
        &mut journal,
        preparation.authorization(
            1,
            1,
            vec![preparation.input.clone()],
            vec![preparation.output.clone()],
        ),
    );
    refused(&journal, result);
    let granted = journal
        .read(|read| read.authorization_head(&preparation.output))
        .expect("authorization reads");
    assert_eq!(granted, None);
    journal.verify().expect("collapse authority verifies");
}

fn covered_by(
    preparation: &Preparation,
    sequence: u64,
    authority: CoverageAuthority,
) -> CandidateCovered {
    CandidateCovered {
        coverage: CandidateCoverage {
            authority,
            ..preparation.coverage(1, sequence).coverage
        },
    }
}

#[test]
fn coverage_authority_is_its_writer_and_never_gates_selection() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let author = uuid("019fe57c-a453-7ec1-a400-26e6901aaeb1");
    let reviewer = uuid("019fe57c-a453-7ec1-a400-26e6901aaeb2");
    let independent = |lane| CoverageAuthority::Independent {
        lane,
        occupant: lane,
    };
    let self_review = |lane| CoverageAuthority::SelfReview {
        lane,
        occupant: lane,
    };
    commit_as(
        &mut journal,
        Actor::Lane(author),
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("a lane retains the candidate");

    for (writer, authority) in [
        (Actor::Lane(author), CoverageAuthority::Operator {}),
        (Actor::Lane(author), independent(author)),
        (Actor::Lane(reviewer), self_review(reviewer)),
        (Actor::Lane(author), independent(reviewer)),
        (Actor::Operator, independent(reviewer)),
    ] {
        let result = commit_as(&mut journal, writer, covered_by(&preparation, 1, authority));
        refused(&journal, result);
    }
    commit_as(
        &mut journal,
        Actor::Lane(author),
        covered_by(&preparation, 1, self_review(author)),
    )
    .expect("the author labels its own review");

    let selected = preparation.decision(
        1,
        DecisionOutcome::Selected {
            candidate: preparation.candidate(1),
            selected_outputs: vec![preparation.output.clone()],
        },
    );
    let result = commit_as(&mut journal, Actor::Lane(reviewer), selected.clone());
    refused(&journal, result);
    commit(&mut journal, selected).expect("the operator decides");

    let exact = preparation.authorization(
        1,
        1,
        vec![preparation.input.clone()],
        vec![preparation.output.clone()],
    );
    let write = authorization_write(
        &journal,
        Actor::Lane(reviewer),
        exact.clone(),
        Conclusion::Untouched,
        true,
    );
    let result = journal.apply(write);
    refused(&journal, result);
    authorize(&mut journal, exact).expect("coverage does not gate exact selection");

    commit_as(
        &mut journal,
        Actor::Lane(reviewer),
        covered_by(&preparation, 2, independent(reviewer)),
    )
    .expect("independent coverage remains readable evidence");
    journal.verify().expect("principal-bound coverage verifies");
}

#[test]
fn authorization_leaves_its_controller_open_and_no_edit_moves_it() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("candidate retained");
    commit(&mut journal, preparation.coverage(1, 1)).expect("coverage");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Selected {
                candidate: preparation.candidate(1),
                selected_outputs: vec![preparation.output.clone()],
            },
        ),
    )
    .expect("selected decision");
    let exact = preparation.authorization(
        1,
        1,
        vec![preparation.input.clone()],
        vec![preparation.output.clone()],
    );

    // Concluding the controller in the same write, or omitting the selected
    // output's statement, refuses.
    for (conclusion, outputs) in [
        (Conclusion::Untouched, false),
        (Conclusion::Closed, true),
        (Conclusion::Completed, true),
        (Conclusion::Completed, false),
    ] {
        let write = authorization_write(
            &journal,
            Actor::Operator,
            exact.clone(),
            conclusion,
            outputs,
        );
        let result = journal.apply(write);
        refused(&journal, result);
    }
    authorize(&mut journal, exact).expect("exact selection authorized");
    let granted = statement(&journal, &preparation.output).authorization;
    assert_eq!(granted, Some(preparation.decision_ref(1)));
    let controller = journal
        .read(|read| read.workflow_head(&preparation.controller))
        .expect("controller workflow reads")
        .expect("controller exists")
        .into_body();
    assert_eq!(
        (controller.status.as_str(), controller.completion),
        ("pending", None),
        "selection leaves the Design open"
    );

    // An ordinary edit keeps the grant; no edit drops it or forges another.
    edit_project(&mut journal, &preparation.output, "elsewhere");
    assert_eq!(
        statement(&journal, &preparation.output).authorization,
        granted
    );
    for (task, authorization) in [
        (&preparation.output, None),
        (&preparation.spare, granted.clone()),
    ] {
        let head = statement(&journal, task);
        let mut write = write();
        let fact = write
            .fact(
                wire::body::TaskEdited {
                    task: task.clone(),
                    revision: 3,
                    dependencies: None,
                    intent: None,
                    evidence: None,
                },
                FactCause::Root,
            )
            .expect("edit fact builds");
        write.task_head(fact, move |revision| TaskHead {
            authorization,
            revision,
            ..head
        });
        let result = journal.apply(write);
        assert!(
            matches!(&result, Err(CommitError::Refused(refusal))
                if refusal.to_string().contains("statement authorization")),
            "an edit cannot move an authorization: {result:?}"
        );
        refused(&journal, result);
    }
    journal.verify().expect("settled authorization verifies");
}

#[test]
fn an_associated_task_keeps_the_intent_it_was_created_with() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);

    let result = declare_plan(&mut journal, &preparation.output);
    refused(&journal, result);
    adopt(
        &mut journal,
        &preparation.spare,
        prepared_by(&preparation.controller),
        prepared_by(&preparation.controller),
    )
    .expect("spare adopted");
    let result = declare_plan(&mut journal, &preparation.spare);
    refused(&journal, result);

    declare_plan(&mut journal, &preparation.input).expect("an unassociated task declares intent");
    journal.verify().expect("associated intent stays fixed");
}

#[test]
fn a_controller_concluded_without_authorization_never_authorizes() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("candidate retained");
    commit(&mut journal, preparation.coverage(1, 1)).expect("coverage");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Selected {
                candidate: preparation.candidate(1),
                selected_outputs: vec![preparation.output.clone()],
            },
        ),
    )
    .expect("selected decision");

    let controller = preparation.controller.clone();
    let workflow = journal
        .read(|read| read.workflow_head(&controller))
        .expect("controller workflow reads")
        .expect("controller exists")
        .into_body();
    let mut write = write();
    let done = write
        .fact(
            wire::body::TaskDone {
                task: controller.clone(),
                validation: "ordinary completion".to_owned(),
                terminal: None,
                git: None,
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Root,
        )
        .expect("completion builds");
    write.workflow_head(controller, done, move |_| WorkflowHead {
        status: "completed".to_owned(),
        completion: Some(instant()),
        ..workflow
    });
    journal.apply(write).expect("ordinary completion commits");

    let result = authorize(
        &mut journal,
        preparation.authorization(
            1,
            1,
            vec![preparation.input.clone()],
            vec![preparation.output.clone()],
        ),
    );
    assert!(
        matches!(&result, Err(CommitError::Refused(refusal))
            if refusal.to_string().contains("already concluded")),
        "authorization cannot follow completion: {result:?}"
    );
    refused(&journal, result);
    assert_eq!(statement(&journal, &preparation.output).authorization, None);
    journal.verify().expect("unauthorized completion verifies");
}

#[test]
fn task_review_fact_is_candidate_evidence_and_never_an_authorization_gate() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let review = commit(
        &mut journal,
        wire::body::TaskReviewed {
            task: preparation.input.clone(),
            reviewer: Actor::Operator,
            verdict: wire::body::Verdict::Finding,
            finding: Some("candidate should account for this finding".to_owned()),
            note: None,
            followups: Vec::new(),
            git: None,
            source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
        },
    )
    .expect("review evidence records");
    let CandidateRetained { mut candidate } =
        preparation.retained(1, vec![preparation.output.clone()]);
    candidate.references = vec![CandidateReference {
        relation: "review".parse().expect("relation"),
        source: RoomSource::Journal {
            fact: SequenceCode::new(review.last().get()),
        },
    }];
    commit(&mut journal, CandidateRetained { candidate }).expect("review citation is admitted");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Selected {
                candidate: preparation.candidate(1),
                selected_outputs: vec![preparation.output.clone()],
            },
        ),
    )
    .expect("the operator selects without coverage");
    authorize(
        &mut journal,
        preparation.authorization(
            1,
            1,
            vec![preparation.input.clone()],
            vec![preparation.output.clone()],
        ),
    )
    .expect("review citation is evidence rather than an authorization gate");
    journal.verify().expect("review citation replays");
}

#[test]
fn retained_two_step_selection_crosses_to_one_exact_decision() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = authorized(&mut journal);
    journal
        .connection
        .execute_batch(
            "UPDATE fact_envelopes
             SET envelope = CAST(json_remove(
                 json_set(CAST(envelope AS TEXT), '$.fact.decision.outcome.kind', 'accepted'),
                 '$.fact.decision.outcome.selected_outputs'
             ) AS BLOB)
             WHERE kind = 'proposal::decided';
             UPDATE fact_envelopes
             SET envelope = CAST(json_set(
                 CAST(envelope AS TEXT),
                 '$.fact.authorization.coverage',
                 json_array(json_object(
                     'candidate',
                     json_extract(
                         CAST(envelope AS TEXT),
                         '$.fact.authorization.candidate'
                     ),
                     'coverage',
                     '1'
                 ))
             ) AS BLOB)
             WHERE kind = 'proposal::authorized';
             UPDATE proposal_heads
             SET body = CAST(json_remove(
                 json_set(CAST(body AS TEXT), '$.outcome.kind', 'accepted'),
                 '$.outcome.selected_outputs'
             ) AS BLOB)
             WHERE json_type(CAST(body AS TEXT), '$.outcome.kind') IS NOT NULL;
             UPDATE authorization_heads
             SET body = CAST(json_set(
                 CAST(body AS TEXT),
                 '$.coverage',
                 json_array(json_object(
                     'candidate',
                     json_extract(CAST(body AS TEXT), '$.candidate'),
                     'coverage',
                     '1'
                 ))
             ) AS BLOB);
             UPDATE journal_schema SET version = 55 WHERE singleton = 1;
             PRAGMA user_version = 55;",
        )
        .expect("restore the retained two-step shape");

    let migration = crate::migrate::admit(&mut journal.connection)
        .expect("the exact retained selection crosses")
        .expect("a predecessor crossing is recorded");
    assert_eq!((migration.from(), migration.to()), (55, SCHEMA_VERSION));

    let decision = journal
        .read(|read| {
            read.proposal_head(&ProposalRef::Decision {
                decision: preparation.decision_ref(1),
            })
        })
        .expect("decision head reads")
        .expect("decision head remains")
        .into_body();
    let crate::ProposalAuthority::Decision(decision) = decision else {
        panic!("decision head changed proposal family");
    };
    assert_eq!(
        decision.outcome,
        DecisionOutcome::Selected {
            candidate: preparation.candidate(1),
            selected_outputs: vec![preparation.output.clone()],
        }
    );
    let authorization = journal
        .read(|read| read.authorization_head(&preparation.output))
        .expect("authorization reads")
        .expect("authorization remains")
        .into_body();
    assert!(
        authorization.coverage.is_empty(),
        "retired coverage stays evidence rather than authorization"
    );
    journal.verify().expect("crossed authority replays");
}

#[test]
fn retained_two_step_return_gains_an_explicit_migration_reason() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("candidate retained");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Returned {
                candidate: preparation.candidate(1),
                reason: "current reason".to_owned(),
            },
        ),
    )
    .expect("return recorded");
    journal
        .connection
        .execute_batch(
            "UPDATE fact_envelopes
             SET envelope = CAST(json_remove(
                 CAST(envelope AS TEXT),
                 '$.fact.decision.outcome.reason'
             ) AS BLOB)
             WHERE kind = 'proposal::decided';
             UPDATE proposal_heads
             SET body = CAST(json_remove(
                 CAST(body AS TEXT),
                 '$.outcome.reason'
             ) AS BLOB)
             WHERE json_type(CAST(body AS TEXT), '$.outcome.kind') IS NOT NULL;
             UPDATE journal_schema SET version = 55 WHERE singleton = 1;
             PRAGMA user_version = 55;",
        )
        .expect("restore the reasonless retained return");

    crate::migrate::admit(&mut journal.connection)
        .expect("the retained return crosses")
        .expect("a predecessor crossing is recorded");
    let decision = journal
        .read(|read| {
            read.proposal_head(&ProposalRef::Decision {
                decision: preparation.decision_ref(1),
            })
        })
        .expect("decision head reads")
        .expect("decision head remains")
        .into_body();
    let crate::ProposalAuthority::Decision(decision) = decision else {
        panic!("decision head changed proposal family");
    };
    assert!(matches!(
        decision.outcome,
        DecisionOutcome::Returned { candidate, reason }
            if candidate == preparation.candidate(1)
                && reason == "returned before preparation decisions required an explicit reason"
    ));
    journal.verify().expect("crossed return replays");
}

#[test]
fn candidate_less_retained_decision_refuses_with_its_owning_release() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("candidate retained");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Collapse {
                candidate: preparation.candidate(1),
                reason: "current reason".to_owned(),
            },
        ),
    )
    .expect("collapse request recorded");
    journal
        .connection
        .execute_batch(
            "UPDATE fact_envelopes
             SET envelope = CAST(json_remove(
                 json_set(
                     CAST(envelope AS TEXT),
                     '$.fact.decision.outcome.kind', 'negative',
                     '$.fact.decision.outcome.conclusion', json_object('kind', 'rejected')
                 ),
                 '$.fact.decision.outcome.candidate',
                 '$.fact.decision.outcome.reason'
             ) AS BLOB)
             WHERE kind = 'proposal::decided';
             UPDATE proposal_heads
             SET body = CAST(json_remove(
                 json_set(
                     CAST(body AS TEXT),
                     '$.outcome.kind', 'negative',
                     '$.outcome.conclusion', json_object('kind', 'rejected')
                 ),
                 '$.outcome.candidate',
                 '$.outcome.reason'
             ) AS BLOB)
             WHERE json_type(CAST(body AS TEXT), '$.outcome.kind') IS NOT NULL;
             UPDATE journal_schema SET version = 55 WHERE singleton = 1;
             PRAGMA user_version = 55;",
        )
        .expect("restore candidate-less retained decision");

    let error = crate::migrate::admit(&mut journal.connection)
        .expect_err("a candidate-less decision cannot cross exactly");
    assert!(
        matches!(
            &error,
            OpenError::Migration(MigrationError::RetainedRow { table, release, .. })
                if *table == "fact_envelopes" && *release == "128e75e97"
        ),
        "{error}"
    );
    assert_eq!(
        journal
            .connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .expect("schema identity reads"),
        55,
        "a refused crossing leaves the predecessor identity"
    );
}
