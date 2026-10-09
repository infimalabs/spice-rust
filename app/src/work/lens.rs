//! Typed relationship enrichment merged with the complete local root graph:
//! the overview's graph and relationship list.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;

use wire::intent::{
    TaskLifecycleKind, TaskRelationEdge, TaskRelationKind, TaskRelationNode, TaskRelationNodeRef,
    TaskRelationsAnswer,
};
use wire::{Actor, ClaimDisposition, TaskStamp, body::Verdict};

use super::{
    TaskOverviewGraph, TaskOverviewNode, TaskOverviewNodeKind, TaskOverviewSelection,
    WorkProjection, WorkRelationshipDirection,
};

pub(crate) struct Presentation {
    pub(crate) nodes: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
    pub(crate) source: String,
}

pub(crate) struct Node {
    pub(crate) id: String,
    pub(crate) kind: &'static str,
    pub(crate) label: String,
    pub(crate) detail: String,
    pub(crate) task: Option<TaskStamp>,
    diagram: String,
    pub(crate) selected: bool,
}

pub(crate) struct Edge {
    pub(crate) from: usize,
    pub(crate) to: usize,
    pub(crate) label: &'static str,
}

pub(crate) fn project(
    local: &TaskOverviewGraph,
    work: &WorkProjection<'_>,
    answer: Option<&TaskRelationsAnswer>,
    relation: TaskRelationKind,
    direction: WorkRelationshipDirection,
    selection: &TaskOverviewSelection,
) -> Option<Presentation> {
    let enriched = answer
        .into_iter()
        .flat_map(|answer| &answer.nodes)
        .filter_map(|node| match node {
            TaskRelationNode::Task { task, .. } => Some((task, node)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let mut identities = BTreeMap::new();
    let mut local_to_merged = BTreeMap::new();
    let mut nodes = Vec::new();
    for (local_index, node) in local.nodes().iter().enumerate() {
        if (relation != TaskRelationKind::Dependency
            || direction == WorkRelationshipDirection::Dependents)
            && !node.selected()
        {
            continue;
        }
        let index = nodes.len();
        identities.insert(
            TaskRelationNodeRef::Task {
                task: node.task().clone(),
            },
            index,
        );
        local_to_merged.insert(local_index, index);
        let enrichment = enriched.get(node.task()).copied();
        nodes.push(present_local_node(node, index, work, enrichment));
    }

    let mut edge_keys = BTreeSet::new();
    let mut edges = Vec::new();
    if relation == TaskRelationKind::Dependency {
        for edge in local.edges() {
            let (Some(&dependent), Some(&prerequisite)) = (
                local_to_merged.get(&edge.dependent()),
                local_to_merged.get(&edge.prerequisite()),
            ) else {
                continue;
            };
            if direction == WorkRelationshipDirection::Dependents
                && !(local.nodes()[edge.dependent()].selected()
                    && local.nodes()[edge.prerequisite()].selected())
            {
                continue;
            }
            push_edge(
                &mut edges,
                &mut edge_keys,
                prerequisite,
                dependent,
                edge_label(relation),
            );
        }
    }

    if let Some(answer) = answer {
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
        for (answer_index, node) in answer.nodes.iter().enumerate() {
            if !included.contains(&answer_index) {
                continue;
            }
            let identity = node.identity();
            if identities.contains_key(&identity) {
                continue;
            }
            let index = nodes.len();
            identities.insert(identity, index);
            nodes.push(present_remote_node(node, index, selection));
        }
        for edge in admitted_edges {
            let old_from = *by_identity.get(&edge.from)?;
            let old_to = *by_identity.get(&edge.to)?;
            if !included.contains(&old_from) || !included.contains(&old_to) {
                continue;
            }
            let (from, to) = if relation == TaskRelationKind::Dependency {
                (*identities.get(&edge.to)?, *identities.get(&edge.from)?)
            } else {
                (*identities.get(&edge.from)?, *identities.get(&edge.to)?)
            };
            push_edge(&mut edges, &mut edge_keys, from, to, edge_label(relation));
        }
    }

    let source = mermaid_source(&nodes, &edges);
    Some(Presentation {
        nodes,
        edges,
        source,
    })
}

fn push_edge(
    edges: &mut Vec<Edge>,
    keys: &mut BTreeSet<(usize, usize, &'static str)>,
    from: usize,
    to: usize,
    label: &'static str,
) {
    if keys.insert((from, to, label)) {
        edges.push(Edge { from, to, label });
    }
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

/// Present one node of the admitted-board graph. An admitted row is
/// authoritative; a node the board lacks, such as a completed task, takes the
/// same-stamp task from the typed enrichment answer when it has one.
fn present_local_node(
    node: &TaskOverviewNode,
    index: usize,
    work: &WorkProjection<'_>,
    enrichment: Option<&TaskRelationNode>,
) -> Node {
    let resolved = node
        .task_index()
        .map(|task_index| {
            let row = work.tasks()[task_index].row();
            (
                format!(
                    "{} · {}",
                    row.statement.body.handle,
                    compact(&row.presentation_title),
                ),
                format!("{} · {}", row.workflow.body.phase, row.workflow.body.status),
            )
        })
        .or_else(|| match enrichment {
            Some(TaskRelationNode::Task {
                handle,
                title,
                phase,
                status,
                ..
            }) => Some((
                format!("{handle} · {}", compact(title)),
                format!("{phase} · {status}"),
            )),
            _ => None,
        });
    let (kind, label, detail) = match (resolved, node.kind()) {
        (Some((label, detail)), _) if node.selected() => ("Selected task", label, detail),
        (Some((label, detail)), _) => ("External dependency", label, detail),
        (None, TaskOverviewNodeKind::Missing) => (
            "Missing task",
            node.task().to_string(),
            "Missing from the admitted board snapshot".to_owned(),
        ),
        (None, _) => (
            "Unavailable task",
            node.task().to_string(),
            "Unavailable in the admitted board snapshot".to_owned(),
        ),
    };
    Node {
        id: format!("n{index}"),
        kind,
        diagram: format!("{label}\n{detail}"),
        label,
        detail,
        task: Some(node.task().clone()),
        selected: node.selected(),
    }
}

fn present_remote_node(
    node: &TaskRelationNode,
    index: usize,
    selection: &TaskOverviewSelection,
) -> Node {
    let id = format!("n{index}");
    match node {
        TaskRelationNode::Task {
            task,
            handle,
            title,
            phase,
            status,
        } => {
            let title = compact(title);
            Node {
                id,
                kind: "Related task",
                label: format!("{handle} · {title}"),
                detail: format!("Phase {phase} · {status}"),
                diagram: format!("{handle}\n{title}\nPhase {phase} — {status}"),
                task: Some(task.clone()),
                selected: selection.tasks().contains(task),
            }
        }
        TaskRelationNode::Origin { origin } => Node {
            id,
            kind: "ACK / origin",
            label: format!("Origin · {}", compact(origin.as_str())),
            detail: origin.to_string(),
            diagram: format!("Origin\n{origin}"),
            task: None,
            selected: false,
        },
        TaskRelationNode::Review {
            position,
            phase,
            author,
            reviewer,
            verdict,
            ..
        } => {
            let author_label = compact_actor(author);
            let reviewer_label = compact_actor(reviewer);
            let verdict = verdict_label(*verdict);
            Node {
                id,
                kind: "Completed review",
                label: format!("{phase} · {author_label} → {reviewer_label} · {verdict}"),
                detail: format!(
                    "Phase {phase}: author {author} → reviewer {reviewer} → verdict {verdict} · fact {position}"
                ),
                diagram: format!(
                    "{phase} review\n{author_label} → {reviewer_label}\n{verdict}\nfact {position}"
                ),
                task: None,
                selected: false,
            }
        }
        TaskRelationNode::Lifecycle {
            position,
            at,
            actor,
            event,
            ..
        } => {
            let actor_label = compact_actor(actor);
            let event = lifecycle_label(*event);
            Node {
                id,
                kind: "Lifecycle transition",
                label: format!("{event} · {actor_label}"),
                detail: format!("{at} · {actor} · fact {position}"),
                diagram: format!("{event}\n{actor_label}\n{at}\nfact {position}"),
                task: None,
                selected: false,
            }
        }
        TaskRelationNode::Claim {
            position,
            actor,
            lane,
            occupant,
            disposition,
            ..
        } => {
            let actor_is_resident_lane =
                matches!(actor, Actor::Lane(actor_lane) if actor_lane == lane);
            let actor = compact_actor(actor);
            let disposition = disposition_label(*disposition);
            let detail =
                format!("{disposition} · lane {lane} · occupant {occupant} · fact {position}");
            let lane = compact_identity(lane);
            let occupant = compact_identity(occupant);
            let owner = if actor_is_resident_lane {
                format!("lane {lane}")
            } else {
                format!("{actor}\nlane {lane}")
            };
            Node {
                id,
                kind: "Current possession",
                label: format!("Current claim · {actor}"),
                detail,
                diagram: format!(
                    "Current claim\n{owner}\n{disposition}\noccupant {occupant}\nfact {position}"
                ),
                task: None,
                selected: false,
            }
        }
    }
}

fn mermaid_source(nodes: &[Node], edges: &[Edge]) -> String {
    let mut source = String::from("flowchart LR\n");
    for node in nodes {
        let label = crate::mermaid_label::multiline(&node.diagram);
        writeln!(source, "  {}[\"{label}\"]", node.id).expect("String writes are infallible");
    }
    for edge in edges {
        writeln!(source, "  n{} -->|{}| n{}", edge.from, edge.label, edge.to)
            .expect("String writes are infallible");
    }
    let selected = nodes
        .iter()
        .filter(|node| node.selected)
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
        Actor::Lane(lane) => format!("lane {}", compact_identity(lane)),
        Actor::Operator => "operator".to_owned(),
        Actor::Spice => "spice".to_owned(),
    }
}

fn compact_identity(identity: impl ToString) -> String {
    let mut identity = identity.to_string();
    identity.truncate(8);
    identity
}

pub(crate) const fn edge_label(relation: TaskRelationKind) -> &'static str {
    match relation {
        TaskRelationKind::Origin => "origin",
        TaskRelationKind::Dependency => "blocks",
        TaskRelationKind::Review => "review",
        TaskRelationKind::Lifecycle => "transition",
        TaskRelationKind::Claim => "held by",
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

#[cfg(test)]
mod tests;
