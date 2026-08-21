use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::{self, Write};

use crate::task::Entry;
use wire::{Actor, ClaimDisposition, Phase, Seq, TaskHandle, TaskStamp, body};

/// Named deterministic diagrams the board exposes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Origin,
    Dependencies,
    Review,
    Workflow,
    Claims,
}

impl View {
    /// Every named view in handout order.
    pub const ALL: [Self; 5] = [
        Self::Origin,
        Self::Dependencies,
        Self::Review,
        Self::Workflow,
        Self::Claims,
    ];

    /// Stable view name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Origin => "origin",
            Self::Dependencies => "dependencies",
            Self::Review => "review",
            Self::Workflow => "workflow",
            Self::Claims => "claims",
        }
    }
}

/// One rendered board diagram.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagram {
    view: View,
    text: String,
}

/// One completed durable review joined to the author of the reviewed phase.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ReviewEdge {
    task: TaskHandle,
    phase: Phase,
    reviewer: Actor,
    author: Actor,
    verdict: body::Verdict,
    reviewed: Seq,
}

impl ReviewEdge {
    /// Build one already-validated historical review relation.
    #[must_use]
    pub const fn new(
        task: TaskHandle,
        phase: Phase,
        reviewer: Actor,
        author: Actor,
        verdict: body::Verdict,
        reviewed: Seq,
    ) -> Self {
        Self {
            task,
            phase,
            reviewer,
            author,
            verdict,
            reviewed,
        }
    }
}

/// A completed review could not be joined to durable phase-author evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReviewHistoryError {
    /// A visible task review has no preceding completed phase author.
    AuthorMissing { task: TaskStamp, reviewed: Seq },
    /// The review body contradicts the authenticated fact actor.
    ReviewerMismatch {
        task: TaskStamp,
        body: Actor,
        fact: Actor,
        reviewed: Seq,
    },
}

impl fmt::Display for ReviewHistoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AuthorMissing { task, reviewed } => write!(
                formatter,
                "task {task} review at {reviewed} has no preceding phase author"
            ),
            Self::ReviewerMismatch {
                task,
                body,
                fact,
                reviewed,
            } => write!(
                formatter,
                "task {task} review at {reviewed} names reviewer {body} under fact actor {fact}"
            ),
        }
    }
}

impl Error for ReviewHistoryError {}

/// Join completed review facts to the most recent completed phase author.
///
/// Only tasks present in `entries` contribute edges, preserving the board's
/// actor-filtered visibility boundary. Input is sorted by durable position so
/// callers cannot change history by changing collection order.
pub fn review_history<'a>(
    facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
    entries: &[Entry],
) -> Result<Vec<ReviewEdge>, ReviewHistoryError> {
    let handles = entries
        .iter()
        .map(|entry| (entry.born().clone(), entry.handle().clone()))
        .collect::<BTreeMap<_, _>>();
    let mut ordered = facts.into_iter().collect::<Vec<_>>();
    ordered.sort_by_key(|envelope| envelope.seq);
    let mut phases = BTreeMap::<TaskStamp, Phase>::new();
    let mut authors = BTreeMap::<TaskStamp, (Phase, Actor)>::new();
    let mut reviews = Vec::new();
    for envelope in ordered {
        match envelope.fact.kind() {
            wire::FactKind::TaskAdded => {
                let added = envelope
                    .fact
                    .read::<body::TaskAdded>()
                    .expect("registered task-added fact decodes");
                if handles.contains_key(&added.task) {
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
                if handles.contains_key(&phased.task) {
                    phases.insert(phased.task, phased.phase);
                }
            }
            wire::FactKind::TaskReviewed => {
                let reviewed = envelope
                    .fact
                    .read::<body::TaskReviewed>()
                    .expect("registered task-reviewed fact decodes");
                let Some(task) = handles.get(&reviewed.task) else {
                    continue;
                };
                if reviewed.reviewer != envelope.actor {
                    return Err(ReviewHistoryError::ReviewerMismatch {
                        task: reviewed.task,
                        body: reviewed.reviewer,
                        fact: envelope.actor.clone(),
                        reviewed: envelope.seq,
                    });
                }
                let Some((phase, author)) = authors.get(&reviewed.task) else {
                    return Err(ReviewHistoryError::AuthorMissing {
                        task: reviewed.task,
                        reviewed: envelope.seq,
                    });
                };
                reviews.push(ReviewEdge::new(
                    task.clone(),
                    phase.clone(),
                    reviewed.reviewer,
                    author.clone(),
                    reviewed.verdict,
                    envelope.seq,
                ));
            }
            _ => {}
        }
    }
    reviews.sort();
    Ok(reviews)
}

impl Diagram {
    /// Diagram identity.
    #[must_use]
    pub const fn view(&self) -> View {
        self.view
    }

    /// Deterministic textual rendering.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// Render one named view from current heads and durable review history.
#[must_use]
pub fn render(view: View, entries: &[Entry], reviews: &[ReviewEdge]) -> Diagram {
    let mut ordered: Vec<&Entry> = entries.iter().collect();
    ordered.sort_by(|left, right| left.handle().cmp(right.handle()));
    let handles = entries
        .iter()
        .map(|entry| (entry.born(), entry.handle()))
        .collect::<std::collections::BTreeMap<_, _>>();

    let mut text = String::new();
    writeln!(text, "graph {}", view.as_str()).expect("String writes are infallible");
    match view {
        View::Origin => {
            for entry in ordered {
                writeln!(
                    text,
                    "{} -> {}",
                    entry.statement().provenance,
                    entry.handle()
                )
                .expect("String writes are infallible");
            }
        }
        View::Dependencies => {
            for entry in ordered {
                for edge in entry.dependencies() {
                    let task = handles.get(&edge.task).copied();
                    let after = handles.get(&edge.after).copied();
                    writeln!(
                        text,
                        "{} -> {}",
                        task.map_or(edge.task.as_str(), wire::TaskHandle::as_str),
                        after.map_or(edge.after.as_str(), wire::TaskHandle::as_str)
                    )
                    .expect("String writes are infallible");
                }
            }
        }
        View::Review => {
            let mut reviews = reviews.to_vec();
            reviews.sort();
            for edge in reviews {
                writeln!(
                    text,
                    "{} -> {} [{} {} {}]",
                    edge.reviewer,
                    edge.author,
                    edge.task,
                    edge.phase,
                    verdict(edge.verdict),
                )
                .expect("String writes are infallible");
            }
        }
        View::Workflow => {
            for entry in ordered {
                writeln!(
                    text,
                    "{}: {} [{}]",
                    entry.handle(),
                    entry.workflow().phase,
                    entry.workflow().status
                )
                .expect("String writes are infallible");
            }
        }
        View::Claims => {
            for entry in ordered {
                if let Some(claim) = entry.claim() {
                    writeln!(
                        text,
                        "{}: {} [{}]",
                        entry.handle(),
                        claim.actor,
                        disposition(claim.disposition)
                    )
                    .expect("String writes are infallible");
                }
            }
        }
    }

    Diagram { view, text }
}

const fn verdict(verdict: body::Verdict) -> &'static str {
    match verdict {
        body::Verdict::Clean => "clean",
        body::Verdict::Finding => "finding",
    }
}

const fn disposition(disposition: ClaimDisposition) -> &'static str {
    match disposition {
        ClaimDisposition::Active => "active",
        ClaimDisposition::Freed => "freed",
        ClaimDisposition::Lapsed => "lapsed",
    }
}

#[cfg(test)]
mod tests;
