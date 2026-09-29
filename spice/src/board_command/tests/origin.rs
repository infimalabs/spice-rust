use super::*;
fn seed_steering_origin(bus: &Bus, key: &str) {
    let published: Instant = "2030-04-12T18:44:09.271Z"
        .parse()
        .expect("fixture publication instant");
    let expires: Instant = "2030-04-12T19:44:09.271Z"
        .parse()
        .expect("fixture expiry instant");
    let sent = body::SteerSent {
        from: None,
        directive: uuid(16_201),
        fanout: None,
        key: key.to_owned(),
        lane: uuid(16_202),
        text: "Preserve this directive\nverbatim across hydration.".to_owned(),
        control: None,
        attachments: vec![body::DirectiveAttachment {
            name: "operator capture.png".to_owned(),
            content_type: "image/png".to_owned(),
            data_url: "data:image/png;base64,iVBORw0KGgo=".to_owned(),
        }],
        expires,
    };
    let origin: wire::Origin = format!("ack:{key}")
        .parse()
        .expect("fixture steering origin");
    let mut write = JournalWrite::new(
        Actor::Operator,
        origin.clone(),
        published,
        Producer::CommandDoor,
    );
    let pending = write
        .fact(sent.clone(), FactCause::Root)
        .expect("steering fact builds");
    write.steering_head(pending, move |created| wire::SteeringHead {
        from: None,
        directive: sent.directive,
        fanout: sent.fanout,
        key: sent.key,
        lane: sent.lane,
        text: sent.text,
        control: sent.control,
        attachments: sent.attachments,
        published,
        expires: sent.expires,
        stage: body::SteeringStage::Published,
        escalations: Vec::new(),
        disposition: wire::Disposition::Live,
        origin,
        created,
        minted_micros: None,
    });
    assert!(matches!(
        bus.commit(write).expect("steering origin commits"),
        CommitOutcome::Committed(_)
    ));
}

fn add_and_claim_causal_parent(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
) -> (TaskHandle, Actor, wire::Origin) {
    let parent_stamp = stamp("ORIGIN-parent1");
    let parent = TaskHandle::for_project("board.origin", &parent_stamp);
    seed_steering_origin(bus, "1kLGp2NJ");
    let ack: wire::Origin = "ack:1kLGp2NJ".parse().expect("steering origin");
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: parent_stamp }],
        &TaskAdd {
            title: "Materialize one steering directive".to_owned(),
            project: Some("board.origin".to_owned()),
            description: None,
            acceptance: vec!["the causal root is durable".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(ack.clone()),
            flow: Some("todo,review".to_owned()),
            depends: Vec::new(),
            claim: false,
        },
    ));
    let lane = Actor::Lane(seat.lane);
    let before = board(bus, epoch, lane.clone());
    let parent_heads = at(&before, &parent);
    let capture: Name = "capture".parse().expect("capture rule");
    let CommandAnswer::Claim(claim) = assert_committed(issue(
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
        &ClaimWin {
            rule: capture,
            recovery: None,
        },
    )) else {
        panic!("claim win must hydrate the claimed task");
    };
    let expected_origin = concat!(
        "origin_steering key 1kLGp2NJ\n",
        "origin_steering_text:\n",
        "Preserve this directive\nverbatim across hydration.\n",
        "origin_steering_attachments operator capture.png",
    );
    assert_eq!(claim.origin_context.as_deref(), Some(expected_origin));
    let claimed = board(bus, epoch, lane.clone());
    let shown = task_show_with_origin(
        &claimed,
        at(&claimed, &parent),
        &[],
        claim.origin_context.as_deref(),
    );
    assert!(
        shown.contains(expected_origin),
        "task show prints the same resolved origin hydration"
    );
    (parent, lane, ack)
}

fn assert_invalid_creation_origins_refuse(bus: &Bus, epoch: CanonicalUuid) {
    let ungrounded_stamp = stamp("ORIGIN-ground00");
    let ungrounded = TaskHandle::for_project("board.origin", &ungrounded_stamp);
    let refused = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence {
            task: ungrounded_stamp,
        }],
        &TaskAdd {
            title: "Refuse synthetic team".to_owned(),
            project: Some("board.origin".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            origin: None,
            flow: Some("todo,review".to_owned()),
            depends: Vec::new(),
            claim: false,
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
    assert!(
        refusal.explanation().contains("task:<parent-born-stamp>"),
        "causal guidance names durable task identity rather than a rendered handle"
    );
    assert!(
        board(bus, epoch, Actor::Operator)
            .tasks
            .iter()
            .all(|task| task.statement.body.handle != ungrounded)
    );

    let self_referential_stamp = stamp("ORIGIN-self000");
    let self_referential = TaskHandle::for_project("board.origin", &self_referential_stamp);
    let self_origin = task_origin(&self_referential_stamp).expect("task forms its own origin");
    let refused = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence {
            task: self_referential_stamp,
        }],
        &TaskAdd {
            title: "Refuse a caller-authored self edge".to_owned(),
            project: Some("board.origin".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(self_origin.clone()),
            flow: Some("todo,review".to_owned()),
            depends: Vec::new(),
            claim: false,
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
    let expected_creation = authorize_lane_creation(&bus, epoch, &seats[0]);
    let dependency = add(
        &bus,
        epoch,
        Actor::Operator,
        "ORIGIN-order01",
        "Independent scheduling predecessor",
        Some("todo,review"),
    );
    let (parent, lane, ack) = add_and_claim_causal_parent(&bus, epoch, &seats[0]);
    let child_stamp = stamp("ORIGIN-child01");
    let child = TaskHandle::for_project("board.origin", &child_stamp);
    assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: child_stamp }],
        &TaskAdd {
            title: "Create causal child".to_owned(),
            project: Some("board.origin".to_owned()),
            description: None,
            acceptance: vec!["origin and dependency remain independent".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: None,
            flow: Some("todo,review".to_owned()),
            depends: vec![dependency.clone()],
            claim: false,
        },
    ));
    let causal = task_origin(&parent.born().expect("parent handle carries a birth stamp"))
        .expect("parent origin");
    let current = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&current, &parent).statement.body.provenance, ack);
    assert_eq!(creation_fact_origin(&bus, &parent), ack);
    assert_eq!(at(&current, &child).statement.body.provenance, causal);
    assert_eq!(
        at(&current, &child).statement.body.creation,
        expected_creation
    );
    assert_eq!(creation_fact_origin(&bus, &child), causal);
    assert_eq!(
        at(&current, &child).statement.body.dependencies,
        [dependency
            .born()
            .expect("dependency handle carries a birth stamp")],
        "causal provenance was inferred from a scheduling edge"
    );
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(at(&current, &child))],
        &TaskEdit {
            task: child.clone(),
            title: Some("Create the same causal child after correction".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: None,
        },
    ));
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &child)
            .statement
            .body
            .creation,
        expected_creation,
        "later statement revisions cannot rewrite creation authority"
    );
    assert_invalid_creation_origins_refuse(&bus, epoch);
    clean(&root, bus);
}

#[test]
fn resolved_steering_origin_renders_verbatim_directive_and_attachment_name() {
    let (root, bus, _epoch, _seats) = fixture("task-resolved-steering-origin", 0);
    seed_steering_origin(&bus, "1kLGp2NJ");
    let origin = "ack:1kLGp2NJ".parse().expect("fixture steering origin");
    let output = task_steering_origin_output(&bus, &origin)
        .expect("steering index reads")
        .expect("ack origin renders");
    assert_eq!(
        output.as_ref(),
        concat!(
            "origin_steering key 1kLGp2NJ\n",
            "origin_steering_text:\n",
            "Preserve this directive\nverbatim across hydration.\n",
            "origin_steering_attachments operator capture.png",
        )
    );
    clean(&root, bus);
}

#[test]
fn unresolved_steering_origin_is_explicit() {
    assert_eq!(
        render_task_steering_origin("1kLGp2NZ", None).as_ref(),
        "origin_steering key 1kLGp2NZ\norigin_steering unavailable unresolved-key"
    );
}
