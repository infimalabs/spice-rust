use std::fmt::Write;

use crate::task::Entry;
use wire::ClaimDisposition;

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

/// Render one named view from current authoritative heads.
#[must_use]
pub fn render(view: View, entries: &[Entry]) -> Diagram {
    let mut ordered: Vec<&Entry> = entries.iter().collect();
    ordered.sort_by(|left, right| left.handle().cmp(right.handle()));
    let handles = entries
        .iter()
        .map(|entry| (entry.incepted(), entry.handle()))
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
            for entry in ordered {
                let workflow = entry.workflow();
                if let Some(reviewer) = &workflow.assignment {
                    writeln!(
                        text,
                        "{} -> {} [{}]",
                        reviewer,
                        entry.handle(),
                        workflow.review
                    )
                    .expect("String writes are infallible");
                } else if workflow.review != "unassigned" {
                    writeln!(text, "{}: {}", entry.handle(), workflow.review)
                        .expect("String writes are infallible");
                }
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

const fn disposition(disposition: ClaimDisposition) -> &'static str {
    match disposition {
        ClaimDisposition::Active => "active",
        ClaimDisposition::Freed => "freed",
        ClaimDisposition::Lapsed => "lapsed",
    }
}
