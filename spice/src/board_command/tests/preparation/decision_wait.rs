use super::withdrawal::withdraw;
use super::*;

fn offered(bus: &Bus, epoch: CanonicalUuid, seat: &Seat) -> Option<TaskStamp> {
    match next_board(bus, epoch, Actor::Lane(seat.lane)).next {
        Some(NextWorkState::Candidate { task }) => Some(task.statement.body.born),
        _ => None,
    }
}

#[test]
fn an_operator_nudge_readies_only_an_open_design_gathering_its_first_pass() {
    let (root, bus, epoch, seats) = fixture("preparation-design-nudge", 2);
    let design = add_controller(&bus, epoch, "Nudged design", wire::TaskIntent::Design, None);
    let plan = add_controller(&bus, epoch, "Not a Design", wire::TaskIntent::Plan, None);
    capture_for(&bus, epoch, &seats[1], &plan);
    stand_design(&bus, epoch, &design);
    assert_eq!(
        offered(&bus, epoch, &seats[0]),
        None,
        "an active Design short of Steeped maturity gathers input"
    );

    let current = board(&bus, epoch, Actor::Operator);
    assert!(matches!(
        nudge_design(&bus, epoch, Actor::Operator, &current, &plan),
        Outcome::Refusal(refusal) if refusal.reason() == "preparation::nudge-not-open-design"
    ));
    assert_committed(nudge_design(
        &bus,
        epoch,
        Actor::Operator,
        &current,
        &design,
    ));
    assert_eq!(
        offered(&bus, epoch, &seats[0]),
        Some(design.born().expect("Design stamp"))
    );
    capture_for(&bus, epoch, &seats[0], &design);
    clean(&root, bus);
}

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
    claim_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &design);
    let accepted = add_plan_output(&bus, epoch, &design, "Accepted output");
    let rejected = add_plan_output(&bus, epoch, &design, "Rejected output");
    let undecided = add_plan_output(&bus, epoch, &design, "Undecided output");
    let design_stamp = design.born().expect("Design stamp");

    let (mixed, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&accepted, &rejected, &undecided],
        1_760_000_000_000_001,
    );
    let mixed_decision = select_candidate_outputs(&bus, epoch, &design, mixed, &[&accepted], 1);
    let after_mixed = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        at(&after_mixed, &rejected).preparation_decision,
        Some(mixed_decision.clone())
    );
    assert!(
        at(&after_mixed, &rejected)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert!(
        at(&after_mixed, &rejected)
            .workflow
            .body
            .status
            .is_pending()
    );
    assert!(
        !at(&after_mixed, &accepted)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert_eq!(
        at(&after_mixed, &undecided).preparation_decision,
        Some(mixed_decision)
    );
    assert!(open_awaiting_selection(&bus, epoch).is_empty());

    assert!(
        !open_workflow_waiting(&bus, epoch).contains(&design.born().expect("Design stamp")),
        "a recorded rejection is counted input: the Design is ready for its next pass"
    );
    let CommandAnswer::Task(answer) = assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &design,
        done_conclusion(),
    )) else {
        panic!("Design conclusion returned another answer")
    };
    assert_eq!(answer.closed.len(), 2);
    assert!(answer.closed.contains(&rejected));
    assert!(answer.closed.contains(&undecided));
    let settled = board(&bus, epoch, Actor::Operator);
    assert!(at(&settled, &accepted).workflow.body.status.is_pending());
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
    claim_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &design);
    let output = add_plan_output(&bus, epoch, &design, "Re-proposed output");
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

    // Withdrawal is not counted input; the operator readies the corrected pass.
    let current = board(&bus, epoch, Actor::Operator);
    assert_committed(nudge_design(
        &bus,
        epoch,
        Actor::Operator,
        &current,
        &design,
    ));
    let (corrected, _) = retain_candidate_version(
        &bus,
        epoch,
        &seats[0],
        &design,
        &[&output],
        1_760_000_000_000_003,
    );
    assert_eq!(
        open_awaiting_selection(&bus, epoch),
        [design_stamp.clone()].into()
    );
    let renewed = select_candidate_outputs(&bus, epoch, &design, corrected, &[], 2);
    assert!(open_awaiting_selection(&bus, epoch).is_empty());
    assert_eq!(decision_of(&bus), Some(renewed));
    let current = board(&bus, epoch, Actor::Operator);
    assert!(
        at(&current, &output)
            .statement
            .body
            .dependencies
            .contains(&design_stamp)
    );
    assert!(at(&current, &output).workflow.body.status.is_pending());
    clean(&root, bus);
}
