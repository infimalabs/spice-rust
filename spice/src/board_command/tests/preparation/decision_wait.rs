use super::withdrawal::withdraw;
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

#[test]
fn a_later_open_candidate_shadows_an_older_verdict_until_decided_or_withdrawn() {
    let (root, bus, epoch, seats) = fixture("preparation-reproposed-selection", 1);
    let design = add_controller(
        &bus,
        epoch,
        "Reproposal design",
        wire::TaskIntent::Design,
        None,
    );
    group_design(&bus, epoch, &design);
    let output = add_output(&bus, epoch, &design, "Re-proposed output");
    let design_stamp = design.born().expect("Design stamp");
    let decision_of = |bus: &Bus| {
        at(&board(bus, epoch, Actor::Operator), &output)
            .preparation_decision
            .clone()
    };

    let (first, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&output],
        1_760_000_000_000_001,
    );
    let rejection = select_candidate_outputs(&bus, epoch, &design, first, &[], 1);
    assert!(open_awaiting_selection(&bus, epoch).is_empty());
    assert_eq!(decision_of(&bus), Some(rejection.clone()));

    let (withdrawn, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&output],
        1_760_000_000_000_002,
    );
    assert_eq!(
        open_awaiting_selection(&bus, epoch),
        [design_stamp.clone()].into(),
        "re-proposing a rejected output owes the operator a decision again"
    );
    assert_eq!(decision_of(&bus), None);
    assert_committed(withdraw(
        &bus,
        epoch,
        Actor::Lane(seats[0].lane),
        &design,
        &withdrawn,
        "the next version corrects it",
    ));
    assert!(open_awaiting_selection(&bus, epoch).is_empty());
    assert_eq!(
        decision_of(&bus),
        Some(rejection),
        "withdrawal restores the verdict beneath it"
    );

    let (corrected, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&output],
        1_760_000_000_000_003,
    );
    assert_eq!(open_awaiting_selection(&bus, epoch), [design_stamp].into());
    let renewed = select_candidate_outputs(&bus, epoch, &design, corrected, &[], 2);
    assert!(open_awaiting_selection(&bus, epoch).is_empty());
    assert_eq!(decision_of(&bus), Some(renewed));
    let current = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&current, &output).statement.body.authorization, None);
    assert_eq!(at(&current, &output).workflow.body.status, "pending");
    clean(&root, bus);
}

#[test]
fn settling_an_older_version_leaves_a_later_open_version_awaiting_selection() {
    let (root, bus, epoch, seats) = fixture("preparation-overlapping-selection", 1);
    let design = add_controller(
        &bus,
        epoch,
        "Overlapping design",
        wire::TaskIntent::Design,
        None,
    );
    group_design(&bus, epoch, &design);
    let accepted = add_output(&bus, epoch, &design, "Accepted output");
    let contested = add_output(&bus, epoch, &design, "Contested output");

    let (older, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&accepted, &contested],
        1_760_000_000_000_001,
    );
    let (later, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&contested],
        1_760_000_000_000_002,
    );
    let older_decision = select_candidate_outputs(&bus, epoch, &design, older, &[&accepted], 1);
    let after_older = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&after_older, &accepted).statement.body.authorization,
        Some(older_decision.clone())
    );
    assert_eq!(at(&after_older, &contested).preparation_decision, None);
    assert_eq!(
        open_awaiting_selection(&bus, epoch),
        [design.born().expect("Design stamp")].into(),
        "an older version's verdict never masks a later open version"
    );

    let later_decision = select_candidate_outputs(&bus, epoch, &design, later, &[], 2);
    assert_ne!(later_decision, older_decision);
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &contested).preparation_decision,
        Some(later_decision)
    );
    assert!(open_awaiting_selection(&bus, epoch).is_empty());
    clean(&root, bus);
}
