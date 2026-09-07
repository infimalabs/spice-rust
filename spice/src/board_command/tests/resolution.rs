use super::*;

mod completion_replay;
mod document;
mod drain_guidance;
mod plan_advance;

#[test]
fn lane_claim_free_done_and_review_finding_survive_restart() {
    let (root, bus, epoch, seats) = fixture("workflow", 2);
    authorize_lane_creation(&bus, epoch, &seats[1]);
    let lane_one = Actor::Lane(seats[0].lane);
    let lane_two = Actor::Lane(seats[1].lane);
    assert_lane_resolution(&bus, epoch, &seats, &lane_one);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "FLOW-flow0001",
        "Complete and review",
        None,
    );
    advance_task_to_review(&bus, epoch, &seats[0], &lane_one, &task);
    complete_review_with_finding(&bus, epoch, &seats[1], &lane_two, &task);
    let settled = assert_review_settled(&bus, epoch, &task);
    assert_restart_preserves(&root, bus, epoch, &settled);
}

#[test]
fn one_captured_band_drives_board_answer_server_selection_and_guard_replay() {
    let (root, bus, epoch, seats) = fixture("allocation-band", 1);
    let lane = Actor::Lane(seats[0].lane);
    let lower = add(
        &bus,
        epoch,
        Actor::Operator,
        "BAND-a1",
        "Short lower handle",
        Some("todo"),
    );
    let higher = add(
        &bus,
        epoch,
        Actor::Operator,
        "BAND-z1",
        "Longer urgent flow",
        Some("todo,verify,review"),
    );
    let allocation = board::NextConfig::new(1);
    let available = board_with_allocation(&bus, epoch, lane.clone(), &allocation);
    assert_eq!(available.allocation_band, 1);

    let entries = actor_entries(&available).expect("configured board projects to entries");
    let subscription = allocation_subscription(&available.residency.body, &lane)
        .expect("fixture lane has one allocation subscription");
    let selected = board::select(
        &entries,
        &lane,
        subscription,
        available.allocation_band,
        available.recent_cell.as_ref(),
    )
    .expect("configured client preselection has work");
    assert_eq!(selected.entry().handle(), &lower);

    assert_committed(issue_with_allocation(
        &bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, &lower, &seats[0]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
        &allocation,
    ));
    let held = board_with_allocation(&bus, epoch, lane, &allocation);
    assert!(at(&held, &higher).claim.is_none());
    assert_eq!(
        at(&held, &lower)
            .claim
            .as_ref()
            .expect("server and guard accepted the configured winner")
            .body
            .disposition,
        ClaimDisposition::Active,
    );
    clean(&root, bus);
}

fn assert_lane_resolution(bus: &Bus, epoch: CanonicalUuid, seats: &[Seat], lane_one: &Actor) {
    let operator = board(bus, epoch, Actor::Operator);
    assert!(matches!(
        actor_seat(&operator),
        Err(Outcome::Refusal(ref refusal))
            if refusal.reason() == "claim::operator-lane-ambiguous"
    ));
    let lane_board = board(bus, epoch, lane_one.clone());
    assert_eq!(
        actor_seat(&lane_board).expect("lane resolves its own seat"),
        (seats[0].lane, seats[0].occupant)
    );
}

pub(super) fn advance_task_to_review(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    lane: &Actor,
    task: &TaskHandle,
) {
    let available = board(bus, epoch, lane.clone());
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, task, seat),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let held = board(bus, epoch, lane.clone());
    let held_heads = at(&held, task);
    let released = assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimFree,
        vec![
            claim_witness(held_heads),
            Witness::Lease {
                task: held_heads.statement.body.born.clone(),
                head: held_heads.lease.as_ref().expect("lease head").position,
            },
            residency(&held, seat),
        ],
        &TaskRef { task: task.clone() },
    ));
    let CommandAnswer::Claim(released) = released else {
        panic!("claim release did not return a claim answer");
    };
    assert_eq!(released.next.as_deref(), Some("spice task next"));
    claim_and_complete_task(bus, epoch, seat, lane, task);
}

pub(super) fn claim_and_complete_task(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    lane: &Actor,
    task: &TaskHandle,
) {
    let available = board(bus, epoch, lane.clone());
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, task, seat),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let held = board(bus, epoch, lane.clone());
    let expected = progress_witnesses(&held, at(&held, task), seat.lane, seat.occupant, false)
        .expect("held task has progress witnesses");
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::TaskDone,
        expected,
        &TaskDone {
            task: task.clone(),
            validation: "implementation and focused checks complete".to_owned(),
            operation: Some(completion_operation()),
        },
    ));
}

pub(super) fn complete_review_with_finding(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    lane: &Actor,
    task: &TaskHandle,
) {
    let reviewable = board(bus, epoch, lane.clone());
    assert_eq!(at(&reviewable, task).workflow.body.phase.as_str(), "review");
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&reviewable, task, seat),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let assigned = board(bus, epoch, lane.clone());
    assert_eq!(
        at(&assigned, task).workflow.body.assignment,
        Some(lane.clone())
    );
    assert_review_assignment_fact(bus, &assigned, task, lane);
    let expected = progress_witnesses(
        &assigned,
        at(&assigned, task),
        seat.lane,
        seat.occupant,
        true,
    )
    .expect("assigned review has exact witnesses");
    let tracked: wire::TaskStamp = "1kKRRsDP".parse().expect("follow-up stamp");
    let mut expected = expected;
    expected.push(Witness::Absence {
        task: tracked.clone(),
    });
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: task.clone(),
            finding: "missing negative-path evidence".to_owned(),
            note: Some("recorded for the next authored correction".to_owned()),
            then: vec![ReviewFollowup {
                task: tracked,
                add: TaskAdd {
                    title: "Cover the negative path the review named".to_owned(),
                    project: None,
                    description: None,
                    acceptance: Vec::new(),
                    origin: None,
                    flow: None,
                    depends: Vec::new(),
                },
            }],
            followup: Vec::new(),
        },
    ));
}

pub(super) fn assert_review_assignment_fact(
    bus: &Bus,
    assigned: &BoardAnswer,
    task: &TaskHandle,
    reviewer: &Actor,
) {
    let heads = at(assigned, task);
    let snapshot = bus.fact_snapshot().expect("assignment facts read");
    let envelope = snapshot
        .facts()
        .iter()
        .find(|fact| fact.seq == heads.workflow.position)
        .expect("review workflow head names a durable fact");
    let typed: wire::Envelope<wire::Fact<wire::body::TaskAssigned>> =
        wire::decode(&wire::encode(envelope).expect("assignment fact encodes"))
            .expect("workflow head is the registered review assignment fact");
    assert_eq!(typed.fact.body.task, heads.statement.body.born);
    assert_eq!(typed.fact.body.reviewer, *reviewer);
    assert_eq!(typed.fact.body.phase.as_str(), "review");
    assert!(typed.is_root(), "assignment begins the claim causal chain");

    let claim = heads.claim.as_ref().expect("review claim head");
    let won = snapshot
        .facts()
        .iter()
        .find(|fact| fact.seq == claim.position)
        .expect("review claim head names a durable fact");
    assert_eq!(won.fact.kind(), wire::FactKind::ClaimWon);
    assert_eq!(won.cause, Some(typed.seq));
}

fn assert_review_settled(bus: &Bus, epoch: CanonicalUuid, task: &TaskHandle) -> BoardAnswer {
    let settled = board(bus, epoch, Actor::Operator);
    let heads = at(&settled, task);
    assert_eq!(heads.workflow.body.status, "completed");
    assert_eq!(heads.workflow.body.review, "finding");
    assert_eq!(
        heads
            .claim
            .as_ref()
            .expect("review claim head")
            .body
            .disposition,
        ClaimDisposition::Freed
    );
    let entries = actor_entries(&settled).expect("settled board projects to entries");
    let entry = entries
        .iter()
        .find(|entry| entry.handle() == task)
        .expect("settled task remains on the board");
    assert_eq!(
        accepted_transition(&settled.principal, &settled.residency.body, &entries, entry,),
        NextTransition::Settled,
        "completion outranks the retained final review phase",
    );
    let facts = bus.fact_snapshot().expect("workflow facts read");
    assert!(facts.facts().iter().any(|fact| {
        fact.fact.kind() == wire::FactKind::TaskReviewed
            && fact
                .fact
                .body()
                .get("finding")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|finding| finding.contains("negative-path evidence"))
    }));
    settled
}

/// A finding must name what will carry it, and the word clean must not.
///
/// The two halves share one fixture on purpose: the refusal has to leave
/// the claim exactly where it was, or the clean review that follows it
/// could not be issued against the same witnesses.
#[test]
fn an_unclean_review_needs_follow_up_tracking_and_clean_does_not() {
    let (root, bus, epoch, seats) = fixture("review-followup", 2);
    let lane_one = Actor::Lane(seats[0].lane);
    let lane_two = Actor::Lane(seats[1].lane);
    assert_lane_resolution(&bus, epoch, &seats, &lane_one);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "TRACK-track001",
        "Track what a finding asks for",
        None,
    );
    advance_task_to_review(&bus, epoch, &seats[0], &lane_one, &task);
    let reviewable = board(&bus, epoch, lane_two.clone());
    assert_committed(issue(
        &bus,
        epoch,
        lane_two.clone(),
        CommandKind::ClaimWin,
        claim_expected(&reviewable, &task, &seats[1]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let assigned = board(&bus, epoch, lane_two.clone());
    let expected = progress_witnesses(
        &assigned,
        at(&assigned, &task),
        seats[1].lane,
        seats[1].occupant,
        true,
    )
    .expect("assigned review has exact witnesses");

    let untracked = issue(
        &bus,
        epoch,
        lane_two.clone(),
        CommandKind::TaskReview,
        expected.clone(),
        &TaskReview {
            task: task.clone(),
            finding: "the negative path has no test".to_owned(),
            note: None,
            then: Vec::new(),
            followup: Vec::new(),
        },
    );
    assert!(
        matches!(
            untracked,
            Outcome::Refusal(ref refusal)
                if refusal.reason() == "review::followup-required"
        ),
        "a finding landed with nothing tracking it: {untracked:?}",
    );

    // The predecessor default and its allocator hint both spell the
    // verdict this way, so it has to settle the task clean rather than
    // demand follow-up tracking for a review that asked for nothing.
    let clean_review = assert_committed(issue(
        &bus,
        epoch,
        lane_two,
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: task.clone(),
            finding: "CLEAN".to_owned(),
            note: None,
            then: Vec::new(),
            followup: Vec::new(),
        },
    ));
    let CommandAnswer::Task(clean_review) = clean_review else {
        panic!("clean review did not return a task answer");
    };
    assert_eq!(clean_review.next.as_deref(), Some("spice task next"));
    let settled = board(&bus, epoch, Actor::Operator);
    let heads = at(&settled, &task);
    assert_eq!(heads.workflow.body.status, "completed");
    assert_eq!(
        heads.workflow.body.review, "clean",
        "the word clean recorded a finding named after the verdict"
    );
    clean(&root, bus);
}

/// Both follow-up forms persist the dependency they promise.
#[test]
fn a_review_creates_and_links_its_follow_up_tracking() {
    let (root, bus, epoch, seats) = fixture("review-followup-persistence", 2);
    authorize_lane_creation(&bus, epoch, &seats[1]);
    let implementer = Actor::Lane(seats[0].lane);
    let reviewer = Actor::Lane(seats[1].lane);
    assert_lane_resolution(&bus, epoch, &seats, &implementer);
    let reviewed = add(
        &bus,
        epoch,
        Actor::Operator,
        "TRACK-track002",
        "Persist review tracking",
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
    let linked = add(
        &bus,
        epoch,
        Actor::Operator,
        "TRACK-track003",
        "Link review tracking",
        None,
    );
    let assigned = board(&bus, epoch, reviewer.clone());
    let mut expected = progress_witnesses(
        &assigned,
        at(&assigned, &reviewed),
        seats[1].lane,
        seats[1].occupant,
        true,
    )
    .expect("assigned review has exact witnesses");
    let created_stamp: wire::TaskStamp = "1kKRRsDQ".parse().expect("follow-up stamp");
    let created = TaskHandle::for_project("board.test", &created_stamp);
    expected.push(Witness::Absence {
        task: created_stamp.clone(),
    });
    let linked_heads = at(&assigned, &linked);
    expected.push(statement_witness(linked_heads));
    expected.push(workflow_witness(linked_heads));
    expected.push(Witness::Edge {
        task: linked_heads.statement.body.born.clone(),
        after: at(&assigned, &reviewed).statement.body.born.clone(),
    });
    assert_committed(issue(
        &bus,
        epoch,
        reviewer,
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: reviewed.clone(),
            finding: "two corrections need durable owners".to_owned(),
            note: None,
            then: vec![ReviewFollowup {
                task: created_stamp,
                add: TaskAdd {
                    title: "Create the missing review fixture".to_owned(),
                    project: Some("board.test".to_owned()),
                    description: None,
                    acceptance: vec!["the review path is covered".to_owned()],
                    origin: None,
                    flow: None,
                    depends: Vec::new(),
                },
            }],
            followup: vec![linked.clone()],
        },
    ));
    assert_review_followup_tracking(&bus, epoch, &reviewed, &created, &linked);
    clean(&root, bus);
}

/// Review follow-ups do not retroactively become prerequisites of existing dependents.
/// A caller that needs that ordering must wire the downstream task to the follow-up.
#[test]
fn completed_unclean_review_needs_an_explicit_followup_edge_to_block_downstream() {
    let (root, bus, epoch, seats) = fixture("review-followup-downstream", 2);
    authorize_lane_creation(&bus, epoch, &seats[1]);
    let implementer = Actor::Lane(seats[0].lane);
    let reviewer = Actor::Lane(seats[1].lane);
    assert_lane_resolution(&bus, epoch, &seats, &implementer);
    let reviewed = add(
        &bus,
        epoch,
        Actor::Operator,
        "TRACK-track004",
        "Reviewed prerequisite",
        None,
    );
    let downstream = add(
        &bus,
        epoch,
        Actor::Operator,
        "TRACK-track005",
        "Downstream delivery",
        None,
    );
    connect_child(&bus, epoch, &downstream, &reviewed);
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
    let assigned = board(&bus, epoch, reviewer.clone());
    let mut expected = progress_witnesses(
        &assigned,
        at(&assigned, &reviewed),
        seats[1].lane,
        seats[1].occupant,
        true,
    )
    .expect("assigned review has exact witnesses");
    let followup_stamp: wire::TaskStamp = "1kKRRsDS".parse().expect("follow-up stamp");
    let followup = TaskHandle::for_project("board.test", &followup_stamp);
    expected.push(Witness::Absence {
        task: followup_stamp.clone(),
    });
    assert_committed(issue(
        &bus,
        epoch,
        reviewer,
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: reviewed.clone(),
            finding: "downstream needs the reviewed correction".to_owned(),
            note: None,
            then: vec![ReviewFollowup {
                task: followup_stamp,
                add: TaskAdd {
                    title: "Correct the reviewed prerequisite".to_owned(),
                    project: Some("board.test".to_owned()),
                    description: None,
                    acceptance: Vec::new(),
                    origin: None,
                    flow: None,
                    depends: Vec::new(),
                },
            }],
            followup: Vec::new(),
        },
    ));

    let settled = board(&bus, epoch, implementer.clone());
    assert_eq!(at(&settled, &reviewed).workflow.body.status, "completed");
    assert_eq!(
        at(&settled, &followup).statement.body.dependencies,
        vec![at(&settled, &reviewed).statement.body.born.clone()],
    );
    let entries = actor_entries(&settled).expect("lane projects allocatable entries");
    let downstream_entry = entries
        .iter()
        .find(|entry| entry.handle() == &downstream)
        .expect("downstream entry is visible");
    assert!(
        board::eligible_entry(&entries, downstream_entry, &implementer),
        "a completed original satisfies downstream even while its sibling follow-up is unresolved",
    );

    connect_child(&bus, epoch, &downstream, &followup);
    let explicitly_blocked = board(&bus, epoch, implementer.clone());
    let entries = actor_entries(&explicitly_blocked).expect("lane projects updated entries");
    let downstream_entry = entries
        .iter()
        .find(|entry| entry.handle() == &downstream)
        .expect("downstream entry remains visible");
    assert!(
        !board::eligible_entry(&entries, downstream_entry, &implementer),
        "an explicit follow-up dependency blocks downstream without reopening the reviewed task",
    );
    clean(&root, bus);
}

fn assert_review_followup_tracking(
    bus: &Bus,
    epoch: CanonicalUuid,
    reviewed: &TaskHandle,
    created: &TaskHandle,
    linked: &TaskHandle,
) {
    let settled = board(bus, epoch, Actor::Operator);
    let causal = task_origin(
        &reviewed
            .born()
            .expect("reviewed handle carries a birth stamp"),
    )
    .expect("reviewed task forms follow-up origin");
    assert_eq!(
        at(&settled, created).statement.body.provenance,
        causal,
        "--then stored provenance diverged from its reviewed parent"
    );
    assert_eq!(
        creation_fact_origin(bus, created),
        causal,
        "--then creation envelope diverged from stored provenance"
    );
    assert_eq!(
        at(&settled, created).statement.body.dependencies,
        vec![at(&settled, reviewed).statement.body.born.clone()],
        "--then did not create a task behind the reviewed task"
    );
    assert!(
        at(&settled, linked)
            .statement
            .body
            .dependencies
            .contains(&at(&settled, reviewed).statement.body.born),
        "--followup did not wire the existing task behind the review"
    );
    assert_eq!(at(&settled, reviewed).workflow.body.review, "finding");
}

fn assert_restart_preserves(root: &Path, bus: Bus, epoch: CanonicalUuid, settled: &BoardAnswer) {
    bus.verify()
        .expect("workflow authority verifies before restart");
    bus.shutdown().expect("workflow bus stops");
    let reopened =
        journal::Journal::open(root.join("authority.sqlite3")).expect("workflow journal reopens");
    let reopened = Bus::start(reopened).expect("workflow bus restarts");
    assert_eq!(board(&reopened, epoch, Actor::Operator), *settled);
    clean(root, reopened);
}

fn add_raw_task(
    bus: &Bus,
    epoch: CanonicalUuid,
    task: &str,
    title: &str,
    acceptance: &[&str],
    flow: Option<&str>,
) -> TaskHandle {
    let requested = task.parse::<TaskHandle>().expect("fixture task handle");
    let born = requested
        .born()
        .expect("fixture task handle carries a birth stamp");
    let task = TaskHandle::for_project("board.test", &born);
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            title: title.to_owned(),
            project: Some("board.test".to_owned()),
            description: None,
            acceptance: acceptance.iter().map(|item| (*item).to_owned()).collect(),
            origin: Some("fixture:board-command".parse().expect("fixture origin")),
            flow: flow.map(str::to_owned),
            depends: Vec::new(),
        },
    ));
    task
}

fn assert_workflow(board: &BoardAnswer, task: &TaskHandle, phase_name: &str, flow: &[&str]) {
    let heads = at(board, task);
    assert_eq!(heads.workflow.body.phase.as_str(), phase_name);
    assert_eq!(
        heads
            .workflow
            .body
            .flow
            .iter()
            .map(|phase| phase.as_str())
            .collect::<Vec<_>>(),
        flow
    );
}

fn titled<'a>(board: &'a BoardAnswer, title: &str) -> &'a BoardTask {
    board
        .tasks
        .iter()
        .find(|task| task.presentation_title == title)
        .expect("document task is visible by title")
}

fn assert_refusal(outcome: Outcome<CommandAnswer>, reason: &str) {
    assert!(
        matches!(outcome, Outcome::Refusal(ref refusal) if refusal.reason() == reason),
        "expected refusal {reason}, got {outcome:?}",
    );
}

pub(super) fn claim_task(
    bus: &Bus,
    epoch: CanonicalUuid,
    seat: &Seat,
    lane: &Actor,
    task: &TaskHandle,
) {
    let available = board(bus, epoch, lane.clone());
    assert_committed(issue(
        bus,
        epoch,
        lane.clone(),
        CommandKind::ClaimWin,
        claim_expected(&available, task, seat),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
}

pub(super) fn connect_child(
    bus: &Bus,
    epoch: CanonicalUuid,
    parent: &TaskHandle,
    child: &TaskHandle,
) {
    let operator_view = board(bus, epoch, Actor::Operator);
    let actor = at(&operator_view, parent)
        .claim
        .as_ref()
        .filter(|claim| claim.body.disposition == ClaimDisposition::Active)
        .map_or(Actor::Operator, |claim| claim.body.actor.clone());
    let visible = board(bus, epoch, actor.clone());
    let parent_heads = at(&visible, parent);
    let child_heads = at(&visible, child);
    let mut expected = if parent_heads.claim.as_ref().is_some_and(|claim| {
        claim.body.actor == actor && claim.body.disposition == ClaimDisposition::Active
    }) {
        let (lane, occupant) = actor_seat(&visible).expect("claimant has one occupied seat");
        progress_witnesses(
            &visible,
            parent_heads,
            lane,
            occupant,
            parent_heads.workflow.body.phase.as_str() == "review",
        )
        .expect("claimant has dependency-edit witnesses")
    } else {
        vec![
            statement_witness(parent_heads),
            workflow_witness(parent_heads),
            claim_witness(parent_heads),
        ]
    };
    expected.extend([
        statement_witness(child_heads),
        workflow_witness(child_heads),
        Witness::Edge {
            task: parent_heads.statement.body.born.clone(),
            after: child_heads.statement.body.born.clone(),
        },
    ]);
    assert_committed(issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskDepends,
        expected,
        &TaskDepends {
            task: parent.clone(),
            after: vec![child.clone()],
        },
    ));
}

fn claim_free_witnesses(
    bus: &Bus,
    epoch: CanonicalUuid,
    lane: &Actor,
    seat: &Seat,
    task: &TaskHandle,
) -> Vec<Witness> {
    let held = board(bus, epoch, lane.clone());
    let heads = at(&held, task);
    let lease = heads.lease.as_ref().expect("claimed task carries lease");
    vec![
        claim_witness(heads),
        Witness::Lease {
            task: heads.statement.body.born.clone(),
            head: lease.position,
        },
        residency(&held, seat),
    ]
}
