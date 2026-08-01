use crate::task::{Project, TaskError};

/// A report entering the board's oops seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Report {
    /// Tooling friction that should become ordinary wakeable work.
    Friction(Friction),
    /// Evidence worth retaining that must not allocate by side effect.
    Observation(String),
}

/// Authored friction before the authority assigns identity and heads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Friction {
    title: String,
    description: String,
    project: Project,
}

impl Friction {
    /// Build a wakeable friction draft.
    pub fn new(
        title: impl Into<String>,
        description: impl Into<String>,
        project: &str,
    ) -> Result<Self, TaskError> {
        Ok(Self {
            title: title.into(),
            description: description.into(),
            project: project.parse()?,
        })
    }
}

/// Outcome of routing one report.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Routed {
    /// Ordinary task draft; the command transaction supplies its identity.
    Wakeable(TaskDraft),
    /// A note only, never an allocator candidate.
    Note(String),
}

/// Minimum task statement authored by the oops seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskDraft {
    pub title: String,
    pub description: String,
    pub project: Project,
}

/// Route friction into wakeable work and observations into notes.
#[must_use]
pub fn route(report: Report) -> Routed {
    match report {
        Report::Friction(friction) => Routed::Wakeable(TaskDraft {
            title: friction.title,
            description: friction.description,
            project: friction.project,
        }),
        Report::Observation(note) => Routed::Note(note),
    }
}
