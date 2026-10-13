use super::*;
use wire::intent::{BoardStatement, Positioned};
use wire::{Actor, CanonicalUuid, ClaimCell, ClaimHead, Environment, EnvironmentRequirement};

fn seq(value: u64) -> wire::Seq {
    wire::Seq::new(value).expect("fixture sequence is positive")
}

pub(crate) fn stamp(value: i64) -> TaskStamp {
    TaskStamp::for_micros(value).expect("fixture stamp is representable")
}

pub(crate) fn task(value: i64, project: &str) -> BoardTask {
    let born = stamp(value);
    BoardTask {
        visibility: if project.starts_with('.') {
            ProjectVisibility::Hidden {
                stem: project
                    .trim_start_matches('.')
                    .split('.')
                    .next()
                    .expect("hidden fixture has a stem")
                    .to_owned(),
            }
        } else {
            ProjectVisibility::Public
        },
        origin_thread: "operator".to_owned(),
        creator_team: None,
        presentation_title: format!("task {value}"),
        statement: Positioned {
            position: seq(1),
            body: BoardStatement {
                preparation: None,
                authorization: None,
                engagement: None,
                evidence: Vec::new(),
                intent: None,
                handle: wire::TaskHandle::for_project(project, &born),
                born: born.clone(),
                queue_origin: seq(value.unsigned_abs() + 1),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::EMPTY,
                project: project.to_owned(),
                tags: Vec::new(),
                dependencies: Vec::new(),
                provenance: format!("task:{born}").parse().expect("fixture origin"),
                creation: wire::body::TaskCreationContext::unavailable(
                    wire::body::TaskCreationUnavailable::OperatorActor,
                ),
                revision: seq(1),
            },
        },
        workflow: Positioned {
            position: seq(2),
            body: wire::WorkflowHead {
                flow: vec!["todo".parse().expect("fixture phase")],
                phase: "todo".parse().expect("fixture phase"),
                status: "pending".to_owned(),
                review: "unassigned".to_owned(),
                assignment: None,
                deferral: None,
                completion: None,
                generation: 0,
            },
        },
        claim: None,
        lease: None,
        explicit_release: false,
        claim_context: None,
        git_context: None,
        parked_commits: None,
    }
}

pub(crate) fn claim(task: &BoardTask, disposition: ClaimDisposition) -> Positioned<ClaimHead> {
    Positioned {
        position: seq(3),
        body: ClaimHead {
            actor: Actor::Operator,
            fact: seq(3),
            claimed: seq(3),
            cell: ClaimCell {
                project: task.statement.body.project.clone(),
                phase: task.workflow.body.phase.clone(),
            },
            lane: "11111111-1111-4111-8111-111111111111"
                .parse::<CanonicalUuid>()
                .expect("fixture lane"),
            occupant: "22222222-2222-4222-8222-222222222222"
                .parse::<CanonicalUuid>()
                .expect("fixture occupant"),
            epoch: "33333333-3333-4333-8333-333333333333"
                .parse::<CanonicalUuid>()
                .expect("fixture epoch"),
            recovery: None,
            disposition,
            acknowledged: 0,
        },
    }
}

pub(crate) fn board(tasks: Vec<BoardTask>) -> BoardView {
    BoardView {
        schema: 1,
        cursor: seq(100),
        tasks,
        room_anchors: Vec::new(),
        offer: Vec::new(),
        blocked: std::collections::BTreeSet::new(),
        quote_tasks: std::rc::Rc::default(),
        environment: Environment::journal_schema(1).expect("fixture schema"),
        seats: crate::runtime::Seats::new(residency(), std::collections::BTreeMap::new()),
    }
}

/// A seated resident with no lanes.
pub(crate) fn residency() -> wire::ResidencyHead {
    wire::ResidencyHead {
        epoch: "33333333-3333-4333-8333-333333333333"
            .parse::<CanonicalUuid>()
            .expect("fixture epoch"),
        state: wire::ResidentState::Seated,
        topology_revision: 0,
        lanes: Vec::new(),
        subscriptions: Vec::new(),
        renewals: Vec::new(),
    }
}

#[test]
fn lifecycle_precedence_follows_task_status_standings() {
    let deferral = || {
        Some(
            "2026-09-24T06:00:00.000Z"
                .parse()
                .expect("fixture deferral"),
        )
    };
    let upgrade = || {
        wire::EnvironmentRequirements::one(
            EnvironmentRequirement::journal_schema(2).expect("fixture requirement"),
        )
    };
    let review = || "review".parse().expect("review phase");

    let mut terminal = task(1, "alpha.lifecycle");
    terminal.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    terminal.claim = Some(claim(&terminal, ClaimDisposition::Active));

    let mut in_flight = task(2, "alpha.lifecycle");
    in_flight.workflow.body.deferral = deferral();
    in_flight.statement.body.requirements = upgrade();
    in_flight.claim = Some(claim(&in_flight, ClaimDisposition::Active));

    let mut stale = task(3, "alpha.lifecycle");
    stale.workflow.body.deferral = deferral();
    stale.claim = Some(claim(&stale, ClaimDisposition::Lapsed));

    let mut deferred = task(4, "alpha.lifecycle");
    deferred.workflow.body.deferral = deferral();
    deferred.statement.body.requirements = upgrade();

    let mut gated_review = task(5, "alpha.lifecycle");
    gated_review.workflow.body.phase = review();
    gated_review.statement.body.requirements = upgrade();

    let mut held_review = task(6, "alpha.lifecycle");
    held_review.workflow.body.phase = review();
    held_review.statement.body.requirements = upgrade();

    let mut held = task(7, "alpha.lifecycle");
    held.statement.body.requirements = upgrade();

    let ready = task(8, "alpha.lifecycle");
    let mut board = board(vec![
        terminal,
        in_flight,
        stale,
        deferred,
        gated_review,
        held_review,
        held,
        ready,
    ]);
    // The resident's gate holds back the stale, deferred, and gated rows.
    board.blocked.extend([3, 4, 5].map(stamp));
    let projection = WorkProjection::from_board(&board);

    assert_eq!(
        projection
            .tasks
            .iter()
            .map(|task| task.availability)
            .collect::<Vec<_>>(),
        [
            WorkAvailability::Terminal,
            WorkAvailability::InFlight,
            WorkAvailability::Stale,
            WorkAvailability::Deferred,
            WorkAvailability::Blocked,
            WorkAvailability::Review,
            WorkAvailability::Held,
            WorkAvailability::Ready,
        ]
    );
    let counts = projection.counts();
    assert_eq!(
        [
            counts.ready(),
            counts.review(),
            counts.stale(),
            counts.blocked(),
            counts.waiting(),
        ],
        [1, 1, 1, 1, 2],
        "each open category counts exactly its own rows"
    );
    assert_eq!(projection.tasks[0].working_claimant(), None);
    assert_eq!(
        projection.tasks[1].working_claimant(),
        Some(&Actor::Operator),
    );
    assert!(projection.tasks[1].environment_blocked);
    assert!(projection.tasks[6].environment_blocked);
}

#[test]
fn visibility_qualified_stems_share_counts_without_collapsing_projects() {
    let mut active = task(1, "alpha.runtime");
    active.claim = Some(claim(&active, ClaimDisposition::Active));
    let ready = task(2, "alpha.render");
    let hidden = task(3, ".alpha.audit");
    let mut terminal = task(4, "alpha.runtime");
    terminal.workflow.body.status = "completed".to_owned();
    let board = board(vec![active, ready, hidden, terminal]);
    let projection = WorkProjection::from_board(&board);

    assert_eq!(projection.stems.len(), 2);
    assert_eq!(
        projection
            .stems
            .iter()
            .map(|stem| (&stem.identity.visibility, stem.identity.stem.as_str()))
            .collect::<Vec<_>>(),
        [
            (&WorkVisibility::Public, "alpha"),
            (&WorkVisibility::Hidden, "alpha"),
        ]
    );
    assert!(!projection.stems[0].identity.hidden());
    assert!(projection.stems[1].identity.hidden());
    assert_eq!(
        projection.stems[0].counts,
        WorkCounts {
            in_flight: 1,
            ready: 1,
            terminal: 1,
            ..WorkCounts::default()
        }
    );
    assert_eq!(
        projection.stems[1].counts,
        WorkCounts {
            parked: 1,
            ..WorkCounts::default()
        },
        "the allocator never offers a hidden row"
    );
    assert_eq!(
        projection.stems[0]
            .projects
            .iter()
            .map(|&index| projection.projects[index].project)
            .collect::<Vec<_>>(),
        ["alpha.render", "alpha.runtime"]
    );
    assert!(projection.stems[0].counts.has_open());
    assert!(
        !WorkCounts {
            terminal: 1,
            ..WorkCounts::default()
        }
        .has_open()
    );
    let public_scope = WorkScope::Stem(projection.stems[0].identity.clone());
    let hidden_scope = WorkScope::Stem(projection.stems[1].identity.clone());
    assert_eq!(
        public_scope.stem().map(ToString::to_string).as_deref(),
        Some("alpha.*")
    );
    assert_eq!(
        hidden_scope.stem().map(ToString::to_string).as_deref(),
        Some(".alpha.*")
    );
    assert_eq!(projection.contains_scope(&public_scope), Some(true));
    assert_eq!(projection.contains_scope(&hidden_scope), Some(true));
    assert_eq!(projection.scope_counts(&public_scope).open(), 2);
    assert_eq!(projection.scope_counts(&hidden_scope).open(), 1);

    let absent = WorkScope::Stem(WorkStemIdentity {
        visibility: WorkVisibility::Public,
        stem: "gone".to_owned(),
    });
    assert_eq!(projection.contains_scope(&absent), Some(false));
    assert_eq!(projection.scope_counts(&absent), projection.counts());
}

#[test]
fn relationships_claim_dispositions_and_ranking_are_deterministic() {
    let mut complete = task(1, "graph.core");
    complete.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    let mut prerequisite = task(2, "graph.core");
    prerequisite.claim = Some(claim(&prerequisite, ClaimDisposition::Lapsed));
    // The open board omits only terminal rows, so an absent prerequisite settled.
    let settled = stamp(99);
    let mut blocked = task(3, "graph.core");
    blocked.statement.body.dependencies = vec![
        complete.statement.body.born.clone(),
        prerequisite.statement.body.born.clone(),
        settled.clone(),
    ];
    let mut freed = task(4, "graph.core");
    freed.claim = Some(claim(&freed, ClaimDisposition::Freed));

    let mut board = board(vec![complete, prerequisite, blocked, freed]);
    board
        .blocked
        .insert(board.tasks[2].statement.body.born.clone());
    let projection = WorkProjection::from_board(&board);
    assert_eq!(projection.tasks[1].availability, WorkAvailability::Stale);
    assert_eq!(projection.tasks[2].availability, WorkAvailability::Blocked);
    assert_eq!(projection.tasks[3].availability, WorkAvailability::Ready);
    assert_eq!(
        projection.tasks[2]
            .dependencies
            .iter()
            .map(|dependency| (dependency.born, dependency.task, dependency.satisfied))
            .collect::<Vec<_>>(),
        [
            (&projection.tasks[0].row.statement.body.born, Some(0), true),
            (&projection.tasks[1].row.statement.body.born, Some(1), false),
            (&settled, None, true),
        ]
    );
    assert_eq!(projection.tasks[1].dependents, [WorkDependent { task: 2 }]);

    let lane = &projection.projects[0];
    assert_eq!(lane.project, "graph.core");
    assert_eq!(lane.stem, 0);
    assert_eq!(
        lane.tasks
            .iter()
            .map(|&index| projection.tasks[index].availability)
            .collect::<Vec<_>>(),
        [
            WorkAvailability::Ready,
            WorkAvailability::Stale,
            WorkAvailability::Blocked,
            WorkAvailability::Terminal,
        ]
    );
    assert_eq!(lane.tasks, [3, 1, 2, 0]);
    assert_eq!(projection.tasks[2].project, 0);
    assert_eq!(lane.counts.blocked(), 1);
}

#[test]
fn owners_read_as_agent_names_and_unknown_lanes_stay_distinguishable() {
    let lane = "11111111-1111-4111-8111-111111111111"
        .parse::<CanonicalUuid>()
        .expect("fixture lane");
    let other = "44444444-4444-4444-8444-444444444444"
        .parse::<CanonicalUuid>()
        .expect("fixture lane");
    let mut member = LaneMember {
        lane,
        name: None,
        worktree: "/work/spice-rust-h".to_owned(),
        speech: wire::SpeechPlayback::Disabled {},
        life: "steer".to_owned(),
        vendor: "omp".parse().expect("fixture vendor"),
        model: None,
        seat_model: None,
        effort: None,
        seat_effort: None,
        fast_mode: false,
        seat_fast_mode: false,
        occupant: None,
        session: None,
        disposition: "seated".to_owned(),
        liveness: wire::intent::LaneLiveness::Working,
        bound: true,
        wakeable: true,
        jobs: Vec::new(),
    };
    let actor = Actor::Lane(lane);
    assert_eq!(owner_label(&actor, Some(&member)), "spice-rust-h");
    member.name = Some("builder".to_owned());
    assert_eq!(owner_label(&actor, Some(&member)), "builder");
    assert_eq!(owner_label(&actor, None), "lane 11111111");
    assert_eq!(
        owner_label(&Actor::Lane(other), Some(&member)),
        "lane 44444444",
        "a member of another lane never lends its name"
    );
    assert_eq!(owner_label(&Actor::Operator, None), "Operator");

    let mut held = task(1, "alpha.core");
    held.workflow.body.assignment = Some(Actor::Spice);
    held.claim = Some(claim(&held, ClaimDisposition::Active));
    let mut lapsed = task(2, "alpha.core");
    lapsed.workflow.body.assignment = Some(Actor::Spice);
    lapsed.claim = Some(claim(&lapsed, ClaimDisposition::Lapsed));
    let board = board(vec![held, lapsed, task(3, "alpha.core")]);
    let projection = WorkProjection::from_board(&board);
    assert_eq!(projection.tasks[0].owner(), Some(&Actor::Operator));
    assert_eq!(projection.tasks[1].owner(), Some(&Actor::Spice));
    assert_eq!(projection.tasks[2].owner(), None);
    assert_eq!(
        projection.tasks[0].working_claimant(),
        Some(&Actor::Operator),
    );
    assert_eq!(projection.tasks[1].working_claimant(), None);
    assert_eq!(projection.tasks[2].working_claimant(), None);
}
