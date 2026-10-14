//! Only the operator concludes an unheld open Design, done or collapsed.

use std::collections::BTreeMap;

use super::*;

pub(super) fn done_conclusion() -> body::DesignConclusion {
    body::DesignConclusion::Done {}
}

fn collapse(reason: &str) -> body::DesignConclusion {
    body::DesignConclusion::Collapsed {
        reason: reason.to_owned(),
    }
}

fn conclusion_witnesses(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> Vec<Witness> {
    let visible = board(bus, epoch, Actor::Operator);
    let heads = at(&visible, task);
    vec![
        statement_witness(heads),
        workflow_witness(heads),
        claim_witness(heads),
    ]
}

fn conclude_witnessed(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    task: &TaskHandle,
    conclusion: body::DesignConclusion,
    expected: Vec<Witness>,
) -> Outcome<CommandAnswer> {
    issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskConclude,
        expected,
        &TaskConclude {
            task: task.clone(),
            conclusion,
        },
    )
}

/// Drive one conclusion against the heads the board shows right now.
pub(super) fn conclude(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    task: &TaskHandle,
    conclusion: body::DesignConclusion,
) -> Outcome<CommandAnswer> {
    let expected = conclusion_witnesses(bus, epoch, task);
    conclude_witnessed(bus, epoch, actor, task, conclusion, expected)
}

fn refusal_reason(outcome: &Outcome<CommandAnswer>) -> &str {
    match outcome {
        Outcome::Refusal(refusal) => refusal.reason(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn workflow(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> wire::WorkflowHead {
    at(&board(bus, epoch, Actor::Operator), task)
        .workflow
        .body
        .clone()
}

/// Admit a conclusion as `actor` without dispatching it.
fn fixture_command_admission(
    epoch: CanonicalUuid,
    actor: Actor,
    expected: Vec<Witness>,
    task: &TaskHandle,
) -> Result<wire::AdmittedCommand, wire::CommandRejection> {
    Command {
        v: WireVersion::CURRENT,
        id: uuid(9_999),
        epoch,
        kind: CommandKind::TaskConclude,
        expected,
        body: serde_json::to_value(TaskConclude {
            task: task.clone(),
            conclusion: done_conclusion(),
        })
        .expect("conclusion encodes"),
    }
    .admit(&Accepted {
        wire: WireVersion::CURRENT,
        actor,
        epoch,
        release: "1.2.3".parse().expect("fixture release"),
    })
}

#[test]
fn the_operator_alone_concludes_an_unheld_open_design() {
    let (root, bus, epoch, seats) = fixture("design-conclude-done", 1);
    let lane = Actor::Lane(seats[0].lane);
    let design = add_controller(
        &bus,
        epoch,
        "Concluded design",
        wire::TaskIntent::Design,
        None,
    );
    let by_lane = fixture_command_admission(
        epoch,
        lane.clone(),
        conclusion_witnesses(&bus, epoch, &design),
        &design,
    );
    assert!(
        matches!(&by_lane, Err(wire::CommandRejection::Refusal(refusal)) if refusal.reason() == "command::authority"),
        "a lane principal never concludes a Design: {by_lane:?}"
    );
    group_design(&bus, epoch, &design);
    claim_task(&bus, epoch, &seats[0], &lane, &design);
    let held = conclude(&bus, epoch, Actor::Operator, &design, done_conclusion());
    assert_eq!(refusal_reason(&held), "task::conclude-claimed");
    let Outcome::Refusal(refusal) = &held else {
        unreachable!()
    };
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
        Some(lane.to_string().as_str()),
        "the refusal names the holder who must free it"
    );
    assert_eq!(workflow(&bus, epoch, &design).status, "pending");

    // A conclusion witnessed while the Design was held refuses once the holder
    // frees it, rather than concluding over heads it did not read.
    let stale = conclusion_witnesses(&bus, epoch, &design);
    free_active(&bus, epoch, &lane, &seats[0], &design);
    let raced = conclude_witnessed(
        &bus,
        epoch,
        Actor::Operator,
        &design,
        done_conclusion(),
        stale,
    );
    assert!(matches!(raced, Outcome::Refusal(_)), "{raced:?}");
    assert_eq!(workflow(&bus, epoch, &design).status, "pending");

    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &design,
        done_conclusion(),
    ));
    let concluded = workflow(&bus, epoch, &design);
    assert_eq!(concluded.status, "completed");
    assert!(concluded.completion.is_some());
    assert!(
        concluded.concluded_positively(),
        "done concludes positively"
    );
    for again in [done_conclusion(), collapse("premise failed")] {
        let repeated = conclude(&bus, epoch, Actor::Operator, &design, again);
        assert_eq!(refusal_reason(&repeated), "task::settled");
    }
    assert_eq!(workflow(&bus, epoch, &design), concluded);
    clean(&root, bus);
}

#[test]
fn conclusion_and_closure_each_refuse_the_other_purpose() {
    let (root, bus, epoch, _seats) = fixture("design-conclude-purpose", 1);
    let design = add_controller(&bus, epoch, "Closed design", wire::TaskIntent::Design, None);
    let plan = add_plan(&bus, epoch, "Concluded plan");
    let ordinary = add(
        &bus,
        epoch,
        Actor::Operator,
        "Concluded ordinary",
        "Concluded ordinary",
        None,
    );
    for task in [&plan, &ordinary] {
        let refused = conclude(&bus, epoch, Actor::Operator, task, done_conclusion());
        assert_eq!(refusal_reason(&refused), "task::conclude-not-design");
    }
    let visible = board(&bus, epoch, Actor::Operator);
    let heads = at(&visible, &design);
    let closed = issue(
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
            task: design.clone(),
            validation: "closed".to_owned(),
            force_claimed: true,
        },
    );
    assert_eq!(refusal_reason(&closed), "task::close-design");
    assert_eq!(workflow(&bus, epoch, &design).status, "pending");
    clean(&root, bus);
}

#[test]
fn collapse_records_its_reason_and_gates_every_output_again() {
    let (root, bus, epoch, seats) = fixture("design-conclude-collapse", 2);
    let design = add_controller(
        &bus,
        epoch,
        "Collapsed design",
        wire::TaskIntent::Design,
        None,
    );
    let output = add_output(&bus, epoch, &design, "Selected collapsed output");
    let spare = add_output(&bus, epoch, &design, "Unselected collapsed output");
    let (candidate, source) = retain_test_candidate(&bus, epoch, &seats, &design, &output, &spare);
    record_test_critique(&bus, epoch, &seats, &design, &candidate, &source);
    let _coverage = record_test_coverage(&bus, epoch, &seats, &design, &candidate, &source);
    record_test_decisions(&bus, epoch, &design, &output, &candidate, &source);
    authorize_test_output(&bus, epoch, &design, &output, &spare, candidate);
    let output_stamp = output.born().expect("output stamp");
    assert!(!open_blocked(&bus, epoch).contains(&output_stamp));

    for blank in ["", "  \n"] {
        let refused = conclude(&bus, epoch, Actor::Operator, &design, collapse(blank));
        assert_eq!(refusal_reason(&refused), "task::collapse-reason-empty");
    }
    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &design,
        collapse("the premise did not hold"),
    ));
    let collapsed = workflow(&bus, epoch, &design);
    assert_eq!(collapsed.status, "completed");
    assert!(
        !collapsed.concluded_positively(),
        "collapse concludes negatively"
    );
    assert_eq!(
        open_blocked(&bus, epoch),
        [output_stamp, spare.born().expect("spare stamp")].into(),
        "a collapsed Design releases nothing, even what it selected"
    );
    let design_stamp = design.born().expect("design stamp");
    let recorded = bus
        .fact_snapshot()
        .expect("conclusion facts read")
        .facts()
        .iter()
        .filter_map(|fact| fact.fact.read::<body::TaskConcluded>())
        .filter(|concluded| concluded.task == design_stamp)
        .collect::<Vec<_>>();
    assert_eq!(
        recorded,
        vec![body::TaskConcluded {
            task: design_stamp,
            conclusion: collapse("the premise did not hold"),
        }],
        "the collapse keeps its stated reason durably"
    );
    clean(&root, bus);
}

#[test]
fn terminal_standing_outranks_retained_group_on_board_and_group_reads() {
    let (root, bus, epoch, _) = fixture("design-conclude-standing", 0);
    let design = |name| add_controller(&bus, epoch, name, wire::TaskIntent::Design, None);
    let (collapsed, done, open) = (
        design("Collapsed grouped design"),
        design("Done grouped design"),
        design("Open grouped design"),
    );
    for design in [&collapsed, &done, &open] {
        group_design(&bus, epoch, design);
    }
    let reason = "the premise did not hold";
    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &collapsed,
        collapse(reason),
    ));
    assert_committed(conclude(
        &bus,
        epoch,
        Actor::Operator,
        &done,
        done_conclusion(),
    ));

    let expected = BTreeMap::from([
        (
            born(&collapsed),
            wire::intent::DesignStanding::Collapsed {
                reason: Some(reason.to_owned()),
            },
        ),
        (born(&done), wire::intent::DesignStanding::Done),
        (born(&open), wire::intent::DesignStanding::Active),
    ]);
    let board = board(&bus, epoch, Actor::Operator)
        .tasks
        .iter()
        .filter_map(|task| Some((task.statement.body.born.clone(), task.design_standing()?)))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(board, expected, "board rows");
    let Outcome::Reply(Reply {
        body: CommandAnswer::WorkingGroupPage(page),
        ..
    }) = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::WorkingGroupRead,
        Vec::new(),
        &wire::intent::WorkingGroupRead {},
    )
    else {
        panic!("working-group read did not return a page");
    };
    let grouped = page
        .groups
        .into_iter()
        .flat_map(|group| group.anchors)
        .filter_map(|anchor| match anchor {
            wire::intent::WorkingGroupAnchor::Available { born, design, .. } => {
                Some((born, design))
            }
            wire::intent::WorkingGroupAnchor::Unavailable { .. } => None,
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(grouped, expected, "working-group anchors");
    clean(&root, bus);
}

fn born(task: &TaskHandle) -> TaskStamp {
    task.born().expect("fixture task stamp")
}
