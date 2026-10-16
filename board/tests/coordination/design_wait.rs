use super::*;

use wire::rooms::{DesignCycleFacts, DesignFacts, DesignPassEnd};

fn seq(position: u64) -> Seq {
    Seq::new(position).expect("fixture position")
}

/// An open Design named `name` holding `cycle` evidence under `active` standing.
fn design_with(name: &str, active: bool, cycle: DesignCycleFacts) -> Entry {
    planning(name, wire::TaskIntent::Design, Standing::Open)
        .with_grouped(true)
        .with_design(DesignFacts { active, cycle })
}

/// Cycle evidence after one proposal pass released its claim at seq 10.
fn passed() -> DesignCycleFacts {
    DesignCycleFacts {
        horizon: Some(seq(10)),
        end: Some(DesignPassEnd::Proposed),
        ..DesignCycleFacts::default()
    }
}

fn wait_of(entries: &[Entry], name: &str) -> Option<board::WorkWait> {
    board::work_waits(entries, &environment())
        .get(&stamp(name))
        .copied()
}

fn cycle_of(entries: &[Entry]) -> Option<wire::rooms::DesignCycle> {
    board::design_cycle(&entries[0], &board::resolved_tasks(entries))
}

#[test]
fn a_first_pass_waits_for_steeped_maturity_or_a_nudge() {
    let fresh = [design_with(
        "FIRST-design",
        true,
        DesignCycleFacts::default(),
    )];
    assert!(!first_eligible(&fresh), "an unsteeped Design gathers input");
    assert_eq!(
        cycle_of(&fresh),
        Some(wire::rooms::DesignCycle::GatheringInput)
    );
    assert_eq!(
        wait_of(&fresh, "FIRST-design"),
        Some(board::WorkWait::Workflow)
    );

    for (reason, cycle) in [
        (
            "steeped",
            DesignCycleFacts {
                steeped: true,
                ..DesignCycleFacts::default()
            },
        ),
        (
            "nudged",
            DesignCycleFacts {
                nudged: true,
                ..DesignCycleFacts::default()
            },
        ),
    ] {
        let ready = [design_with("FIRST-design", true, cycle)];
        assert!(first_eligible(&ready), "{reason}");
        assert_eq!(wait_of(&ready, "FIRST-design"), None, "{reason}");

        let passive = [design_with("FIRST-design", false, cycle)];
        assert!(
            !first_eligible(&passive),
            "{reason}: passive standing forbids possession whatever the cycle says"
        );
        assert_eq!(wait_of(&passive, "FIRST-design"), None, "{reason}");
    }
}

/// Cycle evidence after selected work settled at seq 20, past the pass.
fn settled_since() -> DesignCycleFacts {
    DesignCycleFacts {
        settled: Some(seq(20)),
        ..passed()
    }
}

fn waiting_design(cycle: DesignCycleFacts) -> Entry {
    design_with("WAIT-design", true, cycle)
}

/// An output `name` the waiting Design selected, at `workflow`.
fn selected(name: &str, workflow: WorkflowHead) -> Entry {
    released_output(name, "WAIT-design", "WAIT-design", workflow)
}

fn settled(status: wire::WorkflowStatus) -> WorkflowHead {
    WorkflowHead {
        status,
        ..workflow("review")
    }
}

fn completed() -> WorkflowHead {
    settled(wire::WorkflowStatus::reviewed(instant(
        "2026-08-01T06:00:00.000Z",
    )))
}

#[test]
fn an_open_design_waits_while_its_released_work_is_in_progress() {
    let settled_since = settled_since();
    let design = waiting_design;
    let deferred = WorkflowHead {
        deferral: Some(instant("2026-09-01T00:00:00.000Z")),
        ..workflow("todo")
    };

    for (reason, entries) in [
        (
            "a selected output is in progress",
            vec![
                design(passed()),
                selected("WAIT-selected", workflow("todo")),
            ],
        ),
        (
            "an execution its positively concluded Plan released is in progress",
            vec![
                design(passed()),
                planning("WAIT-plan", wire::TaskIntent::Plan, Standing::Positive),
                released_output(
                    "WAIT-execution",
                    "WAIT-plan",
                    "WAIT-design",
                    workflow("todo"),
                ),
            ],
        ),
        (
            "one selected output settled while another is still in progress",
            vec![
                design(settled_since),
                selected("WAIT-selected", completed()),
                selected("WAIT-running", workflow("todo")),
            ],
        ),
        (
            "its only output is deferred, which never settles the pass",
            vec![
                design(passed()),
                selected("WAIT-selected", deferred.clone()),
            ],
        ),
        (
            "one selected output settled while another is deferred",
            vec![
                design(settled_since),
                selected("WAIT-selected", completed()),
                selected("WAIT-deferred", deferred),
            ],
        ),
    ] {
        assert!(
            !first_eligible(&entries),
            "{reason}: the Design is not handed out"
        );
        assert_eq!(
            wait_of(&entries, "WAIT-design"),
            Some(board::WorkWait::Workflow),
            "{reason}"
        );
    }
}

#[test]
fn an_open_design_is_assignable_once_its_released_work_settles_or_input_arrives() {
    let settled_since = settled_since();
    let design = waiting_design;
    let input_since = DesignCycleFacts {
        input: Some(seq(20)),
        ..passed()
    };

    for (reason, entries) in [
        (
            "its output completed",
            vec![
                design(settled_since),
                selected("WAIT-selected", completed()),
            ],
        ),
        (
            "its output was deleted",
            vec![
                design(settled_since),
                selected(
                    "WAIT-selected",
                    settled(wire::WorkflowStatus::deleted(instant(
                        "2026-08-01T06:00:00.000Z",
                    ))),
                ),
            ],
        ),
        (
            "counted input arrived while its output is still in progress",
            vec![
                design(input_since),
                selected("WAIT-selected", workflow("todo")),
            ],
        ),
        (
            "only another Design's released work is in progress",
            vec![
                design(settled_since),
                design_with("WAIT-other", true, passed()),
                released_output("WAIT-foreign", "WAIT-other", "WAIT-other", workflow("todo")),
            ],
        ),
    ] {
        assert!(
            first_eligible(&entries),
            "{reason}: the Design is assignable again"
        );
        assert_eq!(wait_of(&entries, "WAIT-design"), None, "{reason}");
    }
}

#[test]
fn only_an_undecided_proposal_holds_the_design_for_selection() {
    let input_since = DesignCycleFacts {
        input: Some(seq(20)),
        ..passed()
    };
    let proposed = [
        design_with(
            "WAIT-design",
            true,
            DesignCycleFacts {
                proposal: Some(seq(12)),
                ..input_since
            },
        ),
        released_output(
            "WAIT-selected",
            "WAIT-design",
            "WAIT-design",
            workflow("todo"),
        ),
    ];
    assert!(!first_eligible(&proposed));
    assert_eq!(
        wait_of(&proposed, "WAIT-design"),
        Some(board::WorkWait::Selection),
        "an undecided proposal names the operator's action before input or running work"
    );

    // A pass that constructed an output, then freed or lapsed without proposing.
    let abandoned = DesignCycleFacts {
        horizon: Some(seq(10)),
        end: Some(DesignPassEnd::Abandoned),
        ..DesignCycleFacts::default()
    };
    let constructed = |cycle| {
        [
            design_with("WAIT-design", true, cycle),
            named_output("WAIT-unproposed", "WAIT-design", "WAIT-design"),
        ]
    };
    let gathering = constructed(abandoned);
    assert!(!first_eligible(&gathering));
    assert_eq!(
        cycle_of(&gathering),
        Some(wire::rooms::DesignCycle::GatheringInput),
        "an output no proposal names awaits no decision"
    );
    assert!(
        first_eligible(&constructed(DesignCycleFacts {
            input: Some(seq(20)),
            ..abandoned
        })),
        "counted input readies the next pass"
    );
}

#[test]
fn decided_rejections_end_selection_wait_without_releasing_outputs() {
    let feedback = DesignCycleFacts {
        input: Some(seq(11)),
        ..passed()
    };
    let design = |cycle| design_with("DECIDE-design", true, cycle);
    let rejected = |name| rejected_output(name, "DECIDE-design");
    let accepted = || {
        released_output(
            "DECIDE-accepted",
            "DECIDE-design",
            "DECIDE-design",
            workflow("todo"),
        )
    };

    let all_rejected = vec![
        design(feedback),
        rejected("DECIDE-first"),
        rejected("DECIDE-second"),
    ];
    assert_eq!(wait_of(&all_rejected, "DECIDE-design"), None);
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

    let mixed = vec![design(feedback), accepted(), rejected("DECIDE-rejected")];
    assert!(
        first_eligible(&mixed),
        "rejection feedback readies the Design while its accepted work runs"
    );
    let clean = vec![design(passed()), accepted()];
    assert_eq!(
        wait_of(&clean, "DECIDE-design"),
        Some(board::WorkWait::Workflow),
        "a clean acceptance waits on the work it released"
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
