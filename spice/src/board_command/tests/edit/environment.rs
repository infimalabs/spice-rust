use super::*;

#[test]
fn environmental_requirements_add_set_clear_and_report_under_claim_witness_authority() {
    let (_root, bus, epoch, seats) = fixture("task-environment-requirements", 1);
    let actor = Actor::Lane(seats[0].lane);
    let (task, blocked) = add_blocked_requirement(&bus, epoch, &actor);
    let satisfied = clear_and_set_schema_requirement(&bus, epoch, &actor, &task, &blocked);
    assert_trust_transition(&bus, epoch, &task, &satisfied);
}

#[test]
fn active_holder_requirement_ices_and_correction_leaves_the_same_claim_thaw_ready() {
    let (_root, bus, epoch, seats) = fixture("task-environment-ice", 1);
    let actor = Actor::Lane(seats[0].lane);
    let (task, claim, digest) = ice_active_holder(&bus, epoch, &seats[0], &actor);
    clear_blocker_leaves_sentinel(&bus, epoch, &actor, &task, &claim);
    satisfied_requirement_preserves_lease(&bus, epoch, actor, &task, digest);
}

#[test]
fn appliance_requirement_holds_a_task_until_a_later_appliance_runs() {
    let (_root, bus, epoch, seats) = fixture("task-environment-appliance", 1);
    let actor = Actor::Lane(seats[0].lane);
    let running = spice::version::running()
        .expect("test executable identity")
        .product
        .clone();
    let born = stamp("ENVTEST-1kKfYW07");
    let task = TaskHandle::for_project("board.test", &born);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            preparation: None,
            intent: None,
            title: "Wait for the next appliance".to_owned(),
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::one(
                wire::EnvironmentRequirement::appliance_after(running.clone()),
            ),
            origin: Some(fixture_origin()),
            flow: Some("todo".to_owned()),
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    ));
    let blocked = board(&bus, epoch, actor.clone());
    assert_eq!(blocked.appliance, running);
    let shown = task_show(&blocked, at(&blocked, &task), &[]);
    assert!(shown.contains(&format!(
        "appliance-version after={running} observed={running} satisfied=false"
    )));
    assert!(
        allocated(&bus, epoch, &actor).is_none(),
        "the running appliance is not later than itself"
    );

    let earlier = wire::manifest::ProductVersion::parse("0.0.0").expect("earlier version");
    let heads = at(&blocked, &task);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(heads), claim_witness(heads)],
        &TaskEdit {
            requirements: Some(wire::intent::TaskRequirementsEdit::one(
                wire::EnvironmentRequirement::appliance_after(earlier),
            )),
            ..only(&task)
        },
    ));
    assert_eq!(
        allocated(&bus, epoch, &actor)
            .expect("a later running appliance releases the task")
            .0,
        task.to_string()
    );
}

fn ice_active_holder(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    actor: &Actor,
) -> (TaskHandle, ClaimHead, wire::Digest) {
    let task = add(
        bus,
        epoch,
        Actor::Operator,
        "ICE-1kKfYW07",
        "Preserve blocked possession",
        Some("todo"),
    );
    capture_for(bus, epoch, seat, &task);
    let before = board(bus, epoch, actor.clone());
    let before_heads = at(&before, &task);
    let claim = before_heads
        .claim
        .as_ref()
        .expect("active claim")
        .body
        .clone();
    let lease = before_heads.lease.as_ref().expect("active lease").clone();
    let digest: wire::Digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        .parse()
        .expect("fixture digest");
    let requirements = wire::EnvironmentRequirements::new(vec![
        wire::EnvironmentRequirement::journal_schema(journal::SCHEMA_VERSION)
            .expect("current schema"),
        wire::EnvironmentRequirement::trust_grant(digest.clone()),
    ])
    .expect("canonical conjunction");
    assert_missing_lease_refuses(
        bus,
        epoch,
        actor,
        &task,
        before_heads,
        &lease,
        &requirements,
    );
    assert_committed(issue(
        bus,
        epoch,
        actor.clone(),
        CommandKind::TaskEdit,
        vec![
            statement_witness(before_heads),
            claim_witness(before_heads),
            Witness::Lease {
                task: before_heads.statement.body.born.clone(),
                head: lease.position,
            },
        ],
        &TaskEdit {
            requirements: Some(wire::intent::TaskRequirementsEdit::Set { requirements }),
            ..only(&task)
        },
    ));
    let iced = board(bus, epoch, actor.clone());
    let iced_heads = at(&iced, &task);
    let iced_lease = iced_heads.lease.as_ref().expect("iced lease");
    assert_holder_kept(iced_heads, &claim);
    assert_eq!(iced_lease.body.start, lease.body.start);
    assert_eq!(iced_lease.body.renewal, lease.body.renewal);
    assert_eq!(iced_lease.body.epoch, lease.body.epoch);
    assert!(spice::claim_lease::is_indefinite(&iced_lease.body));
    assert_eq!(iced_lease.position, iced_heads.statement.position);
    (task, claim, digest)
}

fn assert_missing_lease_refuses(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: &Actor,
    task: &TaskHandle,
    before_heads: &BoardTask,
    lease: &Positioned<LeaseHead>,
    requirements: &wire::EnvironmentRequirements,
) {
    let missing_lease = issue(
        bus,
        epoch,
        actor.clone(),
        CommandKind::TaskEdit,
        vec![statement_witness(before_heads), claim_witness(before_heads)],
        &TaskEdit {
            requirements: Some(wire::intent::TaskRequirementsEdit::Set {
                requirements: requirements.clone(),
            }),
            ..only(task)
        },
    );
    assert!(matches!(
        missing_lease,
        Outcome::Fault(ref fault) if fault.reason == "task::require-lease-witness-missing"
    ));
    let unchanged = board(bus, epoch, actor.clone());
    assert_eq!(
        at(&unchanged, task).statement.position,
        before_heads.statement.position
    );
    assert_eq!(at(&unchanged, task).lease.as_ref(), Some(lease));
}

fn clear_blocker_leaves_sentinel(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: &Actor,
    task: &TaskHandle,
    claim: &ClaimHead,
) {
    let iced = board(bus, epoch, actor.clone());
    let iced_heads = at(&iced, task);
    let iced_lease = iced_heads.lease.as_ref().expect("iced lease").clone();
    assert_committed(issue(
        bus,
        epoch,
        actor.clone(),
        CommandKind::TaskEdit,
        vec![
            statement_witness(iced_heads),
            claim_witness(iced_heads),
            Witness::Lease {
                task: iced_heads.statement.body.born.clone(),
                head: iced_lease.position,
            },
        ],
        &TaskEdit {
            requirements: Some(wire::intent::TaskRequirementsEdit::Clear {}),
            ..only(task)
        },
    ));
    let thaw_ready = board(bus, epoch, actor.clone());
    let thaw_ready_heads = at(&thaw_ready, task);
    let thaw_ready_lease = thaw_ready_heads.lease.as_ref().expect("retained lease");
    assert!(thaw_ready_heads.statement.body.requirements.is_empty());
    assert_holder_kept(thaw_ready_heads, claim);
    assert_eq!(thaw_ready_lease, &iced_lease);
    assert!(spice::claim_lease::is_indefinite(&thaw_ready_lease.body));
    assert_ne!(
        thaw_ready_lease.position, thaw_ready_heads.statement.position,
        "removing the blocker leaves thawing to the seating transaction"
    );
}

fn satisfied_requirement_preserves_lease(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    task: &TaskHandle,
    digest: wire::Digest,
) {
    pend_trust(bus, &digest);
    grant_trust(bus, &digest);
    let satisfied = board(bus, epoch, actor);
    let satisfied_heads = at(&satisfied, task);
    let lease_before_satisfied = satisfied_heads
        .lease
        .as_ref()
        .expect("sentinel lease")
        .clone();
    assert_committed(issue(
        bus,
        epoch,
        satisfied.principal.clone(),
        CommandKind::TaskEdit,
        vec![
            statement_witness(satisfied_heads),
            claim_witness(satisfied_heads),
            Witness::Lease {
                task: satisfied_heads.statement.body.born.clone(),
                head: lease_before_satisfied.position,
            },
        ],
        &TaskEdit {
            requirements: Some(wire::intent::TaskRequirementsEdit::one(
                wire::EnvironmentRequirement::trust_grant(digest),
            )),
            ..only(task)
        },
    ));
    let after_satisfied = board(bus, epoch, satisfied.principal);
    assert_eq!(
        at(&after_satisfied, task)
            .lease
            .as_ref()
            .expect("sentinel lease remains"),
        &lease_before_satisfied,
        "an already-satisfied requirement changes no lease"
    );
}

fn add_blocked_requirement(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: &Actor,
) -> (TaskHandle, BoardAnswer) {
    let born = stamp("ENVTEST-1kKfYW06");
    let task = TaskHandle::for_project("board.test", &born);
    let required = journal::SCHEMA_VERSION
        .checked_add(1)
        .expect("fixture schema advances");
    let requirements = wire::EnvironmentRequirements::one(
        wire::EnvironmentRequirement::journal_schema(required).expect("positive schema"),
    );
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            preparation: None,
            intent: None,
            title: "Require a future journal".to_owned(),
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: requirements.clone(),
            origin: Some(fixture_origin()),
            flow: Some("todo".to_owned()),
            depends: Vec::new(),
            claim: false,
            draft: false,
        },
    ));
    let blocked = board(bus, epoch, actor.clone());
    let shown = task_show(&blocked, at(&blocked, &task), &[]);
    assert!(shown.contains(&format!(
        "journal-schema required={required} observed={} satisfied=false",
        journal::SCHEMA_VERSION
    )));
    assert!(shown.contains("environment blocked"));
    let status = task_status(&blocked).expect("status renders");
    assert!(status.contains("blocked 0"));
    assert!(status.contains("waiting 0"));
    assert!(status.contains(&format!("journal-schema required={required} observed=")));
    assert!(status.contains("satisfied=0 blocked=1"));
    assert!(allocated(bus, epoch, actor).is_none());
    (task, blocked)
}

fn clear_and_set_schema_requirement(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: &Actor,
    task: &TaskHandle,
    blocked: &BoardAnswer,
) -> BoardAnswer {
    let heads = at(blocked, task);
    let workflow_position = heads.workflow.position;
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(heads), claim_witness(heads)],
        &TaskEdit {
            requirements: Some(wire::intent::TaskRequirementsEdit::Clear {}),
            ..only(task)
        },
    ));
    let cleared = board(bus, epoch, actor.clone());
    assert!(at(&cleared, task).statement.body.requirements.is_empty());
    assert_eq!(at(&cleared, task).workflow.position, workflow_position);
    assert_eq!(
        allocated(bus, epoch, actor)
            .expect("cleared task becomes eligible")
            .0,
        task.to_string()
    );
    let heads = at(&cleared, task);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(heads), claim_witness(heads)],
        &TaskEdit {
            requirements: Some(wire::intent::TaskRequirementsEdit::one(
                wire::EnvironmentRequirement::journal_schema(journal::SCHEMA_VERSION)
                    .expect("current schema is positive"),
            )),
            ..only(task)
        },
    ));
    let satisfied = board(bus, epoch, actor.clone());
    assert!(task_show(&satisfied, at(&satisfied, task), &[]).contains("environment satisfied"));
    let status = task_status(&satisfied).expect("status renders");
    assert!(status.contains("satisfied=1 blocked=0"));
    satisfied
}

fn assert_trust_transition(
    bus: &Bus,
    epoch: CanonicalUuid,
    task: &TaskHandle,
    satisfied: &BoardAnswer,
) {
    let digest: wire::Digest = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        .parse()
        .expect("fixture digest");
    let heads = at(satisfied, task);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskEdit,
        vec![statement_witness(heads), claim_witness(heads)],
        &TaskEdit {
            requirements: Some(wire::intent::TaskRequirementsEdit::one(
                wire::EnvironmentRequirement::trust_grant(digest.clone()),
            )),
            ..only(task)
        },
    ));
    let trust_blocked = board(bus, epoch, Actor::Operator);
    let statement_position = at(&trust_blocked, task).statement.position;
    assert!(at(&trust_blocked, task).lease.is_none());
    assert!(allocated(bus, epoch, &Actor::Operator).is_none());
    let shown = task_show(&trust_blocked, at(&trust_blocked, task), &[]);
    assert!(shown.contains(&format!(
        "trust-grant required={digest} standing=missing producer=- satisfied=false"
    )));
    pend_trust(bus, &digest);
    assert!(allocated(bus, epoch, &Actor::Operator).is_none());
    grant_trust(bus, &digest);
    let trust_satisfied = board(bus, epoch, Actor::Operator);
    assert_eq!(
        at(&trust_satisfied, task).statement.position,
        statement_position,
        "granting the environment does not rewrite task authority"
    );
    assert_eq!(
        allocated(bus, epoch, &Actor::Operator)
            .expect("matching grant makes the task eligible")
            .0,
        task.to_string()
    );
    let shown = task_show(&trust_satisfied, at(&trust_satisfied, task), &[]);
    assert!(shown.contains(&format!(
        "trust-grant required={digest} standing=granted producer=/producer/worktree satisfied=true"
    )));
    let status = task_status(&trust_satisfied).expect("trust status renders");
    assert!(status.contains(&format!("trust-grant required={digest}")));
    assert!(status.contains("satisfied=1 blocked=0"));
}
fn pend_trust(bus: &Bus, digest: &wire::Digest) {
    let mut write = JournalWrite::new(
        Actor::Operator,
        "repo:genesis".parse().expect("fixture origin"),
        Instant::new(jiff::Timestamp::now()).expect("fixture instant"),
        Producer::CommandDoor,
    );
    let pended = write
        .fact(
            wire::body::TrustPended {
                surface: "spice".to_owned(),
                digest: digest.clone(),
                worktree: Some("/producer/worktree".to_owned()),
                previous: None,
            },
            FactCause::Root,
        )
        .expect("trust pend fact builds");
    let digest = digest.clone();
    write.trust_head(pended, move |_| wire::TrustHead {
        digest,
        state: wire::TrustState::Pended,
        worktree: Some("/producer/worktree".to_owned()),
    });
    assert!(matches!(
        bus.commit(write).expect("trust pend reaches writer"),
        CommitOutcome::Committed(_)
    ));
}

fn grant_trust(bus: &Bus, digest: &wire::Digest) {
    let current = bus
        .trust_head(digest)
        .expect("pended trust reads")
        .expect("pended trust exists");
    let mut write = JournalWrite::new(
        Actor::Operator,
        "repo:genesis".parse().expect("fixture origin"),
        Instant::new(jiff::Timestamp::now()).expect("fixture instant"),
        Producer::CommandDoor,
    );
    let granted = write
        .fact(
            wire::body::TrustGranted {
                surface: "spice".to_owned(),
                digest: digest.clone(),
            },
            FactCause::Existing(current.position()),
        )
        .expect("trust grant fact builds");
    let digest = digest.clone();
    let worktree = current.body().worktree.clone();
    write.trust_head(granted, move |_| wire::TrustHead {
        digest,
        state: wire::TrustState::Granted,
        worktree,
    });
    assert!(matches!(
        bus.commit(write).expect("trust grant reaches writer"),
        CommitOutcome::Committed(_)
    ));
}
