use super::*;

use wire::rooms::{
    CandidateCoverage, CandidateCoverageRecord, CandidateCritique, CandidateCritiqueRecord,
    CandidateRef, CandidateReference, CandidateRetain, CoverageAuthority, CoverageRef, CritiqueRef,
    CritiqueStanding, DecisionOutcome, DecisionRef, OutputAdopt, OutputDecision, OutputVerdict,
    PreparationDecide, PreparationDecision, ProposalRef, RetainedCandidate, RoomAuthorityStamp,
    RoomSource,
};

fn add_plan(bus: &Bus, epoch: CanonicalUuid, name: &str) -> TaskHandle {
    add_controller(bus, epoch, name, wire::TaskIntent::Plan, None)
}

pub(super) fn add_controller(
    bus: &Bus,
    epoch: CanonicalUuid,
    name: &str,
    intent: wire::TaskIntent,
    flow: Option<&str>,
) -> TaskHandle {
    let born = stamp(name);
    let task = TaskHandle::for_project("board.test", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            preparation: None,
            intent: Some(intent),
            title: name.to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The exact selected output is justified".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some("fixture:preparation".parse().expect("fixture origin")),
            flow: flow.map(str::to_owned),
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    ));
    task
}

fn preparation_page(root: &FixtureRoot, words: &[&str]) -> serde_json::Value {
    let matches = crate::cli::command()
        .try_get_matches_from(["spice", "preparation", "show"].iter().chain(words))
        .expect("preparation read parses");
    let contract = crate::cli::selected_contract(&matches).expect("preparation read is registered");
    let journal = journal::JournalReader::open(root.as_ref().join("authority.sqlite3"))
        .expect("fixture journal opens to read");
    let outcome = crate::preparation_command::answer(
        &journal,
        crate::cli::selected_matches(&matches),
        contract.path,
    );
    let Outcome::Reply(reply) = outcome else {
        panic!("preparation read returned another outcome: {outcome:?}");
    };
    serde_json::to_value(reply.body).expect("preparation read encodes")
}

pub(super) fn transition_witnesses(task: &BoardTask) -> Vec<Witness> {
    vec![
        statement_witness(task),
        workflow_witness(task),
        claim_witness(task),
    ]
}

fn journal_source(answer: &BoardAnswer, task: &TaskHandle) -> RoomSource {
    RoomSource::Journal {
        fact: wire::SequenceCode::new(at(answer, task).statement.position.get()),
    }
}

#[test]
fn preparation_commands_derive_review_authority_and_authorize_only_the_exact_selection() {
    let (root, bus, epoch, seats) = fixture("preparation-command-loop", 2);
    let controller = add_controller(
        &bus,
        epoch,
        "Preparation controller",
        wire::TaskIntent::Design,
        None,
    );
    let output = add_output(&bus, epoch, &controller, "Selected output");
    let spare = add_output(&bus, epoch, &controller, "Unselected output");
    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &controller, &output, &spare);
    record_test_critique(&bus, epoch, &seats, &controller, &candidate, &source);
    let _coverage = record_test_coverage(&bus, epoch, &seats, &controller, &candidate, &source);
    authorize_test_output(&bus, epoch, &controller, &output, &spare, candidate);
    assert_eq!(
        open_workflow_waiting(&bus, epoch),
        [spare.born().expect("spare output stamp")].into(),
        "the authorized controller releases only its selected output"
    );
    let first = preparation_page(&root, &[controller.as_str(), "--limit", "2"]);
    assert_eq!(first["controller"], controller.as_str());
    assert_eq!(first["page"]["rows"].as_array().map(Vec::len), Some(2));
    assert_eq!(first["page"]["omission"]["shown"], 2);
    let mut continuation: Vec<String> =
        serde_json::from_value(first["page"]["omission"]["continuation"].clone())
            .expect("preparation continuation is one command");
    let limit = continuation
        .iter()
        .position(|word| word == "--limit")
        .expect("continuation retains its row limit");
    continuation[limit + 1] = "20".to_owned();
    let words = continuation[2..]
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let second = preparation_page(&root, &words);
    assert!(second["page"]["omission"].is_null());
    let rows = first["page"]["rows"]
        .as_array()
        .expect("first preparation rows")
        .iter()
        .chain(
            second["page"]["rows"]
                .as_array()
                .expect("continued preparation rows"),
        )
        .collect::<Vec<_>>();
    let record = |kind: &str| {
        rows.iter()
            .copied()
            .find(|row| row["kind"] == kind)
            .unwrap_or_else(|| panic!("preparation output names {kind}: {rows:?}"))
    };
    assert_eq!(
        record("candidate")["candidate"]["outputs"],
        serde_json::json!([
            output.born().expect("selected output stamp"),
            spare.born().expect("spare output stamp")
        ])
    );
    assert_eq!(
        record("critique")["critique"]["source"],
        serde_json::to_value(&source).expect("source encodes")
    );
    assert!(
        rows.iter()
            .filter(|row| row["kind"] == "coverage")
            .all(|row| row["coverage"]["source"] == serde_json::to_value(&source).unwrap())
    );
    let outcomes = rows
        .iter()
        .filter(|row| row["kind"] == "decision")
        .map(|row| row["decision"]["outcome"]["kind"].as_str())
        .collect::<Vec<_>>();
    assert!(outcomes.contains(&Some("resolved")), "{outcomes:?}");
    assert_eq!(
        record("authorization")["authorization"]["selected_outputs"],
        serde_json::json!([output.born().expect("selected output stamp")])
    );

    clean(&root, bus);
}

#[test]
fn operator_selection_releases_outputs_and_leaves_a_held_design_open() {
    let (root, bus, epoch, seats) = fixture("preparation-held-selection", 2);
    let controller = add_controller(
        &bus,
        epoch,
        "Held preparation controller",
        wire::TaskIntent::Design,
        None,
    );
    group_design(&bus, epoch, &controller);
    claim_task(
        &bus,
        epoch,
        &seats[0],
        &Actor::Lane(seats[0].lane),
        &controller,
    );
    let output = add_output(&bus, epoch, &controller, "Selected held output");
    let spare = add_output(&bus, epoch, &controller, "Unselected held output");
    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &controller, &output, &spare);
    record_test_critique(&bus, epoch, &seats, &controller, &candidate, &source);
    let _coverage = record_test_coverage(&bus, epoch, &seats, &controller, &candidate, &source);
    assert!(
        open_awaiting_selection(&bus, epoch).is_empty(),
        "the active claim retains its in-flight standing"
    );

    authorize_test_output(&bus, epoch, &controller, &output, &spare, candidate);

    let current = board(&bus, epoch, Actor::Operator);
    assert!(
        matches!(
            at(&current, &controller).claim.as_ref(),
            Some(claim) if claim.body.disposition == ClaimDisposition::Active
        ),
        "selection never takes a Design from its holder"
    );
    let gated = open_gated(&bus, epoch);
    assert!(
        !gated.contains(&output.born().expect("output stamp")),
        "the selected output runs while its Design stays open"
    );
    assert!(gated.contains(&spare.born().expect("spare stamp")));
    assert!(
        open_awaiting_selection(&bus, epoch).is_empty(),
        "the held Design remains in flight while its spare waits neutrally"
    );
    clean(&root, bus);
}

#[test]
fn later_candidate_versions_release_additional_exact_output_subsets() {
    let (root, bus, epoch, seats) = fixture("preparation-multi-version-selection", 1);
    let design = add_controller(
        &bus,
        epoch,
        "Versioned preparation controller",
        wire::TaskIntent::Design,
        None,
    );
    let first = add_output(&bus, epoch, &design, "First version output");
    let second = add_output(&bus, epoch, &design, "Later version output");
    let (initial, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&first, &second],
        1_760_000_000_000_001,
    );
    let first_decision = select_candidate_outputs(&bus, epoch, &design, initial, &[&first], 1);
    let after_first = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&after_first, &first).statement.body.authorization,
        Some(first_decision.clone())
    );
    assert_eq!(at(&after_first, &second).statement.body.authorization, None);

    let (later, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&second],
        1_760_000_000_000_002,
    );
    let second_decision = select_candidate_outputs(&bus, epoch, &design, later, &[&second], 2);
    let current = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&current, &first).statement.body.authorization,
        Some(first_decision),
        "a later candidate never revises prior authority"
    );
    assert_eq!(
        at(&current, &second).statement.body.authorization,
        Some(second_decision)
    );
    assert!(
        !open_awaiting_selection(&bus, epoch).contains(&design.born().expect("Design stamp")),
        "all direct outputs now carry exact selection authority"
    );
    assert_eq!(
        (
            at(&current, &design).workflow.body.status.as_str(),
            at(&current, &design).workflow.body.completion,
        ),
        ("pending", None),
        "selections never conclude the Design"
    );
    clean(&root, bus);
}

pub(super) fn retain_test_candidate(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &[Seat],
    controller: &TaskHandle,
    output: &TaskHandle,
    spare: &TaskHandle,
) -> (CandidateRef, RoomSource) {
    retain_candidate_version(
        bus,
        epoch,
        &seats[0],
        controller,
        &[output, spare],
        1_760_000_000_000_001,
    )
}

fn retain_candidate_version(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    controller: &TaskHandle,
    outputs: &[&TaskHandle],
    candidate_micros: i64,
) -> (CandidateRef, RoomSource) {
    let current = board(bus, epoch, Actor::Lane(seat.lane));
    let candidate = CandidateRef {
        controller: controller.born().expect("controller stamp"),
        candidate: RoomAuthorityStamp::for_micros(candidate_micros).expect("candidate identity"),
    };
    let retained = RetainedCandidate {
        candidate: candidate.clone(),
        governed_inputs: Vec::new(),
        outputs: outputs
            .iter()
            .map(|output| output.born().expect("output stamp"))
            .collect(),
        evidence: Vec::new(),
        references: vec![CandidateReference {
            relation: "contract".parse().expect("relation"),
            source: journal_source(&current, controller),
        }],
    };
    let mut expected = vec![Witness::Proposal {
        proposal: ProposalRef::Candidate {
            candidate: candidate.clone(),
        },
        head: None,
    }];
    expected.extend(transition_witnesses(at(&current, controller)));
    for output in outputs {
        expected.extend(transition_witnesses(at(&current, output)));
    }
    assert_committed(issue(
        bus,
        epoch,
        Actor::Lane(seat.lane),
        CommandKind::CandidateRetain,
        expected,
        &CandidateRetain {
            candidate: retained,
        },
    ));
    (candidate, journal_source(&current, controller))
}

fn record_test_critique(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &[Seat],
    controller: &TaskHandle,
    candidate: &CandidateRef,
    source: &RoomSource,
) {
    let critique = CandidateCritique {
        critique: CritiqueRef {
            candidate: candidate.clone(),
            critique: wire::SequenceCode::new(1),
        },
        category: "composition".parse().expect("category"),
        source: source.clone(),
        standing: CritiqueStanding::Clean,
    };
    let current = board(&bus, epoch, Actor::Lane(seats[0].lane));
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Lane(seats[0].lane),
        CommandKind::CandidateCritiqueRecord,
        transition_witnesses(at(&current, &controller))
            .into_iter()
            .chain([Witness::Proposal {
                proposal: ProposalRef::Critique {
                    critique: critique.critique.clone(),
                },
                head: None,
            }])
            .collect(),
        &CandidateCritiqueRecord { critique },
    ));
}

pub(super) fn record_test_coverage(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &[Seat],
    controller: &TaskHandle,
    candidate: &CandidateRef,
    source: &RoomSource,
) -> Vec<CoverageRef> {
    let mut coverage_refs = Vec::new();
    for (index, seat) in seats.iter().enumerate() {
        let coverage = CoverageRef {
            candidate: candidate.clone(),
            coverage: wire::SequenceCode::new(index as u64 + 1),
        };
        let current = board(&bus, epoch, Actor::Lane(seat.lane));
        assert_committed(issue(
            &bus,
            epoch,
            Actor::Lane(seat.lane),
            CommandKind::CandidateCoverageRecord,
            transition_witnesses(at(&current, &controller))
                .into_iter()
                .chain([Witness::Proposal {
                    proposal: ProposalRef::Coverage {
                        coverage: coverage.clone(),
                    },
                    head: None,
                }])
                .collect(),
            &CandidateCoverageRecord {
                coverage: CandidateCoverage {
                    coverage: coverage.clone(),
                    authority: CoverageAuthority::Operator {},
                    scopes: vec!["contracts".parse().expect("coverage scope")],
                    source: source.clone(),
                },
            },
        ));
        coverage_refs.push(coverage);
    }
    for (coverage, expected) in [
        (
            &coverage_refs[0],
            CoverageAuthority::SelfReview {
                lane: seats[0].lane,
                occupant: seats[0].occupant,
            },
        ),
        (
            &coverage_refs[1],
            CoverageAuthority::Independent {
                lane: seats[1].lane,
                occupant: seats[1].occupant,
            },
        ),
    ] {
        let proposal = ProposalRef::Coverage {
            coverage: coverage.clone(),
        };
        let observed = bus
            .read(move |read| read.proposal_head(&proposal))
            .expect("coverage head reads")
            .expect("coverage was recorded");
        let journal::ProposalAuthority::Coverage(coverage) = observed.into_body() else {
            panic!("coverage proposal decoded as another authority")
        };
        assert_eq!(coverage.authority, expected);
    }
    coverage_refs
}

/// Issue one operator decision that authorizes nothing, witnessed as the
/// decide client witnesses it.
fn decide(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
    sequence: u64,
    source: &RoomSource,
    outcome: DecisionOutcome,
) -> Outcome<CommandAnswer> {
    let decision = DecisionRef {
        controller: controller.born().expect("controller stamp"),
        decision: wire::SequenceCode::new(sequence),
    };
    let current = board(bus, epoch, Actor::Operator);
    let mut expected = transition_witnesses(at(&current, controller));
    expected.push(candidate_witness(bus, outcome.candidate()));
    expected.push(Witness::Proposal {
        proposal: ProposalRef::Decision {
            decision: decision.clone(),
        },
        head: None,
    });
    issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::PreparationDecide,
        expected,
        &PreparationDecide {
            decision: PreparationDecision {
                decision,
                source: source.clone(),
                outcome,
            },
        },
    )
}

/// `candidate`'s current head as a decision witnesses it.
fn candidate_witness(bus: &Bus, candidate: &CandidateRef) -> Witness {
    let proposal = ProposalRef::Candidate {
        candidate: candidate.clone(),
    };
    let head = bus
        .read({
            let proposal = proposal.clone();
            move |read| read.proposal_head(&proposal)
        })
        .expect("candidate head reads")
        .expect("candidate was retained")
        .position();
    Witness::Proposal {
        proposal,
        head: Some(head),
    }
}

fn select_candidate_outputs(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
    candidate: CandidateRef,
    outputs: &[&TaskHandle],
    sequence: u64,
) -> DecisionRef {
    let decision = DecisionRef {
        controller: controller.born().expect("controller stamp"),
        decision: wire::SequenceCode::new(sequence),
    };
    let current = board(bus, epoch, Actor::Operator);
    let mut expected = transition_witnesses(at(&current, controller));
    for output in outputs {
        expected.extend(transition_witnesses(at(&current, output)));
        expected.push(Witness::Authorization {
            output: output.born().expect("output stamp"),
            head: None,
        });
    }
    expected.push(candidate_witness(bus, &candidate));
    expected.push(Witness::Proposal {
        proposal: ProposalRef::Decision {
            decision: decision.clone(),
        },
        head: None,
    });
    let retained = bus
        .read({
            let candidate = candidate.clone();
            move |read| {
                read.proposal_head(&ProposalRef::Candidate { candidate })
                    .map(|head| head.map(journal::JournalHead::into_body))
            }
        })
        .expect("candidate head reads")
        .expect("candidate was retained");
    let journal::ProposalAuthority::Candidate(retained) = retained else {
        panic!("candidate head decoded as another authority")
    };
    let accepted = outputs
        .iter()
        .map(|output| output.born().expect("output stamp"))
        .collect::<std::collections::BTreeSet<_>>();
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::PreparationDecide,
        expected,
        &PreparationDecide {
            decision: PreparationDecision {
                decision: decision.clone(),
                source: journal_source(&current, controller),
                outcome: DecisionOutcome::Resolved {
                    candidate,
                    outputs: retained
                        .outputs
                        .into_iter()
                        .map(|output| OutputDecision {
                            verdict: if accepted.contains(&output) {
                                OutputVerdict::Accepted
                            } else {
                                OutputVerdict::Rejected
                            },
                            output,
                            comment: None,
                        })
                        .collect(),
                    comment: None,
                },
            },
        },
    ));
    decision
}

pub(super) fn authorize_test_output(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
    output: &TaskHandle,
    spare: &TaskHandle,
    candidate: CandidateRef,
) {
    let decision = select_candidate_outputs(bus, epoch, controller, candidate, &[output], 3);
    let current = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&current, &output).statement.body.authorization,
        Some(decision)
    );
    assert_eq!(at(&current, &spare).statement.body.authorization, None);
    let design = &at(&current, &controller).workflow.body;
    assert_eq!(
        (design.status.as_str(), design.completion),
        ("pending", None),
        "selection releases outputs and leaves the Design open"
    );
}

#[test]
fn adoption_refuses_claimed_work_and_proposal_writes_refuse_stale_subjects() {
    let (root, bus, epoch, seats) = fixture("preparation-command-refusals", 1);
    let output = add(
        &bus,
        epoch,
        Actor::Operator,
        "Claimed output",
        "Claimed output",
        None,
    );
    claim_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &output);
    let controller = add_plan(&bus, epoch, "Refusal controller");
    let current = board(&bus, epoch, Actor::Operator);
    let mut expected = transition_witnesses(at(&current, &output));
    expected.extend(transition_witnesses(at(&current, &controller)));
    let refused = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::OutputAdopt,
        expected,
        &OutputAdopt {
            output: output.clone(),
            preparation: controller.clone(),
        },
    );
    assert!(matches!(
        refused,
        Outcome::Refusal(refusal)
            if refusal.reason() == "preparation::adoption-output-ineligible"
    ));

    let current = board(&bus, epoch, Actor::Operator);
    let candidate = CandidateRef {
        controller: controller.born().expect("controller stamp"),
        candidate: RoomAuthorityStamp::for_micros(1_760_000_000_000_002)
            .expect("candidate identity"),
    };
    let stale = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::CandidateRetain,
        vec![
            statement_witness(at(&current, &controller)),
            Witness::Proposal {
                proposal: ProposalRef::Candidate {
                    candidate: candidate.clone(),
                },
                head: Some(Seq::new(1).expect("fixture position")),
            },
        ],
        &CandidateRetain {
            candidate: RetainedCandidate {
                candidate,
                governed_inputs: Vec::new(),
                outputs: vec![output.born().expect("output stamp")],
                evidence: Vec::new(),
                references: vec![CandidateReference {
                    relation: "contract".parse().expect("relation"),
                    source: journal_source(&current, &controller),
                }],
            },
        },
    );
    assert!(matches!(
        stale,
        Outcome::Refusal(refusal) if refusal.reason() == "preparation::witness-missing"
    ));
    clean(&root, bus);
}

fn assert_controlled_output_refusal(outcome: Outcome<CommandAnswer>, outputs: &[&TaskHandle]) {
    let Outcome::Refusal(refusal) = outcome else {
        panic!("expected controlled-output refusal, got {outcome:?}");
    };
    assert_eq!(refusal.reason(), "preparation::controlled-outputs-pending");
    let observed = refusal
        .diagnostic()
        .and_then(wire::RefusalDiagnostic::observed)
        .expect("refusal names the pending outputs");
    for output in outputs {
        assert!(
            observed.contains(&output.to_string()),
            "refusal omitted pending output {output}: {observed}"
        );
    }
    let detail = refusal
        .diagnostic()
        .and_then(wire::RefusalDiagnostic::detail)
        .expect("refusal names the authorization or settlement repair");
    assert!(
        detail.contains("spice preparation decide")
            && detail.contains("close or delete")
            && !detail.contains("working group")
            && !detail.contains("engag"),
        "controlled-output repair must not offer group engagement: {detail}"
    );
}

#[test]
fn controller_settlement_refuses_a_held_descendant_until_forced_then_closes_the_tree() {
    for (fixture_name, deleted) in [
        ("preparation-close-controller", false),
        ("preparation-delete-controller", true),
    ] {
        let (root, bus, epoch, seats) = fixture(fixture_name, 2);
        let controller_holder = Actor::Lane(seats[0].lane);
        let lane = Actor::Lane(seats[1].lane);
        let controller = add_plan(&bus, epoch, fixture_name);
        let branch = add_plan_output(&bus, epoch, &controller, "Controlled Plan");
        let leaf = add_output(&bus, epoch, &branch, "Controlled task");
        claim_task(&bus, epoch, &seats[0], &controller_holder, &controller);
        claim_task(&bus, epoch, &seats[1], &lane, &branch);
        free_active(&bus, epoch, &controller_holder, &seats[0], &controller);

        let settle = |force_claimed| {
            let visible = board(&bus, epoch, Actor::Operator);
            let heads = at(&visible, &controller);
            let expected = vec![
                statement_witness(heads),
                workflow_witness(heads),
                claim_witness(heads),
            ];
            if deleted {
                issue(
                    &bus,
                    epoch,
                    Actor::Operator,
                    CommandKind::TaskDelete,
                    expected,
                    &TaskDelete {
                        task: controller.clone(),
                        reason: "candidate rejected".to_owned(),
                        force_claimed,
                    },
                )
            } else {
                issue(
                    &bus,
                    epoch,
                    Actor::Operator,
                    CommandKind::TaskClose,
                    expected,
                    &TaskClose {
                        task: controller.clone(),
                        validation: "no selected execution is justified".to_owned(),
                        force_claimed,
                    },
                )
            }
        };
        let refused = settle(false);
        let Outcome::Refusal(refusal) = refused else {
            panic!("held controlled task did not refuse settlement: {refused:?}")
        };
        assert_eq!(refusal.reason(), "task::controlled-descendant-claimed");
        assert!(
            refusal
                .diagnostic()
                .and_then(wire::RefusalDiagnostic::observed)
                .is_some_and(|observed| {
                    observed.contains(branch.as_str()) && observed.contains(&lane.to_string())
                }),
            "the refusal names the controlled task and its holder"
        );

        let CommandAnswer::Task(answer) = assert_committed(settle(true)) else {
            panic!("controller settlement did not answer with task state");
        };
        assert_eq!(answer.closed.len(), 2);
        assert!(answer.closed.contains(&branch));
        assert!(answer.closed.contains(&leaf));
        let settled = board(&bus, epoch, Actor::Operator);
        assert_eq!(
            at(&settled, &controller).workflow.body.status,
            if deleted { "deleted" } else { "completed" }
        );
        for controlled in [&branch, &leaf] {
            let heads = at(&settled, controlled);
            assert_eq!(heads.workflow.body.status, "completed");
            assert!(
                heads
                    .claim
                    .as_ref()
                    .is_none_or(|claim| claim.body.disposition != ClaimDisposition::Active),
                "forced settlement frees every controlled claim"
            );
        }
        clean(&root, bus);
    }
}
#[test]
fn legacy_negative_controller_state_is_reported_as_stranded_without_mutation() {
    let (root, bus, epoch, _) = fixture("preparation-stranded-readout", 1);
    let controller = add_plan(&bus, epoch, "Legacy closed Plan");
    let branch = add_plan_output(&bus, epoch, &controller, "Legacy stranded Plan");
    let leaf = add_output(&bus, epoch, &branch, "Legacy stranded task");
    legacy_close_without_controlled_settlement(&bus, epoch, &controller);

    let current = board(&bus, epoch, Actor::Operator);
    for task in [&branch, &leaf] {
        let stranded = at(&current, task)
            .stranded
            .as_ref()
            .expect("legacy controlled work is classified as stranded");
        assert_eq!(stranded.controller, controller);
        assert!(
            stranded
                .settle
                .contains(&format!("spice task close {task}"))
        );
        let shown = task_show(&current, at(&current, task), &[]);
        assert!(shown.contains(&format!("stranded controller {controller}")));
        assert!(shown.contains(&format!("settle {}", stranded.settle)));
    }
    let status = task_status(&current).expect("stranded task status renders");
    for task in [&branch, &leaf] {
        assert!(status.contains(&format!(
            "stranded {task} controller {controller} settle spice task close {task}"
        )));
    }

    let Outcome::Reply(Reply {
        body: CommandAnswer::TaskDetail(detail),
        ..
    }) = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDetail,
        Vec::new(),
        &wire::intent::TaskDetail {
            task: leaf.born().expect("leaf stamp"),
        },
    )
    else {
        panic!("stranded task detail did not return");
    };
    assert_eq!(
        detail
            .task
            .stranded
            .as_ref()
            .map(|stranded| &stranded.controller),
        Some(&controller)
    );
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &branch)
            .workflow
            .body
            .status,
        "pending",
        "readout never repairs retained history"
    );
    clean(&root, bus);
}

/// Reproduce the retained pre-fix shape without passing through the current
/// settlement command, which now closes controlled work atomically.
fn legacy_close_without_controlled_settlement(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
) {
    let current = board(bus, epoch, Actor::Operator);
    let heads = at(&current, controller);
    let born = heads.statement.body.born.clone();
    let mut workflow = heads.workflow.body.clone();
    let phase = workflow.phase.clone();
    let at = Instant::new(jiff::Timestamp::now()).expect("current fixture instant");
    workflow.status = "completed".to_owned();
    workflow.completion = Some(at);
    workflow.review = "closed".to_owned();
    workflow.assignment = None;
    let mut write = JournalWrite::new(
        Actor::Operator,
        "fixture:legacy-strand"
            .parse()
            .expect("fixture origin parses"),
        at,
        Producer::CommandDoor,
    );
    let closed = write
        .fact(
            body::TaskClosed {
                task: born.clone(),
                validation: "legacy close".to_owned(),
                phase,
                claimed: None,
            },
            FactCause::Root,
        )
        .expect("legacy closure fact builds");
    write.workflow_head(born, closed, move |_| workflow);
    assert!(matches!(
        bus.commit(write).expect("legacy closure commits"),
        CommitOutcome::Committed(_)
    ));
}

/// Add an output under `controller` through explicit preparation.
fn add_output(bus: &Bus, epoch: CanonicalUuid, controller: &TaskHandle, name: &str) -> TaskHandle {
    add_prepared(bus, epoch, controller, name, None)
}

/// Add a reviewed Plan under `controller` through explicit preparation.
pub(super) fn add_plan_output(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
    name: &str,
) -> TaskHandle {
    add_prepared(bus, epoch, controller, name, Some(wire::TaskIntent::Plan))
}

fn add_prepared(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
    name: &str,
    intent: Option<wire::TaskIntent>,
) -> TaskHandle {
    let born = stamp(name);
    let current = board(bus, epoch, Actor::Operator);
    let parent = at(&current, controller);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![
            Witness::Absence { task: born.clone() },
            statement_witness(parent),
            workflow_witness(parent),
        ],
        &TaskAdd {
            preparation: Some(controller.clone()),
            intent,
            title: name.to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The output is delivered".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some("fixture:preparation".parse().expect("fixture origin")),
            flow: intent.map(|_| "review".to_owned()),
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    ));
    TaskHandle::for_project("board.test", &born)
}

/// Move `design` into `to`, or out of every group, at the current partition
/// revision.
fn move_design(
    bus: &Bus,
    epoch: CanonicalUuid,
    design: TaskStamp,
    to: Option<&RoomAuthorityStamp>,
) {
    assert_committed(move_design_now(bus, epoch, &design, to));
}

fn engagement(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> Option<Seq> {
    at(&board(bus, epoch, Actor::Operator), task)
        .statement
        .body
        .engagement
}

fn review_clean(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    controller: &TaskHandle,
) -> Outcome<CommandAnswer> {
    let reviewer = Actor::Lane(seat.lane);
    let assigned = board(bus, epoch, reviewer.clone());
    issue(
        bus,
        epoch,
        reviewer,
        CommandKind::TaskReview,
        progress_witnesses(
            &assigned,
            at(&assigned, controller),
            seat.lane,
            seat.occupant,
            true,
        )
        .expect("review completion witnesses"),
        &TaskReview {
            task: controller.clone(),
            finding: "clean".to_owned(),
            note: Some("the outputs are coherent".to_owned()),
            then: Vec::new(),
            followup: Vec::new(),
        },
    )
}

mod conclude;
pub(super) use conclude::{conclude, done_conclusion};
mod contribution;
mod returned;
mod selected_plan;
mod withdrawal;

#[test]
fn working_group_moves_organize_designs_without_authorizing_outputs() {
    let (root, bus, epoch, _seats) = fixture("preparation-group-organization", 1);
    let design = add_controller(
        &bus,
        epoch,
        "Grouped design",
        wire::TaskIntent::Design,
        None,
    );
    let current = add_output(&bus, epoch, &design, "Current output");

    let group = RoomAuthorityStamp::for_micros(1_760_000_000_000_000).expect("fixture group");
    move_design(&bus, epoch, stamp("Grouped design"), Some(&group));
    let later = add_output(&bus, epoch, &design, "Later output");
    for output in [&current, &later] {
        assert_eq!(
            engagement(&bus, epoch, output),
            None,
            "group organization granted output authority"
        );
    }

    move_design(&bus, epoch, stamp("Grouped design"), None);
    let regroup = RoomAuthorityStamp::for_micros(1_760_000_000_000_001).expect("fixture group");
    move_design(&bus, epoch, stamp("Grouped design"), Some(&regroup));
    for output in [&current, &later] {
        assert_eq!(
            engagement(&bus, epoch, output),
            None,
            "regrouping granted output authority"
        );
    }
    assert_controlled_output_refusal(
        conclude(&bus, epoch, Actor::Operator, &design, done_conclusion()),
        &[&current, &later],
    );
    clean(&root, bus);
}

#[test]
fn a_design_awaiting_selection_waits_on_the_operator_until_its_outputs_settle() {
    let (root, bus, epoch, seats) = fixture("preparation-selection-wait", 2);
    let design = add_controller(
        &bus,
        epoch,
        "Waiting design",
        wire::TaskIntent::Design,
        None,
    );
    group_design(&bus, epoch, &design);
    let lane = Actor::Lane(seats[0].lane);

    // An open Design with an unselected output waits on the operator: no lane
    // can claim it, nor any output it gates.
    let closed = add_output(&bus, epoch, &design, "Closed output");
    let deleted = add_output(&bus, epoch, &design, "Deleted output");
    assert_eq!(
        open_awaiting_selection(&bus, epoch),
        [design.born().expect("design stamp")].into()
    );
    let available = board(&bus, epoch, lane.clone());
    let claim = issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, &design, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    );
    assert!(
        matches!(&claim, Outcome::Refusal(refusal) if refusal.reason() == "board::allocator-empty"),
        "{claim:?}"
    );

    // Settling one output leaves the wait; settling the last ends it with no
    // lane action, and only the operator concludes the Design.
    settle_output(&bus, epoch, &closed, false);
    settle_output(&bus, epoch, &deleted, true);
    assert!(open_awaiting_selection(&bus, epoch).is_empty());
    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &design,
        done_conclusion(),
    ));
    clean(&root, bus);
}

/// Close or delete one unclaimed output as the operator.
fn settle_output(bus: &Bus, epoch: CanonicalUuid, output: &TaskHandle, deleted: bool) {
    let current = board(bus, epoch, Actor::Operator);
    let heads = at(&current, output);
    let expected = vec![
        statement_witness(heads),
        workflow_witness(heads),
        claim_witness(heads),
    ];
    assert_committed(if deleted {
        issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskDelete,
            expected,
            &TaskDelete {
                task: output.clone(),
                reason: "not selected".to_owned(),
                force_claimed: false,
            },
        )
    } else {
        issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskClose,
            expected,
            &TaskClose {
                task: output.clone(),
                validation: "not selected".to_owned(),
                force_claimed: false,
            },
        )
    });
}

#[test]
fn a_standalone_plan_releases_its_tasks_on_clean_review_without_selection() {
    let (root, bus, epoch, seats) = fixture("preparation-standalone-plan", 2);
    let plan = add_controller(
        &bus,
        epoch,
        "Standalone plan",
        wire::TaskIntent::Plan,
        Some("review"),
    );
    let task = add_output(&bus, epoch, &plan, "Constructed task");
    let task_stamp = task.born().expect("task stamp");
    assert_eq!(
        open_workflow_waiting(&bus, epoch),
        [task_stamp.clone()].into()
    );

    let current = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let candidate = CandidateRef {
        controller: plan.born().expect("plan stamp"),
        candidate: RoomAuthorityStamp::for_micros(1_760_000_000_000_001)
            .expect("candidate identity"),
    };
    let retained = issue(
        &bus,
        epoch,
        Actor::Lane(seats[0].lane),
        CommandKind::CandidateRetain,
        transition_witnesses(at(&current, &plan))
            .into_iter()
            .chain(transition_witnesses(at(&current, &task)))
            .chain([Witness::Proposal {
                proposal: ProposalRef::Candidate {
                    candidate: candidate.clone(),
                },
                head: None,
            }])
            .collect(),
        &CandidateRetain {
            candidate: RetainedCandidate {
                candidate,
                governed_inputs: Vec::new(),
                outputs: vec![task_stamp],
                evidence: Vec::new(),
                references: vec![CandidateReference {
                    relation: "contract".parse().expect("relation"),
                    source: journal_source(&current, &plan),
                }],
            },
        },
    );
    assert!(
        matches!(&retained, Outcome::Refusal(refusal)
            if refusal.reason() == "preparation::controller-not-design"),
        "{retained:?}"
    );

    let reviewer = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &reviewer, &plan);
    assert_committed(review_clean(&bus, epoch, &seats[0], &plan));
    let concluded = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&concluded, &plan).workflow.body.status, "completed");
    assert_eq!(at(&concluded, &task).statement.body.authorization, None);
    assert!(open_blocked(&bus, epoch).is_empty());
    let worker = Actor::Lane(seats[1].lane);
    claim_task(&bus, epoch, &seats[1], &worker, &task);
    clean(&root, bus);
}

#[test]
fn a_design_selects_one_plan_whose_own_review_then_releases_its_tasks() {
    let (root, bus, epoch, seats) = fixture("preparation-design-plans", 2);
    let design = add_controller(&bus, epoch, "Plan design", wire::TaskIntent::Design, None);
    let selected = add_plan_output(&bus, epoch, &design, "Selected plan");
    let sibling = add_plan_output(&bus, epoch, &design, "Sibling plan");
    let stamps = |tasks: &[&TaskHandle]| {
        tasks
            .iter()
            .map(|task| task.born().expect("task stamp"))
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        open_workflow_waiting(&bus, epoch),
        stamps(&[&selected, &sibling]),
        "a Design's Plans wait for its selection"
    );

    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &design, &selected, &sibling);
    let _coverage = record_test_coverage(&bus, epoch, &seats, &design, &candidate, &source);
    authorize_test_output(&bus, epoch, &design, &selected, &sibling, candidate);
    assert_eq!(
        open_workflow_waiting(&bus, epoch),
        stamps(&[&sibling]),
        "the unselected sibling Plan stays gated"
    );

    let planner = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &planner, &selected);
    let task = add_output(&bus, epoch, &selected, "Selected plan task");
    assert_eq!(
        open_workflow_waiting(&bus, epoch),
        stamps(&[&sibling, &task]),
        "the selected Plan holds its task until its own review"
    );
    assert_committed(review_clean(&bus, epoch, &seats[0], &selected));
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &task)
            .statement
            .body
            .authorization,
        None
    );
    assert_eq!(
        open_workflow_waiting(&bus, epoch),
        stamps(&[&sibling]),
        "the selected Plan's clean review releases its task without another selection"
    );
    let worker = Actor::Lane(seats[1].lane);
    claim_task(&bus, epoch, &seats[1], &worker, &task);
    let visible = board(&bus, epoch, Actor::Operator);
    let heads = at(&visible, &design);
    let CommandAnswer::Task(answer) = assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskConclude,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskConclude {
            task: design.clone(),
            conclusion: body::DesignConclusion::Collapsed {
                reason: "remaining proposal rejected".to_owned(),
            },
        },
    )) else {
        panic!("collapse did not answer with task state");
    };
    assert_eq!(answer.closed, vec![sibling.clone()]);
    let collapsed = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&collapsed, &task).workflow.body.status, "pending");
    assert_eq!(
        at(&collapsed, &task)
            .claim
            .as_ref()
            .map(|claim| &claim.body.actor),
        Some(&worker),
        "a task released by positive Plan review remains held and live"
    );
    clean(&root, bus);
}

#[test]
fn next_work_offers_a_plan_released_task_whose_open_design_is_off_the_frontier() {
    let (root, bus, epoch, seats) = fixture("preparation-design-next-nested", 2);
    let design = add_controller(&bus, epoch, "Nested design", wire::TaskIntent::Design, None);
    let selected = add_plan_output(&bus, epoch, &design, "Selected plan");
    let sibling = add_plan_output(&bus, epoch, &design, "Sibling plan");
    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &design, &selected, &sibling);
    let _coverage = record_test_coverage(&bus, epoch, &seats, &design, &candidate, &source);
    authorize_test_output(&bus, epoch, &design, &selected, &sibling, candidate);
    let planner = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &planner, &selected);
    let task = add_output(&bus, epoch, &selected, "Selected plan task");
    assert_committed(review_clean(&bus, epoch, &seats[0], &selected));
    // With the sibling gone and the open Design held, no frontier row names
    // the Design as its parent; only the task's controller link reaches it.
    let visible = board(&bus, epoch, Actor::Operator);
    let heads = at(&visible, &sibling);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDelete,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskDelete {
            task: sibling,
            reason: "unselected".to_owned(),
            force_claimed: false,
        },
    ));
    group_design(&bus, epoch, &design);
    claim_task(&bus, epoch, &seats[0], &planner, &design);

    let worker = Actor::Lane(seats[1].lane);
    let Some(NextWorkState::Candidate { task: offered }) =
        next_board(&bus, epoch, worker.clone()).next
    else {
        panic!("the allocator must offer the Plan-released task")
    };
    assert_eq!(
        offered.statement.body.born,
        task.born().expect("task stamp")
    );
    claim_task(&bus, epoch, &seats[1], &worker, &task);
    clean(&root, bus);
}

#[test]
fn a_design_refuses_authorization_until_its_pending_dependencies_settle() {
    let (root, bus, epoch, seats) = fixture("preparation-design-blocked", 2);
    let design = add_controller(
        &bus,
        epoch,
        "Blocked design",
        wire::TaskIntent::Design,
        None,
    );
    let selected = add_plan_output(&bus, epoch, &design, "Selected plan");
    let sibling = add_plan_output(&bus, epoch, &design, "Sibling plan");
    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &design, &selected, &sibling);
    let _coverage = record_test_coverage(&bus, epoch, &seats, &design, &candidate, &source);
    let correction = todo_task(&bus, epoch, "PREP-correct01", "Open review correction");
    connect_child(&bus, epoch, &design, &correction);

    let current = board(&bus, epoch, Actor::Operator);
    let decision = DecisionRef {
        controller: design.born().expect("design stamp"),
        decision: wire::SequenceCode::new(3),
    };
    let mut expected = transition_witnesses(at(&current, &design));
    expected.extend(transition_witnesses(at(&current, &selected)));
    expected.push(candidate_witness(&bus, &candidate));
    expected.push(Witness::Proposal {
        proposal: ProposalRef::Decision {
            decision: decision.clone(),
        },
        head: None,
    });
    expected.push(Witness::Authorization {
        output: selected.born().expect("output stamp"),
        head: None,
    });
    let refused = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::PreparationDecide,
        expected,
        &PreparationDecide {
            decision: PreparationDecision {
                decision,
                source: source.clone(),
                outcome: DecisionOutcome::Resolved {
                    candidate: candidate.clone(),
                    outputs: vec![
                        OutputDecision {
                            output: selected.born().expect("output stamp"),
                            verdict: OutputVerdict::Accepted,
                            comment: None,
                        },
                        OutputDecision {
                            output: sibling.born().expect("output stamp"),
                            verdict: OutputVerdict::Rejected,
                            comment: None,
                        },
                    ],
                    comment: None,
                },
            },
        },
    );
    assert!(
        matches!(&refused, Outcome::Refusal(refusal)
            if refusal.reason() == "preparation::controller-blocked"),
        "{refused:?}"
    );
    let unchanged = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&unchanged, &design).workflow.body.status, "pending");
    assert_eq!(at(&unchanged, &selected).statement.body.authorization, None);

    let fixer = Actor::Lane(seats[1].lane);
    claim_and_complete_task(&bus, epoch, &seats[1], &fixer, &correction);
    authorize_test_output(&bus, epoch, &design, &selected, &sibling, candidate);
    clean(&root, bus);
}
