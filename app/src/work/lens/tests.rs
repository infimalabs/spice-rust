use wire::intent::{TaskRelationEdge, TaskRelationKind, TaskRelationNode, TaskRelationNodeRef};

use super::*;
use crate::work::tests::{board, stamp, task};

fn remote_task(value: i64, title: &str, phase: &str, status: &str) -> TaskRelationNode {
    TaskRelationNode::Task {
        task: stamp(value),
        handle: wire::TaskHandle::for_project("remote.fixture", &stamp(value)),
        title: title.to_owned(),
        phase: phase.parse().expect("fixture phase"),
        status: status.to_owned(),
    }
}

fn answer(nodes: Vec<TaskRelationNode>) -> TaskRelationsAnswer {
    TaskRelationsAnswer {
        snapshot: wire::Seq::new(7).expect("fixture snapshot"),
        roots: vec![stamp(1), stamp(2)],
        nodes,
        edges: vec![TaskRelationEdge {
            relation: TaskRelationKind::Dependency,
            from: TaskRelationNodeRef::Task { task: stamp(1) },
            to: TaskRelationNodeRef::Task { task: stamp(2) },
        }],
        truncation: None,
    }
}

/// A selected task the open board no longer admits, such as a completed
/// prerequisite, resolves from the same-stamp task in the typed answer while
/// admitted rows stay authoritative and no node or edge is duplicated.
#[test]
fn enrichment_resolves_a_missing_selected_task_in_place() {
    let mut open = task(1, "alpha.open");
    open.statement.body.dependencies = vec![stamp(2)];
    let board = board(vec![open]);
    let work = WorkProjection::from_board(&board);
    let selection = TaskOverviewSelection::new(
        [stamp(1), stamp(2)],
        super::super::TaskOverviewContext::Task,
    );
    let local = work.overview_graph(&selection);
    let project = |answer: Option<&TaskRelationsAnswer>| {
        super::project(
            &local,
            &work,
            answer,
            TaskRelationKind::Dependency,
            WorkRelationshipDirection::Both,
            &selection,
        )
        .expect("overview presentation")
    };

    let unresolved = project(None);
    assert_eq!(unresolved.nodes[1].kind, "Missing task");

    let enriched = answer(vec![
        remote_task(1, "remote title", "review", "pending"),
        remote_task(2, "Completed prerequisite", "review", "completed"),
    ]);
    let resolved = project(Some(&enriched));
    let tasks = resolved
        .nodes
        .iter()
        .map(|node| node.task.clone())
        .collect::<Vec<_>>();
    assert_eq!(tasks, vec![Some(stamp(1)), Some(stamp(2))]);
    assert_eq!(
        resolved.nodes[0].label,
        format!("{} · task 1", board.tasks[0].statement.body.handle)
    );
    assert_eq!(resolved.nodes[0].detail, "todo · pending");
    assert_eq!(resolved.nodes[0].status, None);
    let completed = &resolved.nodes[1];
    assert_eq!(completed.kind, "Selected task");
    assert!(completed.selected);
    assert_eq!(
        completed.label,
        format!(
            "{} · Completed prerequisite",
            wire::TaskHandle::for_project("remote.fixture", &stamp(2))
        )
    );
    assert_eq!(completed.detail, "review · completed");
    assert_eq!(completed.status.as_deref(), Some("completed"));
    assert_eq!(resolved.edges.len(), 1);
    assert_eq!((resolved.edges[0].from, resolved.edges[0].to), (1, 0));
    assert!(resolved.source.contains("Completed prerequisite"));
    assert!(!resolved.source.contains("Missing from the admitted board"));

    let mut elsewhere = answer(vec![remote_task(1, "remote title", "review", "pending")]);
    elsewhere.edges.clear();
    assert_eq!(project(Some(&elsewhere)).nodes[1].kind, "Missing task");
}
