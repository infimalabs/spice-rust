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
            status: wire::WorkflowStatusKind::Pending,
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
fn authorization_refreshes_dependency_graphs_for_its_controller_and_selected_outputs() {
    let controller = stamp(1);
    let selected = stamp(2);
    let governed = stamp(3);
    let unrelated = stamp(4);
    let fact: wire::StreamFact = serde_json::from_value(serde_json::json!({
        "v": wire::WireVersion::CURRENT,
        "seq": 7,
        "at": "2026-09-23T06:00:00.000Z",
        "actor": "operator",
        "origin": format!("task:{controller}"),
        "cause": 6,
        "fact": {
            "kind": "proposal::authorized",
            "authorization": {
                "decision": {"controller": controller, "decision": "1"},
                "candidate": {"controller": controller, "candidate": "dVsYdDj7h"},
                "governed_inputs": [governed],
                "selected_outputs": [selected],
                "coverage": [{
                    "candidate": {"controller": controller, "candidate": "dVsYdDj7h"},
                    "coverage": "1"
                }]
            }
        }
    }))
    .expect("admitted authorization fact");
    let mut states = [controller.clone(), selected.clone(), governed, unrelated]
        .iter()
        .flat_map(available_states)
        .collect();
    let mut revision = 0;

    invalidate_task_relation_states(
        &mut states,
        &mut revision,
        &[TaskRelationInvalidation::from_fact(&fact).expect("authorization changes dependencies")],
    );

    assert_eq!(revision, 1);
    for (key, state) in states {
        let affected = key.relation() == TaskRelationKind::Dependency
            && key
                .roots()
                .iter()
                .any(|task| task == &controller || task == &selected);
        assert_eq!(
            matches!(state, TaskRelationsState::Refreshing(_)),
            affected,
            "only graphs containing a released edge endpoint need a new relationship read: {key:?}"
        );
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
