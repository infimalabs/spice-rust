use super::*;

fn add_and_claim_causal_parent(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
) -> (TaskHandle, Actor, wire::Origin) {
    let parent = "ORIGIN-parent1"
        .parse::<TaskHandle>()
        .expect("parent handle");
    let ack: wire::Origin = "ack:1kLGp2NJ".parse().expect("steering origin");
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence {
            task: parent.clone(),
        }],
        &TaskAdd {
            title: "Materialize one steering directive".to_owned(),
            project: Some("board.origin".to_owned()),
            description: None,
            acceptance: vec!["the causal root is durable".to_owned()],
            origin: Some(ack.clone()),
            flow: Some("todo,review".to_owned()),
            depends: Vec::new(),
        },
    ));
    let lane = Actor::Lane(seat.lane);
    let before = board(bus, epoch, lane.clone());
    let parent_heads = at(&before, &parent);
    let capture: Name = "capture".parse().expect("capture rule");
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(parent_heads),
            workflow_witness(parent_heads),
            claim_witness(parent_heads),
            Witness::Eligibility {
                rule: capture.clone(),
            },
            residency(&before, seat),
        ],
        &ClaimWin { rule: capture },
    ));
    (parent, lane, ack)
}

fn assert_invalid_creation_origins_refuse(bus: &Bus, epoch: CanonicalUuid) {
    let ungrounded = "ORIGIN-ground00"
        .parse::<TaskHandle>()
        .expect("ungrounded handle");
    let refused = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence {
            task: ungrounded.clone(),
        }],
        &TaskAdd {
            title: "Refuse synthetic lineage".to_owned(),
            project: Some("board.origin".to_owned()),
            description: None,
            acceptance: Vec::new(),
            origin: None,
            flow: Some("todo,review".to_owned()),
            depends: Vec::new(),
        },
    );
    let Outcome::Refusal(ref refusal) = refused else {
        panic!("ungrounded task creation must refuse: {refused:?}")
    };
    assert_eq!(refusal.reason(), "task::origin-required");
    assert!(refusal.diagnostic().is_some());
    assert!(
        refusal
            .explanation()
            .contains("--origin ack:<steering-key>")
    );
    assert!(refusal.explanation().contains("task:<parent-handle>"));
    assert!(
        board(bus, epoch, Actor::Operator)
            .tasks
            .iter()
            .all(|task| task.statement.body.handle != ungrounded)
    );

    let self_referential = "ORIGIN-self000"
        .parse::<TaskHandle>()
        .expect("self-referential handle");
    let self_origin = task_origin(&self_referential).expect("task forms its own origin");
    let refused = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence {
            task: self_referential.clone(),
        }],
        &TaskAdd {
            title: "Refuse a caller-authored self edge".to_owned(),
            project: Some("board.origin".to_owned()),
            description: None,
            acceptance: Vec::new(),
            origin: Some(self_origin.clone()),
            flow: Some("todo,review".to_owned()),
            depends: Vec::new(),
        },
    );
    let self_origin_text = self_origin.to_string();
    let Outcome::Refusal(ref refusal) = refused else {
        panic!("self-originating task creation must refuse: {refused:?}")
    };
    assert_eq!(refusal.reason(), "task::origin-self");
    assert_eq!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::observed),
        Some(self_origin_text.as_str())
    );
    assert!(
        board(bus, epoch, Actor::Operator)
            .tasks
            .iter()
            .all(|task| task.statement.body.handle != self_referential)
    );
}

#[test]
fn shape_case_162_builds_a_causal_tree_and_refuses_ungrounded_creation() {
    let (root, bus, epoch, seats) = fixture("task-causal-origin", 1);
    let dependency = add(
        &bus,
        epoch,
        Actor::Operator,
        "ORIGIN-order01",
        "Independent scheduling predecessor",
        Some("todo,review"),
    );
    let (parent, lane, ack) = add_and_claim_causal_parent(&bus, epoch, &seats[0]);
    let child = "ORIGIN-child01"
        .parse::<TaskHandle>()
        .expect("child handle");
    assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskAdd,
        vec![Witness::Absence {
            task: child.clone(),
        }],
        &TaskAdd {
            title: "Create causal child".to_owned(),
            project: Some("board.origin".to_owned()),
            description: None,
            acceptance: vec!["origin and dependency remain independent".to_owned()],
            origin: None,
            flow: Some("todo,review".to_owned()),
            depends: vec![dependency.clone()],
        },
    ));
    let causal: wire::Origin = format!("task:{parent}").parse().expect("parent origin");
    let current = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&current, &parent).statement.body.provenance, ack);
    assert_eq!(creation_fact_origin(&bus, &parent), ack);
    assert_eq!(at(&current, &child).statement.body.provenance, causal);
    assert_eq!(creation_fact_origin(&bus, &child), causal);
    assert_eq!(
        at(&current, &child).statement.body.dependencies,
        [dependency],
        "causal provenance was inferred from a scheduling edge"
    );
    assert_invalid_creation_origins_refuse(&bus, epoch);
    clean(&root, bus);
}
