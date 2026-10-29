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
    let input = vec![preparation.input.clone()];
    let output = vec![preparation.output.clone()];
    let result = authorize(
        &mut journal,
        preparation.authorization(1, 1, input.clone(), output.clone()),
    );
    refused(&journal, result);

    commit(
        &mut journal,
        preparation.decision(1, selected(preparation.candidate(1), output.clone())),
    )
    .expect("selected decision");
    for inexact in [
        preparation.authorization(1, 1, input.clone(), vec![preparation.spare.clone()]),
        preparation.authorization(1, 1, Vec::new(), output.clone()),
    ] {
        let result = authorize(&mut journal, inexact);
        refused(&journal, result);
    }
    let mut covered = preparation.authorization(1, 1, input.clone(), output.clone());
    covered.authorization.coverage.push(CoverageRef {
        candidate: preparation.candidate(1),
        coverage: SequenceCode::new(1),
    });
    let result = authorize(&mut journal, covered);
    refused(&journal, result);
    authorize(
        &mut journal,
        preparation.authorization(1, 1, input.clone(), output.clone()),
    )
    .expect("exact selection authorized");

    // A sealed selection admits no second authorization of the same output.
    let result = authorize(
        &mut journal,
        preparation.authorization(1, 1, input.clone(), output.clone()),
    );
    refused(&journal, result);
    let result = authorize(&mut journal, preparation.authorization(2, 1, input, output));
    refused(&journal, result);
    journal.verify().expect("exact authority verifies");
}

#[test]
fn a_readded_design_edge_admits_another_decision_for_the_same_output() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let output = vec![preparation.output.clone()];
    let input = vec![preparation.input.clone()];

    for (candidate, decision) in [(1, 1), (2, 2)] {
        if candidate == 2 {
            set_dependencies(
                &mut journal,
                &preparation.output,
                vec![preparation.controller.clone()],
            );
        }
        commit(
            &mut journal,
            preparation.retained(candidate, output.clone()),
        )
        .expect("candidate retained");
        commit(
            &mut journal,
            preparation.decision(
                decision,
                selected(preparation.candidate(candidate), output.clone()),
            ),
        )
        .expect("decision recorded");
        authorize(
            &mut journal,
            preparation.authorization(decision, candidate, input.clone(), output.clone()),
        )
        .expect("exact Design edge authorized");
    }

    assert_eq!(
        projection(&journal)
            .authorizations
            .keys()
            .filter(|(authorized, _)| authorized == preparation.output.as_str())
            .count(),
        2,
        "both decisions retain their projection rows"
    );
    let latest = journal
        .read(|read| read.authorization_head(&preparation.output))
        .expect("authorization reads")
        .expect("authorization exists");
    assert_eq!(
        latest.body().decision,
        preparation.decision_ref(2),
        "the guarded read witnesses the latest decision"
    );
    journal
        .verify()
        .expect("ordinary dependency re-add verifies");
}

#[test]
fn a_collapse_request_authorizes_nothing() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    commit(
        &mut journal,
        preparation.proposal(
            1,
            wire::rooms::DesignPassRecommendation::Collapse,
            Vec::new(),
        ),
    )
    .expect("candidate retained");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
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
        selected(preparation.candidate(1), vec![preparation.output.clone()]),
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
fn authorization_releases_its_output_and_leaves_its_controller_open() {
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
            selected(preparation.candidate(1), vec![preparation.output.clone()]),
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
    let released = statement(&journal, &preparation.output);
    assert!(!released.dependencies.contains(&preparation.controller));
    let controller = journal
        .read(|read| read.workflow_head(&preparation.controller))
        .expect("controller workflow reads")
        .expect("controller exists")
        .into_body();
    assert!(
        controller.status.is_pending(),
        "selection leaves the Design open"
    );
    edit_project(&mut journal, &preparation.output, "elsewhere");
    assert!(
        !statement(&journal, &preparation.output)
            .dependencies
            .contains(&preparation.controller)
    );

    let later = preparation.retained(2, vec![preparation.output.clone()]);
    let result = commit(&mut journal, later.clone());
    refused(&journal, result);

    let snapshot = journal.fact_snapshot().expect("fact snapshot reads");
    let appended = Seq::new(
        snapshot
            .horizon()
            .expect("authorization has a journal position")
            .get()
            + 1,
    )
    .expect("next replay position");
    let actor = Actor::Operator;
    let at = instant();
    let fact: AnyFact =
        wire::decode(&wire::encode(&wire::Fact { body: later }).expect("later candidate encodes"))
            .expect("later candidate decodes as an opaque fact");
    let mut facts = snapshot
        .facts()
        .iter()
        .map(|envelope| (envelope.seq, &envelope.actor, envelope.at, &envelope.fact))
        .collect::<Vec<_>>();
    facts.push((appended, &actor, at, &fact));
    let (_, refusals) = crate::rooms::History::rebuild(facts);
    assert!(
        refusals.iter().any(|(seq, refusal)| {
            *seq == appended && refusal.to_string().contains("is not gated by")
        }),
        "replay must reject a candidate after its Design edge was released: {refusals:?}"
    );

    set_dependencies(
        &mut journal,
        &preparation.output,
        vec![preparation.controller.clone()],
    );
    commit(
        &mut journal,
        preparation.retained(2, vec![preparation.output.clone()]),
    )
    .expect("an ordinary edit can restore the Design gate");

    journal.verify().expect("settled authorization verifies");
}

/// A lease renewal extends the claim a Design pass names without moving it, so
/// replay admits the pass the live commit admitted.
#[test]
fn a_design_pass_names_its_claim_across_lease_renewals() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let mut renewal = JournalWrite::new(
        Actor::Spice,
        "task:rooms".parse().expect("room origin"),
        instant(),
        Producer::BusTimer,
    );
    renewal
        .fact(
            wire::body::ClaimRenewed {
                task: preparation.controller.clone(),
                epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163d3"),
                until: instant(),
                maximum: instant(),
            },
            FactCause::Root,
        )
        .expect("renewal builds");
    journal.apply(renewal).expect("renewal commits");
    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("the pass names the renewed claim");
    journal
        .verify()
        .expect("replay admits the pass across the renewal");
}

/// A retained manual renewal of a claim that trailed its task's generation
/// rewrote the claim head at its renewal, so the next Design pass named that
/// renewal. A current claim's renewal moved only its lease. Replay preserves
/// both historical forms exactly as their live commits did.
#[test]
fn a_design_pass_names_the_claim_head_a_retained_stale_renewal_rewrote() {
    let mut journal = Journal::in_memory().expect("journal");
    let mut preparation = Preparation::new(&mut journal);
    let controller = preparation.controller.clone();
    let at = |text: &str| -> Instant { text.parse().expect("fixture instant") };

    retained_renewal(
        &mut journal,
        &controller,
        at("2030-04-12T18:45:00.000Z"),
        false,
    );
    let head = statement(&journal, &controller);
    let mut edit = JournalWrite::new(
        Actor::Lane(uuid("018f82d4-6f59-7d06-92e3-bce9131163d9")),
        "task:rooms".parse().expect("room origin"),
        at("2030-04-12T18:46:00.000Z"),
        Producer::CommandDoor,
    );
    let edited = edit
        .fact(
            wire::body::TaskEdited {
                task: controller.clone(),
                revision: u32::try_from(head.revision.get() + 1).expect("fixture revision fits"),
                dependencies: None,
                intent: None,
            },
            FactCause::Root,
        )
        .expect("peer edit builds");
    edit.task_head(edited, move |revision| TaskHead {
        project: "elsewhere".to_owned(),
        revision,
        ..head
    });
    preparation.statement = journal.apply(edit).expect("peer edit commits").first();
    preparation.claim = retained_renewal(
        &mut journal,
        &controller,
        at("2030-04-12T18:47:00.000Z"),
        true,
    );
    retained_renewal(
        &mut journal,
        &controller,
        at("2030-04-12T18:48:00.000Z"),
        false,
    );

    commit(
        &mut journal,
        preparation.retained(1, vec![preparation.output.clone()]),
    )
    .expect("live admits the pass naming the renewal's claim head");
    journal
        .verify()
        .expect("replay admits the pass the live commit admitted");
}

/// Reproduce a retained manual renewal, optionally with the historical
/// generation acknowledgement that rewrote the claim head at the same fact.
fn retained_renewal(
    journal: &mut Journal,
    task: &TaskStamp,
    at: Instant,
    acknowledge: bool,
) -> Seq {
    let claim = journal
        .read(|read| read.claim_head(task))
        .expect("claim reads")
        .expect("claim exists")
        .into_body();
    let generation = journal
        .read(|read| read.workflow_head(task))
        .expect("workflow reads")
        .expect("workflow exists")
        .into_body()
        .generation;
    let mut write = JournalWrite::new(
        Actor::Operator,
        "task:rooms".parse().expect("room origin"),
        at,
        Producer::BusTimer,
    );
    let renewed = write
        .fact(
            wire::body::ClaimRenewed {
                task: task.clone(),
                epoch: claim.epoch,
                until: at,
                maximum: at,
            },
            FactCause::Root,
        )
        .expect("renewal builds");
    if acknowledge {
        write.claim_head(task.clone(), renewed, move |fact| wire::ClaimHead {
            fact,
            acknowledged: generation,
            ..claim
        });
    }
    journal.apply(write).expect("renewal commits").first()
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
            selected(preparation.candidate(1), vec![preparation.output.clone()]),
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
                disposition: None,
                terminal: None,
                git: None,
                source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
            },
            FactCause::Root,
        )
        .expect("completion builds");
    write.workflow_head(controller, done, move |_| WorkflowHead {
        status: wire::WorkflowStatus::completed(instant()),
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
        matches!(&result, Err(CommitError::Refused(_))),
        "authorization cannot follow completion: {result:?}"
    );
    refused(&journal, result);
    assert!(
        statement(&journal, &preparation.output)
            .dependencies
            .contains(&preparation.controller)
    );
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
            status: wire::body::TaskCompletionStatus::Pending,
            finding: Some("candidate should account for this finding".to_owned()),
            note: None,
            followups: Vec::new(),
            git: None,
            source: wire::body::SourceBoundary::Proven { heads: Vec::new() },
        },
    )
    .expect("review evidence records");
    let DesignProposed { mut candidate } =
        preparation.retained(1, vec![preparation.output.clone()]);
    candidate.references = vec![CandidateReference {
        relation: "review".parse().expect("relation"),
        source: RoomSource::Journal {
            fact: SequenceCode::new(review.last().get()),
        },
    }];
    commit(&mut journal, DesignProposed { candidate }).expect("review citation is admitted");
    commit(
        &mut journal,
        preparation.decision(
            1,
            selected(preparation.candidate(1), vec![preparation.output.clone()]),
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

fn restore_candidate(
    candidate: &mut serde_json::Value,
    governed_inputs: &[TaskStamp],
    outputs: &[TaskStamp],
) {
    let candidate = candidate
        .as_object_mut()
        .expect("proposal candidate is an object");
    for field in ["statement", "claim", "recommendation", "justification"] {
        candidate.remove(field);
    }
    candidate.insert(
        "governed_inputs".to_owned(),
        serde_json::to_value(governed_inputs).expect("governed inputs encode"),
    );
    candidate.insert(
        "outputs".to_owned(),
        serde_json::to_value(outputs).expect("outputs encode"),
    );
}
fn retain_predecessor_proposal(
    journal: &Journal,
    applied: &crate::guard::Applied,
    proposal: &DesignPassProposal,
    legacy_governed_inputs: &[TaskStamp],
    legacy_outputs: &[TaskStamp],
    command_id: &str,
    witness_claim: bool,
) {
    let seq = applied.first();
    let seq_value = i64::try_from(seq.get()).expect("fixture sequence fits SQLite");
    let bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            [seq_value],
            |row| row.get(0),
        )
        .expect("proposal fact reads");
    let mut envelope: serde_json::Value =
        serde_json::from_slice(&bytes).expect("proposal envelope decodes");
    restore_candidate(
        &mut envelope["fact"]["candidate"],
        legacy_governed_inputs,
        legacy_outputs,
    );
    journal
        .connection
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
            params![
                serde_json::to_vec(&envelope).expect("predecessor envelope encodes"),
                seq_value,
            ],
        )
        .expect("restore predecessor proposal fact");

    let subject = crate::rooms::subject(&ProposalRef::Candidate {
        candidate: proposal.candidate.clone(),
    })
    .expect("proposal subject encodes");
    let bytes: Vec<u8> = journal
        .connection
        .query_row(
            "SELECT body FROM proposal_heads WHERE subject = ?1",
            [&subject],
            |row| row.get(0),
        )
        .expect("proposal head reads");
    let mut head: serde_json::Value =
        serde_json::from_slice(&bytes).expect("proposal head decodes");
    restore_candidate(&mut head, legacy_governed_inputs, legacy_outputs);
    journal
        .connection
        .execute(
            "UPDATE proposal_heads SET body = ?1 WHERE subject = ?2",
            params![
                serde_json::to_vec(&head).expect("predecessor head encodes"),
                subject,
            ],
        )
        .expect("restore predecessor proposal head");

    let mut expected = vec![Witness::Task {
        task: proposal.candidate.controller.clone(),
        head: proposal.statement,
    }];
    if witness_claim {
        expected.push(Witness::Claim {
            task: proposal.candidate.controller.clone(),
            head: proposal.claim,
        });
    }
    let command = Command {
        v: WireVersion::CURRENT,
        id: uuid(command_id),
        epoch: uuid("018f82d4-6f59-7d06-92e3-bce9131163c3"),
        kind: CommandKind::DesignPropose,
        expected,
        body: wire::rooms::DesignPropose {
            candidate: proposal.clone(),
        },
    };
    let mut request = serde_json::to_value(&command).expect("proposal command encodes");
    restore_candidate(
        &mut request["body"]["candidate"],
        legacy_governed_inputs,
        legacy_outputs,
    );
    let response = Response {
        v: WireVersion::CURRENT,
        id: command.id,
        epoch: command.epoch,
        outcome: Outcome::Reply(Reply {
            position: ReplyPosition::Committed(SeqRange {
                first: applied.first(),
                last: applied.last(),
            }),
            body: json!({"text": "candidate retained"}),
        }),
    };
    journal
        .connection
        .execute(
            "INSERT INTO command_receipts(
                 command_id, actor, command_kind, request, evaluated_witnesses, response,
                 first_seq, last_seq, snapshot_seq, epoch
             ) VALUES (?1, 'operator', ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8)",
            params![
                command.id.to_string(),
                command.kind.as_str(),
                wire::encode(&request).expect("predecessor request encodes"),
                wire::encode(&command.expected).expect("witnesses encode"),
                wire::encode(&response).expect("response encodes"),
                i64::try_from(applied.first().get()).expect("fixture sequence fits SQLite"),
                i64::try_from(applied.last().get()).expect("fixture sequence fits SQLite"),
                command.epoch.to_string(),
            ],
        )
        .expect("insert predecessor proposal receipt");
}

fn restore_design_pass_predecessor(journal: &Journal) {
    journal
        .connection
        .execute_batch(&format!(
            "DROP TABLE release_corrections;
             DROP TABLE release_landings;
             DROP TABLE oops_generation_head;
             DROP TABLE design_shadows; {}
             UPDATE journal_schema SET version = 72 WHERE singleton = 1;
             PRAGMA user_version = 72;",
            crate::tests::PREDECESSOR_STEERING_HEADS
        ))
        .expect("restore Design pass predecessor schema");
}

fn retained_design_proposal(journal: &Journal, candidate: CandidateRef) -> DesignPassProposal {
    let authority = journal
        .read(|read| read.proposal_head(&ProposalRef::Candidate { candidate }))
        .expect("proposal reads")
        .expect("proposal remains")
        .into_body();
    let crate::ProposalAuthority::Candidate(proposal) = authority else {
        panic!("proposal changed authority family");
    };
    proposal
}

#[test]
fn retained_proposals_with_and_without_claim_witnesses_cross_together() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let proposed = preparation.retained(1, vec![preparation.output.clone()]);
    let proposal = proposed.candidate.clone();
    let applied = commit(&mut journal, proposed).expect("proposal records");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Resolved {
                candidate: preparation.candidate(1),
                outputs: vec![OutputDecision {
                    output: preparation.output.clone(),
                    verdict: OutputVerdict::Rejected,
                    comment: Some("revise the boundary".to_owned()),
                }],
                comment: Some("another pass is needed".to_owned()),
            },
        ),
    )
    .expect("resolved decision records");
    retain_predecessor_proposal(
        &journal,
        &applied,
        &proposal,
        std::slice::from_ref(&preparation.input),
        std::slice::from_ref(&preparation.output),
        "018f82d4-6f59-7d06-92e3-bce913116310",
        true,
    );
    let historical = preparation.retained(2, vec![preparation.output.clone()]);
    let historical_proposal = historical.candidate.clone();
    let historical_applied = commit(&mut journal, historical).expect("later proposal records");
    retain_predecessor_proposal(
        &journal,
        &historical_applied,
        &historical_proposal,
        std::slice::from_ref(&preparation.input),
        std::slice::from_ref(&preparation.output),
        "018f82d4-6f59-7d06-92e3-bce913116312",
        false,
    );
    journal
        .connection
        .execute_batch(
            "UPDATE fact_envelopes
             SET envelope = CAST(json_remove(
                 json_set(
                     CAST(envelope AS TEXT),
                     '$.fact.decision.outcome.kind', 'selected',
                     '$.fact.decision.outcome.selected_outputs', json_array()
                 ),
                 '$.fact.decision.outcome.outputs',
                 '$.fact.decision.outcome.comment'
             ) AS BLOB)
             WHERE kind = 'proposal::decided';
             UPDATE proposal_heads
             SET body = CAST(json_remove(
                 json_set(
                     CAST(body AS TEXT),
                     '$.outcome.kind', 'selected',
                     '$.outcome.selected_outputs', json_array()
                 ),
                 '$.outcome.outputs',
                 '$.outcome.comment'
             ) AS BLOB)
             WHERE json_type(CAST(body AS TEXT), '$.outcome.kind') IS NOT NULL;",
        )
        .expect("restore predecessor selected decision");
    restore_design_pass_predecessor(&journal);

    crate::migrate::admit(&mut journal.connection)
        .expect("Design pass predecessor crosses")
        .expect("migration records");
    let crossed = retained_design_proposal(&journal, preparation.candidate(1));
    assert_eq!(crossed.statement, preparation.statement);
    assert_eq!(crossed.claim, Some(preparation.claim));
    assert_eq!(
        crossed.recommendation,
        wire::rooms::DesignPassRecommendation::Work
    );
    assert_eq!(
        crossed.justification,
        "retained work proposal predates explicit Design pass justification"
    );
    let decision = journal
        .read(|read| {
            read.proposal_head(&ProposalRef::Decision {
                decision: preparation.decision_ref(1),
            })
        })
        .expect("historical decision reads")
        .expect("historical decision remains")
        .into_body();
    assert!(matches!(
        decision,
        crate::ProposalAuthority::Decision(PreparationDecision {
            outcome: DecisionOutcome::Resolved { .. },
            ..
        })
    ));
    journal.verify().expect("crossed work proposal replays");
    let historical = retained_design_proposal(&journal, preparation.candidate(2));
    assert_eq!(historical.statement, preparation.statement);
    assert_eq!(historical.claim, None);
}

#[test]
fn collapse_decision_crosses_to_an_outputless_accepted_proposal() {
    let mut journal = Journal::in_memory().expect("journal");
    let preparation = Preparation::new(&mut journal);
    let proposed = preparation.proposal(
        1,
        wire::rooms::DesignPassRecommendation::Collapse,
        Vec::new(),
    );
    let proposal = proposed.candidate.clone();
    let applied = commit(&mut journal, proposed).expect("collapse proposal records");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
            },
        ),
    )
    .expect("collapse acceptance records");
    retain_predecessor_proposal(
        &journal,
        &applied,
        &proposal,
        std::slice::from_ref(&preparation.input),
        std::slice::from_ref(&preparation.output),
        "018f82d4-6f59-7d06-92e3-bce913116311",
        true,
    );
    journal
        .connection
        .execute_batch(
            "UPDATE fact_envelopes
             SET envelope = CAST(json_set(
                 CAST(envelope AS TEXT),
                 '$.fact.decision.outcome.kind', 'collapse',
                 '$.fact.decision.outcome.reason', 'the premise no longer holds'
             ) AS BLOB)
             WHERE kind = 'proposal::decided';
             UPDATE proposal_heads
             SET body = CAST(json_set(
                 CAST(body AS TEXT),
                 '$.outcome.kind', 'collapse',
                 '$.outcome.reason', 'the premise no longer holds'
             ) AS BLOB)
             WHERE json_type(CAST(body AS TEXT), '$.outcome.kind') IS NOT NULL;",
        )
        .expect("restore predecessor collapse decision");
    restore_design_pass_predecessor(&journal);

    crate::migrate::admit(&mut journal.connection)
        .expect("collapse predecessor crosses")
        .expect("migration records");
    let proposal = journal
        .read(|read| {
            read.proposal_head(&ProposalRef::Candidate {
                candidate: preparation.candidate(1),
            })
        })
        .expect("proposal reads")
        .expect("proposal remains")
        .into_body();
    let crate::ProposalAuthority::Candidate(proposal) = proposal else {
        panic!("proposal changed authority family");
    };
    assert_eq!(
        proposal.recommendation,
        wire::rooms::DesignPassRecommendation::Collapse
    );
    assert!(proposal.governed_inputs.is_empty());
    assert!(proposal.outputs.is_empty());
    assert_eq!(proposal.justification, "the premise no longer holds");
    let decision = journal
        .read(|read| {
            read.proposal_head(&ProposalRef::Decision {
                decision: preparation.decision_ref(1),
            })
        })
        .expect("decision reads")
        .expect("decision remains")
        .into_body();
    assert!(matches!(
        decision,
        crate::ProposalAuthority::Decision(PreparationDecision {
            outcome: DecisionOutcome::Accepted { .. },
            ..
        })
    ));
    journal.verify().expect("crossed collapse replays");
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
                 '$.fact.decision.outcome.outputs',
                 '$.fact.decision.outcome.comment'
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
                 '$.outcome.outputs',
                 '$.outcome.comment'
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
             DROP TABLE principal_heads;
             DROP TABLE oops_generation_head;
             DROP TABLE design_shadows;
             DROP TABLE release_corrections;
             DROP TABLE release_landings;
             DROP INDEX steering_heads_lane_room_created;
             ALTER TABLE steering_heads DROP COLUMN room;
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
        selected(preparation.candidate(1), vec![preparation.output.clone()],)
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
             DROP TABLE principal_heads;
             DROP TABLE oops_generation_head;
             DROP TABLE design_shadows;
             DROP TABLE release_corrections;
             DROP TABLE release_landings;
             DROP INDEX steering_heads_lane_room_created;
             ALTER TABLE steering_heads DROP COLUMN room;
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
        preparation.proposal(
            1,
            wire::rooms::DesignPassRecommendation::Collapse,
            Vec::new(),
        ),
    )
    .expect("candidate retained");
    commit(
        &mut journal,
        preparation.decision(
            1,
            DecisionOutcome::Accepted {
                candidate: preparation.candidate(1),
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
             DROP TABLE principal_heads;
             DROP TABLE oops_generation_head;
             DROP TABLE design_shadows;
             DROP INDEX steering_heads_lane_room_created;
             ALTER TABLE steering_heads DROP COLUMN room;
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
