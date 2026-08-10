use std::fmt::Write;

use crate::task::Entry;
use wire::ClaimDisposition;

/// Named deterministic diagrams the board exposes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Dependencies,
    Workflow,
    Claims,
}

impl View {
    /// Every named view in handout order.
    pub const ALL: [Self; 3] = [Self::Dependencies, Self::Workflow, Self::Claims];

    /// Stable view name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dependencies => "dependencies",
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
