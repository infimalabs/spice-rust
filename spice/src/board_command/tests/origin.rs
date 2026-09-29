use super::*;
fn seed_steering_origin(bus: &Bus, key: &str) {
    seed_steering_directive(bus, key, uuid(16_201));
}

fn seed_steering_directive(bus: &Bus, key: &str, directive: CanonicalUuid) {
    let published: Instant = "2030-04-12T18:44:09.271Z"
        .parse()
        .expect("fixture publication instant");
    let expires: Instant = "2030-04-12T19:44:09.271Z"
        .parse()
        .expect("fixture expiry instant");
    let sent = body::SteerSent {
        from: None,
        directive,
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
    let expected_origin = format!(
        "origin_steering key 1kLGp2NJ\n\
         origin_steering_directive {} lane={} author=operator published=2030-04-12T18:44:09.271Z\n\
         origin_steering_text:\n\
         Preserve this directive\nverbatim across hydration.\n\
         origin_steering_attachments operator capture.png\n\
         origin_steering_answer unavailable live",
        uuid(16_201),
        uuid(16_202),
    );
    assert_eq!(
        claim.origin_context.as_deref(),
        Some(expected_origin.as_str())
    );
    let claimed = board(bus, epoch, lane.clone());
    let shown = task_show_with_origin(
        &claimed,
        at(&claimed, &parent),
        &[],
        TaskShowHistory::default(),
        claim.origin_context.as_deref(),
    );
    assert!(
        shown.contains(&expected_origin),
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

/// Commit `declared` sessions on one tape, then retire `key` by an
/// acknowledgment at member 1 offset 40 of that tape.
fn answer_steering_origin(bus: &Bus, key: &str, source: CanonicalUuid, declared: &[(&str, u64)]) {
    let lane = uuid(16_202);
    let epoch = uuid(16_204);
    let origin: wire::Origin = format!("resident:{epoch}")
        .parse()
        .expect("resident fixture origin");
    let at = fixture_instant("acknowledgment fixture instant");
    for (identity, offset) in declared {
        let mut write = JournalWrite::new(
            Actor::Spice,
            origin.clone(),
            at,
            Producer::ResidentRecognition,
        );
        write
            .fact(
                body::LaneSessionDeclared {
                    lane,
                    occupant: uuid(16_205),
                    worktree: "/fixture/worktree".to_owned(),
                    vendor: "omp".parse().expect("fixture vendor"),
                    launch: None,
                    epoch,
                    stream: "stdout".to_owned(),
                    source: Some(source),
                    identity: (*identity).to_owned(),
                    member: 1,
                    offset: *offset,
                    length: 5,
                },
                FactCause::Root,
            )
            .expect("session declaration builds");
        assert!(matches!(
            bus.commit(write).expect("session declaration commits"),
            CommitOutcome::Committed(_)
        ));
    }
    let lookup = key.to_owned();
    let head = bus
        .read(move |read| read.steering_key(&lookup))
        .expect("steering index reads")
        .expect("seeded directive");
    let mut write = JournalWrite::new(
        Actor::Lane(lane),
        head.body().origin.clone(),
        at,
        Producer::AckScanner,
    );
    let pending = write
        .fact(
            body::SteerAcked {
                directive: head.body().directive,
                key: key.to_owned(),
                lane,
                epoch,
                locus: body::TranscriptLocus {
                    source: Some(source),
                    member: 1,
                    offset: 40,
                    length: 7,
                    event: 0,
                    line: 1,
                    column: 1,
                },
            },
            FactCause::Existing(head.position()),
        )
        .expect("acknowledgment builds");
    let mut acked = head.body().clone();
    acked.disposition = wire::Disposition::Acked;
    write.steering_head(pending, move |_| acked);
    let outcome = bus.commit(write).expect("acknowledgment commits");
    assert!(
        matches!(outcome, CommitOutcome::Committed(_)),
        "{outcome:?}"
    );
}

/// Add one task under `ack:1kLGp2NJ`, note it under `ack:1kLGp2NK`, and read
/// it back with its notes.
fn add_task_noted_under_a_second_ask(
    bus: &Bus,
    epoch: CanonicalUuid,
    born: &wire::TaskStamp,
) -> BoardAnswer {
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born.clone() }],
        &TaskAdd {
            title: "Follow an acknowledged ask".to_owned(),
            project: Some("board.origin".to_owned()),
            description: None,
            acceptance: vec!["the ask is traceable".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some("ack:1kLGp2NJ".parse().expect("steering origin")),
            flow: Some("todo,review".to_owned()),
            depends: Vec::new(),
            claim: false,
        },
    ));
    // A note written under another directive carries that directive's key.
    let mut write = JournalWrite::new(
        Actor::Operator,
        "ack:1kLGp2NK".parse().expect("note origin"),
        fixture_instant("note fixture instant"),
        Producer::CommandDoor,
    );
    write
        .fact(
            body::TaskNoted {
                task: born.clone(),
                note: "observed under a second ask".to_owned(),
            },
            FactCause::Root,
        )
        .expect("note builds");
    assert!(matches!(
        bus.commit(write).expect("note commits"),
        CommitOutcome::Committed(_)
    ));
    let Outcome::Reply(Reply {
        body: CommandAnswer::Board(answer),
        ..
    }) = issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::BoardRead,
        Vec::new(),
        &BoardRead::Tasks {
            tasks: vec![born.clone()],
            dependencies: false,
            claims: false,
            notes: true,
        },
    )
    else {
        panic!("task read answers a board");
    };
    *answer
}

#[test]
fn task_show_follows_every_acknowledged_key_to_the_session_that_answered_it() {
    let (root, bus, epoch, _seats) = fixture("task-acknowledgment-provenance", 0);
    let source = uuid(16_206);
    seed_steering_directive(&bus, "1kLGp2NJ", uuid(16_201));
    seed_steering_directive(&bus, "1kLGp2NK", uuid(16_203));
    // The answer names the session declared at or before its record, not a
    // later declaration on the same tape.
    answer_steering_origin(
        &bus,
        "1kLGp2NJ",
        source,
        &[("creator-session", 10), ("later-session", 50)],
    );
    let born = stamp("ORIGIN-ackshow");
    let task = TaskHandle::for_project("board.origin", &born);
    let answer = add_task_noted_under_a_second_ask(&bus, epoch, &born);
    let acknowledgments = &answer.acknowledgments[&born];
    assert_eq!(
        acknowledgments
            .iter()
            .map(|row| (row.kind, row.key.as_str()))
            .collect::<Vec<_>>(),
        [
            (wire::FactKind::TaskAdded, "1kLGp2NJ"),
            (wire::FactKind::TaskNoted, "1kLGp2NK"),
        ]
    );
    let keys = task_acknowledgment_keys(
        &at(&answer, &task).statement.body.provenance,
        acknowledgments,
    );
    let context = bus
        .read(move |read| {
            ["1kLGp2NJ", "1kLGp2NK"]
                .iter()
                .map(|key| {
                    read_steering_provenance(read, key)
                        .map(|provenance| render_task_steering_origin(key, provenance.as_ref()))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .expect("steering provenance reads")
        .join("\n");
    assert_eq!(keys, ["1kLGp2NJ", "1kLGp2NK"]);
    let shown = task_show_with_origin(
        &answer,
        at(&answer, &task),
        &[],
        TaskShowHistory {
            notes: &answer.notes[&born],
            acknowledgments,
        },
        Some(&context),
    );
    let (added, noted) = (acknowledgments[0].position, acknowledgments[1].position);
    assert!(shown.contains(&format!(
        "acknowledgment ack:1kLGp2NJ task::added@{added}\n\
         acknowledgment ack:1kLGp2NK task::noted@{noted}\n\
         origin_steering key 1kLGp2NJ\n"
    )));
    assert!(shown.contains(&format!(
        "origin_steering_answer acked lane={lane} at=2030-04-12T18:44:09.271Z\n\
         spice session context creator-session --lane {lane} --vendor omp --stream session \
         --source {source} --member 1 --offset 40\n\
         origin_steering key 1kLGp2NK\n",
        lane = uuid(16_202),
    )));
    assert!(shown.contains("origin_steering_answer unavailable live\n"));
    assert!(shown.contains(&format!(
        "  {noted} 2030-04-12T18:44:09.271Z operator ack:1kLGp2NK\n    observed under a second ask"
    )));
    clean(&root, bus);
}

#[test]
fn unresolved_steering_origin_is_explicit() {
    assert_eq!(
        render_task_steering_origin("1kLGp2NZ", None).as_ref(),
        "origin_steering key 1kLGp2NZ\norigin_steering unavailable unresolved-key"
    );
}
