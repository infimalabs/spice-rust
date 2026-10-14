use super::*;

use wire::rooms::{
    CandidateCoverage, CandidateCoverageRecord, CandidateCritique, CandidateCritiqueRecord,
    CandidateRef, CandidateReference, CandidateRetain, CoverageAuthority, CoverageRef, CritiqueRef,
    CritiqueStanding, DecisionOutcome, DecisionRef, ExecutionAuthorization, ExecutionAuthorize,
    NegativeDecision, OutputAdopt, PreparationDecide, PreparationDecision, ProposalRef,
    RetainedCandidate, RoomAuthorityStamp, RoomSource, WorkingGroupMove,
};

fn add_plan(bus: &Bus, epoch: CanonicalUuid, name: &str) -> TaskHandle {
    add_controller(bus, epoch, name, wire::TaskIntent::Plan, None)
}

fn add_controller(
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

fn transition_witnesses(task: &BoardTask) -> Vec<Witness> {
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
    let coverage_refs = record_test_coverage(&bus, epoch, &seats, &controller, &candidate, &source);
    record_test_decisions(&bus, epoch, &controller, &output, &candidate, &source);
    authorize_test_output(
        &bus,
        epoch,
        &controller,
        &output,
        &spare,
        candidate,
        &coverage_refs,
    );
    assert_eq!(
        open_blocked(&bus, epoch),
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
    assert!(outcomes.contains(&Some("accepted")), "{outcomes:?}");
    assert!(outcomes.contains(&Some("negative")), "{outcomes:?}");
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
    let coverage_refs = record_test_coverage(&bus, epoch, &seats, &controller, &candidate, &source);
    record_test_decisions(&bus, epoch, &controller, &output, &candidate, &source);
    assert_eq!(
        open_awaiting_selection(&bus, epoch),
        [controller.born().expect("controller stamp")].into()
    );

    authorize_test_output(
        &bus,
        epoch,
        &controller,
        &output,
        &spare,
        candidate,
        &coverage_refs,
    );

    let current = board(&bus, epoch, Actor::Operator);
    assert!(
        matches!(
            at(&current, &controller).claim.as_ref(),
            Some(claim) if claim.body.disposition == ClaimDisposition::Active
        ),
        "selection never takes a Design from its holder"
    );
    let blocked = open_blocked(&bus, epoch);
    assert!(
        !blocked.contains(&output.born().expect("output stamp")),
        "the selected output runs while its Design stays open"
    );
    assert!(blocked.contains(&spare.born().expect("spare stamp")));
    assert_eq!(
        open_awaiting_selection(&bus, epoch),
        [controller.born().expect("controller stamp")].into(),
        "the unselected spare still waits on the operator"
    );
    clean(&root, bus);
}
fn adopted_outputs(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
) -> (TaskHandle, TaskHandle) {
    let output = add(
        &bus,
        epoch,
        Actor::Operator,
        "Selected output",
        "Selected output",
        None,
    );
    let spare = add(
        &bus,
        epoch,
        Actor::Operator,
        "Unselected output",
        "Unselected output",
        None,
    );

    for task in [&output, &spare] {
        let current = board(&bus, epoch, Actor::Operator);
        let mut expected = transition_witnesses(at(&current, task));
        expected.extend(transition_witnesses(at(&current, &controller)));
        assert_committed(issue(
            &bus,
            epoch,
            Actor::Operator,
            CommandKind::OutputAdopt,
            expected,
            &OutputAdopt {
                output: task.clone(),
                preparation: controller.clone(),
            },
        ));
    }
    (output, spare)
}

fn retain_test_candidate(
    bus: &Bus,
    epoch: CanonicalUuid,
    seats: &[Seat],
    controller: &TaskHandle,
    output: &TaskHandle,
    spare: &TaskHandle,
) -> (CandidateRef, RoomSource) {
    let current = board(&bus, epoch, Actor::Lane(seats[0].lane));
    let controller_stamp = controller.born().expect("controller stamp");
    let output_stamp = output.born().expect("output stamp");
    let spare_stamp = spare.born().expect("spare stamp");
    let candidate = CandidateRef {
        controller: controller_stamp.clone(),
        candidate: RoomAuthorityStamp::for_micros(1_760_000_000_000_001)
            .expect("candidate identity"),
    };
    let retained = RetainedCandidate {
        candidate: candidate.clone(),
        governed_inputs: Vec::new(),
        outputs: vec![output_stamp.clone(), spare_stamp.clone()],
        evidence: Vec::new(),
        references: vec![CandidateReference {
            relation: "contract".parse().expect("relation"),
            source: journal_source(&current, &controller),
        }],
    };
    let mut expected = vec![Witness::Proposal {
        proposal: ProposalRef::Candidate {
            candidate: candidate.clone(),
        },
        head: None,
    }];
    for task in [&controller, &output, &spare] {
        expected.push(statement_witness(at(&current, task)));
    }
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Lane(seats[0].lane),
        CommandKind::CandidateRetain,
        expected,
        &CandidateRetain {
            candidate: retained.clone(),
        },
    ));

    let source = journal_source(&current, &controller);
    (candidate, source)
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
        vec![
            statement_witness(at(&current, &controller)),
            Witness::Proposal {
                proposal: ProposalRef::Critique {
                    critique: critique.critique.clone(),
                },
                head: None,
            },
        ],
        &CandidateCritiqueRecord { critique },
    ));
}

fn record_test_coverage(
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
            vec![
                statement_witness(at(&current, &controller)),
                Witness::Proposal {
                    proposal: ProposalRef::Coverage {
                        coverage: coverage.clone(),
                    },
                    head: None,
                },
            ],
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

fn record_test_decisions(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
    output: &TaskHandle,
    candidate: &CandidateRef,
    source: &RoomSource,
) {
    let controller_stamp = controller.born().expect("controller stamp");
    for (sequence, outcome) in [
        (
            1,
            DecisionOutcome::Negative {
                conclusion: NegativeDecision::NoWork,
            },
        ),
        (
            2,
            DecisionOutcome::Returned {
                candidate: candidate.clone(),
            },
        ),
        (
            3,
            DecisionOutcome::Accepted {
                candidate: candidate.clone(),
            },
        ),
    ] {
        let decision = DecisionRef {
            controller: controller_stamp.clone(),
            decision: wire::SequenceCode::new(sequence),
        };
        let current = board(&bus, epoch, Actor::Operator);
        assert_committed(issue(
            &bus,
            epoch,
            Actor::Operator,
            CommandKind::PreparationDecide,
            vec![
                statement_witness(at(&current, &controller)),
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
                    source: source.clone(),
                    outcome,
                },
            },
        ));
        let current = board(&bus, epoch, Actor::Operator);
        assert_eq!(
            at(&current, &output).statement.body.authorization,
            None,
            "a decision alone authorizes nothing"
        );
    }
}

fn authorize_test_output(
    bus: &Bus,
    epoch: CanonicalUuid,
    controller: &TaskHandle,
    output: &TaskHandle,
    spare: &TaskHandle,
    candidate: CandidateRef,
    coverage_refs: &[CoverageRef],
) {
    let controller_stamp = controller.born().expect("controller stamp");
    let output_stamp = output.born().expect("output stamp");
    let decision = DecisionRef {
        controller: controller_stamp,
        decision: wire::SequenceCode::new(3),
    };
    let current = board(&bus, epoch, Actor::Operator);
    let mut expected = transition_witnesses(at(&current, &controller));
    expected.extend(transition_witnesses(at(&current, &output)));
    expected.push(Witness::Authorization {
        output: output_stamp,
        head: None,
    });
    let unrecorded = CoverageRef {
        candidate: candidate.clone(),
        coverage: wire::SequenceCode::new(9),
    };
    let unsound = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::ExecutionAuthorize,
        expected.clone(),
        &ExecutionAuthorize {
            authorization: ExecutionAuthorization {
                decision: decision.clone(),
                candidate: candidate.clone(),
                governed_inputs: Vec::new(),
                selected_outputs: vec![output.born().expect("output stamp")],
                coverage: vec![unrecorded.clone()],
            },
        },
    );
    let Outcome::Refusal(refusal) = unsound else {
        panic!("authorization over unrecorded coverage was admitted: {unsound:?}");
    };
    assert_eq!(
        refusal.reason(),
        journal::ROOM_AUTHORITY_REFUSED,
        "an unsound request is not witness movement"
    );
    let detail = refusal
        .diagnostic()
        .and_then(wire::RefusalDiagnostic::detail)
        .expect("room authority names the offending subject");
    let subject = serde_json::to_string(&unrecorded).expect("coverage reference encodes");
    assert!(
        detail.contains(&subject),
        "refusal omitted the unrecorded coverage {subject}: {detail}"
    );
    let unchanged = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&unchanged, &output).statement.body.authorization, None);
    assert_eq!(at(&unchanged, &controller).workflow.body.status, "pending");
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::ExecutionAuthorize,
        expected,
        &ExecutionAuthorize {
            authorization: ExecutionAuthorization {
                decision: decision.clone(),
                candidate,
                governed_inputs: Vec::new(),
                selected_outputs: vec![output.born().expect("output stamp")],
                coverage: vec![coverage_refs[1].clone()],
            },
        },
    ));

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
        detail.contains("spice preparation authorize")
            && detail.contains("close or delete")
            && !detail.contains("working group")
            && !detail.contains("engag"),
        "controlled-output repair must not offer group engagement: {detail}"
    );
}

#[test]
fn operator_negative_settlements_leave_controlled_outputs_pending_and_unauthorized() {
    for (fixture_name, deleted) in [
        ("preparation-close-controller", false),
        ("preparation-delete-controller", true),
    ] {
        let (root, bus, epoch, _) = fixture(fixture_name, 1);
        let controller = add_plan(&bus, epoch, fixture_name);
        let (output, spare) = adopted_outputs(&bus, epoch, &controller);
        let visible = board(&bus, epoch, Actor::Operator);
        let heads = at(&visible, &controller);
        let expected = vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ];
        let outcome = if deleted {
            issue(
                &bus,
                epoch,
                Actor::Operator,
                CommandKind::TaskDelete,
                expected,
                &TaskDelete {
                    task: controller.clone(),
                    reason: "candidate rejected".to_owned(),
                    force_claimed: false,
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
                    force_claimed: false,
                },
            )
        };
        assert_committed(outcome);
        let settled = board(&bus, epoch, Actor::Operator);
        assert_eq!(
            at(&settled, &controller).workflow.body.status,
            if deleted { "deleted" } else { "completed" }
        );
        for controlled in [&output, &spare] {
            let heads = at(&settled, controlled);
            assert_eq!(heads.workflow.body.status, "pending");
            assert_eq!(heads.statement.body.authorization, None);
        }
        assert_eq!(
            open_blocked(&bus, epoch),
            [&output, &spare]
                .map(|task| task.born().expect("output stamp"))
                .into(),
            "a negative settlement releases no controlled output"
        );
        clean(&root, bus);
    }
}

/// Add an output under `controller` through explicit preparation.
fn add_output(bus: &Bus, epoch: CanonicalUuid, controller: &TaskHandle, name: &str) -> TaskHandle {
    add_prepared(bus, epoch, controller, name, None)
}

/// Add a reviewed Plan under `controller` through explicit preparation.
fn add_plan_output(
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
    let revision = bus
        .read(|read| read.group_partition())
        .expect("partition reads")
        .map_or(0, |head| head.body().revision);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupMove,
        vec![
            Witness::Group { revision },
            Witness::Existence {
                task: design.clone(),
            },
        ],
        &WorkingGroupMove {
            design,
            to: to.cloned(),
        },
    ));
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
use conclude::{conclude, done_conclusion};

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
    assert_eq!(open_blocked(&bus, epoch), [task_stamp.clone()].into());

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
        vec![
            Witness::Proposal {
                proposal: ProposalRef::Candidate {
                    candidate: candidate.clone(),
                },
                head: None,
            },
            statement_witness(at(&current, &plan)),
            statement_witness(at(&current, &task)),
        ],
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
        open_blocked(&bus, epoch),
        stamps(&[&selected, &sibling]),
        "a Design's Plans wait for its selection"
    );

    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &design, &selected, &sibling);
    let coverage = record_test_coverage(&bus, epoch, &seats, &design, &candidate, &source);
    record_test_decisions(&bus, epoch, &design, &selected, &candidate, &source);
    authorize_test_output(
        &bus, epoch, &design, &selected, &sibling, candidate, &coverage,
    );
    assert_eq!(
        open_blocked(&bus, epoch),
        stamps(&[&sibling]),
        "the unselected sibling Plan stays gated"
    );

    let planner = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &planner, &selected);
    let task = add_output(&bus, epoch, &selected, "Selected plan task");
    assert_eq!(
        open_blocked(&bus, epoch),
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
        open_blocked(&bus, epoch),
        stamps(&[&sibling]),
        "the selected Plan's clean review releases its task without another selection"
    );
    let worker = Actor::Lane(seats[1].lane);
    claim_task(&bus, epoch, &seats[1], &worker, &task);
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
    let coverage = record_test_coverage(&bus, epoch, &seats, &design, &candidate, &source);
    record_test_decisions(&bus, epoch, &design, &selected, &candidate, &source);
    authorize_test_output(
        &bus, epoch, &design, &selected, &sibling, candidate, &coverage,
    );
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
    let coverage = record_test_coverage(&bus, epoch, &seats, &design, &candidate, &source);
    record_test_decisions(&bus, epoch, &design, &selected, &candidate, &source);
    let correction = todo_task(&bus, epoch, "PREP-correct01", "Open review correction");
    connect_child(&bus, epoch, &design, &correction);

    let current = board(&bus, epoch, Actor::Operator);
    let mut expected = transition_witnesses(at(&current, &design));
    expected.extend(transition_witnesses(at(&current, &selected)));
    expected.push(Witness::Authorization {
        output: selected.born().expect("output stamp"),
        head: None,
    });
    let refused = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::ExecutionAuthorize,
        expected,
        &ExecutionAuthorize {
            authorization: ExecutionAuthorization {
                decision: DecisionRef {
                    controller: design.born().expect("design stamp"),
                    decision: wire::SequenceCode::new(3),
                },
                candidate: candidate.clone(),
                governed_inputs: Vec::new(),
                selected_outputs: vec![selected.born().expect("output stamp")],
                coverage: vec![coverage[1].clone()],
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
    authorize_test_output(
        &bus, epoch, &design, &selected, &sibling, candidate, &coverage,
    );
    clean(&root, bus);
}
