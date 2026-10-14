use super::*;

fn add_parked(bus: &Bus, epoch: CanonicalUuid, name: &str) -> TaskHandle {
    let born = stamp(name);
    let task = TaskHandle::for_project(".oops", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskOopsAdd,
        vec![Witness::Absence { task: born }],
        &TaskOopsAdd {
            title: format!("Triage {name}"),
            description: format!("Evidence for {name}"),
            origin: Some("fixture:oops-promotion".parse().expect("fixture origin")),
            project: None,
        },
    ));
    task
}

fn add_design(bus: &Bus, epoch: CanonicalUuid, name: &str) -> TaskHandle {
    let born = stamp(name);
    let task = TaskHandle::for_project("board.test", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            preparation: None,
            title: name.to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The bounded grooming batch records its disposition".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some("fixture:oops-grooming".parse().expect("fixture origin")),
            flow: Some("design,review".to_owned()),
            depends: Vec::new(),
            intent: Some(wire::TaskIntent::Design),
            claim: false,
        },
    ));
    task
}

fn fact_count(bus: &Bus) -> usize {
    bus.fact_snapshot()
        .expect("fixture facts remain readable")
        .facts()
        .len()
}

fn dependency_witnesses(
    answer: &BoardAnswer,
    task: &TaskHandle,
    after: &[TaskHandle],
) -> Vec<Witness> {
    let heads = at(answer, task);
    let mut expected = if heads.claim.as_ref().is_some_and(|claim| {
        claim.body.actor == answer.principal && claim.body.disposition == ClaimDisposition::Active
    }) {
        let (lane, occupant) = actor_seat(answer).expect("claimant has one occupied seat");
        progress_witnesses(
            answer,
            heads,
            lane,
            occupant,
            heads.workflow.body.phase.as_str() == "review",
        )
        .expect("claimant dependency witnesses")
    } else {
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ]
    };
    for dependency in after {
        let prerequisite = at(answer, dependency);
        expected.push(statement_witness(prerequisite));
        expected.push(workflow_witness(prerequisite));
        expected.push(Witness::Edge {
            task: heads.statement.body.born.clone(),
            after: prerequisite.statement.body.born.clone(),
        });
    }
    expected
}

fn assert_oops_refusal(outcome: Outcome<CommandAnswer>, offenders: &[(&TaskHandle, &str)]) {
    let Outcome::Refusal(refusal) = outcome else {
        panic!("oops dependency must refuse: {outcome:?}");
    };
    assert_eq!(refusal.reason(), "task::dependency-oops-refused");
    let detail = refusal.explanation();
    for (handle, claim) in offenders {
        assert!(
            detail.contains(&handle.to_string()),
            "refusal omitted {handle}: {detail}",
        );
        assert!(detail.contains(claim), "refusal omitted {claim}: {detail}",);
    }
    assert!(
        detail.contains("project=.oops"),
        "refusal omitted authoritative parked state: {detail}",
    );
    assert!(
        detail.contains("status=pending"),
        "refusal omitted workflow state: {detail}",
    );
}

fn capture_parked(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, task: &TaskHandle) -> Actor {
    let lane = Actor::Lane(seat.lane);
    let available = board(bus, epoch, lane.clone());
    let heads = at(&available, task);
    let rule: Name = "capture".parse().expect("capture rule");
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
            Witness::Eligibility { rule: rule.clone() },
            residency(&available, seat),
        ],
        &ClaimWin {
            rule,
            recovery: None,
        },
    ));
    lane
}

#[test]
fn oops_dependency_endpoints_refuse_atomically_while_public_edges_still_commit() {
    let (root, bus, epoch, seats) = fixture("oops-dependency", 1);
    let public = add(
        &bus,
        epoch,
        Actor::Operator,
        "OOPS-public01",
        "Public dependent",
        Some("todo"),
    );
    let prerequisite = add(
        &bus,
        epoch,
        Actor::Operator,
        "OOPS-public02",
        "Public prerequisite",
        Some("todo"),
    );
    let claimed_oops = add_parked(&bus, epoch, "OOPS-claimed01");
    let unclaimed_oops = add_parked(&bus, epoch, "OOPS-unclaim1");
    let lane = capture_parked(&bus, epoch, &seats[0], &claimed_oops);

    target_oops_dependency_refuses(
        &bus,
        epoch,
        lane,
        &prerequisite,
        &claimed_oops,
        &unclaimed_oops,
    );
    let forged_oops =
        prerequisite_oops_dependency_refuses(&bus, epoch, &public, &prerequisite, &unclaimed_oops);
    task_add_oops_dependency_refuses(&bus, epoch, &unclaimed_oops, forged_oops);
    public_dependency_commits(&bus, epoch, &public, &prerequisite);
    clean(&root, bus);
}

fn target_oops_dependency_refuses(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: Actor,
    prerequisite: &TaskHandle,
    claimed_oops: &TaskHandle,
    unclaimed_oops: &TaskHandle,
) {
    let before = board(bus, epoch, lane.clone());
    let facts = fact_count(bus);
    let forged_claimed = TaskHandle::for_project(
        "board.test",
        &claimed_oops
            .born()
            .expect("parked fixture carries a birth stamp"),
    );
    let known = [prerequisite.clone(), unclaimed_oops.clone()];
    let unknown = TaskHandle::for_project("board.test", &stamp("OOPS-unknown01"));
    let after = [prerequisite.clone(), unknown, unclaimed_oops.clone()];
    assert_oops_refusal(
        issue(
            bus,
            epoch,
            lane,
            CommandKind::TaskDepends,
            dependency_witnesses(&before, claimed_oops, &known),
            &TaskDepends {
                task: forged_claimed,
                after: after.to_vec(),
                remove: false,
            },
        ),
        &[
            (claimed_oops, "claim=active"),
            (unclaimed_oops, "claim=none"),
        ],
    );
    assert_eq!(fact_count(bus), facts, "target refusal wrote facts");
    assert!(
        at(&board(bus, epoch, Actor::Operator), claimed_oops)
            .statement
            .body
            .dependencies
            .is_empty(),
        "target refusal appended a partial public edge",
    );
}

fn prerequisite_oops_dependency_refuses(
    bus: &Bus,
    epoch: CanonicalUuid,
    public: &TaskHandle,
    prerequisite: &TaskHandle,
    unclaimed_oops: &TaskHandle,
) -> TaskHandle {
    let before = board(bus, epoch, Actor::Operator);
    let facts = fact_count(bus);
    let forged_oops = TaskHandle::for_project(
        "board.test",
        &unclaimed_oops
            .born()
            .expect("parked fixture carries a birth stamp"),
    );
    let known = [prerequisite.clone()];
    let after = [prerequisite.clone(), forged_oops.clone()];
    assert_oops_refusal(
        issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskDepends,
            dependency_witnesses(&before, public, &known),
            &TaskDepends {
                task: public.clone(),
                after: after.to_vec(),
                remove: false,
            },
        ),
        &[(unclaimed_oops, "claim=none")],
    );
    assert_eq!(fact_count(bus), facts, "prerequisite refusal wrote facts");
    assert!(
        at(&board(bus, epoch, Actor::Operator), public)
            .statement
            .body
            .dependencies
            .is_empty(),
        "mixed prerequisite request appended its public edge",
    );
    forged_oops
}

fn task_add_oops_dependency_refuses(
    bus: &Bus,
    epoch: CanonicalUuid,
    unclaimed_oops: &TaskHandle,
    forged_oops: TaskHandle,
) {
    let born = stamp("OOPS-add-edge1");
    let added = TaskHandle::for_project("board.test", &born);
    let facts = fact_count(bus);
    assert_oops_refusal(
        issue(
            bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskAdd,
            vec![Witness::Absence { task: born }],
            &TaskAdd {
                preparation: None,
                intent: None,
                title: "Creation cannot bypass parked dependency policy".to_owned(),
                project: Some("board.test".to_owned()),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                origin: Some("fixture:oops-promotion".parse().expect("fixture origin")),
                flow: Some("todo".to_owned()),
                depends: vec![forged_oops],
                claim: false,
            },
        ),
        &[(unclaimed_oops, "claim=none")],
    );
    assert_eq!(fact_count(bus), facts, "add refusal wrote facts");
    assert!(
        board(bus, epoch, Actor::Operator)
            .tasks
            .iter()
            .all(|task| task.statement.body.handle != added),
        "refused creation reached the board",
    );
}

fn public_dependency_commits(
    bus: &Bus,
    epoch: CanonicalUuid,
    public: &TaskHandle,
    prerequisite: &TaskHandle,
) {
    let before = board(bus, epoch, Actor::Operator);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        dependency_witnesses(&before, public, std::slice::from_ref(prerequisite)),
        &TaskDepends {
            task: public.clone(),
            after: vec![prerequisite.clone()],
            remove: false,
        },
    ));
    assert_eq!(
        at(&board(bus, epoch, Actor::Operator), public)
            .statement
            .body
            .dependencies,
        [at(&board(bus, epoch, Actor::Operator), prerequisite)
            .statement
            .body
            .born
            .clone()],
    );
}

fn assert_unwitnessed_promotion_refuses(
    bus: &Bus,
    epoch: CanonicalUuid,
    parked: &TaskHandle,
    heads: &BoardTask,
    facts: usize,
) {
    let rejected = wire::Command {
        v: wire::WireVersion::CURRENT,
        id: uuid(9_001),
        epoch,
        kind: CommandKind::TaskWake,
        expected: vec![workflow_witness(heads), deferral_witness(heads)],
        body: serde_json::to_value(TaskWake {
            task: parked.clone(),
            into: Some("board.test".to_owned()),
            preparation: None,
            claim: false,
        })
        .expect("promotion body encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
        release: "1.2.3".parse().expect("fixture release"),
    });
    assert!(matches!(
        rejected,
        Err(wire::CommandRejection::Refusal(ref refusal))
            if refusal.reason() == "command::witness-law"
    ));
    assert_eq!(
        fact_count(bus),
        facts,
        "unwitnessed promotion reached a fact write",
    );
}

fn assert_unrelated_promotion_witness_refuses(
    bus: &Bus,
    epoch: CanonicalUuid,
    parked: &TaskHandle,
    heads: &BoardTask,
    unrelated: &BoardTask,
    facts: usize,
) {
    let unrelated_statement = statement_witness(unrelated);
    let rejected = wire::Command {
        v: wire::WireVersion::CURRENT,
        id: uuid(9_002),
        epoch,
        kind: CommandKind::TaskWake,
        expected: vec![
            workflow_witness(heads),
            deferral_witness(heads),
            unrelated_statement.clone(),
            unrelated_statement,
        ],
        body: serde_json::to_value(TaskWake {
            task: parked.clone(),
            into: Some("board.test".to_owned()),
            preparation: None,
            claim: false,
        })
        .expect("promotion body encodes"),
    }
    .admit(&wire::Accepted {
        wire: wire::WireVersion::CURRENT,
        actor: Actor::Operator,
        epoch,
        release: "1.2.3".parse().expect("fixture release"),
    });
    let Err(wire::CommandRejection::Refusal(refusal)) = rejected else {
        panic!("promotion carrying only unrelated statement authority must refuse");
    };
    assert_eq!(refusal.reason(), "command::witness-law");
    assert!(
        refusal
            .diagnostic()
            .and_then(wire::RefusalDiagnostic::witness)
            .is_some_and(|witness| witness.contains(&parked.to_string()))
    );
    assert_eq!(
        fact_count(bus),
        facts,
        "unrelated statement authority reached a fact write",
    );
}

fn stale_promotion_after_edit(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    parked: &TaskHandle,
    held_heads: &BoardTask,
) -> BoardAnswer {
    let stale_statement = statement_witness(held_heads);
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::TaskEdit,
        vec![stale_statement.clone()],
        &TaskEdit {
            intent: None,
            evidence: None,
            task: parked.clone(),
            title: None,
            description: Some("Edited during promotion capture".to_owned()),
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: None,
        },
    ));
    let edited = board(bus, epoch, lane.clone());
    let edited_heads = at(&edited, parked);
    let facts = fact_count(bus);
    let stale = issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::TaskWake,
        vec![
            workflow_witness(edited_heads),
            deferral_witness(edited_heads),
            stale_statement,
        ],
        &TaskWake {
            task: parked.clone(),
            into: Some("board.test".to_owned()),
            preparation: None,
            claim: false,
        },
    );
    assert!(matches!(
        stale,
        Outcome::Refusal(ref refusal)
            if refusal.reason() == "task::promotion-statement-witness-moved"
    ));
    assert_eq!(fact_count(bus), facts, "stale promotion overwrote an edit");
    edited
}

fn assert_claimed_oops_cannot_finish(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    parked: &TaskHandle,
    held: &BoardAnswer,
    seat: &Seat,
) {
    let facts = fact_count(bus);
    let refused = issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        progress_witnesses(held, at(held, parked), seat.lane, seat.occupant, false)
            .expect("claimed oops has progress witnesses"),
        &TaskDone {
            task: parked.clone(),
            validation: "triage is understood".to_owned(),
            operation: Some(completion_operation()),
        },
    );
    assert!(matches!(
        refused,
        Outcome::Refusal(ref refusal) if refusal.reason() == "task::promotion-required"
    ));
    assert_eq!(
        fact_count(bus),
        facts,
        "oops completion refusal wrote facts"
    );
}

#[test]
fn wake_is_the_only_unclaimed_oops_project_exit() {
    let (root, bus, epoch, _seats) = fixture("oops-unclaimed-promotion", 1);
    let parked = add_parked(&bus, epoch, "OOPS-promote01");
    let unrelated = add(
        &bus,
        epoch,
        Actor::Operator,
        "OOPS-unrelated",
        "Unrelated public task",
        Some("todo"),
    );
    let visible = board(&bus, epoch, Actor::Operator);
    let heads = at(&visible, &parked);
    let unrelated_heads = at(&visible, &unrelated);
    let facts = fact_count(&bus);
    assert_unwitnessed_promotion_refuses(&bus, epoch, &parked, heads, facts);
    assert_unrelated_promotion_witness_refuses(&bus, epoch, &parked, heads, unrelated_heads, facts);
    let bypass = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(heads)],
        &TaskEdit {
            intent: None,
            evidence: None,
            task: parked.clone(),
            title: None,
            description: None,
            acceptance: Vec::new(),
            requirements: None,
            priority: None,
            project: Some("board.test".to_owned()),
        },
    );
    assert!(matches!(
        bypass,
        Outcome::Refusal(ref refusal) if refusal.reason() == "task::promotion-required"
    ));
    assert_eq!(fact_count(&bus), facts, "project edit bypass wrote facts");

    let CommandAnswer::Task(answer) = assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskWake,
        vec![
            workflow_witness(heads),
            deferral_witness(heads),
            statement_witness(heads),
            statement_witness(heads),
            statement_witness(unrelated_heads),
        ],
        &TaskWake {
            task: parked.clone(),
            into: Some("board.test".to_owned()),
            preparation: None,
            claim: false,
        },
    )) else {
        panic!("promotion returned another answer shape");
    };
    let promoted = TaskHandle::for_project(
        "board.test",
        &parked.born().expect("fixture handle carries a birth stamp"),
    );
    assert_eq!(answer.task, promoted);
    assert_eq!(answer.status, "promoted");
    assert_eq!(fact_count(&bus), facts + 2, "promotion fact set drifted");
    let after = board(&bus, epoch, Actor::Operator);
    let heads = at(&after, &promoted);
    assert_eq!(heads.statement.body.project, "board.test");
    assert!(!heads.visibility.is_hidden());
    assert!(heads.claim.is_none());
    assert!(heads.lease.is_none());
    assert!(heads.workflow.body.deferral.is_none());
    assert!(
        after
            .tasks
            .iter()
            .all(|task| task.statement.body.handle != parked)
    );
    let facts = fact_count(&bus);
    let public_into = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskWake,
        vec![
            workflow_witness(heads),
            deferral_witness(heads),
            statement_witness(heads),
        ],
        &TaskWake {
            task: promoted,
            into: Some("board.identity".to_owned()),
            preparation: None,
            claim: false,
        },
    );
    assert!(matches!(
        public_into,
        Outcome::Refusal(ref refusal) if refusal.reason() == "task::promotion-refused"
    ));
    assert_eq!(fact_count(&bus), facts, "public --into refusal wrote facts");
    clean(&root, bus);
}

#[test]
fn claimed_oops_must_promote_before_done_and_keeps_its_exact_claim_and_lease() {
    let (root, bus, epoch, seats) = fixture("oops-claimed-promotion", 1);
    let parked = add_parked(&bus, epoch, "OOPS-held0001");
    let child = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "OOPS-child001",
        "Acceptance-bearing implementation",
        "board.test",
        Some("todo"),
        &["the public implementation is verifiable"],
    );
    let lane = capture_parked(&bus, epoch, &seats[0], &parked);
    let held = board(&bus, epoch, lane.clone());
    let held_heads = at(&held, &parked);
    let claim = held_heads.claim.clone().expect("triage capture claim");
    let lease = held_heads.lease.clone().expect("triage capture lease");
    assert_claimed_oops_cannot_finish(&bus, epoch, &lane, &parked, &held, &seats[0]);

    let edited = stale_promotion_after_edit(&bus, epoch, &lane, &parked, held_heads);
    let edited_heads = at(&edited, &parked);
    let child_heads = at(&edited, &child);
    let facts = fact_count(&bus);

    let CommandAnswer::Task(answer) = assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskWake,
        vec![
            workflow_witness(edited_heads),
            deferral_witness(edited_heads),
            statement_witness(edited_heads),
            statement_witness(edited_heads),
            statement_witness(child_heads),
        ],
        &TaskWake {
            task: parked.clone(),
            into: Some("board.test".to_owned()),
            preparation: None,
            claim: false,
        },
    )) else {
        panic!("promotion returned another answer shape");
    };
    let promoted = answer.task;
    assert_eq!(
        fact_count(&bus),
        facts + 2,
        "claimed promotion fact set drifted"
    );
    let public = board(&bus, epoch, lane.clone());
    let promoted_heads = at(&public, &promoted);
    assert_holder_kept(promoted_heads, &claim.body);
    assert_eq!(promoted_heads.lease.as_ref(), Some(&lease));
    assert_eq!(
        promoted_heads.statement.body.born,
        edited_heads.statement.body.born
    );
    assert_eq!(
        promoted_heads.statement.body.description.as_deref(),
        Some("Edited during promotion capture"),
    );

    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDepends,
        dependency_witnesses(&public, &promoted, std::slice::from_ref(&child)),
        &TaskDepends {
            task: promoted.clone(),
            after: vec![child],
            remove: false,
        },
    ));
    let connected = board(&bus, epoch, lane.clone());
    let connected_heads = at(&connected, &promoted);
    assert_committed(issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        progress_witnesses(
            &connected,
            connected_heads,
            seats[0].lane,
            seats[0].occupant,
            false,
        )
        .expect("promoted plan has guarded progress witnesses"),
        &TaskDone {
            task: promoted.clone(),
            validation: "public dependency captures the implementation contract".to_owned(),
            operation: Some(completion_operation()),
        },
    ));
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &promoted)
            .workflow
            .body
            .phase
            .as_str(),
        "todo",
    );
    clean(&root, bus);
}

#[test]
fn parked_report_promotion_atomically_becomes_controlled_and_unauthorized() {
    let (root, bus, epoch, _seats) = fixture("oops-controlled-promotion", 0);
    let controller = add_design(&bus, epoch, "OOPS-batch001");
    let parked = add_parked(&bus, epoch, "OOPS-report01");
    let current = board(&bus, epoch, Actor::Operator);
    let report = at(&current, &parked);
    let design = at(&current, &controller);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskWake,
        vec![
            statement_witness(report),
            workflow_witness(report),
            deferral_witness(report),
            statement_witness(design),
            workflow_witness(design),
            claim_witness(design),
        ],
        &TaskWake {
            task: parked.clone(),
            into: Some("board.test".to_owned()),
            preparation: Some(controller.clone()),
            claim: false,
        },
    ));

    let promoted = TaskHandle::for_project(
        "board.test",
        &parked.born().expect("fixture report has a birth stamp"),
    );
    let visible = board(&bus, epoch, Actor::Operator);
    let output = at(&visible, &promoted);
    let preparation = output
        .statement
        .body
        .preparation
        .as_ref()
        .expect("promotion records controlling preparation");
    assert_eq!(
        preparation.parent,
        controller
            .born()
            .expect("fixture controller has a birth stamp")
    );
    assert_eq!(preparation.controller, preparation.parent);
    assert_eq!(preparation.construction, output.statement.body.born);
    assert_eq!(output.statement.body.authorization, None);
    assert_eq!(output.claim, None);
    assert_eq!(
        open_blocked(&bus, epoch),
        [output.statement.body.born.clone()].into()
    );

    let facts = bus.fact_snapshot().expect("fixture facts read");
    let control = facts
        .facts()
        .iter()
        .find_map(|envelope| {
            envelope
                .fact
                .read::<wire::rooms::OutputControlled>()
                .filter(|body| body.output == output.statement.body.born)
                .map(|body| (envelope.seq, body))
        })
        .expect("promotion writes its control fact");
    assert_eq!(control.0, output.statement.position);
    assert_eq!(control.1.preparation, *preparation);
    clean(&root, bus);
}

#[test]
fn grooming_and_controlled_wake_preflight_reads_every_named_parked_report() {
    let (root, bus, epoch, _seats) = fixture("oops-named-preflight", 0);
    let batch = add_design(&bus, epoch, "OOPS-batch002");
    let first = add_parked(&bus, epoch, "OOPS-report02");
    let second = add_parked(&bus, epoch, "OOPS-report03");
    let _unnamed = add_parked(&bus, epoch, "OOPS-report04");
    let (batch_name, first_name, second_name) =
        (batch.to_string(), first.to_string(), second.to_string());
    let groom = [
        "spice",
        "room",
        "post",
        &batch_name,
        "--report",
        &first_name,
        "--report",
        &second_name,
        "--disposition",
        "duplicate",
        "--cause",
        "same wrapper refusal",
    ];
    let wake = [
        "spice",
        "task",
        "wake",
        &first_name,
        "--preparation",
        &batch_name,
    ];
    for (argv, named) in [
        (&groom[..], vec![&batch, &first, &second]),
        (&wake[..], vec![&first, &batch]),
    ] {
        let matches = crate::cli::command()
            .try_get_matches_from(argv)
            .expect("fixture command parses");
        let contract = crate::cli::selected_contract(&matches).expect("selected contract");
        let (_, read) = mutation_read(contract, crate::cli::selected_matches(&matches))
            .expect("mutation selects its read");
        let Outcome::Reply(Reply {
            body: CommandAnswer::Board(scoped),
            ..
        }) = issue(
            &bus,
            epoch,
            Actor::Operator,
            CommandKind::BoardRead,
            Vec::new(),
            &read,
        )
        else {
            panic!("scoped preflight read answers a board");
        };
        let read = scoped
            .tasks
            .iter()
            .map(|task| task.statement.body.handle.clone())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(read, named.into_iter().cloned().collect(), "{argv:?}");
    }
    clean(&root, bus);
}
