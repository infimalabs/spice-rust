use super::*;

#[test]
fn an_open_design_waits_while_its_released_work_is_in_progress() {
    let design =
        || planning("WAIT-design", wire::TaskIntent::Design, Standing::Open).with_grouped(true);
    let settled = |status: &str, review: &str| WorkflowHead {
        status: status.to_owned(),
        review: review.to_owned(),
        completion: Some(instant("2026-08-01T06:00:00.000Z")),
        ..workflow("review")
    };
    let deferred = WorkflowHead {
        deferral: Some(instant("2026-09-01T00:00:00.000Z")),
        ..workflow("todo")
    };
    let selected =
        |workflow| released_output("WAIT-selected", "WAIT-design", "WAIT-design", workflow);
    let wait = |entries: &[Entry]| {
        board::work_waits(entries, &environment())
            .get(&stamp("WAIT-design"))
            .copied()
    };

    for (reason, entries) in [
        (
            "a selected output is in progress",
            vec![design(), selected(workflow("todo"))],
        ),
        (
            "an execution its positively concluded Plan released is in progress",
            vec![
                design(),
                planning("WAIT-plan", wire::TaskIntent::Plan, Standing::Positive),
                released_output(
                    "WAIT-execution",
                    "WAIT-plan",
                    "WAIT-design",
                    workflow("todo"),
                ),
            ],
        ),
    ] {
        assert!(
            !first_eligible(&entries),
            "{reason}: the Design is not handed out"
        );
        assert_eq!(wait(&entries), Some(board::WorkWait::Workflow), "{reason}");
    }

    for (reason, entries) in [
        (
            "its output completed",
            vec![design(), selected(settled("completed", "clean"))],
        ),
        (
            "its output was deleted",
            vec![design(), selected(settled("deleted", "unassigned"))],
        ),
        (
            "its only output is deferred",
            vec![design(), selected(deferred)],
        ),
        (
            "only another Design's released work is in progress",
            vec![
                design(),
                planning("WAIT-other", wire::TaskIntent::Design, Standing::Open).with_grouped(true),
                released_output("WAIT-foreign", "WAIT-other", "WAIT-other", workflow("todo")),
            ],
        ),
    ] {
        assert!(
            first_eligible(&entries),
            "{reason}: the Design is assignable again"
        );
        assert_eq!(wait(&entries), None, "{reason}");
    }

    let undecided = [
        design(),
        selected(workflow("todo")),
        named_output("WAIT-unselected", "WAIT-design", "WAIT-design"),
    ];
    assert_eq!(
        wait(&undecided),
        Some(board::WorkWait::Selection),
        "an unselected output still names the operator's action first"
    );
}

#[test]
fn decided_rejections_end_selection_wait_without_releasing_outputs() {
    let design =
        || planning("DECIDE-design", wire::TaskIntent::Design, Standing::Open).with_grouped(true);
    let rejected = |name| rejected_output(name, "DECIDE-design");
    let wait = |entries: &[Entry]| {
        board::work_waits(entries, &environment())
            .get(&stamp("DECIDE-design"))
            .copied()
    };

    let all_rejected = vec![
        design(),
        rejected("DECIDE-first"),
        rejected("DECIDE-second"),
    ];
    assert_eq!(wait(&all_rejected), None);
    assert!(first_eligible(&all_rejected));
    for output in &all_rejected[1..] {
        assert_eq!(output.statement().authorization, None);
        assert!(output.preparation_decision().is_some());
        assert!(
            !board::eligible_entry(
                &board::resolved_tasks(&all_rejected),
                output,
                &Actor::Lane(uuid("019fbbde-5105-75a3-a76a-722afde1e408")),
                &environment(),
            ),
            "a rejection decides selection but grants no execution authority"
        );
    }

    let mixed = vec![
        design(),
        released_output(
            "DECIDE-accepted",
            "DECIDE-design",
            "DECIDE-design",
            workflow("todo"),
        ),
        rejected("DECIDE-rejected"),
    ];
    assert_eq!(
        wait(&mixed),
        Some(board::WorkWait::Workflow),
        "released work, not the decided rejection, holds the Design"
    );

    let still_undecided = vec![
        design(),
        rejected("DECIDE-rejected"),
        named_output("DECIDE-undecided", "DECIDE-design", "DECIDE-design"),
    ];
    assert_eq!(
        wait(&still_undecided),
        Some(board::WorkWait::Selection),
        "an output awaiting its first decision still holds the Design"
    );
}

/// A filed output `name` under `controller`'s control: selected by that Design
/// when it is the preparing `parent`, otherwise prepared by `parent`.
fn released_output(name: &str, parent: &str, controller: &str, workflow: WorkflowHead) -> Entry {
    let mut head = statement(name);
    head.preparation = Some(wire::TaskPreparation {
        parent: stamp(parent),
        controller: stamp(controller),
        construction: head.born.clone(),
    });
    head.authorization = (parent == controller).then(|| wire::rooms::DecisionRef {
        controller: stamp(controller),
        decision: wire::SequenceCode::new(1),
    });
    admitted(head, workflow, None, None).expect("released output")
}

fn rejected_output(name: &str, controller: &str) -> Entry {
    named_output(name, controller, controller).with_preparation_decision(Some(
        wire::rooms::DecisionRef {
            controller: stamp(controller),
            decision: wire::SequenceCode::new(1),
        },
    ))
}
