//! Typed relationship answers projected into one-semantic map lenses.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::rc::Rc;

use wire::intent::{
    TaskLifecycleKind, TaskRelationEdge, TaskRelationKind, TaskRelationNode, TaskRelationNodeRef,
    TaskRelationsAnswer,
};
use wire::{Actor, ClaimDisposition, TaskStamp, body::Verdict};

use super::overlay::MapNodeTarget;
use crate::work::WorkRelationshipDirection;

pub(super) struct Presentation {
    pub(super) nodes: Vec<Node>,
    pub(super) edges: Vec<Edge>,
    pub(super) targets: Rc<Vec<MapNodeTarget>>,
    pub(super) source: String,
}

pub(super) struct Node {
    pub(super) id: String,
    pub(super) kind: &'static str,
    pub(super) label: String,
    pub(super) detail: String,
    pub(super) task: Option<TaskStamp>,
    pub(super) current: bool,
}

pub(super) struct Edge {
    pub(super) from: usize,
    pub(super) to: usize,
    pub(super) label: &'static str,
}

pub(super) fn project(
    answer: &TaskRelationsAnswer,
    relation: TaskRelationKind,
    direction: WorkRelationshipDirection,
    selected: &TaskStamp,
) -> Option<Presentation> {
    let by_identity = answer
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.identity(), index))
        .collect::<BTreeMap<_, _>>();
    let admitted_edges = answer
        .edges
        .iter()
        .filter(|edge| edge.relation == relation)
        .collect::<Vec<_>>();
    let included = included_nodes(answer, &admitted_edges, relation, direction);
    let mut old_to_new = BTreeMap::new();
    let mut nodes = Vec::with_capacity(included.len());
    for (old_index, node) in answer.nodes.iter().enumerate() {
        if !included.contains(&old_index) {
            continue;
        }
        let new_index = nodes.len();
        old_to_new.insert(old_index, new_index);
        nodes.push(present_node(node, new_index, selected));
    }
    let mut edges = Vec::new();
    for edge in admitted_edges {
        let old_from = *by_identity.get(&edge.from)?;
        let old_to = *by_identity.get(&edge.to)?;
        if !included.contains(&old_from) || !included.contains(&old_to) {
            continue;
        }
        edges.push(Edge {
            from: *old_to_new.get(&old_from)?,
            to: *old_to_new.get(&old_to)?,
            label: edge_label(relation),
        });
    }
    let targets = Rc::new(
        nodes
            .iter()
            .filter_map(|node| {
                node.task.as_ref().map(|task| MapNodeTarget {
                    id: node.id.clone(),
                    task: task.clone(),
                    label: node.label.clone(),
                    current: node.current,
                })
            })
            .collect(),
    );
    let source = mermaid_source(&nodes, &edges);
    Some(Presentation {
        nodes,
        edges,
        targets,
        source,
    })
}

fn included_nodes(
    answer: &TaskRelationsAnswer,
    edges: &[&TaskRelationEdge],
    relation: TaskRelationKind,
    direction: WorkRelationshipDirection,
) -> BTreeSet<usize> {
    if relation != TaskRelationKind::Dependency || direction == WorkRelationshipDirection::Both {
        return (0..answer.nodes.len()).collect();
    }
    let by_identity = answer
        .nodes
        .iter()
        .enumerate()
        .map(|(index, node)| (node.identity(), index))
        .collect::<BTreeMap<_, _>>();
    let mut included = answer
        .roots
        .iter()
        .filter_map(|task| by_identity.get(&TaskRelationNodeRef::Task { task: task.clone() }))
        .copied()
        .collect::<BTreeSet<_>>();
    let mut queue = included.iter().copied().collect::<VecDeque<_>>();
    while let Some(current) = queue.pop_front() {
        let current_identity = answer.nodes[current].identity();
        for edge in edges {
            let target = match direction {
                WorkRelationshipDirection::Prerequisites if edge.from == current_identity => {
                    by_identity.get(&edge.to).copied()
                }
                WorkRelationshipDirection::Dependents if edge.to == current_identity => {
                    by_identity.get(&edge.from).copied()
                }
                _ => None,
            };
            if let Some(target) = target
                && included.insert(target)
            {
                queue.push_back(target);
            }
        }
    }
    included
}

fn present_node(node: &TaskRelationNode, index: usize, selected: &TaskStamp) -> Node {
    let id = format!("n{index}");
    match node {
        TaskRelationNode::Task {
            task,
            handle,
            title,
            phase,
            status,
        } => Node {
            id,
            kind: "Task",
            label: format!("{} · {}", handle, compact(title)),
            detail: format!("Phase {phase} · {status}"),
            task: Some(task.clone()),
            current: task == selected,
        },
        TaskRelationNode::Origin { origin } => Node {
            id,
            kind: "ACK / origin",
            label: format!("Origin · {}", compact(origin.as_str())),
            detail: origin.to_string(),
            task: None,
            current: false,
        },
        TaskRelationNode::Review {
            position,
            phase,
            author,
            reviewer,
            verdict,
            ..
        } => Node {
            id,
            kind: "Completed review",
            label: format!(
                "{} · {} → {} · {}",
                phase,
                compact_actor(author),
                compact_actor(reviewer),
                verdict_label(*verdict),
            ),
            detail: format!(
                "Phase {phase}: author {author} → reviewer {reviewer} → verdict {} · fact {position}",
                verdict_label(*verdict),
            ),
            task: None,
            current: false,
        },
        TaskRelationNode::Lifecycle {
            position,
            at,
            actor,
            event,
            ..
        } => Node {
            id,
            kind: "Lifecycle transition",
            label: format!("{} · {}", lifecycle_label(*event), compact_actor(actor)),
            detail: format!("{at} · {actor} · fact {position}"),
            task: None,
            current: false,
        },
        TaskRelationNode::Claim {
            position,
            actor,
            lane,
            occupant,
            disposition,
            ..
        } => Node {
            id,
            kind: "Current possession",
            label: format!("Current claim · {}", compact_actor(actor)),
            detail: format!(
                "{} · lane {lane} · occupant {occupant} · fact {position}",
                disposition_label(*disposition),
            ),
            task: None,
            current: false,
        },
    }
}

fn mermaid_source(nodes: &[Node], edges: &[Edge]) -> String {
    let mut source = String::from("flowchart LR\n");
    for node in nodes {
        let label = escape(&format!("{} · {}", node.label, node.detail));
        writeln!(source, "  {}[\"{label}\"]", node.id).expect("String writes are infallible");
    }
    for edge in edges {
        writeln!(source, "  n{} -->|{}| n{}", edge.from, edge.label, edge.to,)
            .expect("String writes are infallible");
    }
    let selected = nodes
        .iter()
        .filter(|node| node.current)
        .map(|node| node.id.as_str())
        .collect::<Vec<_>>();
    if !selected.is_empty() {
        writeln!(source, "  class {} selected", selected.join(","))
            .expect("String writes are infallible");
        source.push_str("  classDef selected stroke-width:3px\n");
    }
    source
}

fn compact(value: &str) -> String {
    const LIMIT: usize = 48;
    let mut value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() > LIMIT {
        value = value.chars().take(LIMIT - 1).collect();
        value.push('…');
    }
    value
}

fn compact_actor(actor: &Actor) -> String {
    match actor {
        Actor::Lane(lane) => format!("lane {}", &lane.to_string()[..8]),
        Actor::Operator => "operator".to_owned(),
        Actor::Spice => "spice".to_owned(),
    }
}

fn escape(label: &str) -> String {
    label
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub(super) const fn edge_label(relation: TaskRelationKind) -> &'static str {
    match relation {
        TaskRelationKind::Origin => "origin",
        TaskRelationKind::Dependency => "requires",
        TaskRelationKind::Review => "review",
        TaskRelationKind::Lifecycle => "transition",
        TaskRelationKind::Claim => "held by",
    }
}

pub(super) const fn lens_label(relation: TaskRelationKind) -> &'static str {
    match relation {
        TaskRelationKind::Origin => "Lineage",
        TaskRelationKind::Dependency => "Dependencies",
        TaskRelationKind::Review => "Reviews",
        TaskRelationKind::Lifecycle => "Lifecycle",
        TaskRelationKind::Claim => "Ownership",
    }
}

fn verdict_label(verdict: Verdict) -> &'static str {
    match verdict {
        Verdict::Clean => "clean",
        Verdict::Finding => "finding",
    }
}

fn lifecycle_label(event: TaskLifecycleKind) -> &'static str {
    match event {
        TaskLifecycleKind::Added => "Added",
        TaskLifecycleKind::Edited => "Edited",
        TaskLifecycleKind::Phased => "Phase changed",
        TaskLifecycleKind::Completed => "Phase completed",
        TaskLifecycleKind::Assigned => "Reviewer assigned",
        TaskLifecycleKind::Reviewed => "Reviewed",
        TaskLifecycleKind::Deferred => "Deferred",
        TaskLifecycleKind::Woken => "Woken",
        TaskLifecycleKind::Deleted => "Deleted",
    }
}

fn disposition_label(disposition: ClaimDisposition) -> &'static str {
    match disposition {
        ClaimDisposition::Active => "active",
        ClaimDisposition::Freed => "freed",
        ClaimDisposition::Lapsed => "lapsed",
    }
}
