use super::*;

fn seq(value: u64) -> wire::Seq {
    wire::Seq::new(value).expect("fixture sequence is positive")
}

fn stamp(value: i64) -> wire::TaskStamp {
    wire::TaskStamp::for_micros(value).expect("fixture task stamp is representable")
}

fn answer(task: &wire::TaskStamp, snapshot: u64) -> Box<TaskRelationsAnswer> {
    Box::new(TaskRelationsAnswer {
        snapshot: seq(snapshot),
        roots: vec![task.clone()],
        nodes: vec![wire::intent::TaskRelationNode::Task {
            task: task.clone(),
            handle: wire::TaskHandle::for_project("app.overview", task),
            title: "Shown task".to_owned(),
            phase: "todo".parse().expect("fixture phase"),
            status: "pending".to_owned(),
        }],
        edges: Vec::new(),
        truncation: None,
    })
}

fn key(task: &wire::TaskStamp, relation: TaskRelationKind) -> TaskRelationsKey {
    TaskRelationsKey::new(vec![task.clone()], relation, 2, 48)
        .expect("fixture relationship key is bounded")
}

fn available_states(task: &wire::TaskStamp) -> BTreeMap<TaskRelationsKey, TaskRelationsState> {
    [
        TaskRelationKind::Origin,
        TaskRelationKind::Dependency,
        TaskRelationKind::Review,
        TaskRelationKind::Lifecycle,
    ]
    .into_iter()
    .map(|relation| {
        (
            key(task, relation),
            TaskRelationsState::Available(answer(task, 1)),
        )
    })
    .collect()
}

fn change(kind: FactKind, task: wire::TaskStamp) -> TaskRelationInvalidation {
    TaskRelationInvalidation {
        kind,
        task,
        dependency_targets: Vec::new(),
        origin_target: None,
    }
}

#[test]
fn fact_vocabulary_owns_the_complete_task_relation_mapping() {
    use TaskRelationKind::{Dependency, Lifecycle, Origin, Review};

    let expected: &[(FactKind, &[TaskRelationKind])] = &[
        (FactKind::TaskAdded, &[Origin, Dependency, Lifecycle]),
        (
            FactKind::TaskPhased,
            &[Origin, Dependency, Review, Lifecycle],
        ),
        (FactKind::TaskDone, &[Origin, Dependency, Review, Lifecycle]),
        (
            FactKind::TaskReviewed,
            &[Origin, Dependency, Review, Lifecycle],
        ),
        (
            FactKind::TaskAssigned,
            &[Origin, Dependency, Review, Lifecycle],
        ),
        (FactKind::TaskNoted, &[]),
        (
            FactKind::TaskDeferred,
            &[Origin, Dependency, Review, Lifecycle],
        ),
        (
            FactKind::TaskWoken,
            &[Origin, Dependency, Review, Lifecycle],
        ),
        (
            FactKind::TaskEdited,
            &[Origin, Dependency, Review, Lifecycle],
        ),
        (
            FactKind::TaskDeleted,
            &[Origin, Dependency, Review, Lifecycle],
        ),
        (
            FactKind::TaskClosed,
            &[Origin, Dependency, Review, Lifecycle],
        ),
    ];
    let task_kinds = FactKind::ALL
        .iter()
        .copied()
        .filter(|kind| kind.as_str().starts_with("task::"))
        .collect::<Vec<_>>();

    assert_eq!(
        task_kinds,
        expected.iter().map(|(kind, _)| *kind).collect::<Vec<_>>()
    );
    for (kind, relations) in expected {
        assert_eq!(kind.task_relation_kinds(), *relations, "{kind}");
    }
}

#[test]
fn only_changes_reaching_a_cached_graph_refresh_its_relation_families() {
    let shown = stamp(1);
    let unrelated = stamp(2);
    let mut states = available_states(&shown);
    let mut revision = 0;

    invalidate_task_relation_states(
        &mut states,
        &mut revision,
        &[change(FactKind::TaskEdited, unrelated.clone())],
    );
    assert_eq!(revision, 0);
    assert!(
        states
            .values()
            .all(|state| matches!(state, TaskRelationsState::Available(_)))
    );

    invalidate_task_relation_states(
        &mut states,
        &mut revision,
        &[change(FactKind::TaskNoted, shown.clone())],
    );
    assert_eq!(revision, 0);
    assert!(
        states
            .values()
            .all(|state| matches!(state, TaskRelationsState::Available(_)))
    );

    let added = TaskRelationInvalidation {
        kind: FactKind::TaskAdded,
        task: unrelated,
        dependency_targets: vec![shown.clone()],
        origin_target: Some(shown.clone()),
    };
    invalidate_task_relation_states(&mut states, &mut revision, &[added]);
    assert_eq!(revision, 1);
    for relation in [TaskRelationKind::Origin, TaskRelationKind::Dependency] {
        assert!(matches!(
            states.get(&key(&shown, relation)),
            Some(TaskRelationsState::Refreshing(_))
        ));
    }
    for relation in [TaskRelationKind::Review, TaskRelationKind::Lifecycle] {
        assert!(matches!(
            states.get(&key(&shown, relation)),
            Some(TaskRelationsState::Available(_))
        ));
    }
}

#[test]
fn outside_task_edit_adding_a_shown_dependency_refreshes_only_that_lens() {
    let shown = stamp(1);
    let mut states = available_states(&shown);
    let mut revision = 0;
    let edited = TaskRelationInvalidation {
        kind: FactKind::TaskEdited,
        task: stamp(2),
        dependency_targets: vec![shown.clone()],
        origin_target: None,
    };

    invalidate_task_relation_states(&mut states, &mut revision, &[edited]);

    assert_eq!(revision, 1);
    assert!(matches!(
        states.get(&key(&shown, TaskRelationKind::Dependency)),
        Some(TaskRelationsState::Refreshing(_))
    ));
    for relation in [
        TaskRelationKind::Origin,
        TaskRelationKind::Review,
        TaskRelationKind::Lifecycle,
    ] {
        assert!(matches!(
            states.get(&key(&shown, relation)),
            Some(TaskRelationsState::Available(_))
        ));
    }
}

#[test]
fn shown_task_lifecycle_changes_refresh_each_projection_once() {
    let shown = stamp(1);
    let mut states = available_states(&shown);
    let mut revision = 0;

    invalidate_task_relation_states(
        &mut states,
        &mut revision,
        &[change(FactKind::TaskDone, shown)],
    );

    assert_eq!(revision, 1);
    assert!(
        states
            .values()
            .all(|state| matches!(state, TaskRelationsState::Refreshing(_)))
    );
}

#[test]
fn snapshot_only_relationship_answers_do_not_change_visible_content() {
    let task = stamp(1);
    let first = answer(&task, 1);
    let mut advanced = answer(&task, 2);
    assert!(task_relations_content_eq(&first, &advanced));

    let wire::intent::TaskRelationNode::Task { title, .. } = &mut advanced.nodes[0] else {
        panic!("fixture node is a task")
    };
    *title = "Changed title".to_owned();
    assert!(!task_relations_content_eq(&first, &advanced));
}
