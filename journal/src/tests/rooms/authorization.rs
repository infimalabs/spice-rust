use super::*;

#[test]
fn authorization_selects_exactly_an_accepted_candidate() {
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
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
            },
        ),
    )
    .expect("accepted decision");
    for inexact in [
        preparation.authorization(2, 1, input.clone(), vec![preparation.spare.clone()]),
        preparation.authorization(2, 1, Vec::new(), output.clone()),
        ExecutionAuthorized {
            authorization: ExecutionAuthorization {
                coverage: vec![CoverageRef {
                    candidate: preparation.candidate(1),
                    coverage: SequenceCode::new(9),
                }],
                ..preparation
                    .authorization(2, 1, input.clone(), output.clone())
                    .authorization
            },
        },
    ] {
        let result = authorize(&mut journal, inexact);
        refused(&journal, result);
    }
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
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
            },
        ),
    )
    .expect("second acceptance");
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
fn negative_and_no_work_outcomes_authorize_nothing() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    for (sequence, conclusion) in [
        (1, NegativeDecision::Rejected),
        (2, NegativeDecision::NoWork),
    ] {
        commit(
            &mut journal,
            preparation.decision(sequence, DecisionOutcome::Negative { conclusion }),
        )
        .expect("negative decision needs no candidate");
        let result = authorize(
            &mut journal,
            preparation.authorization(
                sequence,
                1,
                vec![preparation.input.clone()],
                vec![preparation.output.clone()],
            ),
        );
        refused(&journal, result);
    }
    let result = commit(
        &mut journal,
        preparation.decision(
            3,
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
            },
        ),
    );
    refused(&journal, result);
    let granted = journal
        .read(|read| read.authorization_head(&preparation.output))
        .expect("authorization reads");
    assert_eq!(granted, None);
    journal.verify().expect("negative authority verifies");
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
fn coverage_authority_is_its_writer_and_self_review_alone_authorizes_nothing() {
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

    let accepted = preparation.decision(
        1,
        DecisionOutcome::Accepted {
            candidate: preparation.candidate(1),
        },
    );
    let result = commit_as(&mut journal, Actor::Lane(reviewer), accepted.clone());
    refused(&journal, result);
    commit(&mut journal, accepted).expect("the operator decides");

    let input = vec![preparation.input.clone()];
    let output = vec![preparation.output.clone()];
    let result = authorize(
        &mut journal,
        preparation.authorization(1, 1, input.clone(), output.clone()),
    );
    refused(&journal, result);

    commit_as(
        &mut journal,
        Actor::Lane(reviewer),
        covered_by(&preparation, 2, independent(reviewer)),
    )
    .expect("an independent lane covers the candidate");
    let covered = ExecutionAuthorized {
        authorization: ExecutionAuthorization {
            coverage: [1, 2]
                .map(|sequence| CoverageRef {
                    candidate: preparation.candidate(1),
                    coverage: SequenceCode::new(sequence),
                })
                .to_vec(),
            ..preparation.authorization(1, 1, input, output).authorization
        },
    };
    let write = authorization_write(
        &journal,
        Actor::Lane(reviewer),
        covered.clone(),
        Conclusion::Completed,
        true,
    );
    let result = journal.apply(write);
    refused(&journal, result);
    authorize(&mut journal, covered).expect("independent coverage authorizes");
    journal.verify().expect("principal-bound coverage verifies");
}

#[test]
fn authorization_lands_only_as_the_controllers_positive_completion_and_no_edit_moves_it() {
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
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
            },
        ),
    )
    .expect("accepted decision");
    let exact = preparation.authorization(
        1,
        1,
        vec![preparation.input.clone()],
        vec![preparation.output.clone()],
    );

    for (conclusion, outputs) in [
        (Conclusion::Untouched, false),
        (Conclusion::Untouched, true),
        (Conclusion::Closed, true),
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
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
            },
        ),
    )
    .expect("accepted decision");

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
                source: wire::body::SourceBoundary::unrecorded(),
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
