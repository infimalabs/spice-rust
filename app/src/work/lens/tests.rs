use wire::intent::{TaskRelationEdge, TaskRelationKind, TaskRelationNode, TaskRelationNodeRef};

use super::*;
use crate::work::tests::{board, stamp, task};

fn remote_task(
    value: i64,
    title: &str,
    phase: &str,
    status: wire::WorkflowStatusKind,
) -> TaskRelationNode {
    TaskRelationNode::Task {
        task: stamp(value),
        handle: wire::TaskHandle::for_project("remote.fixture", &stamp(value)),
        title: title.to_owned(),
        phase: phase.parse().expect("fixture phase"),
        status,
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
            &|at| at.to_string(),
        )
        .expect("overview presentation")
    };

    let unresolved = project(None);
    assert_eq!(unresolved.nodes[1].kind, "Missing task");

    let enriched = answer(vec![
        remote_task(
            1,
            "remote title",
            "review",
            wire::WorkflowStatusKind::Pending,
        ),
        remote_task(
            2,
            "Completed prerequisite",
            "review",
            wire::WorkflowStatusKind::Completed,
        ),
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
    assert_eq!(completed.status, Some(wire::WorkflowStatusKind::Completed));
    assert_eq!(resolved.edges.len(), 1);
    assert_eq!((resolved.edges[0].from, resolved.edges[0].to), (1, 0));
    assert!(resolved.source.contains("Completed prerequisite"));
    assert!(!resolved.source.contains("Missing from the admitted board"));

    let mut elsewhere = answer(vec![remote_task(
        1,
        "remote title",
        "review",
        wire::WorkflowStatusKind::Pending,
    )]);
    elsewhere.edges.clear();
    assert_eq!(project(Some(&elsewhere)).nodes[1].kind, "Missing task");
}

/// A Design's overview draws its Plan outputs downstream of it, a Plan's own
/// overview names its gating Design upstream, and a Plan-to-executable edge
/// remains the only path to nested work.
#[test]
fn design_plan_dependency_edges_do_not_shortcut_nested_work() {
    let mut design = task(1, "rooms.design");
    design.statement.body.intent = Some(wire::TaskIntent::Design);
    let plan = |value| {
        let mut plan = task(value, "rooms.plan");
        plan.statement.body.intent = Some(wire::TaskIntent::Plan);
        plan.statement.body.dependencies = vec![stamp(1)];
        plan
    };
    let mut execution = task(4, "rooms.work");
    execution.statement.body.dependencies = vec![stamp(2)];
    let board = board(vec![design, plan(2), plan(3), execution]);
    let work = WorkProjection::from_board(&board);
    let arrows = |selected: Vec<wire::TaskStamp>| {
        let selection =
            TaskOverviewSelection::new(selected, super::super::TaskOverviewContext::Task);
        let presentation = super::project(
            &work.overview_graph(&selection),
            &work,
            None,
            TaskRelationKind::Dependency,
            WorkRelationshipDirection::Both,
            &selection,
            &|at| at.to_string(),
        )
        .expect("overview presentation");
        presentation
            .edges
            .iter()
            .map(|edge| {
                (
                    presentation.nodes[edge.from].task.clone(),
                    presentation.nodes[edge.to].task.clone(),
                    edge.label,
                )
            })
            .collect::<BTreeSet<_>>()
    };

    assert_eq!(
        arrows(vec![stamp(1), stamp(2), stamp(3)]),
        BTreeSet::from([
            (Some(stamp(1)), Some(stamp(2)), "then"),
            (Some(stamp(1)), Some(stamp(3)), "then"),
        ]),
    );
    assert_eq!(
        arrows(vec![stamp(2)]),
        BTreeSet::from([(Some(stamp(1)), Some(stamp(2)), "then")]),
        "a Plan alone still shows its Design prerequisite upstream",
    );
    assert_eq!(
        arrows(vec![stamp(1), stamp(2), stamp(4)]),
        BTreeSet::from([
            (Some(stamp(1)), Some(stamp(2)), "then"),
            (Some(stamp(2)), Some(stamp(4)), "then"),
        ]),
        "the Design edge does not shortcut the Plan-to-task dependency",
    );
}

fn then(from: usize, to: usize) -> Edge {
    Edge {
        from,
        to,
        label: "then",
    }
}

#[test]
fn disconnected_components_pack_into_rows_of_whole_components() {
    assert_eq!(row_links(6, &[]), [(0, 1), (1, 2), (3, 4), (4, 5)]);
    assert!(row_links(3, &[then(0, 1), then(1, 2)]).is_empty());

    // Ten nodes budget four layers: the three-layer chain fills its row, the
    // two-layer fan opens the next from its first last-layer node, and the
    // unconnected tasks fill rows behind it.
    let mixed = [then(0, 1), then(1, 2), then(3, 4), then(3, 5)];
    assert_eq!(row_links(10, &mixed), [(4, 6), (6, 7), (8, 9)]);

    // A chain deeper than the square side widens the budget and keeps a row.
    let deep = [then(0, 1), then(1, 2), then(2, 3), then(3, 4)];
    assert_eq!(row_links(8, &deep), [(5, 6), (6, 7)]);
}

#[test]
fn a_cycle_still_packs_deterministically() {
    let cycle = [then(0, 1), then(1, 2), then(2, 0)];
    assert_eq!(row_links(4, &cycle), [(0, 3)]);
}
