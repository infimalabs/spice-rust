use super::*;

struct AssignedReview {
    bus: Bus,
    root: FixtureRoot,
    epoch: CanonicalUuid,
    seats: Vec<Seat>,
    implementer: Actor,
    reviewer: Actor,
    reviewed: TaskHandle,
}

impl AssignedReview {
    fn new(name: &str) -> Self {
        let (root, bus, epoch, seats) = fixture(name, 2);
        authorize_lane_creation(&bus, epoch, &seats[1]);
        let implementer = Actor::Lane(seats[0].lane);
        let reviewer = Actor::Lane(seats[1].lane);
        let reviewed = add(
            &bus,
            epoch,
            Actor::Operator,
            "REVIEW-target1",
            "Reviewed target",
            None,
        );
        advance_task_to_review(&bus, epoch, &seats[0], &implementer, &reviewed);
        let reviewable = board(&bus, epoch, reviewer.clone());
        assert_committed(issue(
            &bus,
            epoch,
            reviewer.clone(),
            CommandKind::ClaimWin,
            claim_expected(&reviewable, &reviewed, &seats[1]),
            &ClaimWin {
                rule: "next".parse().expect("allocator rule"),
                recovery: None,
            },
        ));
        Self {
            root,
            bus,
            epoch,
            seats,
            implementer,
            reviewer,
            reviewed,
        }
    }

    fn expected(&self) -> Vec<Witness> {
        let assigned = board(&self.bus, self.epoch, self.reviewer.clone());
        progress_witnesses(
            &assigned,
            at(&assigned, &self.reviewed),
            self.seats[1].lane,
            self.seats[1].occupant,
            true,
        )
        .expect("assigned review has exact witnesses")
    }

    fn finish(self) {
        clean(&self.root, self.bus);
    }
}

fn existing_correction(review: &AssignedReview, handle: &str) -> TaskHandle {
    add(
        &review.bus,
        review.epoch,
        Actor::Operator,
        handle,
        "Existing correction",
        Some("todo"),
    )
}

fn existing_expected(
    review: &AssignedReview,
    correction: &TaskHandle,
    mut expected: Vec<Witness>,
) -> Vec<Witness> {
    let snapshot = board(&review.bus, review.epoch, review.reviewer.clone());
    let correction_heads = at(&snapshot, correction);
    expected.push(statement_witness(correction_heads));
    expected.push(workflow_witness(correction_heads));
    expected.push(Witness::Edge {
        task: at(&snapshot, &review.reviewed).statement.body.born.clone(),
        after: correction_heads.statement.body.born.clone(),
    });
    append_dependency_reachability_witnesses(
        &snapshot,
        std::slice::from_ref(&correction_heads.statement.body.born),
        &mut expected,
    )
    .expect("correction reachability is visible");
    expected
}

#[test]
fn correction_that_reaches_the_reviewed_task_refuses_atomically() {
    let review = AssignedReview::new("review-correction-cycle");
    let correction = existing_correction(&review, "REVIEW-cycle1");
    connect_child(&review.bus, review.epoch, &correction, &review.reviewed);
    let expected = existing_expected(&review, &correction, review.expected());
    let before = review.bus.cursor().expect("cursor before refusal");
    let outcome = issue(
        &review.bus,
        review.epoch,
        review.reviewer.clone(),
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: review.reviewed.clone(),
            finding: "cycle must refuse".to_owned(),
            note: None,
            then: Vec::new(),
            followup: vec![correction],
        },
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal) if refusal.reason() == "task::dependency-refused"),
        "correction cycle must refuse: {outcome:?}",
    );
    assert_eq!(
        review.bus.cursor().expect("cursor after refusal"),
        before,
        "cycle refusal must publish no partial finding or correction edge",
    );
    review.finish();
}

#[test]
fn correction_movement_after_validation_refuses_the_whole_review() {
    let review = AssignedReview::new("review-correction-race");
    let correction = existing_correction(&review, "REVIEW-race01");
    let moved_after = existing_correction(&review, "REVIEW-race02");
    let expected = existing_expected(&review, &correction, review.expected());
    let intent = TaskReview {
        task: review.reviewed.clone(),
        finding: "correction moved".to_owned(),
        note: None,
        then: Vec::new(),
        followup: vec![correction.clone()],
    };
    let command = fixture_command(
        review.epoch,
        review.reviewer.clone(),
        CommandKind::TaskReview,
        expected,
        &intent,
    );
    let reviewed_before = at(
        &board(&review.bus, review.epoch, Actor::Operator),
        &review.reviewed,
    )
    .clone();
    let outcome = task_review_with(
        &review.bus,
        &command,
        &intent,
        None,
        &guard_projects(),
        || connect_child(&review.bus, review.epoch, &correction, &moved_after),
    );
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal) if refusal.reason() == "board::witness-moved"),
        "moved correction must lose the guarded commit: {outcome:?}",
    );
    let current = board(&review.bus, review.epoch, Actor::Operator);
    let reviewed_after = at(&current, &review.reviewed);
    assert_eq!(reviewed_after.statement, reviewed_before.statement);
    assert_eq!(reviewed_after.workflow, reviewed_before.workflow);
    assert_eq!(reviewed_after.claim, reviewed_before.claim);
    assert!(
        !review
            .bus
            .fact_snapshot()
            .expect("race facts")
            .facts()
            .iter()
            .any(|fact| {
                fact.fact.kind() == wire::FactKind::TaskReviewed
                    && fact
                        .fact
                        .body()
                        .get("task")
                        .and_then(serde_json::Value::as_str)
                        == review.reviewed.born().ok().as_ref().map(TaskStamp::as_str)
            })
    );
    review.finish();
}

#[test]
fn finding_command_replays_once_and_allocator_selects_its_correction() {
    let review = AssignedReview::new("review-finding-replay");
    let correction_stamp: TaskStamp = "1kKRRsDT".parse().expect("correction stamp");
    let mut expected = review.expected();
    expected.push(Witness::Absence {
        task: correction_stamp.clone(),
    });
    let intent = TaskReview {
        task: review.reviewed.clone(),
        finding: "one replayable correction".to_owned(),
        note: Some("keep the review pending".to_owned()),
        then: vec![ReviewFollowup {
            task: correction_stamp.clone(),
            add: TaskAdd {
                preparation: None,
                intent: None,
                title: "Replay-safe correction".to_owned(),
                pitch: "This work keeps the fixture honest.".to_owned(),
                project: Some("board.test".to_owned()),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::default(),
                origin: None,
                flow: Some("todo".to_owned()),
                depends: Vec::new(),
                claim: false,
                draft: false,
            },
        }],
        followup: Vec::new(),
    };
    let command = fixture_command(
        review.epoch,
        review.reviewer.clone(),
        CommandKind::TaskReview,
        expected,
        &intent,
    );
    let first = dispatch(&review.bus, &command).expect("review command dispatches");
    let replay_command = command.clone();
    let replay = review
        .bus
        .read(move |read| read.command_replay(&replay_command))
        .expect("review receipt reads");
    assert_eq!(replay, Some(journal::CommandReplay::Exact(first)));
    let facts = review.bus.fact_snapshot().expect("review replay facts");
    assert_eq!(
        facts
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::TaskReviewed)
            .count(),
        1,
    );
    let correction = TaskHandle::for_project("board.test", &correction_stamp);
    let available = board(&review.bus, review.epoch, review.implementer.clone());
    let entries = actor_entries(&available).expect("pending review projects to entries");
    let reviewed = entries
        .iter()
        .find(|entry| entry.handle() == &review.reviewed)
        .expect("reviewed task remains visible");
    assert!(!board::eligible_entry(
        &board::resolved_tasks(&entries),
        reviewed,
        &review.implementer,
        &available.environment(),
    ));
    let subscription = allocation_subscription(&available.residency.body, &review.implementer)
        .expect("implementer has an allocation subscription");
    let selected = board::select(
        &entries,
        &review.implementer,
        &subscription,
        &available.environment(),
    )
    .expect("the correction is allocatable");
    assert_eq!(selected.entry().handle(), &correction);
    review.finish();
}

fn correction_add(title: &str, flow: Option<&str>) -> TaskAdd {
    TaskAdd {
        preparation: None,
        intent: None,
        title: title.to_owned(),
        pitch: "This work keeps the fixture honest.".to_owned(),
        project: Some("board.test".to_owned()),
        description: None,
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        origin: None,
        flow: flow.map(str::to_owned),
        depends: Vec::new(),
        claim: false,
        draft: false,
    }
}

#[test]
fn a_defaulted_correction_omits_review_while_an_explicit_flow_keeps_it() {
    let review = AssignedReview::new("review-correction-flow");
    let defaulted: TaskStamp = "1kKRRsDW".parse().expect("defaulted stamp");
    let explicit: TaskStamp = "1kKRRsDX".parse().expect("explicit stamp");
    let mut expected = review.expected();
    for task in [&defaulted, &explicit] {
        expected.push(Witness::Absence { task: task.clone() });
    }
    assert_committed(issue(
        &review.bus,
        review.epoch,
        review.reviewer.clone(),
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: review.reviewed.clone(),
            finding: "two corrections".to_owned(),
            note: None,
            then: vec![
                ReviewFollowup {
                    task: defaulted.clone(),
                    add: correction_add("Defaulted correction", None),
                },
                ReviewFollowup {
                    task: explicit.clone(),
                    add: correction_add("Material correction", Some("todo,review")),
                },
            ],
            followup: Vec::new(),
        },
    ));
    let current = board(&review.bus, review.epoch, Actor::Operator);
    let flow = |stamp: &TaskStamp| {
        at(&current, &TaskHandle::for_project("board.test", stamp))
            .workflow
            .body
            .flow
            .iter()
            .map(|phase| phase.as_str().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(flow(&defaulted), ["plan", "todo"]);
    assert_eq!(flow(&explicit), ["todo", "review"]);
    review.finish();
}

#[test]
fn repeated_finding_reuses_one_completed_correction_without_a_duplicate_edge() {
    let review = AssignedReview::new("review-repeated-finding");
    let correction_stamp: TaskStamp = "1kKRRsDV".parse().expect("correction stamp");
    let mut expected = review.expected();
    expected.push(Witness::Absence {
        task: correction_stamp.clone(),
    });
    assert_committed(issue(
        &review.bus,
        review.epoch,
        review.reviewer.clone(),
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: review.reviewed.clone(),
            finding: "first finding".to_owned(),
            note: None,
            then: vec![ReviewFollowup {
                task: correction_stamp.clone(),
                add: TaskAdd {
                    preparation: None,
                    intent: None,
                    title: "One durable correction".to_owned(),
                    pitch: "This work keeps the fixture honest.".to_owned(),
                    project: Some("board.test".to_owned()),
                    description: None,
                    acceptance: Vec::new(),
                    requirements: wire::EnvironmentRequirements::default(),
                    origin: None,
                    flow: Some("todo".to_owned()),
                    depends: Vec::new(),
                    claim: false,
                    draft: false,
                },
            }],
            followup: Vec::new(),
        },
    ));
    let correction = TaskHandle::for_project("board.test", &correction_stamp);
    claim_and_complete_task(
        &review.bus,
        review.epoch,
        &review.seats[0],
        &review.implementer,
        &correction,
    );

    let reviewable = board(&review.bus, review.epoch, review.reviewer.clone());
    assert_committed(issue(
        &review.bus,
        review.epoch,
        review.reviewer.clone(),
        CommandKind::ClaimWin,
        claim_expected(&reviewable, &review.reviewed, &review.seats[1]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let assigned = board(&review.bus, review.epoch, review.reviewer.clone());
    let mut expected = progress_witnesses(
        &assigned,
        at(&assigned, &review.reviewed),
        review.seats[1].lane,
        review.seats[1].occupant,
        true,
    )
    .expect("repeated review has exact witnesses");
    let correction_heads = at(&assigned, &correction);
    expected.push(statement_witness(correction_heads));
    expected.push(workflow_witness(correction_heads));
    assert_committed(issue(
        &review.bus,
        review.epoch,
        review.reviewer.clone(),
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: review.reviewed.clone(),
            finding: "the first correction was insufficient".to_owned(),
            note: None,
            then: Vec::new(),
            followup: vec![correction.clone(), correction],
        },
    ));

    let pending = board(&review.bus, review.epoch, review.reviewer.clone());
    let reviewed = at(&pending, &review.reviewed);
    assert!(matches!(
        reviewed.workflow.body.status,
        wire::WorkflowStatus::Pending(wire::WorkflowReview::Unassigned)
    ));
    assert_eq!(reviewed.statement.body.dependencies, vec![correction_stamp]);
    assert_eq!(
        review
            .bus
            .fact_snapshot()
            .expect("repeated review facts")
            .facts()
            .iter()
            .filter(|fact| fact.fact.kind() == wire::FactKind::TaskReviewed)
            .count(),
        2,
    );
    let entries = actor_entries(&pending).expect("repeated review projects to entries");
    let entry = entries
        .iter()
        .find(|entry| entry.handle() == &review.reviewed)
        .expect("reviewed task remains visible");
    assert!(
        board::eligible_entry(
            &board::resolved_tasks(&entries),
            entry,
            &review.reviewer,
            &pending.environment()
        ),
        "a finding whose already-related correction is complete is reviewable again",
    );
    review.finish();
}
