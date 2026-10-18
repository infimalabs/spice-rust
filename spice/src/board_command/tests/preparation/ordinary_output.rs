use super::*;

/// A Design releases its next step whatever its kind: an accepted ordinary
/// output loses only its Design edge and becomes capturable work with no Plan
/// between them, while its rejected sibling stays gated.
#[test]
fn a_design_selects_an_ordinary_output_that_runs_without_a_plan() {
    let (root, bus, epoch, seats) = fixture("preparation-design-ordinary-output", 2);
    let design = add_controller(
        &bus,
        epoch,
        "Ordinary design",
        wire::TaskIntent::Design,
        None,
    );
    group_design(&bus, epoch, &design);
    claim_task(&bus, epoch, &seats[0], &Actor::Lane(seats[0].lane), &design);
    let selected = add_output(&bus, epoch, &design, "Selected ordinary task");
    let sibling = add_output(&bus, epoch, &design, "Sibling ordinary task");
    let born = |task: &TaskHandle| task.born().expect("task stamp");

    let (candidate, source) =
        retain_test_candidate(&bus, epoch, &seats, &design, &selected, &sibling);
    let _coverage = record_test_coverage(&bus, epoch, &seats, &design, &candidate, &source);
    authorize_test_output(&bus, epoch, &design, &selected, &sibling, candidate);
    assert_eq!(
        open_workflow_waiting(&bus, epoch),
        std::collections::BTreeSet::from([born(&sibling)]),
        "only the rejected sibling stays gated"
    );
    assert!(
        !at(&board(&bus, epoch, Actor::Operator), &selected)
            .statement
            .body
            .dependencies
            .contains(&born(&design)),
        "acceptance removes exactly the Design edge"
    );
    capture_for(&bus, epoch, &seats[1], &selected);
    clean(&root, bus);
}
