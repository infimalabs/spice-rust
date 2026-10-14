use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::*;
use wire::intent::{
    TASK_RELATION_DEPTH_LIMIT, TASK_RELATION_EDGE_LIMIT, TASK_RELATION_FRONTIER_LIMIT,
    TASK_RELATION_NODE_LIMIT, TASK_RELATION_ROOT_LIMIT, TaskLifecycleKind, TaskRelationEdge,
    TaskRelationFrontier, TaskRelationKind, TaskRelationNode, TaskRelationNodeRef,
    TaskRelationTruncation, TaskRelations, TaskRelationsAnswer,
};

#[derive(Clone)]
struct Candidate {
    edge: TaskRelationEdge,
    target: TaskRelationNode,
}

#[derive(Clone)]
struct ReviewRecord {
    task: TaskStamp,
    position: Seq,
    /// The most recent completed phase before the review and its author.
    implemented: Option<(wire::Phase, Actor)>,
    reviewer: Actor,
    verdict: body::Verdict,
}

#[derive(Default)]
struct Omitted {
    frontier: BTreeSet<TaskRelationFrontier>,
    beyond_frontier: u32,
}

impl Omitted {
    fn push(&mut self, depth: u8, edge: TaskRelationEdge) {
        let omitted = TaskRelationFrontier { depth, edge };
        if self.frontier.contains(&omitted) {
            return;
        }
        if self.frontier.len() < TASK_RELATION_FRONTIER_LIMIT {
            self.frontier.insert(omitted);
        } else {
            self.beyond_frontier = self.beyond_frontier.saturating_add(1);
        }
    }

    fn finish(self) -> Option<TaskRelationTruncation> {
        (!self.frontier.is_empty() || self.beyond_frontier > 0).then(|| TaskRelationTruncation {
            frontier: self.frontier.into_iter().collect(),
            omitted: self.beyond_frontier,
        })
    }
}

/// Read one bounded typed relationship projection from an immutable journal barrier.
pub(in crate::board_command) fn read_task_relations(
    bus: &Bus,
    request: &TaskRelations,
    principal: Actor,
    projects: &ProjectPolicy,
    allocation: &board::NextConfig,
) -> Outcome<CommandAnswer> {
    let (roots, relations) = match validate(request) {
        Ok(validated) => validated,
        Err(outcome) => return outcome,
    };
    let observed = match bus.task_snapshot() {
        Ok(observed) => observed,
        Err(error) => return store_fault("board::heads-unreadable", &error.to_string()),
    };
    let selected = relationship_tasks(&observed, &roots, &relations, request.depth);
    let snapshot = match validated_snapshot_from_with_occupant(
        &observed,
        principal,
        projects,
        allocation,
        None,
        Some(&selected),
        false,
    ) {
        Ok(snapshot) => snapshot,
        Err(outcome) => return outcome,
    };
    let tasks = snapshot
        .answer
        .tasks
        .iter()
        .map(|task| (task.statement.body.born.clone(), task))
        .collect::<BTreeMap<_, _>>();
    if let Some(unknown) = roots.iter().find(|root| !tasks.contains_key(*root)) {
        return refusal(
            "task::relations-unknown",
            Some(unknown.to_string()),
            "read the board and request current durable task identities visible to this principal",
        );
    }
    let review_records = match review_records(observed.facts().iter(), &tasks) {
        Ok(records) => records,
        Err(detail) => return store_fault("task::relations-review-invalid", &detail),
    };
    let lifecycle = lifecycle_records(observed.facts().iter(), &tasks);
    let reverse_dependencies = reverse_dependencies(&tasks);
    let reverse_origins = reverse_origins(&tasks);
    let mut nodes = BTreeMap::<TaskRelationNodeRef, TaskRelationNode>::new();
    let mut queue = VecDeque::<(TaskStamp, u8)>::new();
    for root in &roots {
        let node = task_node(tasks[root]);
        nodes.insert(node.identity(), node);
        queue.push_back((root.clone(), 0));
    }
    let mut expanded = BTreeSet::new();
    let mut edges = BTreeSet::new();
    let mut omitted = Omitted::default();
    while let Some((task, depth)) = queue.pop_front() {
        if !expanded.insert(task.clone()) {
            continue;
        }
        let mut candidates = candidates(
            &task,
            &relations,
            &tasks,
            &reverse_dependencies,
            &reverse_origins,
            &review_records,
            &lifecycle,
        );
        candidates.sort_by(|left, right| left.edge.cmp(&right.edge));
        candidates.dedup_by(|left, right| left.edge == right.edge);
        for candidate in candidates {
            let target = candidate.target.identity();
            let target_exists = nodes.contains_key(&target);
            if !target_exists && depth >= request.depth {
                omitted.push(depth.saturating_add(1), candidate.edge);
                continue;
            }
            if !target_exists && nodes.len() >= usize::from(request.node_limit) {
                omitted.push(depth.saturating_add(1), candidate.edge);
                continue;
            }
            if !edges.contains(&candidate.edge) && edges.len() >= TASK_RELATION_EDGE_LIMIT {
                omitted.push(depth.saturating_add(1), candidate.edge);
                continue;
            }
            if !target_exists {
                if let TaskRelationNodeRef::Task { task } = &target {
                    queue.push_back((task.clone(), depth.saturating_add(1)));
                }
                nodes.insert(target, candidate.target);
            }
            edges.insert(candidate.edge);
        }
    }
    let answer = TaskRelationsAnswer {
        snapshot: snapshot.cursor,
        roots,
        nodes: nodes.into_values().collect(),
        edges: edges.into_iter().collect(),
        truncation: omitted.finish(),
    };
    snapshot_reply(
        snapshot.cursor,
        CommandAnswer::TaskRelations(Box::new(answer)),
    )
}
fn relationship_tasks(
    observed: &bus::TaskSnapshot,
    roots: &[TaskStamp],
    relations: &BTreeSet<TaskRelationKind>,
    depth: u8,
) -> BTreeSet<TaskStamp> {
    let tasks = observed
        .tasks()
        .iter()
        .map(|heads| (heads.statement().body().born.clone(), heads))
        .collect::<BTreeMap<_, _>>();
    let mut reverse_dependencies = BTreeMap::<TaskStamp, BTreeSet<TaskStamp>>::new();
    let mut reverse_origins = BTreeMap::<TaskStamp, BTreeSet<TaskStamp>>::new();
    for (task, heads) in &tasks {
        let statement = heads.statement().body();
        for dependency in &statement.dependencies {
            if tasks.contains_key(dependency) {
                reverse_dependencies
                    .entry(dependency.clone())
                    .or_default()
                    .insert(task.clone());
            }
        }
        if let Some(parent) = statement
            .provenance
            .as_str()
            .strip_prefix("task:")
            .and_then(|value| value.parse::<TaskStamp>().ok())
            .filter(|parent| tasks.contains_key(parent))
        {
            reverse_origins
                .entry(parent)
                .or_default()
                .insert(task.clone());
        }
    }
    let mut selected = roots.iter().cloned().collect::<BTreeSet<_>>();
    let mut queue = roots
        .iter()
        .cloned()
        .map(|task| (task, 0_u8))
        .collect::<VecDeque<_>>();
    let mut expanded = BTreeSet::new();
    while let Some((task, current_depth)) = queue.pop_front() {
        if !expanded.insert(task.clone()) {
            continue;
        }
        let Some(heads) = tasks.get(&task) else {
            continue;
        };
        let statement = heads.statement().body();
        let mut neighbors = BTreeSet::new();
        if relations.contains(&TaskRelationKind::Dependency) {
            neighbors.extend(
                statement
                    .dependencies
                    .iter()
                    .filter(|dependency| tasks.contains_key(*dependency))
                    .cloned(),
            );
            neighbors.extend(
                reverse_dependencies
                    .get(&task)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
        }
        if relations.contains(&TaskRelationKind::Origin) {
            neighbors.extend(
                statement
                    .provenance
                    .as_str()
                    .strip_prefix("task:")
                    .and_then(|value| value.parse::<TaskStamp>().ok())
                    .filter(|parent| tasks.contains_key(parent)),
            );
            neighbors.extend(reverse_origins.get(&task).into_iter().flatten().cloned());
        }
        for neighbor in neighbors {
            selected.insert(neighbor.clone());
            if current_depth < depth {
                queue.push_back((neighbor, current_depth.saturating_add(1)));
            }
        }
    }
    selected
}

fn validate(
    request: &TaskRelations,
) -> Result<(Vec<TaskStamp>, BTreeSet<TaskRelationKind>), Outcome<CommandAnswer>> {
    if request.roots.is_empty() || request.roots.len() > TASK_RELATION_ROOT_LIMIT {
        return Err(refusal(
            "task::relations-roots-invalid",
            Some(request.roots.len().to_string()),
            "request between one and eight durable task roots",
        ));
    }
    let roots = request.roots.iter().cloned().collect::<BTreeSet<_>>();
    if roots.len() != request.roots.len() {
        return Err(refusal(
            "task::relations-roots-duplicate",
            None,
            "request each durable task root once",
        ));
    }
    let relations = request.relations.iter().copied().collect::<BTreeSet<_>>();
    if relations.is_empty() || relations.len() != request.relations.len() {
        return Err(refusal(
            "task::relations-kinds-invalid",
            Some(request.relations.len().to_string()),
            "request one or more distinct typed relation kinds",
        ));
    }
    if request.depth > TASK_RELATION_DEPTH_LIMIT {
        return Err(refusal(
            "task::relations-depth-excessive",
            Some(request.depth.to_string()),
            "request a relationship depth no greater than four",
        ));
    }
    if request.node_limit > TASK_RELATION_NODE_LIMIT
        || usize::from(request.node_limit) < roots.len()
    {
        return Err(refusal(
            "task::relations-node-limit-invalid",
            Some(request.node_limit.to_string()),
            "request a node limit that contains every root and does not exceed 128",
        ));
    }
    Ok((roots.into_iter().collect(), relations))
}

fn task_node(task: &BoardTask) -> TaskRelationNode {
    TaskRelationNode::Task {
        task: task.statement.body.born.clone(),
        handle: task.statement.body.handle.clone(),
        title: task.presentation_title.clone(),
        phase: task.workflow.body.phase.clone(),
        status: task.workflow.body.status.clone(),
    }
}

fn candidates(
    task: &TaskStamp,
    relations: &BTreeSet<TaskRelationKind>,
    tasks: &BTreeMap<TaskStamp, &BoardTask>,
    reverse_dependencies: &BTreeMap<TaskStamp, BTreeSet<TaskStamp>>,
    reverse_origins: &BTreeMap<TaskStamp, BTreeSet<TaskStamp>>,
    reviews: &BTreeMap<TaskStamp, Vec<ReviewRecord>>,
    lifecycle: &BTreeMap<TaskStamp, Vec<TaskRelationNode>>,
) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    let source = TaskRelationNodeRef::Task { task: task.clone() };
    let row = tasks[task];
    if relations.contains(&TaskRelationKind::Origin) {
        let origin = &row.statement.body.provenance;
        let target = origin
            .as_str()
            .strip_prefix("task:")
            .and_then(|value| value.parse::<TaskStamp>().ok())
            .filter(|parent| tasks.contains_key(parent))
            .map_or_else(
                || TaskRelationNode::Origin {
                    origin: origin.clone(),
                },
                |parent| task_node(tasks[&parent]),
            );
        candidates.push(candidate(TaskRelationKind::Origin, source.clone(), target));
        if let Some(children) = reverse_origins.get(task) {
            for child in children {
                let target = task_node(tasks[child]);
                candidates.push(Candidate {
                    edge: TaskRelationEdge {
                        relation: TaskRelationKind::Origin,
                        from: target.identity(),
                        to: source.clone(),
                    },
                    target,
                });
            }
        }
    }
    if relations.contains(&TaskRelationKind::Dependency) {
        for dependency in &row.statement.body.dependencies {
            if let Some(dependency) = tasks.get(dependency) {
                candidates.push(candidate(
                    TaskRelationKind::Dependency,
                    source.clone(),
                    task_node(dependency),
                ));
            }
        }
        if let Some(dependents) = reverse_dependencies.get(task) {
            for dependent in dependents {
                let target = task_node(tasks[dependent]);
                candidates.push(Candidate {
                    edge: TaskRelationEdge {
                        relation: TaskRelationKind::Dependency,
                        from: target.identity(),
                        to: source.clone(),
                    },
                    target,
                });
            }
        }
    }
    if relations.contains(&TaskRelationKind::Review) {
        for review in reviews.get(task).into_iter().flatten() {
            let (phase, author) = review.implemented.clone().unzip();
            let target = TaskRelationNode::Review {
                task: review.task.clone(),
                position: review.position,
                phase,
                author,
                reviewer: review.reviewer.clone(),
                verdict: review.verdict,
            };
            candidates.push(candidate(TaskRelationKind::Review, source.clone(), target));
        }
    }
    if relations.contains(&TaskRelationKind::Lifecycle) {
        for target in lifecycle.get(task).into_iter().flatten().cloned() {
            candidates.push(candidate(
                TaskRelationKind::Lifecycle,
                source.clone(),
                target,
            ));
        }
    }
    if relations.contains(&TaskRelationKind::Claim)
        && let Some(claim) = row
            .claim
            .as_ref()
            .filter(|claim| claim.body.disposition == ClaimDisposition::Active)
    {
        let target = TaskRelationNode::Claim {
            task: task.clone(),
            position: claim.position,
            actor: claim.body.actor.clone(),
            lane: claim.body.lane,
            occupant: claim.body.occupant,
            disposition: claim.body.disposition,
        };
        candidates.push(candidate(TaskRelationKind::Claim, source, target));
    }
    candidates
}

fn candidate(
    relation: TaskRelationKind,
    from: TaskRelationNodeRef,
    target: TaskRelationNode,
) -> Candidate {
    Candidate {
        edge: TaskRelationEdge {
            relation,
            from,
            to: target.identity(),
        },
        target,
    }
}

fn reverse_origins(
    tasks: &BTreeMap<TaskStamp, &BoardTask>,
) -> BTreeMap<TaskStamp, BTreeSet<TaskStamp>> {
    let mut reverse = BTreeMap::<TaskStamp, BTreeSet<TaskStamp>>::new();
    for (task, row) in tasks {
        let parent = row
            .statement
            .body
            .provenance
            .as_str()
            .strip_prefix("task:")
            .and_then(|value| value.parse::<TaskStamp>().ok());
        if let Some(parent) = parent.filter(|parent| tasks.contains_key(parent)) {
            reverse.entry(parent).or_default().insert(task.clone());
        }
    }
    reverse
}

fn reverse_dependencies(
    tasks: &BTreeMap<TaskStamp, &BoardTask>,
) -> BTreeMap<TaskStamp, BTreeSet<TaskStamp>> {
    let mut reverse = BTreeMap::<TaskStamp, BTreeSet<TaskStamp>>::new();
    for (task, row) in tasks {
        for dependency in &row.statement.body.dependencies {
            if tasks.contains_key(dependency) {
                reverse
                    .entry(dependency.clone())
                    .or_default()
                    .insert(task.clone());
            }
        }
    }
    reverse
}

fn lifecycle_records<'a>(
    facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
    tasks: &BTreeMap<TaskStamp, &BoardTask>,
) -> BTreeMap<TaskStamp, Vec<TaskRelationNode>> {
    let mut records = BTreeMap::<TaskStamp, Vec<TaskRelationNode>>::new();
    let mut ordered = facts.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|envelope| envelope.seq);
    for envelope in ordered {
        let Some((task, event)) = lifecycle_event(envelope) else {
            continue;
        };
        if !tasks.contains_key(&task) {
            continue;
        }
        records
            .entry(task.clone())
            .or_default()
            .push(TaskRelationNode::Lifecycle {
                task,
                position: envelope.seq,
                at: envelope.at,
                actor: envelope.actor.clone(),
                event,
            });
    }
    records
}

fn lifecycle_event(
    envelope: &wire::Envelope<wire::AnyFact>,
) -> Option<(TaskStamp, TaskLifecycleKind)> {
    let (task, event) = match envelope.fact.kind() {
        wire::FactKind::TaskAdded => (
            envelope.fact.read::<body::TaskAdded>()?.task,
            TaskLifecycleKind::Added,
        ),
        wire::FactKind::TaskEdited => (
            envelope.fact.read::<body::TaskEdited>()?.task,
            TaskLifecycleKind::Edited,
        ),
        wire::FactKind::TaskPhased => (
            envelope.fact.read::<body::TaskPhased>()?.task,
            TaskLifecycleKind::Phased,
        ),
        wire::FactKind::TaskDone => (
            envelope.fact.read::<body::TaskDone>()?.task,
            TaskLifecycleKind::Completed,
        ),
        wire::FactKind::TaskClosed => (
            envelope.fact.read::<body::TaskClosed>()?.task,
            TaskLifecycleKind::Completed,
        ),
        wire::FactKind::TaskAssigned => (
            envelope.fact.read::<body::TaskAssigned>()?.task,
            TaskLifecycleKind::Assigned,
        ),
        wire::FactKind::TaskReviewed => (
            envelope.fact.read::<body::TaskReviewed>()?.task,
            TaskLifecycleKind::Reviewed,
        ),
        wire::FactKind::TaskDeferred => (
            envelope.fact.read::<body::TaskDeferred>()?.task,
            TaskLifecycleKind::Deferred,
        ),
        wire::FactKind::TaskWoken => (
            envelope.fact.read::<body::TaskWoken>()?.task,
            TaskLifecycleKind::Woken,
        ),
        wire::FactKind::TaskFiled => (
            envelope.fact.read::<body::TaskFiled>()?.task,
            TaskLifecycleKind::Filed,
        ),
        wire::FactKind::TaskDeleted => (
            envelope.fact.read::<body::TaskDeleted>()?.task,
            TaskLifecycleKind::Deleted,
        ),
        _ => return None,
    };
    Some((task, event))
}

fn review_records<'a>(
    facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
    tasks: &BTreeMap<TaskStamp, &BoardTask>,
) -> Result<BTreeMap<TaskStamp, Vec<ReviewRecord>>, String> {
    let mut ordered = facts.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|envelope| envelope.seq);
    let mut phases = BTreeMap::<TaskStamp, wire::Phase>::new();
    let mut authors = BTreeMap::<TaskStamp, (wire::Phase, Actor)>::new();
    let mut records = BTreeMap::<TaskStamp, Vec<ReviewRecord>>::new();
    for envelope in ordered {
        match envelope.fact.kind() {
            wire::FactKind::TaskAdded => {
                let added = envelope
                    .fact
                    .read::<body::TaskAdded>()
                    .expect("registered task-added fact decodes");
                if tasks.contains_key(&added.task) {
                    phases.insert(added.task, added.phase);
                }
            }
            wire::FactKind::TaskDone => {
                let done = envelope
                    .fact
                    .read::<body::TaskDone>()
                    .expect("registered task-done fact decodes");
                if let Some(phase) = phases.get(&done.task).cloned() {
                    authors.insert(done.task, (phase, envelope.actor.clone()));
                }
            }
            wire::FactKind::TaskPhased => {
                let phased = envelope
                    .fact
                    .read::<body::TaskPhased>()
                    .expect("registered task-phased fact decodes");
                if tasks.contains_key(&phased.task) {
                    phases.insert(phased.task, phased.phase);
                }
            }
            wire::FactKind::TaskReviewed => {
                let reviewed = envelope
                    .fact
                    .read::<body::TaskReviewed>()
                    .expect("registered task-reviewed fact decodes");
                if !tasks.contains_key(&reviewed.task) {
                    continue;
                }
                if reviewed.reviewer != envelope.actor {
                    return Err(format!(
                        "task {} review at {} names reviewer {} under fact actor {}",
                        reviewed.task, envelope.seq, reviewed.reviewer, envelope.actor
                    ));
                }
                let implemented = authors.get(&reviewed.task).cloned();
                records
                    .entry(reviewed.task.clone())
                    .or_default()
                    .push(ReviewRecord {
                        task: reviewed.task,
                        position: envelope.seq,
                        implemented,
                        reviewer: reviewed.reviewer,
                        verdict: reviewed.verdict,
                    });
            }
            _ => {}
        }
    }
    Ok(records)
}
