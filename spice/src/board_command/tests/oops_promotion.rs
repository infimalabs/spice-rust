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
    let mut expected = vec![statement_witness(heads), workflow_witness(heads)];
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
    assert!(refusal.explanation().contains("wake HANDLE --into"));
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
        &ClaimWin { rule },
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
            .expect("parked fixture carries inception"),
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
            .expect("parked fixture carries inception"),
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
                title: "Creation cannot bypass parked dependency policy".to_owned(),
                project: Some("board.test".to_owned()),
                description: None,
                acceptance: Vec::new(),
                origin: Some("fixture:oops-promotion".parse().expect("fixture origin")),
                flow: Some("todo".to_owned()),
                depends: vec![forged_oops],
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

#[test]
fn wake_into_is_the_only_unclaimed_oops_project_exit() {
    let (root, bus, epoch, _seats) = fixture("oops-unclaimed-promotion", 1);
    let parked = add_parked(&bus, epoch, "OOPS-promote01");
    let visible = board(&bus, epoch, Actor::Operator);
    let heads = at(&visible, &parked);
    let facts = fact_count(&bus);
    let plain = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskWake,
        vec![workflow_witness(heads), deferral_witness(heads)],
        &TaskWake {
            task: parked.clone(),
            into: None,
        },
    );
    assert!(matches!(
        plain,
        Outcome::Refusal(ref refusal) if refusal.reason() == "task::promotion-required"
    ));
    assert_eq!(fact_count(&bus), facts, "plain wake mutated parked work");

    let bypass = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(heads)],
        &TaskEdit {
            task: parked.clone(),
            title: None,
            description: None,
            acceptance: Vec::new(),
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
        ],
        &TaskWake {
            task: parked.clone(),
            into: Some("board.test".to_owned()),
        },
    )) else {
        panic!("promotion returned another answer shape");
    };
    let promoted = TaskHandle::for_project(
        "board.test",
        &parked.born().expect("fixture handle carries inception"),
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
    let facts = fact_count(&bus);
    let refused = issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        progress_witnesses(&held, held_heads, seats[0].lane, seats[0].occupant, false)
            .expect("claimed oops has progress witnesses"),
        &TaskDone {
            task: parked.clone(),
            validation: "triage is understood".to_owned(),
        },
    );
    assert!(matches!(
        refused,
        Outcome::Refusal(ref refusal) if refusal.reason() == "task::promotion-required"
    ));
    assert_eq!(
        fact_count(&bus),
        facts,
        "oops completion refusal wrote facts"
    );

    let CommandAnswer::Task(answer) = assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskWake,
        vec![
            workflow_witness(held_heads),
            deferral_witness(held_heads),
            statement_witness(held_heads),
        ],
        &TaskWake {
            task: parked.clone(),
            into: Some("board.test".to_owned()),
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
    assert_eq!(promoted_heads.claim.as_ref(), Some(&claim));
    assert_eq!(promoted_heads.lease.as_ref(), Some(&lease));
    assert_eq!(
        promoted_heads.statement.body.born,
        held_heads.statement.body.born
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
