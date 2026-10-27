//! Operator selection: accepted outputs become the Design's prerequisites,
//! rejected outputs close, and a selection that would loop is refused.

use super::*;

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
    capture_design(&bus, epoch, &seats[0], &controller);
    let output = add_plan_output(&bus, epoch, &controller, "Selected held output");
    let spare = add_plan_output(&bus, epoch, &controller, "Unselected held output");
    let (candidate, _) = retain_test_candidate(&bus, epoch, &seats, &controller, &output, &spare);
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
    assert!(
        !gated.contains(&spare.born().expect("spare stamp"))
            && !at(&current, &spare).workflow.body.status.is_pending(),
        "the rejected spare is closed by the decision rather than left gated"
    );
    assert!(
        open_awaiting_selection(&bus, epoch).is_empty(),
        "the Design waits on its released output rather than on a decision"
    );

    // The released output is the Design's prerequisite on every read, with
    // no edge on the Design's head.
    let design_stamp = controller.born().expect("Design stamp");
    assert!(
        at(&current, &controller)
            .statement
            .body
            .dependencies
            .is_empty()
    );
    assert_reads_released_prerequisite(
        &bus,
        epoch,
        &design_stamp,
        &output.born().expect("output stamp"),
    );
    clean(&root, bus);
}

/// The Design's own task read carries its released output, and the typed
/// dependency relation draws the Design depending on it.
fn assert_reads_released_prerequisite(
    bus: &Bus,
    epoch: CanonicalUuid,
    design_stamp: &TaskStamp,
    output_stamp: &TaskStamp,
) {
    let Outcome::Reply(Reply {
        body: CommandAnswer::Board(shown),
        ..
    }) = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead::Tasks {
            tasks: vec![design_stamp.clone()],
            dependencies: true,
            claims: false,
            history: false,
        },
    )
    else {
        panic!("the Design's task read did not reply with a board")
    };
    assert!(
        shown
            .tasks
            .iter()
            .any(|row| row.statement.body.born == *output_stamp),
        "task show carries the released output beside the Design"
    );
    let (_, projected) = relations::answer(relations::read_relations(
        bus,
        epoch,
        Actor::Operator,
        &wire::intent::TaskRelations {
            roots: vec![design_stamp.clone()],
            relations: vec![wire::intent::TaskRelationKind::Dependency],
            depth: 1,
            node_limit: 16,
        },
    ));
    assert!(
        projected.edges.iter().any(|edge| {
            edge.relation == wire::intent::TaskRelationKind::Dependency
                && edge.from
                    == wire::intent::TaskRelationNodeRef::Task {
                        task: design_stamp.clone(),
                    }
                && edge.to
                    == wire::intent::TaskRelationNodeRef::Task {
                        task: output_stamp.clone(),
                    }
        }),
        "the dependency relation draws the Design waiting on its released output: {:?}",
        projected.edges
    );
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
    capture_design(&bus, epoch, &seats[0], &design);
    let first = add_plan_output(&bus, epoch, &design, "First version output");
    let second = add_plan_output(&bus, epoch, &design, "Later version output");
    // A veto closes an output, so the later version names work the first
    // pass left unproposed rather than re-proposing a rejected sibling.
    let (initial, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&first],
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

#[test]
fn a_decision_refuses_an_accepted_output_that_waits_on_work_the_design_still_gates() {
    let (root, bus, epoch, seats) = fixture("preparation-selection-cycle", 2);
    let design = add_controller(&bus, epoch, "Cyclic design", wire::TaskIntent::Design, None);
    capture_design(&bus, epoch, &seats[0], &design);
    let selected = add_plan_output(&bus, epoch, &design, "Selected plan");
    let sibling = add_plan_output(&bus, epoch, &design, "Sibling plan");
    let unproposed = add_plan_output(&bus, epoch, &design, "Unproposed plan");
    let before = board(&bus, epoch, Actor::Operator);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependencies::dependency_witnesses(&before, &selected, &[&unproposed], None, false),
        &TaskDepends {
            task: selected.clone(),
            after: vec![unproposed.clone()],
            remove: false,
        },
    ));
    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &design, &selected, &sibling);

    let current = board(&bus, epoch, Actor::Operator);
    let decision = DecisionRef {
        controller: design.born().expect("design stamp"),
        decision: wire::SequenceCode::new(3),
    };
    let mut expected = transition_witnesses(at(&current, &design));
    expected.extend(transition_witnesses(at(&current, &selected)));
    expected.extend(transition_witnesses(at(&current, &sibling)));
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
                source,
                outcome: DecisionOutcome::Resolved {
                    candidate,
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
    let Outcome::Refusal(refusal) = &refused else {
        panic!("a selection closing a dependency loop committed: {refused:?}")
    };
    assert_eq!(refusal.reason(), "preparation::selection-cycle");
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
        Some(
            format!(
                "{} -> {}",
                selected.born().expect("output stamp"),
                unproposed.born().expect("output stamp")
            )
            .as_str()
        ),
        "the refusal names the chain from the accepted output to the gated work"
    );
    let unchanged = board(&bus, epoch, Actor::Operator);
    let design_stamp = design.born().expect("Design stamp");
    for task in [&selected, &sibling, &unproposed] {
        let heads = at(&unchanged, task);
        assert!(heads.workflow.body.status.is_pending());
        assert!(heads.statement.body.dependencies.contains(&design_stamp));
        assert_eq!(heads.preparation_decision, None);
    }
    assert!(open_awaiting_selection(&bus, epoch).contains(&design_stamp));
    clean(&root, bus);
}
