//! Actor-facing board intent, evidence, and presentation DTOs.

use serde::{Deserialize, Serialize};

use crate::{Actor, TaskHandle};

/// Read the validated current board, optionally restricted to one family.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardRead {
    /// Upper-case task family to retain, when the caller names one.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub family: Option<String>,
}

/// Render one deterministic graph from the validated current board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardGraph {
    /// Closed board graph view name.
    pub view: String,
    /// Historical barrier requested by the caller, when present.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub at: Option<String>,
}

/// Compose one deterministic handout from the validated current board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardHandout {
    /// Declared output spelling selected by the caller.
    pub output: String,
    /// Historical barrier requested by the caller, when present.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub at: Option<String>,
}

/// Emit the canonical authored-document ledger for one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocLedger {
    /// Task whose current statement becomes canonical Markdown.
    pub task: TaskHandle,
}

/// One authoritative head body together with the fact that established it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Positioned<T> {
    /// Fact position stored beside the current head.
    pub position: crate::Seq,
    /// Typed current head body.
    pub body: T,
}

/// Visibility classification captured when a task project is admitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum ProjectVisibility {
    /// An ordinary scheduler route visible to actors and allocation.
    Public,
    /// A system-owned route retained as evidence but excluded from public work.
    Hidden {
        /// Resolved first project segment, without the leading dot.
        stem: String,
    },
}

impl ProjectVisibility {
    /// Whether this project is system-owned hidden evidence.
    #[must_use]
    pub const fn is_hidden(&self) -> bool {
        matches!(self, Self::Hidden { .. })
    }

    /// Hidden first segment, without the leading dot.
    #[must_use]
    pub fn hidden_stem(&self) -> Option<&str> {
        match self {
            Self::Public => None,
            Self::Hidden { stem } => Some(stem),
        }
    }
}

/// Actor-facing task statement evidence, excluding authored title prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardStatement {
    /// Public task name.
    pub handle: crate::TaskHandle,
    /// Stable task identity.
    pub uuid: crate::CanonicalUuid,
    /// Longer authored context.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub description: Option<String>,
    /// Criteria that decide whether the work is done.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<String>,
    /// Scheduler project grouping.
    pub project: String,
    /// Authored task labels.
    pub tags: Vec<String>,
    /// Tasks that must complete first.
    pub dependencies: Vec<crate::TaskHandle>,
    /// Creation provenance, immutable across statement revisions.
    pub provenance: crate::Origin,
    /// Fact that established this statement revision.
    pub revision: crate::Seq,
}

impl From<&crate::TaskHead> for BoardStatement {
    fn from(statement: &crate::TaskHead) -> Self {
        Self {
            handle: statement.handle.clone(),
            uuid: statement.uuid,
            description: statement.description.clone(),
            acceptance: statement.acceptance.clone(),
            project: statement.project.clone(),
            tags: statement.tags.clone(),
            dependencies: statement.dependencies.clone(),
            provenance: statement.provenance.clone(),
            revision: statement.revision,
        }
    }
}

impl BoardStatement {
    /// Reconstruct a client-side statement using its actor-facing title.
    #[must_use]
    pub fn with_title(&self, title: &str) -> crate::TaskHead {
        crate::TaskHead {
            handle: self.handle.clone(),
            uuid: self.uuid,
            title: title.to_owned(),
            description: self.description.clone(),
            acceptance: self.acceptance.clone(),
            project: self.project.clone(),
            tags: self.tags.clone(),
            dependencies: self.dependencies.clone(),
            provenance: self.provenance.clone(),
            revision: self.revision,
        }
    }
}

/// One complete current task evidence bundle in a board read answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardTask {
    /// Classification derived during board admission from captured policy.
    pub visibility: ProjectVisibility,
    /// Server-resolved actor thread that created this task.
    pub origin_thread: String,
    /// Actor-facing title derived from that classification.
    pub presentation_title: String,
    /// Current statement evidence projected without authored title prose.
    pub statement: Positioned<BoardStatement>,
    pub workflow: Positioned<crate::WorkflowHead>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub claim: Option<Positioned<crate::ClaimHead>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub lease: Option<Positioned<crate::LeaseHead>>,
}

/// Server-resolved public task route metadata for one board snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardProjectPolicy {
    /// Public project roots admitted by the resident, in lexical order.
    pub public_stems: Vec<String>,
    /// Fewest dotted segments admitted for a public project.
    pub public_min_depth: u64,
    /// Most dotted segments admitted for a public project.
    pub public_max_depth: u64,
}

/// Server-resolved predecessor-compatible route for one board principal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardActorRoute {
    /// Actor identity used by predecessor task routing.
    pub actor: String,
    /// Exact actor-private project selected by the route.
    pub project: String,
    /// Exact creation-origin thread selected by the route.
    pub origin_thread: String,
}

impl BoardTask {
    /// Project authoritative heads into one actor-facing task row.
    #[must_use]
    pub fn from_authoritative(
        visibility: ProjectVisibility,
        origin_thread: String,
        statement: Positioned<crate::TaskHead>,
        workflow: Positioned<crate::WorkflowHead>,
        claim: Option<Positioned<crate::ClaimHead>>,
        lease: Option<Positioned<crate::LeaseHead>>,
    ) -> Self {
        let presentation_title = statement.body.title.clone();
        let statement = Positioned {
            position: statement.position,
            body: BoardStatement::from(&statement.body),
        };
        Self {
            visibility,
            origin_thread,
            presentation_title,
            statement,
            workflow,
            claim,
            lease,
        }
        .into_actor_facing()
    }

    /// Normalize this row to the title its actor may retain.
    #[must_use]
    pub fn into_actor_facing(mut self) -> Self {
        if self.visibility.is_hidden() {
            self.presentation_title = "Hidden task".to_owned();
            self.statement.body.description = None;
            self.statement.body.acceptance.clear();
        }
        self
    }

    /// Client-side statement reconstructed from actor-facing evidence.
    #[must_use]
    pub fn client_statement(&self) -> crate::TaskHead {
        self.statement.body.with_title(&self.presentation_title)
    }

    /// Actor-facing title that does not expose hidden system task prose.
    #[must_use]
    pub fn presentation_title(&self) -> &str {
        &self.presentation_title
    }

    /// Whether this row belongs to pending public work inventory.
    #[must_use]
    pub fn is_active_public(&self) -> bool {
        !self.visibility.is_hidden()
            && self.workflow.body.status == "pending"
            && self.workflow.body.completion.is_none()
    }
}

/// Validated current board heads returned from one snapshot barrier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardAnswer {
    /// Journal schema that owns the returned heads.
    pub schema: u32,
    /// Greatest durable fact position covered by every returned head.
    pub cursor: crate::Seq,
    /// Principal derived from the capability that requested this snapshot.
    pub principal: Actor,
    /// Public project route captured by the authority that validated the tasks.
    pub project_policy: BoardProjectPolicy,
    /// Complete actor-specific route captured by the snapshot authority.
    pub actor_route: BoardActorRoute,
    /// Complete current task evidence bundles in ascending handle order.
    pub tasks: Vec<BoardTask>,
    /// Current resident and lane head used by possession witnesses.
    pub residency: Positioned<crate::ResidencyHead>,
}

impl BoardAnswer {
    /// Pending public routes only; hidden evidence never enters active inventory.
    pub fn active_public_tasks(&self) -> impl Iterator<Item = &BoardTask> {
        self.tasks.iter().filter(|task| task.is_active_public())
    }
}

/// One deterministic resident-rendered document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextAnswer {
    pub text: String,
}
