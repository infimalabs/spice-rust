use super::*;

#[test]
fn recorded_rejections_end_selection_wait_without_releasing_outputs() {
    let (root, bus, epoch, seats) = fixture("preparation-rejected-selection", 2);
    let design = add_controller(
        &bus,
        epoch,
        "Decision design",
        wire::TaskIntent::Design,
        None,
    );
    group_design(&bus, epoch, &design);
    let accepted = add_output(&bus, epoch, &design, "Accepted output");
    let rejected = add_output(&bus, epoch, &design, "Rejected output");
    let undecided = add_output(&bus, epoch, &design, "Undecided output");

    let (mixed, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&accepted, &rejected],
        1_760_000_000_000_001,
    );
    let mixed_decision = select_candidate_outputs(&bus, epoch, &design, mixed, &[&accepted], 1);
    let after_mixed = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&after_mixed, &rejected).preparation_decision,
        Some(mixed_decision.clone())
    );
    assert_eq!(
        at(&after_mixed, &rejected).statement.body.authorization,
        None
    );
    assert_eq!(at(&after_mixed, &rejected).workflow.body.status, "pending");
    assert_eq!(
        at(&after_mixed, &accepted).statement.body.authorization,
        Some(mixed_decision)
    );
    assert_eq!(
        open_awaiting_selection(&bus, epoch),
        [design.born().expect("Design stamp")].into(),
        "the output awaiting its first decision still holds selection"
    );

    let (all_rejected, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&undecided],
        1_760_000_000_000_002,
    );
    let rejection = select_candidate_outputs(&bus, epoch, &design, all_rejected, &[], 2);
    let decided = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&decided, &undecided).preparation_decision,
        Some(rejection)
    );
    assert_eq!(at(&decided, &undecided).statement.body.authorization, None);
    assert_eq!(at(&decided, &undecided).workflow.body.status, "pending");
    assert!(open_awaiting_selection(&bus, epoch).is_empty());
    assert!(
        open_workflow_waiting(&bus, epoch).contains(&design.born().expect("Design stamp")),
        "the Design follows its released-work wait after selection is decided"
    );
    assert_controlled_output_refusal(
        conclude(&bus, epoch, Actor::Operator, &design, done_conclusion()),
        &[&rejected, &undecided],
    );
    clean(&root, bus);
}
