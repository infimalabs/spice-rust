use super::*;

use wire::rooms::{
    CandidateCoverage, CandidateCoverageRecord, CandidateCritique, CandidateCritiqueRecord,
    CandidateRef, CandidateReference, CoverageAuthority, CoverageRef, CritiqueRef,
    CritiqueStanding, DecisionOutcome, DecisionRef, DesignPassProposal, DesignPropose, OutputAdopt,
    OutputDecision, OutputVerdict, PreparationDecide, PreparationDecision, ProposalRef,
    RoomAuthorityStamp, RoomSource,
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
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The exact selected output is justified".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
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
    group_design(&bus, epoch, &controller);
    claim_task(
        &bus,
        epoch,
        &seats[0],
        &Actor::Lane(seats[0].lane),
        &controller,
    );
    let output = add_plan_output(&bus, epoch, &controller, "Selected output");
    let spare = add_plan_output(&bus, epoch, &controller, "Unselected output");
    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &controller, &output, &spare);
    record_test_critique(&bus, epoch, &seats, &controller, &candidate, &source);
    let _coverage = record_test_coverage(&bus, epoch, &seats, &controller, &candidate, &source);
    authorize_test_output(&bus, epoch, &controller, &output, &spare, candidate);
    assert_eq!(
        open_workflow_waiting(&bus, epoch),
        [spare.born().expect("spare output stamp")].into(),
        "the rejected sibling stays gated while its rejection readies the Design"
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
fn each_design_decision_removes_only_its_edge_from_a_shared_output() {
    let (root, bus, epoch, seats) = fixture("preparation-shared-output", 2);
    let first = add_controller(
        &bus,
        epoch,
        "First selecting Design",
        wire::TaskIntent::Design,
        None,
    );
    group_design(&bus, epoch, &first);
    let output = add_plan_output(&bus, epoch, &first, "Shared selected output");
    let first_spare = add_plan_output(&bus, epoch, &first, "First spare");

    let (first_candidate, first_source) =
        retain_test_candidate(&bus, epoch, &seats, &first, &output, &first_spare);
    record_test_critique(&bus, epoch, &seats, &first, &first_candidate, &first_source);
    record_test_coverage(&bus, epoch, &seats, &first, &first_candidate, &first_source);
    let second = add_controller(
        &bus,
        epoch,
        "Second selecting Design",
        wire::TaskIntent::Design,
        None,
    );
    group_design(&bus, epoch, &second);
    let second_spare = add_plan_output(&bus, epoch, &second, "Second spare");
    connect_child(&bus, epoch, &output, &second);
    let (second_candidate, second_source) =
        retain_test_candidate(&bus, epoch, &seats, &second, &output, &second_spare);
    record_test_critique(
        &bus,
        epoch,
        &seats,
        &second,
        &second_candidate,
        &second_source,
    );
    record_test_coverage(
        &bus,
        epoch,
        &seats,
        &second,
        &second_candidate,
        &second_source,
    );

    authorize_test_output(&bus, epoch, &first, &output, &first_spare, first_candidate);
    assert!(
        open_gated(&bus, epoch).contains(&output.born().expect("output stamp")),
        "the second Design edge still gates the shared output"
    );
    authorize_test_output(
        &bus,
        epoch,
        &second,
        &output,
        &second_spare,
        second_candidate,
    );
    assert!(
        !open_gated(&bus, epoch).contains(&output.born().expect("output stamp")),
        "both exact Design edges were independently accepted"
    );
    for design in [&first, &second] {
        let page = preparation_page(&root, &[design.as_str(), "--limit", "20"]);
        assert!(
            page["page"]["rows"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row["kind"] == "authorization")),
            "{design} retains its distinct authorization"
        );
    }
    clean(&root, bus);
}

#[test]
fn operator_selection_releases_outputs_and_leaves_the_design_unclaimed_and_open() {
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
    let output = add_plan_output(&bus, epoch, &controller, "Selected held output");
    let spare = add_plan_output(&bus, epoch, &controller, "Unselected held output");
    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &controller, &output, &spare);
    record_test_critique(&bus, epoch, &seats, &controller, &candidate, &source);
    let _coverage = record_test_coverage(&bus, epoch, &seats, &controller, &candidate, &source);
    assert_eq!(
        open_awaiting_selection(&bus, epoch),
        [controller.born().expect("Design stamp")].into(),
        "the completed pass waits on its exact operator decision"
    );

    authorize_test_output(&bus, epoch, &controller, &output, &spare, candidate);

    let current = board(&bus, epoch, Actor::Operator);
    assert!(
        at(&current, &controller)
            .claim
            .as_ref()
            .is_none_or(|claim| claim.body.disposition != ClaimDisposition::Active),
        "the proposal released the Design claim before operator selection"
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
    group_design(&bus, epoch, &design);
    claim_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &design);
    let first = add_plan_output(&bus, epoch, &design, "First version output");
    let second = add_plan_output(&bus, epoch, &design, "Later version output");
    let (initial, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&first, &second],
        1_760_000_000_000_001,
    );
    let _first_decision = select_candidate_outputs(&bus, epoch, &design, initial, &[&first], 1);
    let design_stamp = design.born().expect("Design stamp");
    let after_first = board(&bus, epoch, Actor::Operator);
    assert!(
        !at(&after_first, &first)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert!(
        at(&after_first, &second)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    let current = board(&bus, epoch, Actor::Operator);
    let heads = at(&current, &first);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskClose,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskClose {
            task: first.clone(),
            validation: "the first selected Plan is settled".to_owned(),
            force_claimed: false,
        },
    ));

    let (later, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&second],
        1_760_000_000_000_002,
    );
    let _second_decision = select_candidate_outputs(&bus, epoch, &design, later, &[&second], 2);
    let current = board(&bus, epoch, Actor::Operator);
    assert!(
        !at(&current, &first)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert!(
        !at(&current, &second)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert!(
        !open_awaiting_selection(&bus, epoch).contains(&design.born().expect("Design stamp")),
        "all direct outputs now carry exact selection authority"
    );
    assert!(
        at(&current, &design).workflow.body.status.is_pending(),
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
    propose_candidate_version(
        bus,
        epoch,
        seat,
        controller,
        outputs,
        candidate_micros,
        wire::rooms::DesignPassRecommendation::Work,
    )
}

fn propose_candidate_version(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    controller: &TaskHandle,
    outputs: &[&TaskHandle],
    candidate_micros: i64,
    recommendation: wire::rooms::DesignPassRecommendation,
) -> (CandidateRef, RoomSource) {
    let mut current = board(bus, epoch, Actor::Lane(seat.lane));
    if at(&current, controller)
        .claim
        .as_ref()
        .is_none_or(|claim| claim.body.disposition != ClaimDisposition::Active)
    {
        claim_task(bus, epoch, seat, &Actor::Lane(seat.lane), controller);
        current = board(bus, epoch, Actor::Lane(seat.lane));
    }
    let candidate = CandidateRef {
        controller: controller.born().expect("controller stamp"),
        candidate: RoomAuthorityStamp::for_micros(candidate_micros).expect("candidate identity"),
    };
    let controller_state = at(&current, controller);
    let retained = DesignPassProposal {
        candidate: candidate.clone(),
        statement: controller_state.statement.position,
        claim: Some(
            controller_state
                .claim
                .as_ref()
                .expect("Design is claimed before proposing a pass")
                .position,
        ),
        recommendation,
        governed_inputs: Vec::new(),
        outputs: outputs
            .iter()
            .map(|output| output.born().expect("output stamp"))
            .collect(),
        justification: "the cited contract supports this Design pass".to_owned(),
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
    expected.push(residency(&current, seat));
    assert_committed(issue(
        bus,
        epoch,
        Actor::Lane(seat.lane),
        CommandKind::DesignPropose,
        expected,
        &DesignPropose {
            candidate: retained,
        },
    ));
    (candidate, journal_source(&current, controller))
}

#[test]
fn current_proposal_requires_an_exact_active_claim() {
    let (root, bus, epoch, seats) = fixture("preparation-proposal-claim-required", 1);
    let controller = add_controller(
        &bus,
        epoch,
        "Claimed Design",
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
    let output = add_plan_output(&bus, epoch, &controller, "Proposed output");
    let current = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let candidate = CandidateRef {
        controller: controller.born().expect("controller stamp"),
        candidate: RoomAuthorityStamp::for_micros(1_760_000_000_000_099)
            .expect("candidate identity"),
    };
    let mut expected = vec![Witness::Proposal {
        proposal: ProposalRef::Candidate {
            candidate: candidate.clone(),
        },
        head: None,
    }];
    expected.extend(transition_witnesses(at(&current, &controller)));
    expected.extend(transition_witnesses(at(&current, &output)));
    expected.push(residency(&current, &seats[0]));
    let result = issue(
        &bus,
        epoch,
        Actor::Lane(seats[0].lane),
        CommandKind::DesignPropose,
        expected,
        &DesignPropose {
            candidate: DesignPassProposal {
                candidate,
                statement: at(&current, &controller).statement.position,
                claim: None,
                recommendation: wire::rooms::DesignPassRecommendation::Work,
                governed_inputs: Vec::new(),
                outputs: vec![output.born().expect("output stamp")],
                justification: "the cited contract supports this Design pass".to_owned(),
                references: vec![CandidateReference {
                    relation: "contract".parse().expect("relation"),
                    source: journal_source(&current, &controller),
                }],
            },
        },
    );
    assert!(matches!(
        result,
        Outcome::Refusal(refusal)
            if refusal.reason() == "preparation::proposal-claim-missing"
    ));
    clean(&root, bus);
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

fn authorization_witness(bus: &Bus, output: TaskStamp) -> Witness {
    let head = bus
        .read({
            let output = output.clone();
            move |read| read.authorization_head(&output)
        })
        .expect("authorization head reads")
        .map(|head| head.position());
    Witness::Authorization { output, head }
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
        expected.push(authorization_witness(
            bus,
            output.born().expect("output stamp"),
        ));
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
    let _decision = select_candidate_outputs(bus, epoch, controller, candidate, &[output], 3);
    let controller_stamp = controller.born().expect("Design stamp");
    let current = board(&bus, epoch, Actor::Operator);
    assert!(
        !at(&current, &output)
            .statement
            .body
            .dependencies
            .contains(&controller_stamp)
    );
    assert!(
        at(&current, &spare)
            .statement
            .body
            .dependencies
            .contains(&controller_stamp)
    );
    let design = &at(&current, &controller).workflow.body;
    assert!(
        design.status.is_pending(),
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
        Actor::Lane(seats[0].lane),
        CommandKind::DesignPropose,
        vec![
            statement_witness(at(&current, &controller)),
            Witness::Proposal {
                proposal: ProposalRef::Candidate {
                    candidate: candidate.clone(),
                },
                head: Some(Seq::new(1).expect("fixture position")),
            },
            residency(&current, &seats[0]),
        ],
        &DesignPropose {
            candidate: DesignPassProposal {
                candidate,
                statement: at(&current, &controller).statement.position,
                claim: Some(Seq::new(1).expect("fixture claim position")),
                recommendation: wire::rooms::DesignPassRecommendation::Work,
                governed_inputs: Vec::new(),
                outputs: vec![output.born().expect("output stamp")],
                justification: "the cited contract supports this pass".to_owned(),
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
        claim_task(&bus, epoch, &seats[0], &controller_holder, &controller);
        let branch = add_plan(&bus, epoch, "Controlled Plan");
        claim_task(&bus, epoch, &seats[1], &lane, &branch);
        let leaf = add_output(&bus, epoch, &branch, "Controlled task");
        let visible = board(&bus, epoch, lane.clone());
        assert_committed(issue(
            &bus,
            epoch,
            lane.clone(),
            CommandKind::TaskDepends,
            dependency_witnesses(&visible, &branch, &[&controller], Some(&seats[1]), false),
            &TaskDepends {
                task: branch.clone(),
                after: vec![controller.clone()],
                remove: false,
            },
        ));
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
        if deleted {
            assert!(at(&settled, &controller).workflow.body.status.is_deleted());
        } else {
            assert!(
                at(&settled, &controller)
                    .workflow
                    .body
                    .status
                    .is_completed()
            );
        }
        for controlled in [&branch, &leaf] {
            let heads = at(&settled, controlled);
            assert!(heads.workflow.body.status.is_completed());
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

/// Add an output under `controller` through explicit preparation.
fn add_output(bus: &Bus, epoch: CanonicalUuid, controller: &TaskHandle, name: &str) -> TaskHandle {
    add_prepared(bus, epoch, controller, name, None, None)
}

fn add_output_with_flow(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
    name: &str,
    flow: &str,
) -> TaskHandle {
    add_prepared(bus, epoch, controller, name, None, Some(flow))
}

/// Add a reviewed Plan under `controller` through explicit preparation.
pub(super) fn add_plan_output(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
    name: &str,
) -> TaskHandle {
    add_prepared(
        bus,
        epoch,
        controller,
        name,
        Some(wire::TaskIntent::Plan),
        None,
    )
}

fn add_prepared(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
    name: &str,
    intent: Option<wire::TaskIntent>,
    flow: Option<&str>,
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
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The output is delivered".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: flow
                .map(str::to_owned)
                .or_else(|| intent.map(|_| "review".to_owned())),
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    ));
    TaskHandle::for_project("board.test", &born)
}

pub(super) fn review_clean(
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

mod assembled;
mod conclude;
mod derived;
pub(super) use conclude::{conclude, done_conclusion};
mod contribution;
mod decision_wait;
mod ordinary_output;
mod returned;
mod selected_plan;
mod withdrawal;

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
        CommandKind::DesignPropose,
        transition_witnesses(at(&current, &plan))
            .into_iter()
            .chain(transition_witnesses(at(&current, &task)))
            .chain([
                Witness::Proposal {
                    proposal: ProposalRef::Candidate {
                        candidate: candidate.clone(),
                    },
                    head: None,
                },
                residency(&current, &seats[0]),
            ])
            .collect(),
        &DesignPropose {
            candidate: DesignPassProposal {
                candidate,
                statement: at(&current, &plan).statement.position,
                claim: Some(at(&current, &plan).claim.as_ref().map_or_else(
                    || Seq::new(1).expect("fixture claim position"),
                    |claim| claim.position,
                )),
                recommendation: wire::rooms::DesignPassRecommendation::Work,
                governed_inputs: Vec::new(),
                outputs: vec![task_stamp],
                justification: "the cited contract supports this pass".to_owned(),
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
    assert!(at(&concluded, &plan).workflow.body.status.is_completed());
    assert!(
        at(&concluded, &task)
            .statement
            .body
            .dependencies
            .contains(&plan.born().expect("Plan stamp"))
    );
    assert!(open_blocked(&bus, epoch).is_empty());
    let worker = Actor::Lane(seats[1].lane);
    claim_task(&bus, epoch, &seats[1], &worker, &task);
    clean(&root, bus);
}

#[test]
fn a_design_selects_one_plan_whose_own_review_then_releases_its_tasks() {
    let (root, bus, epoch, seats) = fixture("preparation-design-plans", 2);
    let design = add_controller(&bus, epoch, "Plan design", wire::TaskIntent::Design, None);
    group_design(&bus, epoch, &design);
    claim_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &design);
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
        "the rejected sibling stays gated while its rejection readies the Design"
    );

    capture_for(&bus, epoch, &seats[0], &selected);
    let task = add_output(&bus, epoch, &selected, "Selected plan task");
    assert_eq!(
        open_workflow_waiting(&bus, epoch),
        stamps(&[&sibling, &task]),
        "the selected Plan holds its task for review"
    );
    assert_committed(review_clean(&bus, epoch, &seats[0], &selected));
    assert!(
        at(&board(&bus, epoch, Actor::Operator), &task)
            .statement
            .body
            .dependencies
            .contains(&selected.born().expect("Plan stamp"))
    );
    assert_eq!(
        open_workflow_waiting(&bus, epoch),
        stamps(&[&sibling]),
        "the selected Plan's clean review releases its task"
    );
    let worker = Actor::Lane(seats[1].lane);
    capture_for(&bus, epoch, &seats[1], &task);
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
    assert!(at(&collapsed, &task).workflow.body.status.is_pending());
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
    group_design(&bus, epoch, &design);
    claim_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &design);
    let selected = add_plan_output(&bus, epoch, &design, "Selected plan");
    let sibling = add_plan_output(&bus, epoch, &design, "Sibling plan");
    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &design, &selected, &sibling);
    let _coverage = record_test_coverage(&bus, epoch, &seats, &design, &candidate, &source);
    authorize_test_output(&bus, epoch, &design, &selected, &sibling, candidate);
    capture_for(&bus, epoch, &seats[0], &selected);
    let task = add_output(&bus, epoch, &selected, "Selected plan task");
    assert_committed(review_clean(&bus, epoch, &seats[0], &selected));
    // With the sibling gone, no frontier row names the Design as its parent;
    // only the task's controller link reaches it. The rejection readied the
    // Design, so its lane holds it while the released task is offered.
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
    capture_for(&bus, epoch, &seats[0], &design);

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
    group_design(&bus, epoch, &design);
    claim_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &design);
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
    assert!(at(&unchanged, &design).workflow.body.status.is_pending());
    assert!(
        at(&unchanged, &selected)
            .statement
            .body
            .dependencies
            .contains(&design.born().expect("Design stamp"))
    );

    let fixer = Actor::Lane(seats[1].lane);
    claim_and_complete_task(&bus, epoch, &seats[1], &fixer, &correction);
    authorize_test_output(&bus, epoch, &design, &selected, &sibling, candidate);
    clean(&root, bus);
}
