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
        offer: Vec::new(),
        quote_tasks: std::rc::Rc::default(),
        resident: wire::ResidentState::Seated,
        environment: Environment::journal_schema(1).expect("fixture schema"),
    }
}

#[test]
fn lifecycle_precedence_is_terminal_then_claim_then_deferral_then_blocker() {
    let mut terminal = task(1, "alpha.lifecycle");
    terminal.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    terminal.claim = Some(claim(&terminal, ClaimDisposition::Active));

    let mut in_flight = task(2, "alpha.lifecycle");
    in_flight.workflow.body.deferral = Some(
        "2026-09-24T06:00:00.000Z"
            .parse()
            .expect("fixture deferral"),
    );
    in_flight.statement.body.requirements = wire::EnvironmentRequirements::one(
        EnvironmentRequirement::journal_schema(2).expect("fixture requirement"),
    );
    in_flight.claim = Some(claim(&in_flight, ClaimDisposition::Active));

    let mut deferred = task(3, "alpha.lifecycle");
    deferred.workflow.body.deferral = Some(
        "2026-09-24T06:00:00.000Z"
            .parse()
            .expect("fixture deferral"),
    );
    deferred.statement.body.requirements = wire::EnvironmentRequirements::one(
        EnvironmentRequirement::journal_schema(2).expect("fixture requirement"),
    );

    let mut blocked = task(4, "alpha.lifecycle");
    blocked.statement.body.requirements = wire::EnvironmentRequirements::one(
        EnvironmentRequirement::journal_schema(2).expect("fixture requirement"),
    );

    let ready = task(5, "alpha.lifecycle");
    let board = board(vec![terminal, in_flight, deferred, blocked, ready]);
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
            WorkAvailability::Deferred,
            WorkAvailability::Blocked,
            WorkAvailability::Ready,
        ]
    );
    assert_eq!(projection.tasks[0].working_claimant(), None);
    assert_eq!(
        projection.tasks[1].working_claimant(),
        Some(&Actor::Operator),
    );
    assert!(projection.tasks[1].environment_blocked);
    assert!(projection.tasks[2].environment_blocked);
    assert!(projection.tasks[3].environment_blocked);
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
    assert_eq!(projection.stems[1].counts.ready, 1);
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
    assert_eq!(
        projection.scope_for_project("alpha.runtime"),
        Some(public_scope.clone()),
    );
    assert_eq!(
        projection.scope_for_project(".alpha.audit"),
        Some(hidden_scope.clone()),
    );
    assert_eq!(projection.scope_for_project("gone.project"), None);

    let absent = WorkScope::Stem(WorkStemIdentity {
        visibility: WorkVisibility::Public,
        stem: "gone".to_owned(),
    });
    assert_eq!(projection.contains_scope(&absent), Some(false));
    assert_eq!(projection.scope_counts(&absent), projection.counts());
}

#[test]
fn a_count_badge_wears_its_slowest_open_lifecycle() {
    use WorkAvailability::{Blocked, Deferred, InFlight, Ready, Terminal};
    let slowest = |availabilities: &[WorkAvailability]| {
        availabilities
            .iter()
            .copied()
            .collect::<WorkCounts>()
            .slowest()
    };
    assert_eq!(slowest(&[Terminal]), None);
    assert_eq!(slowest(&[InFlight, Terminal]), Some(InFlight));
    assert_eq!(slowest(&[InFlight, Ready]), Some(Ready));
    assert_eq!(slowest(&[Ready, Blocked, InFlight]), Some(Blocked));
    assert_eq!(slowest(&[Blocked, Deferred, Ready]), Some(Deferred));
}

#[test]
fn opening_a_task_keeps_a_scope_that_shows_it_and_widens_one_that_hides_it() {
    let public = task(1, "alpha.runtime");
    let hidden = task(2, ".alpha.audit");
    let public_born = public.statement.body.born.clone();
    let hidden_born = hidden.statement.body.born.clone();
    let board = board(vec![public, hidden]);
    let projection = WorkProjection::from_board(&board);
    let public_scope = WorkScope::Stem(projection.stems[0].identity.clone());
    let hidden_scope = WorkScope::Stem(projection.stems[1].identity.clone());

    assert_eq!(
        projection.scope_showing(&public_born, &public_scope),
        public_scope
    );
    assert_eq!(
        projection.scope_showing(&hidden_born, &hidden_scope),
        hidden_scope
    );
    assert_eq!(
        projection.scope_showing(&hidden_born, &public_scope),
        WorkScope::AllOpen
    );
    let absent = task(3, "alpha.runtime").statement.body.born;
    assert_eq!(
        projection.scope_showing(&absent, &public_scope),
        WorkScope::AllOpen
    );
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
    let missing = stamp(99);
    let mut blocked = task(3, "graph.core");
    blocked.statement.body.dependencies = vec![
        complete.statement.body.born.clone(),
        prerequisite.statement.body.born.clone(),
        missing.clone(),
    ];
    let mut freed = task(4, "graph.core");
    freed.claim = Some(claim(&freed, ClaimDisposition::Freed));

    let board = board(vec![complete, prerequisite, blocked, freed]);
    let projection = WorkProjection::from_board(&board);
    assert_eq!(projection.tasks[1].availability, WorkAvailability::Ready);
    assert_eq!(projection.tasks[2].availability, WorkAvailability::Blocked);
    assert_eq!(projection.tasks[3].availability, WorkAvailability::Ready);
    assert_eq!(
        projection.tasks[2]
            .blocked_dependencies()
            .map(|dependency| (dependency.born, dependency.task))
            .collect::<Vec<_>>(),
        [
            (&projection.tasks[1].row.statement.body.born, Some(1)),
            (&missing, None),
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
            WorkAvailability::Ready,
            WorkAvailability::Blocked,
            WorkAvailability::Terminal,
        ]
    );
    assert_eq!(lane.tasks, [1, 3, 2, 0]);
    assert_eq!(projection.tasks[2].project, 0);
    assert_eq!(lane.counts.unavailable(), 1);
}

const OPEN_LANES: [WorkAvailability; 4] = [
    WorkAvailability::InFlight,
    WorkAvailability::Ready,
    WorkAvailability::Blocked,
    WorkAvailability::Deferred,
];

fn titles<'a>(clusters: &[WorkCluster<'_, 'a>]) -> Vec<&'a str> {
    clusters
        .iter()
        .flat_map(|cluster| &cluster.tasks)
        .map(|task| task.row().presentation_title.as_str())
        .collect()
}

fn arrangement(order: WorkOrder, clustering: WorkClustering) -> WorkArrangement {
    WorkArrangement { order, clustering }
}

#[test]
fn one_board_follows_the_resident_offer_and_clusters_by_first_card() {
    let mut active = task(6, "beta.core");
    active.claim = Some(claim(&active, ClaimDisposition::Active));
    let mut done = task(1, "alpha.render");
    done.workflow.body.completion = Some(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
    let mut assigned = task(2, "beta.core");
    assigned.workflow.body.assignment = Some(Actor::Spice);
    // Board rows arrive in neither offer, time, nor project order.
    let mut board = board(vec![
        task(9, "alpha.render"),
        task(7, "beta.core"),
        active,
        task(4, ".alpha.audit"),
        done,
        task(5, "alpha.render"),
        assigned,
    ]);
    // The resident offers no hidden work; one stale stamp is ignored.
    board.offer = [7, 5, 99, 9, 2].map(stamp).to_vec();
    let projection = WorkProjection::from_board(&board);
    let all = WorkScope::AllOpen;
    let ready = |order, clustering| {
        projection.lane(
            WorkAvailability::Ready,
            &all,
            arrangement(order, clustering),
        )
    };

    let offered = ready(WorkOrder::Offer, WorkClustering::None);
    assert!(matches!(offered.as_slice(), [lane] if lane.key == WorkClusterKey::Lane));
    assert_eq!(
        titles(&offered),
        ["task 7", "task 5", "task 9", "task 2", "task 4"],
        "the next offer leads every project; unoffered work follows in queue order"
    );
    assert_eq!(
        titles(&projection.lane(WorkAvailability::Ready, &all, WorkArrangement::default())),
        titles(&offered),
        "the offer order is the default arrangement"
    );
    assert_eq!(
        titles(&ready(WorkOrder::Newest, WorkClustering::None)),
        ["task 9", "task 7", "task 5", "task 4", "task 2"]
    );

    let by_project = ready(WorkOrder::Offer, WorkClustering::Project);
    assert_eq!(
        by_project
            .iter()
            .map(|cluster| cluster.key)
            .collect::<Vec<_>>(),
        [
            WorkClusterKey::Project("beta.core"),
            WorkClusterKey::Project("alpha.render"),
            WorkClusterKey::Project(".alpha.audit"),
        ],
        "clusters follow their first card, so the next offer still leads"
    );
    assert_eq!(
        titles(&by_project),
        ["task 7", "task 2", "task 5", "task 9", "task 4"]
    );
    assert_eq!(
        titles(&ready(WorkOrder::Newest, WorkClustering::Project)),
        ["task 9", "task 5", "task 7", "task 2", "task 4"]
    );
    for order in [WorkOrder::Offer, WorkOrder::Newest] {
        for clustering in [
            WorkClustering::None,
            WorkClustering::Project,
            WorkClustering::Phase,
        ] {
            let mut placed = OPEN_LANES
                .into_iter()
                .flat_map(|lane| {
                    titles(&projection.lane(lane, &all, arrangement(order, clustering)))
                })
                .collect::<Vec<_>>();
            placed.sort_unstable();
            assert_eq!(
                placed,
                ["task 2", "task 4", "task 5", "task 6", "task 7", "task 9"],
                "{order:?}/{clustering:?} places every open task in exactly one shared lane"
            );
        }
    }
    assert_eq!(
        titles(&projection.lane(WorkAvailability::InFlight, &all, WorkArrangement::default())),
        ["task 6"]
    );

    let public_alpha = WorkScope::Stem(projection.stems[0].identity.clone());
    assert_eq!(
        titles(&projection.lane(
            WorkAvailability::Ready,
            &public_alpha,
            WorkArrangement::default()
        )),
        ["task 5", "task 9"],
        "a stem scope narrows the one board without splitting it"
    );
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
        seat_fast_mode: None,
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
