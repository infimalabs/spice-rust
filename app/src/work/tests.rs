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
                intent: None,
                handle: wire::TaskHandle::for_project(project, &born),
                born: born.clone(),
                queue_origin: seq(value.unsigned_abs() + 1),
                pitch: "This work keeps the fixture honest.".to_owned(),
                description: None,
                acceptance: Vec::new(),
                requirements: wire::EnvironmentRequirements::EMPTY,
                project: project.to_owned(),
                filing: wire::TaskFiling::Filed,
                priority: None,
                taste: None,
                document_slug: None,
                taste_proposal: None,
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
                status: wire::WorkflowStatus::pending(),
                deferral: None,
                generation: 0,
            },
        },
        claim: None,
        lease: None,
        explicit_release: false,
        grouped: false,
        design_active: false,
        design_cycle: wire::rooms::DesignCycleFacts::default(),
        preparation_decision: None,
        collapse_reason: None,
        controlled: Vec::new(),
        claim_context: None,
        git_context: None,
        parked_commits: None,
    }
}

/// An open Design: Design intent in its pending `design` phase.
pub(crate) fn design(value: i64, project: &str) -> BoardTask {
    let mut row = task(value, project);
    row.statement.body.intent = Some(wire::TaskIntent::Design);
    row.workflow.body.flow = vec!["design".parse().expect("fixture phase")];
    row.workflow.body.phase = "design".parse().expect("fixture phase");
    row
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
        workflow_waiting: std::collections::BTreeSet::new(),
        design_cycles: std::collections::BTreeMap::new(),
        quote_tasks: std::rc::Rc::default(),
        environment: Environment::journal_schema(1).expect("fixture schema"),
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
    terminal.workflow.body.status = wire::WorkflowStatus::completed(
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
    let workflow_waiting = task(9, "alpha.lifecycle");
    let selection = task(10, "alpha.lifecycle");
    let mut board = board(vec![
        terminal,
        in_flight,
        stale,
        deferred,
        gated_review,
        held_review,
        held,
        ready,
        workflow_waiting,
        selection,
    ]);
    // Distinct lifecycle evidence always wins if a malformed snapshot also
    // repeats those rows in the generic blocker set.
    board.blocked.extend([3, 4, 5, 9, 10].map(stamp));
    board.workflow_waiting.insert(stamp(9));
    board
        .design_cycles
        .insert(stamp(10), wire::rooms::DesignCycle::AwaitingDecision);
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
            WorkAvailability::Waiting,
            WorkAvailability::Selection,
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
        [1, 1, 1, 1, 4],
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
fn design_passivity_yields_only_to_possession_active_stance_and_selection() {
    let design = |value| design(value, "alpha.design");
    let mut held = design(1);
    held.claim = Some(claim(&held, ClaimDisposition::Active));
    let mut lapsed = design(2);
    lapsed.claim = Some(claim(&lapsed, ClaimDisposition::Lapsed));
    let mut deferred = design(3);
    deferred.workflow.body.deferral = Some(
        "2026-09-24T06:00:00.000Z"
            .parse()
            .expect("fixture deferral"),
    );
    let mut active = design(4);
    active.grouped = true;
    active.design_active = true;
    let mut active_lapsed = design(5);
    active_lapsed.grouped = true;
    active_lapsed.design_active = true;
    active_lapsed.claim = Some(claim(&active_lapsed, ClaimDisposition::Lapsed));
    let mut board = board(vec![
        held,
        lapsed,
        deferred,
        active,
        active_lapsed,
        design(6),
        design(7),
    ]);
    board.blocked.insert(stamp(6));
    board
        .design_cycles
        .insert(stamp(7), wire::rooms::DesignCycle::AwaitingDecision);
    let projection = WorkProjection::from_board(&board);

    assert_eq!(
        projection
            .tasks
            .iter()
            .map(|task| task.availability)
            .collect::<Vec<_>>(),
        [
            WorkAvailability::InFlight,
            WorkAvailability::Passive,
            WorkAvailability::Passive,
            WorkAvailability::Ready,
            WorkAvailability::Stale,
            WorkAvailability::Passive,
            WorkAvailability::Selection,
        ],
        "a passive Design waits on the operator unless possessed or awaiting selection; an active one moves"
    );
    let counts = projection.counts();
    assert_eq!([counts.passive(), counts.waiting()], [3, 4]);
    assert_eq!(
        projection
            .scope_strip(&WorkScope::planning())
            .iter()
            .map(|task| task.availability)
            .collect::<Vec<_>>(),
        [
            WorkAvailability::InFlight,
            WorkAvailability::Ready,
            WorkAvailability::Stale,
            WorkAvailability::Passive,
            WorkAvailability::Passive,
            WorkAvailability::Passive,
            WorkAvailability::Selection,
        ],
        "passive Designs stay on the Design rail after every Design an agent can move"
    );
}

#[test]
fn visibility_qualified_stems_share_counts_without_collapsing_projects() {
    let mut active = task(1, "alpha.runtime");
    active.claim = Some(claim(&active, ClaimDisposition::Active));
    let ready = task(2, "alpha.render");
    let hidden = task(3, ".alpha.audit");
    let mut terminal = task(4, "alpha.runtime");
    terminal.workflow.body.status = wire::WorkflowStatus::completed(
        "2026-09-23T06:00:00.000Z"
            .parse()
            .expect("fixture completion"),
    );
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
    complete.workflow.body.status = wire::WorkflowStatus::completed(
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
    assert_eq!(owner_label(&actor, Some(&member), None), "spice-rust-h");
    member.name = Some("builder".to_owned());
    assert_eq!(owner_label(&actor, Some(&member), None), "builder");
    assert_eq!(owner_label(&actor, None, None), "lane 11111111");
    assert_eq!(
        owner_label(&Actor::Lane(other), Some(&member), None),
        "lane 44444444",
        "a member of another lane never lends its name"
    );
    assert_eq!(owner_label(&Actor::Operator, None, None), "Operator");

    let mut held = task(1, "alpha.core");
    held.workflow.body.status = wire::WorkflowStatus::assigned(Actor::Spice);
    held.claim = Some(claim(&held, ClaimDisposition::Active));
    let mut lapsed = task(2, "alpha.core");
    lapsed.workflow.body.status = wire::WorkflowStatus::assigned(Actor::Spice);
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
#[test]
fn human_owners_read_as_their_short_name_alone() {
    let named_principal = "018f82d4-6f59-7d06-92e3-bce913116391"
        .parse::<CanonicalUuid>()
        .expect("fixture named principal");
    let unnamed_principal = "018f82d4-6f59-7d06-92e3-bce913116392"
        .parse::<CanonicalUuid>()
        .expect("fixture unnamed principal");
    let unresolved_principal = "018f82d4-6f59-7d06-92e3-bce913116393"
        .parse::<CanonicalUuid>()
        .expect("fixture unresolved principal");
    let named = wire::principal::PrincipalHead {
        principal: named_principal,
        born: seq(18_730),
        profile: wire::principal::PrincipalProfile {
            name: Some("Ada Example".to_owned()),
            ..wire::principal::PrincipalProfile::default()
        },
        roles: BTreeSet::new(),
        capability: None,
    };
    let unnamed = wire::principal::PrincipalHead {
        principal: unnamed_principal,
        born: seq(18_731),
        profile: wire::principal::PrincipalProfile::default(),
        roles: BTreeSet::new(),
        capability: None,
    };
    let unnamed_code = wire::principal::PrincipalCode::of(&unnamed);
    let principals = wire::principal::PrincipalDirectory::display(&[named, unnamed]);

    assert_eq!(
        owner_label(&Actor::Human(named_principal), None, Some(&principals)),
        "AE",
    );
    assert_eq!(
        owner_label(&Actor::Operator, None, Some(&principals)),
        "AE",
        "the historical operator shares the bootstrap principal's presentation",
    );
    assert_eq!(
        owner_label(&Actor::Human(unnamed_principal), None, Some(&principals),),
        unnamed_code.to_string(),
    );
    let unresolved = Actor::Human(unresolved_principal);
    assert_eq!(
        owner_label(&unresolved, None, Some(&principals)),
        unresolved.to_string(),
    );
    assert_eq!(owner_label(&unresolved, None, None), unresolved.to_string(),);
}

#[test]
fn a_tab_marks_only_its_partitions_most_actionable_state() {
    let counts = |blocked, held, selection, proposal_ready, deferred| WorkCounts {
        blocked,
        held,
        selection,
        proposal_ready,
        deferred,
        ..WorkCounts::default()
    };
    assert_eq!(
        counts(1, 1, 3, 4, 5).mark(),
        Some((WorkMark::Blocked, 2)),
        "work only the operator can unblock outranks a decision, a ready pass, and deferral"
    );
    assert_eq!(
        counts(0, 0, 3, 4, 5).mark(),
        Some((WorkMark::Decision, 3)),
        "a proposal awaiting decision outranks a pass ready to propose"
    );
    assert_eq!(
        counts(0, 0, 0, 4, 5).mark(),
        Some((WorkMark::Proposal, 4)),
        "a pass ready to propose outranks deferred work"
    );
    assert_eq!(counts(0, 0, 0, 0, 5).mark(), Some((WorkMark::Deferred, 5)));
    assert_eq!(counts(0, 0, 0, 0, 0).mark(), None);
}

#[test]
fn each_partition_marks_exactly_its_own_operator_transitions() {
    let mut in_flight = task(1, "alpha.quiet");
    in_flight.claim = Some(claim(&in_flight, ClaimDisposition::Active));
    let mut stale = task(2, "alpha.quiet");
    stale.claim = Some(claim(&stale, ClaimDisposition::Lapsed));
    let mut review = task(3, "alpha.quiet");
    review.workflow.body.phase = "review".parse().expect("review phase");
    let ready = task(4, "alpha.quiet");
    let workflow_waiting = task(5, "alpha.quiet");
    let mut deferred = task(6, "beta.later");
    deferred.workflow.body.deferral = Some(
        "2026-09-24T06:00:00.000Z"
            .parse()
            .expect("fixture deferral"),
    );
    let mut held = task(7, "gamma.trust");
    held.statement.body.requirements = wire::EnvironmentRequirements::one(
        EnvironmentRequirement::journal_schema(2).expect("fixture requirement"),
    );
    let blocked = task(8, "delta.wall");
    let selection = design(9, "alpha.design");
    let passive = design(10, "beta.design");
    let mut decided = design(11, "gamma.design");
    decided.grouped = true;
    decided.design_active = true;
    let proposing = design(12, "alpha.design");
    let mut claimed = design(13, "epsilon.design");
    claimed.grouped = true;
    claimed.design_active = true;
    claimed.claim = Some(claim(&claimed, ClaimDisposition::Active));
    let mut board = board(vec![
        in_flight,
        stale,
        review,
        ready,
        workflow_waiting,
        deferred,
        held,
        blocked,
        selection,
        passive,
        decided,
        proposing,
        claimed,
    ]);
    board.workflow_waiting.insert(stamp(5));
    board.blocked.insert(stamp(8));
    board
        .design_cycles
        .insert(stamp(9), wire::rooms::DesignCycle::AwaitingDecision);
    for ready in [10, 11, 12, 13] {
        board
            .design_cycles
            .insert(stamp(ready), wire::rooms::DesignCycle::ReadyForProposal);
    }
    let projection = WorkProjection::from_board(&board);
    let marks = |scope: &WorkScope| {
        projection
            .stem_task_groups(scope)
            .iter()
            .map(|group| (group.stem().stem().to_owned(), group.counts().mark()))
            .collect::<BTreeMap<_, _>>()
    };

    assert_eq!(
        marks(&WorkScope::AllOpen),
        BTreeMap::from([
            ("alpha".to_owned(), None),
            ("beta".to_owned(), Some((WorkMark::Deferred, 1))),
            ("delta".to_owned(), Some((WorkMark::Blocked, 1))),
            ("gamma".to_owned(), Some((WorkMark::Blocked, 1))),
        ]),
        "in-flight, stale, review, ready, and neutral waiting need nothing now; a blocked \
         wall, an environment hold, and work intentionally set aside mark their own stem"
    );
    assert_eq!(
        projection.scope_counts(&WorkScope::AllOpen).mark(),
        Some((WorkMark::Blocked, 2)),
        "the execution total never counts a Design"
    );
    assert_eq!(
        marks(&WorkScope::planning()),
        BTreeMap::from([
            ("alpha".to_owned(), Some((WorkMark::Decision, 1))),
            ("beta".to_owned(), Some((WorkMark::Proposal, 1))),
            ("epsilon".to_owned(), None),
            ("gamma".to_owned(), Some((WorkMark::Proposal, 1))),
        ]),
        "a Design awaiting decision outranks a ready pass in its stem; an offered or passive \
         Design ready for proposal marks its stem; one a lane already holds marks nothing"
    );
    assert_eq!(
        projection.scope_counts(&WorkScope::planning()).mark(),
        Some((WorkMark::Decision, 1)),
        "the Design total never counts execution work"
    );
    let rail = OperatorQueue::new(&projection)
        .waits()
        .iter()
        .map(|wait| {
            projection.tasks()[wait.task()]
                .row()
                .statement
                .body
                .born
                .clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        rail,
        [stamp(7), stamp(8), stamp(9)],
        "the rail lists exactly the held, blocked, and selection work the blocked and \
         decision marks count, and never the deferral or a Design ready for proposal"
    );
}

#[test]
fn every_work_scope_writes_only_its_stable_route_identity() {
    let public = WorkStemIdentity::new(WorkVisibility::Public, "alpha".to_owned());
    let hidden = WorkStemIdentity::new(WorkVisibility::Hidden, "private".to_owned());
    let team = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        .parse()
        .expect("fixture team");
    let group: wire::rooms::RoomAuthorityStamp = "dVxHkZhQY".parse().expect("fixture group");
    for (scope, route) in [
        (WorkScope::AllOpen, WorkRoute::AllOpen),
        (
            WorkScope::Stem(public.clone()),
            WorkRoute::Stem(WorkRouteStem::new(false, "alpha".to_owned()).expect("public stem")),
        ),
        (
            WorkScope::Stem(hidden.clone()),
            WorkRoute::Stem(WorkRouteStem::new(true, "private".to_owned()).expect("hidden stem")),
        ),
        (
            WorkScope::Planning { stem: None },
            WorkRoute::Planning { stem: None },
        ),
        (
            WorkScope::Planning { stem: Some(public) },
            WorkRoute::Planning {
                stem: Some(WorkRouteStem::new(false, "alpha".to_owned()).expect("planning stem")),
            },
        ),
        (WorkScope::Team { team }, WorkRoute::Team(team)),
        (
            WorkScope::Group {
                group: group.clone(),
                designs: vec![stamp(1), stamp(2)],
            },
            WorkRoute::Group(group),
        ),
    ] {
        assert_eq!(scope.route(), route);
    }
}

#[test]
fn durable_work_identities_resolve_only_after_their_mutable_authority_settles() {
    let board = board(vec![task(1, "alpha.one"), task(2, ".private.two")]);
    let pending = WorkProjection::from_board_and_lanes(&board, None);
    let public = WorkRouteStem::new(false, "alpha".to_owned()).expect("public route stem");
    let hidden = WorkRouteStem::new(true, "private".to_owned()).expect("hidden route stem");
    let missing = WorkRouteStem::new(false, "missing".to_owned()).expect("missing route stem");

    assert_eq!(
        resolve_work_route(&WorkRoute::AllOpen, None, None),
        WorkRouteResolution::Resolved(WorkScope::AllOpen)
    );
    assert_eq!(
        resolve_work_route(&WorkRoute::Planning { stem: None }, None, None,),
        WorkRouteResolution::Resolved(WorkScope::planning())
    );
    assert_eq!(
        resolve_work_route(&WorkRoute::Stem(public.clone()), None, None),
        WorkRouteResolution::Pending
    );
    assert!(matches!(
        resolve_work_route(&WorkRoute::Stem(public), Some(&pending), None),
        WorkRouteResolution::Resolved(WorkScope::Stem(_))
    ));
    assert!(matches!(
        resolve_work_route(
            &WorkRoute::Planning { stem: Some(hidden) },
            Some(&pending),
            None,
        ),
        WorkRouteResolution::Resolved(WorkScope::Planning { stem: Some(_) })
    ));
    assert_eq!(
        resolve_work_route(&WorkRoute::Stem(missing), Some(&pending), None),
        WorkRouteResolution::Missing
    );

    let team = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
        .parse()
        .expect("fixture team");
    assert_eq!(
        resolve_work_route(&WorkRoute::Team(team), Some(&pending), None),
        WorkRouteResolution::Pending
    );
    let with_team = WorkProjection::from_board_with_team_memberships(
        &board,
        Some(BTreeSet::from([team])),
        BTreeMap::new(),
    );
    assert_eq!(
        resolve_work_route(&WorkRoute::Team(team), Some(&with_team), None),
        WorkRouteResolution::Resolved(WorkScope::Team { team })
    );

    let group: wire::rooms::RoomAuthorityStamp = "dVxHkZhQY".parse().expect("fixture group");
    assert_eq!(
        resolve_work_route(&WorkRoute::Group(group.clone()), Some(&pending), None),
        WorkRouteResolution::Pending
    );
    let partition = wire::rooms::WorkingGroupPartition {
        revision: 1,
        groups: vec![wire::rooms::WorkingGroup {
            group: group.clone(),
            anchors: vec![stamp(1), stamp(2)],
            stance: wire::rooms::WorkingGroupStance::Explore,
        }],
        fast: Vec::new(),
    };
    assert_eq!(
        resolve_work_route(
            &WorkRoute::Group(group.clone()),
            Some(&pending),
            Some(&partition),
        ),
        WorkRouteResolution::Resolved(WorkScope::Group {
            group: group.clone(),
            designs: vec![stamp(1), stamp(2)],
        })
    );
    let changed_partition = wire::rooms::WorkingGroupPartition {
        revision: 2,
        groups: vec![wire::rooms::WorkingGroup {
            group: group.clone(),
            anchors: vec![stamp(2)],
            stance: wire::rooms::WorkingGroupStance::Converge,
        }],
        fast: Vec::new(),
    };
    assert_eq!(
        resolve_work_route(
            &WorkRoute::Group(group.clone()),
            Some(&pending),
            Some(&changed_partition),
        ),
        WorkRouteResolution::Resolved(WorkScope::Group {
            group: group.clone(),
            designs: vec![stamp(2)],
        }),
        "the same durable group route follows current partition membership"
    );
    assert_eq!(
        resolve_work_route(
            &WorkRoute::Group("dVxHkZhQZ".parse().expect("absent group")),
            Some(&pending),
            Some(&partition),
        ),
        WorkRouteResolution::Missing
    );
}
