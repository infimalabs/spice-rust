use super::*;
use wire::intent::{
    TaskLifecycleKind, TaskRelationKind, TaskRelationNode, TaskRelationNodeRef, TaskRelations,
};

fn read_relations(
    bus: &Bus,
    epoch: CanonicalUuid,
    actor: Actor,
    request: &TaskRelations,
) -> Outcome<CommandAnswer> {
    issue(
        bus,
        epoch,
        actor,
        CommandKind::TaskRelations,
        Vec::new(),
        request,
    )
}

fn answer(outcome: Outcome<CommandAnswer>) -> (wire::Seq, Box<wire::intent::TaskRelationsAnswer>) {
    let Outcome::Reply(Reply {
        position: ReplyPosition::Snapshot(position),
        body: CommandAnswer::TaskRelations(answer),
    }) = outcome
    else {
        panic!("task relations did not return a snapshot: {outcome:?}")
    };
    assert_eq!(position, answer.snapshot);
    (position, answer)
}

fn related_task(bus: &Bus, epoch: CanonicalUuid, parent: &TaskHandle, name: &str) -> TaskHandle {
    let born = stamp(name);
    let task = TaskHandle::for_project("board.relations", &born);
    let parent_stamp = parent
        .born()
        .expect("fixture parent handle carries a birth stamp");
    assert_committed(issue(
        bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskAdd,
        vec![Witness::Absence { task: born }],
        &TaskAdd {
            preparation: None,
            intent: None,
            title: "Project typed task relationships".to_owned(),
            pitch: "This work keeps the fixture honest.".to_owned(),
            project: Some("board.relations".to_owned()),
            description: None,
            acceptance: Vec::new(),
            requirements: wire::EnvironmentRequirements::default(),
            origin: Some(task_origin(&parent_stamp).expect("fixture task origin")),
            flow: Some("todo".to_owned()),
            depends: vec![parent.clone()],
            claim: false,
            draft: false,
        },
    ));
    task
}

#[test]
fn connected_lane_reads_stable_origin_dependency_lifecycle_and_live_claim_nodes() {
    let (root, bus, epoch, seats) = fixture("task-relations-projection", 1);
    let lane = Actor::Lane(seats[0].lane);
    let parent = todo_task(&bus, epoch, "REL-parent001", "Hold the relationship parent");
    claim_task(&bus, epoch, &seats[0], &lane, &parent);
    let task = related_task(&bus, epoch, &parent, "REL-child0001");

    let before_edit = board(&bus, epoch, Actor::Operator);
    let heads = at(&before_edit, &task);
    assert_committed(issue(
        &bus,
        epoch,
        Actor::Operator,
        CommandKind::TaskDepends,
        vec![
            statement_witness(heads),
            workflow_witness(heads),
            claim_witness(heads),
        ],
        &TaskDepends {
            task: task.clone(),
            after: vec![parent.clone()],
            remove: false,
        },
    ));

    let task_stamp = task.born().expect("fixture task stamp");
    let parent_stamp = parent.born().expect("fixture parent stamp");
    let (_, projected) = answer(read_relations(
        &bus,
        epoch,
        lane.clone(),
        &TaskRelations {
            roots: vec![task_stamp.clone()],
            relations: vec![
                TaskRelationKind::Origin,
                TaskRelationKind::Dependency,
                TaskRelationKind::Lifecycle,
                TaskRelationKind::Claim,
            ],
            depth: 2,
            node_limit: 64,
        },
    ));

    assert_eq!(
        projected.roots.as_slice(),
        std::slice::from_ref(&task_stamp)
    );
    assert!(projected.truncation.is_none());
    assert!(
        projected
            .nodes
            .windows(2)
            .all(|nodes| { nodes[0].identity() < nodes[1].identity() })
    );
    assert!(projected.edges.windows(2).all(|edges| edges[0] < edges[1]));

    let child = TaskRelationNodeRef::Task {
        task: task_stamp.clone(),
    };
    let parent_ref = TaskRelationNodeRef::Task {
        task: parent_stamp.clone(),
    };
    for relation in [TaskRelationKind::Origin, TaskRelationKind::Dependency] {
        assert!(projected.edges.iter().any(|edge| {
            edge.relation == relation && edge.from == child && edge.to == parent_ref
        }));
    }

    let mut lifecycle = projected.nodes.iter().filter_map(|node| match node {
        TaskRelationNode::Lifecycle {
            task,
            position,
            event,
            ..
        } if task == &task_stamp => Some((*position, *event)),
        _ => None,
    });
    let added = lifecycle.next().expect("task-added lifecycle node");
    let edited = lifecycle.next().expect("task-edited lifecycle node");
    assert_eq!(added.1, TaskLifecycleKind::Added);
    assert_eq!(edited.1, TaskLifecycleKind::Edited);
    assert!(added.0 < edited.0);
    assert!(lifecycle.next().is_none());

    let claim = projected
        .nodes
        .iter()
        .find(|node| matches!(node, TaskRelationNode::Claim { task, .. } if task == &parent_stamp))
        .expect("reachable parent exposes its active claim");
    let TaskRelationNode::Claim {
        actor,
        lane: claim_lane,
        occupant,
        disposition,
        ..
    } = claim
    else {
        unreachable!()
    };
    assert_eq!(actor, &lane);
    assert_eq!(*claim_lane, seats[0].lane);
    assert_eq!(*occupant, seats[0].occupant);
    assert_eq!(*disposition, ClaimDisposition::Active);

    clean(&root, bus);
}

#[test]
fn origin_depth_two_includes_parent_and_siblings_without_reversing_semantics() {
    let (root, bus, epoch, _) = fixture("task-relations-origin-siblings", 0);
    let parent = todo_task(&bus, epoch, "REL-originpar1", "Origin parent");
    let selected = related_task(&bus, epoch, &parent, "REL-originone1");
    let sibling = related_task(&bus, epoch, &parent, "REL-origintwo1");
    let selected = selected.born().expect("fixture selected stamp");
    let parent = parent.born().expect("fixture parent stamp");
    let sibling = sibling.born().expect("fixture sibling stamp");

    let (_, projected) = answer(read_relations(
        &bus,
        epoch,
        Actor::Operator,
        &TaskRelations {
            roots: vec![selected.clone()],
            relations: vec![TaskRelationKind::Origin],
            depth: 2,
            node_limit: 16,
        },
    ));

    for child in [&selected, &sibling] {
        assert!(projected.edges.iter().any(|edge| {
            edge.relation == TaskRelationKind::Origin
                && edge.from
                    == (TaskRelationNodeRef::Task {
                        task: child.clone(),
                    })
                && edge.to
                    == (TaskRelationNodeRef::Task {
                        task: parent.clone(),
                    })
        }));
    }

    clean(&root, bus);
}

#[test]
fn depth_and_node_bounds_name_the_exact_stable_frontier() {
    let (root, bus, epoch, _) = fixture("task-relations-frontier", 0);
    let parent = todo_task(&bus, epoch, "REL-frontpar1", "Frontier parent");
    let task = related_task(&bus, epoch, &parent, "REL-frontkid1");
    let task_stamp = task.born().expect("fixture task stamp");
    let parent_stamp = parent.born().expect("fixture parent stamp");

    for (depth, node_limit) in [(0, 64), (1, 1)] {
        let (_, projected) = answer(read_relations(
            &bus,
            epoch,
            Actor::Operator,
            &TaskRelations {
                roots: vec![task_stamp.clone()],
                relations: vec![TaskRelationKind::Dependency],
                depth,
                node_limit,
            },
        ));

        assert_eq!(projected.nodes.len(), 1);
        assert!(projected.edges.is_empty());
        let truncation = projected.truncation.expect("bound is explicit");
        assert_eq!(truncation.omitted, 0);
        assert_eq!(truncation.frontier.len(), 1);
        assert_eq!(truncation.frontier[0].depth, 1);
        let edge = &truncation.frontier[0].edge;
        assert_eq!(edge.relation, TaskRelationKind::Dependency);
        let TaskRelationNodeRef::Task { task: from } = &edge.from else {
            panic!("dependency frontier source is not a task")
        };
        let TaskRelationNodeRef::Task { task: to } = &edge.to else {
            panic!("dependency frontier target is not a task")
        };
        assert_eq!(from, &task_stamp);
        assert_eq!(to, &parent_stamp);
    }

    clean(&root, bus);
}

#[test]
fn malformed_excessive_and_unknown_requests_refuse_with_stable_reasons() {
    let (root, bus, epoch, _) = fixture("task-relations-refusal", 0);
    let task = todo_task(&bus, epoch, "REL-bounds001", "Bound relationship reads");
    let task = task.born().expect("fixture task stamp");
    let cases = [
        (
            TaskRelations {
                roots: Vec::new(),
                relations: vec![TaskRelationKind::Origin],
                depth: 0,
                node_limit: 1,
            },
            "task::relations-roots-invalid",
        ),
        (
            TaskRelations {
                roots: vec![task.clone(), task.clone()],
                relations: vec![TaskRelationKind::Origin],
                depth: 0,
                node_limit: 2,
            },
            "task::relations-roots-duplicate",
        ),
        (
            TaskRelations {
                roots: vec![task.clone()],
                relations: vec![TaskRelationKind::Origin, TaskRelationKind::Origin],
                depth: 0,
                node_limit: 1,
            },
            "task::relations-kinds-invalid",
        ),
        (
            TaskRelations {
                roots: vec![task.clone()],
                relations: vec![TaskRelationKind::Origin],
                depth: wire::intent::TASK_RELATION_DEPTH_LIMIT + 1,
                node_limit: 1,
            },
            "task::relations-depth-excessive",
        ),
        (
            TaskRelations {
                roots: vec![task.clone()],
                relations: vec![TaskRelationKind::Origin],
                depth: 0,
                node_limit: wire::intent::TASK_RELATION_NODE_LIMIT + 1,
            },
            "task::relations-node-limit-invalid",
        ),
        (
            TaskRelations {
                roots: vec![stamp("REL-unknown01")],
                relations: vec![TaskRelationKind::Origin],
                depth: 0,
                node_limit: 1,
            },
            "task::relations-unknown",
        ),
    ];

    for (request, expected) in cases {
        let outcome = read_relations(&bus, epoch, Actor::Operator, &request);
        assert!(
            matches!(&outcome, Outcome::Refusal(refusal) if refusal.reason() == expected),
            "request should refuse as {expected}: {outcome:?}",
        );
    }

    clean(&root, bus);
}

#[test]
fn completed_review_preserves_phase_author_and_reviewer_identity() {
    let (root, bus, epoch, seats) = fixture("task-relations-review", 2);
    authorize_lane_creation(&bus, epoch, &seats[1]);
    let author = Actor::Lane(seats[0].lane);
    let reviewer = Actor::Lane(seats[1].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "REL-review001",
        "Project a durable review handoff",
        None,
    );
    advance_task_to_review(&bus, epoch, &seats[0], &author, &task);
    complete_review_with_finding(&bus, epoch, &seats[1], &reviewer, &task);
    let task = task.born().expect("fixture task stamp");
    let (_, projected) = answer(read_relations(
        &bus,
        epoch,
        Actor::Operator,
        &TaskRelations {
            roots: vec![task.clone()],
            relations: vec![TaskRelationKind::Review],
            depth: 1,
            node_limit: 8,
        },
    ));

    let reviews = projected
        .nodes
        .iter()
        .filter_map(|node| match node {
            TaskRelationNode::Review {
                task: reviewed,
                phase,
                author,
                reviewer,
                verdict,
                ..
            } => Some((reviewed, phase, author, reviewer, verdict)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(reviews.len(), 1);
    assert_eq!(reviews[0].0, &task);
    assert_eq!(reviews[0].1.as_ref().map(wire::Phase::as_str), Some("todo"));
    assert_eq!(reviews[0].2.as_ref(), Some(&author));
    assert_eq!(reviews[0].3, &reviewer);
    assert_eq!(*reviews[0].4, body::Verdict::Finding);

    clean(&root, bus);
}

#[test]
fn a_review_no_completed_phase_precedes_projects_without_phase_or_author() {
    let (root, bus, epoch, seats) = fixture("relations-review-first", 2);
    let reviewer = Actor::Lane(seats[1].lane);
    let task = add(
        &bus,
        epoch,
        Actor::Operator,
        "REL-review002",
        "Review an operator-staged change",
        Some("review"),
    );
    let reviewable = board(&bus, epoch, reviewer.clone());
    assert_committed(issue(
        &bus,
        epoch,
        reviewer.clone(),
        CommandKind::ClaimWin,
        claim_expected(&reviewable, &task, &seats[1]),
        &ClaimWin {
            rule: "next".parse().expect("allocator rule"),
            recovery: None,
        },
    ));
    let assigned = board(&bus, epoch, reviewer.clone());
    let expected = progress_witnesses(
        &assigned,
        at(&assigned, &task),
        seats[1].lane,
        seats[1].occupant,
        true,
    )
    .expect("assigned review has exact witnesses");
    assert_committed(issue(
        &bus,
        epoch,
        reviewer.clone(),
        CommandKind::TaskReview,
        expected,
        &TaskReview {
            task: task.clone(),
            finding: "clean".to_owned(),
            note: None,
            then: Vec::new(),
            followup: Vec::new(),
        },
    ));
    let task = task.born().expect("fixture task stamp");
    let (_, projected) = answer(read_relations(
        &bus,
        epoch,
        Actor::Operator,
        &TaskRelations {
            roots: vec![task.clone()],
            relations: vec![TaskRelationKind::Review],
            depth: 1,
            node_limit: 8,
        },
    ));

    let reviews = projected
        .nodes
        .iter()
        .filter_map(|node| match node {
            TaskRelationNode::Review {
                task: reviewed,
                phase,
                author,
                reviewer,
                verdict,
                ..
            } => Some((reviewed, phase, author, reviewer, verdict)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        reviews,
        [(&task, &None, &None, &reviewer, &body::Verdict::Clean)]
    );

    clean(&root, bus);
}
