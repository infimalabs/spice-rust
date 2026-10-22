use super::*;

fn add_planning(
    bus: &Bus,
    epoch: CanonicalUuid,
    name: &str,
    intent: wire::TaskIntent,
    flow: Option<&str>,
) -> TaskHandle {
    add_planning_with(
        bus,
        epoch,
        name,
        intent,
        flow,
        &["The declared planning deliverable is supported"],
    )
}

fn add_planning_with(
    bus: &Bus,
    epoch: CanonicalUuid,
    name: &str,
    intent: wire::TaskIntent,
    flow: Option<&str>,
    acceptance: &[&str],
) -> TaskHandle {
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
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: acceptance.iter().map(|item| (*item).to_owned()).collect(),
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: flow.map(str::to_owned),
            depends: Vec::new(),
            intent: Some(intent),
            claim: false,
            draft: false,
        },
    ));
    task
}

fn done(bus: &Bus, epoch: CanonicalUuid, seat: &Seat, task: &TaskHandle) -> Outcome<CommandAnswer> {
    claim_task(bus, epoch, seat, &Actor::Lane(seat.lane), task);
    complete(bus, epoch, seat, task)
}

/// Complete `task`'s current phase under the claim `seat` already holds.
fn complete(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    task: &TaskHandle,
) -> Outcome<CommandAnswer> {
    let lane = Actor::Lane(seat.lane);
    let held = board(bus, epoch, lane.clone());
    let expected = progress_witnesses(&held, at(&held, task), seat.lane, seat.occupant, false)
        .expect("completion witnesses");
    issue(
        bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        expected,
        &TaskDone {
            task: task.clone(),
            validation: "Planning evidence retained".to_owned(),
            disposition: None,
            operation: Some(completion_operation()),
        },
    )
}

#[test]
fn declared_planning_defaults_give_each_purpose_its_flow() {
    let (root, bus, epoch, _) = fixture("planning-defaults", 1);
    let question = &["Which boundary owns the contract?"][..];
    for (name, intent, questions, flow) in [
        (
            "Plan contracts",
            wire::TaskIntent::Plan,
            question,
            &["plan", "review"][..],
        ),
        (
            "Design assembly",
            wire::TaskIntent::Design,
            question,
            &["design"][..],
        ),
        // A Design without an open question is framed before it opens, as
        // ordinary work without acceptance is.
        (
            "Design unframed",
            wire::TaskIntent::Design,
            &[][..],
            &["plan", "design"][..],
        ),
    ] {
        let task = add_planning_with(&bus, epoch, name, intent, None, questions);
        let view = board(&bus, epoch, Actor::Operator);
        let task = at(&view, &task);
        assert_eq!(task.statement.body.intent, Some(intent));
        assert_eq!(
            task.workflow
                .body
                .flow
                .iter()
                .map(wire::Phase::as_str)
                .collect::<Vec<_>>(),
            flow
        );
    }
    for flow in ["design", "plan,design", "review,design"] {
        let task = add_planning(
            &bus,
            epoch,
            &format!("Admitted Design flow {flow}"),
            wire::TaskIntent::Design,
            Some(flow),
        );
        let view = board(&bus, epoch, Actor::Operator);
        assert_eq!(
            at(&view, &task).workflow.body.phase.as_str(),
            flow.split(',').next().expect("flow has a first phase"),
        );
    }
    for flow in ["design,review", "review", "plan", "todo", "plan,review"] {
        let born = stamp(&format!("Design flow {flow}"));
        let result = issue(
            &bus,
            epoch,
            Actor::Operator,
            CommandKind::TaskAdd,
            vec![Witness::Absence { task: born.clone() }],
            &TaskAdd {
                preparation: None,
                title: format!("Design flow {flow}"),
                pitch: "This work keeps the fixture honest.".to_owned(),
                project: Some("board.test".to_owned()),
                description: None,
                acceptance: vec!["The design is supported".to_owned()],
                requirements: wire::EnvironmentRequirements::default(),
                origin: Some(fixture_origin()),
                flow: Some(flow.to_owned()),
                depends: Vec::new(),
                intent: Some(wire::TaskIntent::Design),
                claim: false,
                draft: false,
            },
        );
        assert!(
            matches!(&result, Outcome::Refusal(refusal) if refusal.reason() == "task::design-flow"),
            "a Design with flow {flow} must refuse: {result:?}"
        );
    }
    clean(&root, bus);
}
#[test]
fn plan_intake_releases_its_claim_into_a_passive_design() {
    let (root, bus, epoch, seats) = fixture("planning-design-plan-intake", 1);
    let design = add_planning(
        &bus,
        epoch,
        "Plan the Design statement",
        wire::TaskIntent::Design,
        Some("plan,design"),
    );
    let lane = Actor::Lane(seats[0].lane);
    assert_eq!(
        allocated(&bus, epoch, &lane).map(|(handle, _)| handle),
        Some(design.to_string()),
        "plan intake remains ordinary allocatable work",
    );
    let premature = stamp("Premature Design output");
    let view = board(&bus, epoch, Actor::Operator);
    let parent = at(&view, &design);
    let refused = issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![
            Witness::Absence {
                task: premature.clone(),
            },
            statement_witness(parent),
            workflow_witness(parent),
        ],
        &TaskAdd {
            preparation: Some(design.clone()),
            title: "Premature Design output".to_owned(),
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The output is executable".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: Some("todo".to_owned()),
            depends: Vec::new(),
            intent: None,
            claim: false,
            draft: false,
        },
    );
    assert!(
        matches!(&refused, Outcome::Refusal(refusal)
            if refusal.reason() == "task::preparation-refused"),
        "a Design intake cannot control an output: {refused:?}",
    );
    assert!(
        board(&bus, epoch, Actor::Operator)
            .tasks
            .iter()
            .all(|task| task.statement.body.born != premature),
        "refused pre-Design preparation wrote no task",
    );
    assert_committed(done(&bus, epoch, &seats[0], &design));

    assert_eq!(
        allocated(&bus, epoch, &lane).map(|(handle, _)| handle),
        None,
        "the transitioned passive Design leaves allocation",
    );
    let view = board(&bus, epoch, Actor::Operator);
    let heads = at(&view, &design);
    assert_eq!(heads.workflow.body.phase.as_str(), "design");
    assert!(heads.workflow.body.status.is_pending());
    assert_eq!(
        heads.claim.as_ref().map(|claim| claim.body.disposition),
        Some(ClaimDisposition::Freed),
    );
    assert_eq!(
        heads.design_standing(),
        Some(wire::intent::DesignStanding::Passive),
    );
    bus.verify()
        .expect("transitioned Design authority verifies before restart");
    bus.shutdown().expect("planning bus stops");
    let reopened =
        journal::Journal::open(root.join("authority.sqlite3")).expect("planning journal reopens");
    let reopened = Bus::start(reopened).expect("planning bus restarts");
    let restarted = board(&reopened, epoch, Actor::Operator);
    assert_eq!(
        at(&restarted, &design).design_standing(),
        Some(wire::intent::DesignStanding::Passive),
    );
    clean(&root, reopened);
}

/// A plan intake's claimant sharpens the Design statement through ordinary
/// `TaskEdit` authority before advancing; an edit witnessing the head it drew
/// from after a concurrent edit landed refuses and keeps that edit.
#[test]
fn a_plan_intake_claimant_revises_the_statement_and_a_stale_head_refuses() {
    let (root, bus, epoch, seats) = fixture("planning-design-intake-edit", 1);
    let design = add_planning(
        &bus,
        epoch,
        "Plan the Design statement",
        wire::TaskIntent::Design,
        Some("plan,design"),
    );
    let lane = Actor::Lane(seats[0].lane);
    claim_task(&bus, epoch, &seats[0], &lane, &design);
    let drawn = statement_witness(at(&board(&bus, epoch, lane.clone()), &design));
    assert_committed(super::edit::correct(
        &bus,
        epoch,
        lane.clone(),
        &TaskEdit {
            title: Some("Sharpened Design statement".to_owned()),
            description: Some("Who needs this decision, and why now.".to_owned()),
            ..super::edit::only(&design)
        },
    ));
    let stale = issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskEdit,
        vec![drawn],
        &TaskEdit {
            description: Some("A stale rewrite".to_owned()),
            ..super::edit::only(&design)
        },
    );
    assert!(
        matches!(&stale, Outcome::Refusal(refusal) if refusal.reason() == "board::witness-moved"),
        "an edit drawn from a moved head refuses: {stale:?}",
    );
    let view = board(&bus, epoch, Actor::Operator);
    let heads = at(&view, &design);
    assert_eq!(heads.presentation_title(), "Sharpened Design statement");
    assert_eq!(
        heads.statement.body.description.as_deref(),
        Some("Who needs this decision, and why now."),
    );
    assert_eq!(heads.workflow.body.phase.as_str(), "plan");
    assert_eq!(
        heads.claim.as_ref().map(|claim| claim.body.disposition),
        Some(ClaimDisposition::Active),
        "refining the statement keeps the intake claim",
    );
    assert_committed(super::edit::correct(
        &bus,
        epoch,
        lane.clone(),
        &TaskEdit {
            description: Some(String::new()),
            ..super::edit::only(&design)
        },
    ));
    assert_eq!(
        at(&board(&bus, epoch, Actor::Operator), &design)
            .statement
            .body
            .description,
        None,
        "an empty replacement clears the description",
    );
    assert_committed(complete(&bus, epoch, &seats[0], &design));
    let heads = at(&board(&bus, epoch, Actor::Operator), &design).clone();
    assert_eq!(heads.presentation_title(), "Sharpened Design statement");
    assert_eq!(
        heads.design_standing(),
        Some(wire::intent::DesignStanding::Passive),
    );
    clean(&root, bus);
}

#[test]
fn clean_review_intake_releases_its_claim_into_a_passive_design() {
    let (root, bus, epoch, seats) = fixture("planning-design-review-intake", 1);
    let seat = &seats[0];
    let actor = Actor::Lane(seat.lane);
    let design = add_planning(
        &bus,
        epoch,
        "Review the Design statement",
        wire::TaskIntent::Design,
        Some("review,design"),
    );
    claim_task(&bus, epoch, seat, &actor, &design);
    let view = board(&bus, epoch, actor.clone());
    assert_committed(issue(
        &bus,
        epoch,
        actor,
        CommandKind::TaskReview,
        progress_witnesses(&view, at(&view, &design), seat.lane, seat.occupant, true)
            .expect("review intake witnesses"),
        &TaskReview {
            task: design.clone(),
            finding: "clean".to_owned(),
            note: None,
            then: Vec::new(),
            followup: Vec::new(),
        },
    ));

    let view = board(&bus, epoch, Actor::Operator);
    assert_eq!(
        latest_review_status(&bus, &design),
        wire::body::TaskCompletionStatus::Pending,
        "a clean intake review advances the Design and settles nothing"
    );
    let design = at(&view, &design);
    assert_eq!(design.workflow.body.phase.as_str(), "design");
    assert!(matches!(
        design.workflow.body.status,
        wire::WorkflowStatus::Pending(wire::WorkflowReview::Clean)
    ));
    assert_eq!(
        design.claim.as_ref().map(|claim| claim.body.disposition),
        Some(ClaimDisposition::Freed),
    );
    assert_eq!(
        design.design_standing(),
        Some(wire::intent::DesignStanding::Passive),
    );
    clean(&root, bus);
}

#[test]
fn finding_review_intake_enters_design_with_its_correction_recorded() {
    let (root, bus, epoch, seats) = fixture("planning-design-review-finding", 1);
    let seat = &seats[0];
    let actor = Actor::Lane(seat.lane);
    authorize_lane_creation(&bus, epoch, seat);
    let design = add_planning(
        &bus,
        epoch,
        "Review a Design that needs correction",
        wire::TaskIntent::Design,
        Some("review,design"),
    );
    claim_task(&bus, epoch, seat, &actor, &design);
    let view = board(&bus, epoch, actor.clone());
    let correction = stamp("Correct the Design statement");
    let mut expected =
        progress_witnesses(&view, at(&view, &design), seat.lane, seat.occupant, true)
            .expect("finding intake witnesses");
    expected.push(Witness::Absence {
        task: correction.clone(),
    });
    assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: design.clone(),
            finding: "The failure boundary is underspecified".to_owned(),
            note: None,
            then: vec![wire::intent::ReviewFollowup {
                task: correction.clone(),
                add: TaskAdd {
                    preparation: None,
                    title: "Correct the Design statement".to_owned(),
                    pitch: "This work keeps the fixture honest.".to_owned(),
                    project: Some("board.test".to_owned()),
                    description: None,
                    acceptance: vec!["The failure boundary is explicit".to_owned()],
                    requirements: wire::EnvironmentRequirements::default(),
                    origin: None,
                    flow: None,
                    depends: Vec::new(),
                    intent: None,
                    claim: false,
                    draft: false,
                },
            }],
            followup: Vec::new(),
        },
    ));

    let finding_view = board(&bus, epoch, Actor::Operator);
    let finding_design = at(&finding_view, &design);
    assert_eq!(finding_design.workflow.body.phase.as_str(), "review");
    assert!(matches!(
        finding_design.workflow.body.status,
        wire::WorkflowStatus::Pending(wire::WorkflowReview::Unassigned)
    ));
    assert_eq!(
        finding_design
            .claim
            .as_ref()
            .map(|claim| claim.body.disposition),
        Some(ClaimDisposition::Freed),
    );
    assert!(
        finding_design
            .statement
            .body
            .dependencies
            .contains(&correction),
        "the finding remains an explicit input until its correction completes",
    );
    assert_eq!(finding_design.design_standing(), None);

    let correction = TaskHandle::for_project("board.test", &correction);
    claim_and_complete_task(&bus, epoch, seat, &actor, &correction);
    claim_task(&bus, epoch, seat, &actor, &design);
    let assigned = board(&bus, epoch, actor.clone());
    assert_committed(issue(
        &bus,
        epoch,
        actor,
        CommandKind::TaskReview,
        progress_witnesses(
            &assigned,
            at(&assigned, &design),
            seat.lane,
            seat.occupant,
            true,
        )
        .expect("re-review witnesses"),
        &TaskReview {
            task: design.clone(),
            finding: "clean".to_owned(),
            note: Some("the correction is ready".to_owned()),
            then: Vec::new(),
            followup: Vec::new(),
        },
    ));
    let view = board(&bus, epoch, Actor::Operator);
    let design = at(&view, &design);
    assert_eq!(design.workflow.body.phase.as_str(), "design");
    assert!(matches!(
        design.workflow.body.status,
        wire::WorkflowStatus::Pending(wire::WorkflowReview::Clean)
    ));
    assert_eq!(
        design.claim.as_ref().map(|claim| claim.body.disposition),
        Some(ClaimDisposition::Freed),
    );
    assert_eq!(
        design.design_standing(),
        Some(wire::intent::DesignStanding::Passive),
    );
    clean(&root, bus);
}

#[test]
fn a_lane_never_completes_a_design_it_holds() {
    let (root, bus, epoch, seats) = fixture("planning-design-done", 1);
    let design = add_planning(
        &bus,
        epoch,
        "Design held open",
        wire::TaskIntent::Design,
        None,
    );
    group_design(&bus, epoch, &design);
    let result = done(&bus, epoch, &seats[0], &design);
    let Outcome::Refusal(refusal) = &result else {
        panic!("lane done must leave a Design open: {result:?}");
    };
    assert_eq!(refusal.reason(), "task::design-open");
    let view = board(&bus, epoch, Actor::Operator);
    let held = at(&view, &design);
    assert!(held.workflow.body.status.is_pending());
    clean(&root, bus);
}

#[test]
fn atomic_recovery_cannot_create_a_declared_planning_task() {
    let (root, bus, epoch, seats) = fixture("planning-recovery", 1);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    let lane = Actor::Lane(seats[0].lane);
    let born = stamp("Recovered implementation");
    let available = board(&bus, epoch, lane.clone());
    let recover: Name = "recover".parse().expect("recovery rule");
    let result = issue(
        &bus,
        epoch,
        lane,
        CommandKind::TaskAdd,
        vec![
            Witness::Absence { task: born },
            Witness::Eligibility { rule: recover },
            residency(&available, &seats[0]),
        ],
        &TaskAdd {
            preparation: None,
            title: "Recovered implementation".to_owned(),
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: vec!["The recovered behavior is implemented".to_owned()],
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(fixture_origin()),
            flow: None,
            depends: Vec::new(),
            intent: Some(wire::TaskIntent::Plan),
            claim: true,
            draft: false,
        },
    );
    assert!(
        matches!(
            &result,
            Outcome::Refusal(refusal) if refusal.reason() == "task::capture-planning"
        ),
        "planning recovery must refuse before repository recovery admission: {result:?}",
    );
    clean(&root, bus);
}

#[test]
fn claimed_wake_cannot_adopt_implementation_into_a_planning_deliverable() {
    let (root, bus, epoch, seats) = fixture("planning-wake-capture", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    authorize_lane_creation(&bus, epoch, seat);
    let task = add_planning(&bus, epoch, "Woken plan", wire::TaskIntent::Plan, None);
    let before = board(&bus, epoch, lane.clone());
    let heads = at(&before, &task);
    let until = instant_after(current_instant().expect("clock"), 300).expect("future instant");
    assert_committed(issue(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDefer,
        vec![workflow_witness(heads), deferral_witness(heads)],
        &TaskDefer {
            task: task.clone(),
            wait: until,
        },
    ));
    let deferred = board(&bus, epoch, lane.clone());
    let heads = at(&deferred, &task);
    let wake = |rule: &str| {
        issue(
            &bus,
            epoch,
            lane.clone(),
            CommandKind::TaskWake,
            vec![
                workflow_witness(heads),
                deferral_witness(heads),
                Witness::Eligibility {
                    rule: rule.parse().expect("possession rule"),
                },
                residency(&deferred, seat),
            ],
            &TaskWake {
                task: task.clone(),
                claim: true,
            },
        )
    };
    let facts = || bus.fact_snapshot().expect("facts read").facts().len();
    let written = facts();
    for rule in [CAPTURE_WORK_RULE, "recover"] {
        let refused = wake(rule);
        assert!(
            matches!(
                &refused,
                Outcome::Refusal(refusal) if refusal.reason() == "task::capture-planning"
            ),
            "{rule} wake must not adopt implementation into a plan: {refused:?}",
        );
    }
    assert_eq!(facts(), written, "a refused wake writes nothing");

    let CommandAnswer::Task(woken) = assert_committed(wake("capture")) else {
        panic!("wake answers with a task status");
    };
    assert_eq!(
        woken.claim.and_then(|claim| claim.holder),
        Some(lane),
        "an ordinary claimed wake still holds the plan",
    );
    clean(&root, bus);
}

#[test]
fn plan_review_cannot_replace_required_child_construction_with_own_acceptance() {
    let (root, bus, epoch, seats) = fixture("planning-construction", 1);
    let task = add_planning(
        &bus,
        epoch,
        "Construct contracts",
        wire::TaskIntent::Plan,
        Some("plan,review"),
    );
    let result = done(&bus, epoch, &seats[0], &task);
    let Outcome::Refusal(refusal) = result else {
        panic!("planning without a constructed child must refuse: {result:?}");
    };
    assert_eq!(refusal.reason(), "task::plan-incomplete");
    let view = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&view, &task).workflow.body.phase.as_str(), "plan");
    clean(&root, bus);
}

/// Complete `plan`'s plan phase with `disposition`, witnessing `cover` when a
/// task answers the need.
fn dispose(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    plan: &TaskHandle,
    disposition: wire::body::PlanDisposition,
    cover: Option<&TaskHandle>,
) -> Outcome<CommandAnswer> {
    let lane = Actor::Lane(seat.lane);
    let held = board(bus, epoch, lane.clone());
    let mut expected = progress_witnesses(&held, at(&held, plan), seat.lane, seat.occupant, false)
        .expect("disposition witnesses");
    expected.extend(cover.map(|cover| statement_witness(at(&held, cover))));
    issue(
        bus,
        epoch,
        lane,
        CommandKind::TaskDone,
        expected,
        &TaskDone {
            task: plan.clone(),
            validation: "The landed contract already answers this need".to_owned(),
            disposition: Some(disposition),
            operation: Some(completion_operation()),
        },
    )
}

#[test]
fn an_ordinary_disposition_refusal_names_claimed_task_deletion() {
    let (root, bus, epoch, seats) = fixture("ordinary-disposition-refusal", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "TASK-duplicate",
        "Remove duplicate work",
        "board.test",
        Some("todo"),
        &["The duplicate is removed"],
    );
    claim_task(&bus, epoch, seat, &lane, &task);

    let result = dispose(
        &bus,
        epoch,
        seat,
        &task,
        wire::body::PlanDisposition::NoWork {},
        None,
    );
    let Outcome::Refusal(refusal) = result else {
        panic!("ordinary disposition must refuse: {result:?}");
    };
    assert_eq!(refusal.reason(), "task::disposition-refused");
    let detail = refusal
        .diagnostic()
        .and_then(wire::RefusalDiagnostic::detail)
        .expect("actionable disposition refusal");
    assert!(detail.contains("spice task delete HANDLE REASON --force-claimed"));
    let current = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&current, &task).workflow.body.phase.as_str(), "todo");
    assert!(at(&current, &task).workflow.body.status.is_pending());
    clean(&root, bus);
}

#[test]
fn an_intentless_plan_flow_still_records_a_disposition() {
    let (root, bus, epoch, seats) = fixture("intentless-plan-disposition", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    let plan = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "PLAN-flow",
        "Plan by flow alone",
        "board.test",
        Some("plan,review"),
        &["The need is planned"],
    );
    claim_task(&bus, epoch, seat, &lane, &plan);

    assert_committed(dispose(
        &bus,
        epoch,
        seat,
        &plan,
        wire::body::PlanDisposition::NoWork {},
        None,
    ));
    let disposed = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&disposed, &plan).workflow.body.phase.as_str(), "review");
    clean(&root, bus);
}

#[test]
fn a_disposed_plan_settles_on_clean_review_without_constructing_or_authorizing_work() {
    let (root, bus, epoch, seats) = fixture("planning-disposition", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    let cover = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "COVER-landed",
        "Answer the reported need",
        "board.test",
        Some("todo"),
        &["The reported need is answered"],
    );
    claim_and_complete_task(&bus, epoch, seat, &lane, &cover);
    let covering = || wire::body::DispositionCover::Task(cover.born().expect("cover stamp"));
    let commit = || wire::body::DispositionCover::Commit("a".repeat(40));
    for (name, disposition, witness) in [
        (
            "Dispose no work",
            wire::body::PlanDisposition::NoWork {},
            None,
        ),
        (
            "Dispose duplicate",
            wire::body::PlanDisposition::Duplicate { cover: covering() },
            Some(&cover),
        ),
        (
            "Dispose covered",
            wire::body::PlanDisposition::Covered { cover: commit() },
            None,
        ),
        (
            "Dispose deferred",
            wire::body::PlanDisposition::Deferred { cover: covering() },
            Some(&cover),
        ),
    ] {
        let plan = add_planning(
            &bus,
            epoch,
            name,
            wire::TaskIntent::Plan,
            Some("plan,review"),
        );
        let before = board(&bus, epoch, Actor::Operator);
        claim_task(&bus, epoch, seat, &lane, &plan);
        assert_committed(dispose(&bus, epoch, seat, &plan, disposition, witness));
        let disposed = board(&bus, epoch, Actor::Operator);
        assert_eq!(at(&disposed, &plan).workflow.body.phase.as_str(), "review");
        assert!(at(&disposed, &plan).workflow.body.status.is_pending());

        claim_task(&bus, epoch, seat, &lane, &plan);
        assert_committed(super::preparation::review_clean(&bus, epoch, seat, &plan));
        let settled = board(&bus, epoch, Actor::Operator);
        assert!(at(&settled, &plan).workflow.body.status.is_completed());
        // Nothing was constructed, released, or moved beside the Plan itself.
        assert_eq!(settled.tasks.len(), before.tasks.len(), "{name}");
        assert_eq!(
            at(&settled, &cover).workflow.position,
            at(&before, &cover).workflow.position,
            "{name}"
        );
    }
    clean(&root, bus);
}

#[test]
fn a_plan_disposition_refuses_beside_a_live_output_or_an_unwitnessed_cover() {
    let (root, bus, epoch, seats) = fixture("planning-disposition-refusals", 1);
    let seat = &seats[0];
    let lane = Actor::Lane(seat.lane);
    let plan = add_planning(
        &bus,
        epoch,
        "Dispose refused",
        wire::TaskIntent::Plan,
        Some("plan,review"),
    );
    claim_task(&bus, epoch, seat, &lane, &plan);
    let cover = add_associated(
        &bus,
        epoch,
        Actor::Operator,
        "Unwitnessed cover",
        None,
        None,
    );
    let unwitnessed = dispose(
        &bus,
        epoch,
        seat,
        &plan,
        wire::body::PlanDisposition::Duplicate {
            cover: wire::body::DispositionCover::Task(cover.born().expect("cover stamp")),
        },
        None,
    );
    assert!(
        matches!(&unwitnessed, Outcome::Refusal(refusal)
            if refusal.reason() == "task::disposition-cover-invalid"),
        "{unwitnessed:?}"
    );

    // Linked accepted work already discharges construction, so no disposition stands beside it.
    connect_child(&bus, epoch, &plan, &cover);
    let beside = dispose(
        &bus,
        epoch,
        seat,
        &plan,
        wire::body::PlanDisposition::NoWork {},
        None,
    );
    assert!(
        matches!(&beside, Outcome::Refusal(refusal)
            if refusal.reason() == "task::disposition-beside-output"),
        "{beside:?}"
    );
    let view = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&view, &plan).workflow.body.phase.as_str(), "plan");
    clean(&root, bus);
}

#[test]
fn operator_closed_child_cannot_satisfy_plan_construction() {
    let (root, bus, epoch, seats) = fixture("planning-closed-construction", 1);
    let plan = add_planning(
        &bus,
        epoch,
        "Construct retained contracts",
        wire::TaskIntent::Plan,
        Some("plan,review"),
    );
    let child = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "CONTRACT-negative",
        "Investigate one possible contract",
        "board.test",
        Some("todo"),
        &["The possible contract is justified"],
    );
    connect_child(&bus, epoch, &plan, &child);
    let view = board(&bus, epoch, Actor::Operator);
    let heads = at(&view, &child);
    assert_committed(issue(
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
            task: child,
            validation: "Investigation rejected this contract".to_owned(),
            force_claimed: false,
        },
    ));

    let result = done(&bus, epoch, &seats[0], &plan);
    let Outcome::Refusal(refusal) = result else {
        panic!("operator-closed work must not construct a plan: {result:?}");
    };
    assert_eq!(refusal.reason(), "task::plan-incomplete");
    let view = board(&bus, epoch, Actor::Operator);
    assert_eq!(at(&view, &plan).workflow.body.phase.as_str(), "plan");
    clean(&root, bus);
}

#[test]
fn ordinary_done_cannot_supply_a_required_review_verdict() {
    let (root, bus, epoch, seats) = fixture("planning-review-verdict", 1);
    let task = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "REVIEW-verdict",
        "Review the declared deliverable",
        "board.test",
        Some("review"),
        &["The review judges the deliverable"],
    );
    let result = done(&bus, epoch, &seats[0], &task);
    let Outcome::Refusal(refusal) = result else {
        panic!("ordinary completion cannot supply a verdict: {result:?}");
    };
    assert_eq!(refusal.reason(), "task::review-required");
    let view = board(&bus, epoch, Actor::Operator);
    assert!(at(&view, &task).workflow.body.status.is_pending());
    clean(&root, bus);
}

fn add_associated(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    name: &str,
    parent: Option<&TaskHandle>,
    intent: Option<wire::TaskIntent>,
) -> TaskHandle {
    let born = stamp(name);
    let task = TaskHandle::for_project("board.test", &born);
    let add = TaskAdd {
        preparation: parent.cloned(),
        intent,
        title: name.to_owned(),
        pitch: "This work keeps the fixture honest.".to_owned(),
        project: Some("board.test".to_owned()),
        description: None,
        acceptance: vec!["The executable contract supports the requested behavior".to_owned()],
        requirements: wire::EnvironmentRequirements::default(),
        origin: if actor == Actor::Operator {
            Some(fixture_origin())
        } else {
            None
        },
        flow: None,
        depends: Vec::new(),
        claim: false,
        draft: false,
    };
    let view = board(bus, epoch, actor.clone());
    let mut expected = vec![Witness::Absence { task: born }];
    append_preparation_witnesses(&view, &add, &mut expected).expect("preparation witnesses");
    assert_committed(issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskAdd,
        expected,
        &add,
    ));
    task
}

#[test]
fn negative_controller_closure_closes_every_dependent_branch() {
    let (root, bus, epoch, _seats) = fixture("negative-controller", 1);
    let plan = add_planning(
        &bus,
        epoch,
        "Construct selected contract",
        wire::TaskIntent::Plan,
        None,
    );
    let output = add_associated(
        &bus,
        epoch,
        Actor::Operator,
        "Controlled implementation",
        Some(&plan),
        None,
    );
    let ordinary = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "TASK-ordinary-edge",
        "Unrelated follow-on",
        "board.test",
        Some("todo"),
        &["The ordinary follow-on is supported"],
    );
    connect_child(&bus, epoch, &ordinary, &plan);
    let view = board(&bus, epoch, Actor::Operator);
    let heads = at(&view, &plan);
    assert_committed(issue(
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
            task: plan,
            validation: "No executable output is selected".to_owned(),
            force_claimed: false,
        },
    ));
    let view = board(&bus, epoch, Actor::Operator);
    for dependent in [&output, &ordinary] {
        assert!(
            at(&view, dependent).workflow.body.status.is_completed(),
            "negative Plan settlement closes every ordinary dependent"
        );
    }
    clean(&root, bus);
}

#[test]
fn settling_one_controller_preserves_a_branch_owned_by_another_open_controller() {
    let (root, bus, epoch, _seats) = fixture("independent-controller", 1);
    let first = add_planning(
        &bus,
        epoch,
        "First controller",
        wire::TaskIntent::Plan,
        None,
    );
    let second = add_planning(
        &bus,
        epoch,
        "Second controller",
        wire::TaskIntent::Plan,
        None,
    );
    let output = add_associated(
        &bus,
        epoch,
        Actor::Operator,
        "Shared controlled implementation",
        Some(&first),
        None,
    );
    connect_child(&bus, epoch, &output, &second);
    let branch = add_project_with_acceptance(
        &bus,
        epoch,
        Actor::Operator,
        "TASK-shared-branch",
        "Follow shared output",
        "board.test",
        Some("todo"),
        &["The follow-on remains owned with its shared output"],
    );
    connect_child(&bus, epoch, &branch, &output);

    for (controller, should_close) in [(&first, false), (&second, true)] {
        let view = board(&bus, epoch, Actor::Operator);
        let heads = at(&view, controller);
        assert_committed(issue(
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
                task: controller.clone(),
                validation: "No executable output is selected".to_owned(),
                force_claimed: false,
            },
        ));
        let view = board(&bus, epoch, Actor::Operator);
        for retained in [&output, &branch] {
            assert_eq!(
                at(&view, retained).workflow.body.status.is_completed(),
                should_close,
                "{retained} follows the last open controller"
            );
        }
    }
    clean(&root, bus);
}

#[test]
fn planning_corrections_keep_the_original_construction_duty_and_later_declared_phases() {
    let (root, bus, epoch, seats) = fixture("planning-correction-duty", 1);
    let actor = Actor::Lane(seats[0].lane);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    let plan = add_planning(
        &bus,
        epoch,
        "Construct executable contract",
        wire::TaskIntent::Plan,
        Some("review,verify"),
    );
    claim_task(&bus, epoch, &seats[0], &actor, &plan);
    let view = board(&bus, epoch, actor.clone());
    let correction_born = stamp("Construct missing output");
    let correction = TaskHandle::for_project("board.test", &correction_born);
    let mut expected = progress_witnesses(
        &view,
        at(&view, &plan),
        seats[0].lane,
        seats[0].occupant,
        true,
    )
    .expect("review witnesses");
    expected.push(Witness::Absence {
        task: correction_born.clone(),
    });
    assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: plan.clone(),
            finding: "The executable contract is absent".to_owned(),
            note: None,
            followup: Vec::new(),
            then: vec![wire::intent::ReviewFollowup {
                task: correction_born,
                add: TaskAdd {
                    preparation: None,
                    intent: None,
                    title: "Construct missing output".to_owned(),
                    pitch: "This work keeps the fixture honest.".to_owned(),
                    project: Some("board.test".to_owned()),
                    description: None,
                    acceptance: Vec::new(),
                    requirements: wire::EnvironmentRequirements::default(),
                    origin: None,
                    flow: None,
                    depends: Vec::new(),
                    claim: false,
                    draft: false,
                },
            }],
        },
    ));
    claim_task(&bus, epoch, &seats[0], &actor, &correction);
    let view = board(&bus, epoch, actor.clone());
    let verdict = TaskReview {
        task: correction.clone(),
        finding: "clean".to_owned(),
        note: None,
        then: Vec::new(),
        followup: Vec::new(),
    };
    let refused = issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskReview,
        progress_witnesses(
            &view,
            at(&view, &correction),
            seats[0].lane,
            seats[0].occupant,
            true,
        )
        .expect("correction witnesses"),
        &verdict,
    );
    assert!(
        matches!(refused, Outcome::Refusal(refusal) if refusal.reason() == "task::plan-incomplete")
    );
    add_associated(
        &bus,
        epoch,
        Actor::Operator,
        "Executable output contract",
        Some(&plan),
        None,
    );
    let view = board(&bus, epoch, actor.clone());
    let reply = assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskReview,
        progress_witnesses(
            &view,
            at(&view, &correction),
            seats[0].lane,
            seats[0].occupant,
            true,
        )
        .expect("constructed correction witnesses"),
        &verdict,
    ));
    let CommandAnswer::Task(answer) = reply else {
        panic!("review returns task state");
    };
    assert_eq!(answer.status, "pending");
    let view = board(&bus, epoch, actor);
    assert_eq!(
        at(&view, &correction).workflow.body.phase.as_str(),
        "verify"
    );
    assert!(at(&view, &plan).workflow.body.status.is_pending());
    clean(&root, bus);
}

#[test]
fn a_correction_outlives_its_settled_plan_without_the_construction_duty() {
    let (root, bus, epoch, seats) = fixture("planning-correction-settled", 1);
    let actor = Actor::Lane(seats[0].lane);
    authorize_lane_creation(&bus, epoch, &seats[0]);
    let plan = add_planning(
        &bus,
        epoch,
        "Construct executable contract",
        wire::TaskIntent::Plan,
        Some("review"),
    );
    claim_task(&bus, epoch, &seats[0], &actor, &plan);
    let view = board(&bus, epoch, actor.clone());
    let correction_born = stamp("Repair the plan");
    let correction = TaskHandle::for_project("board.test", &correction_born);
    let mut expected = progress_witnesses(
        &view,
        at(&view, &plan),
        seats[0].lane,
        seats[0].occupant,
        true,
    )
    .expect("review witnesses");
    expected.push(Witness::Absence {
        task: correction_born.clone(),
    });
    assert_committed(issue(
        &bus,
        epoch,
        actor.clone(),
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: plan.clone(),
            finding: "The executable contract is absent".to_owned(),
            note: None,
            followup: Vec::new(),
            then: vec![wire::intent::ReviewFollowup {
                task: correction_born,
                add: TaskAdd {
                    preparation: None,
                    intent: None,
                    title: "Repair the plan".to_owned(),
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
        },
    ));
    let operator_view = board(&bus, epoch, Actor::Operator);
    let heads = at(&operator_view, &plan);
    assert_committed(issue(
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
            task: plan,
            validation: "The plan is withdrawn".to_owned(),
            force_claimed: false,
        },
    ));
    claim_and_complete_task(&bus, epoch, &seats[0], &actor, &correction);
    assert!(
        at(&board(&bus, epoch, actor), &correction)
            .workflow
            .body
            .status
            .is_completed()
    );
    clean(&root, bus);
}
