//! Current authoritative content owned by the journal head tables.
//!
//! These are wire DTOs, not alternate authorities. The journal updates them
//! in the same transaction as the facts that justify the new current state.

use serde::{Deserialize, Serialize};

use crate::Name;
use crate::body::{
    DirectiveAttachment, LaneLaunchIdentity, SteeringStage, SteeringTransition, TranscriptPosition,
};
use crate::envelope::{Actor, Origin, Seq};
use crate::ident::{CanonicalUuid, Digest, Instant};
use crate::witness::{Disposition, Phase, TaskStamp};

mod workflow;

pub use workflow::{
    WorkflowCompletionReview, WorkflowConclusion, WorkflowHead, WorkflowNegative, WorkflowReview,
    WorkflowStatus, WorkflowStatusKind,
};

/// The (project, phase) cell a claim occupied when it was won.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimCell {
    /// Project occupied by the claimed task.
    pub project: String,
    /// Workflow phase occupied by the claimed task.
    pub phase: Phase,
}

/// Declared planning purpose of a task, independent of its workflow phase.
///
/// A task's phase says which step of its flow is current; its intent says
/// what the task as a whole exists to deliver. A Plan constructs and reviews
/// executable contracts; a Design decides which assembly is justified, or
/// concludes that none is. The repository-owned opportunistic Design has the
/// same Design semantics but is not an author-declarable intent. Neither grants
/// permission to implement or authorizes the outputs it plans. A task without
/// declared intent carries no value: intent is never inferred from a flow or a
/// phase name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskIntent {
    /// Construct and review executable contracts.
    Plan,
    /// Justify an assembly, or a negative disposition.
    Design,
    /// Repository-owned opportunistic OOPS Design generation.
    #[serde(rename = "oops-design")]
    OpportunisticDesign,
}

impl TaskIntent {
    /// Every author-declarable intent, in presentation order.
    pub const ALL: [Self; 2] = [Self::Plan, Self::Design];

    /// Whether this intent follows the Design lifecycle.
    #[must_use]
    pub const fn is_design(self) -> bool {
        matches!(self, Self::Design | Self::OpportunisticDesign)
    }

    /// Whether this is the repository-owned opportunistic Design intent.
    #[must_use]
    pub const fn is_opportunistic_design(self) -> bool {
        matches!(self, Self::OpportunisticDesign)
    }

    /// Stable text spelling shared by the wire and the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Design => "design",
            Self::OpportunisticDesign => "oops-design",
        }
    }
}
/// Public owner of the repository's opportunistic OOPS Design.
pub const OOPS_DESIGN_PROJECT: &str = "oops.design";
/// Fixed title of every opportunistic OOPS Design generation.
pub const OOPS_DESIGN_TITLE: &str = "OOPS";
/// Stable context of every opportunistic OOPS Design generation: the holder's
/// job, stated so an agent works the reports as a whole.
pub const OOPS_DESIGN_DESCRIPTION: &str = "Every `spice task oops` report lands here as a Plan \
gated by this Design. Like any Design, it is worked once an Explore or Converge group holds it.

Work the reports as a whole, not line by line. Read each pending report, its origin, and the \
code it names. Group reports that share a cause, find the defect behind a cluster of symptoms, \
and note reports that another task or another report already covers.

Give each cause one output: rewrite its report Plan's title, description, and acceptance with \
`spice task modify` to name the real fix, or file a better-scoped task under this Design. Name \
in the justification every report the operator should reject, and why. Propose with \
`spice preparation propose` only when every output carries concrete acceptance, citing the \
evidence.";
/// Fixed pitch of every opportunistic OOPS Design generation.
pub const OOPS_DESIGN_PITCH: &str = "Unexpected behavior lands here as reports that an agent \
triages as a whole into one proposal of real fixes.";
/// What every opportunistic OOPS Design generation's holder must deliver.
pub const OOPS_DESIGN_ACCEPTANCE: [&str; 3] = [
    "Every pending report is read and is an output with concrete acceptance, folded into one, \
     or named for rejection with a reason.",
    "Reports that share a cause become one output that names the cause and its fix.",
    "The proposal justification lists the clusters, their evidence, and what was set aside and \
     why.",
];

impl std::fmt::Display for TaskIntent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for TaskIntent {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|intent| intent.as_str() == text)
            .ok_or_else(|| format!("task intent {text:?} is not one of plan, design"))
    }
}

/// One explicit change to a task's declared intent.
///
/// Omission means unchanged; declaring and withdrawing are distinct
/// spellings, so no statement correction can move intent by accident.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase", deny_unknown_fields)]
pub enum TaskIntentEdit {
    /// Declare this intent, replacing any previous one.
    Set {
        /// Intent the task declares from this revision on.
        intent: TaskIntent,
    },
    /// Withdraw the declared intent.
    Clear {},
}

impl TaskIntentEdit {
    /// Declared intent after this change.
    #[must_use]
    pub const fn intent(self) -> Option<TaskIntent> {
        match self {
            Self::Set { intent } => Some(intent),
            Self::Clear {} => None,
        }
    }
}

/// Explicit association for preparation within an existing planning context.
///
/// An unassociated Plan or Design owns its own controller and construction
/// context. Corrections keep the original construction owner instead of
/// acquiring a fresh deliverable merely by entering another phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskPreparation {
    /// Immediate planning task whose context this preparation joins.
    pub parent: TaskStamp,
    /// Native Plan or Design task controlling the eventual output selection.
    pub controller: TaskStamp,
    /// Native task whose construction contract this preparation carries.
    pub construction: TaskStamp,
}

/// Whether a task is filed allocator work or prospective work its author has
/// not filed yet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskFiling {
    /// Ordinary work admitted to its workflow and allocator.
    #[default]
    Filed,
    /// Editable prospective work that asserts nothing until it is filed.
    Draft,
}

impl TaskFiling {
    /// Whether this task participates in ordinary filed work.
    #[must_use]
    pub const fn is_filed(&self) -> bool {
        matches!(self, Self::Filed)
    }

    /// Whether this task is an unfiled draft.
    #[must_use]
    pub const fn is_draft(&self) -> bool {
        matches!(self, Self::Draft)
    }

    /// Stable lower-case spelling shared by the wire and the command line.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Filed => "filed",
            Self::Draft => "draft",
        }
    }
}

/// Declared scheduling priority of a task.
///
/// Absence is distinct from [`Self::None`]: a task that declared `none` still
/// outranks one that declared nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskPriority {
    /// Ahead of every other declared priority.
    Critical,
    /// Ahead of medium.
    High,
    /// Ahead of low.
    Medium,
    /// Ahead of none.
    Low,
    /// Declared, and ahead only of undeclared work.
    None,
}

impl TaskPriority {
    /// Every priority, strongest first.
    pub const ALL: [Self; 5] = [
        Self::Critical,
        Self::High,
        Self::Medium,
        Self::Low,
        Self::None,
    ];

    /// Stable lower-case spelling shared by the wire and the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Critical => "critical",
            Self::High => "high",
            Self::Medium => "medium",
            Self::Low => "low",
            Self::None => "none",
        }
    }

    /// The priority one exact stored spelling names.
    #[must_use]
    pub fn from_wire(text: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|priority| priority.as_str() == text)
    }
}

impl std::fmt::Display for TaskPriority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Board-owned hold keeping a task in a plan phase until its statement passes
/// taste review.
///
/// Authors never write it; task creation, filing, and statement edits set it
/// from the repository taste policy, and an edit that passes clears it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TasteHold {
    /// Where the plan phase holding the task came from.
    pub plan: TastePlan,
    /// Every place the latest policy read found a taste in the statement.
    pub matches: Vec<TasteMatch>,
    /// Statement revisions read since the hold began that still matched.
    pub rechecks: u32,
}

impl TasteHold {
    /// Withhold what each match found, keeping where it sits and which taste
    /// it names.
    ///
    /// An author told the matched word swaps the word instead of rewriting the
    /// statement, so a lane reads the field, line, and taste of each match and
    /// the operator who owns the policy reads the rest.
    pub fn withhold_findings(&mut self) {
        for found in &mut self.matches {
            found.finding = None;
        }
    }
}

/// Where the plan phase of a taste hold came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TastePlan {
    /// The authored flow already opened in plan.
    Authored,
    /// The hold prepended plan to the authored flow; clearing the hold
    /// withdraws that phase.
    Prepended,
}

/// One place a held statement matched a taste.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TasteMatch {
    /// Statement field the match sits in.
    pub field: StatementField,
    /// One-based line of the match within that field's text, or within one
    /// acceptance criterion's text.
    pub line: usize,
    /// Taste that matched.
    pub taste: String,
    /// What matched and why; absent only from a lane's read.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub finding: Option<TasteFinding>,
}

/// The matched text of one taste match and the taste's own message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TasteFinding {
    /// Trigger text the taste selected.
    pub trigger: String,
    /// One-based byte column at which the trigger begins.
    pub column: usize,
    /// The taste's message to the author.
    pub message: String,
}

/// Authored statement field a taste reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatementField {
    /// The statement title.
    Title,
    /// The longer authored context.
    Description,
    /// One acceptance criterion.
    Acceptance,
    /// The one-sentence pitch.
    Pitch,
}

impl StatementField {
    /// Stored spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Pitch => "pitch",
            Self::Description => "description",
            Self::Acceptance => "acceptance",
        }
    }
}

/// One current task statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskHead {
    /// Sole task identity, minted from its birth instant.
    pub born: TaskStamp,
    /// Fact that first placed this task in the queue.
    pub queue_origin: Seq,
    /// Authored title.
    pub title: String,
    /// One plain sentence saying why the task matters.
    pub pitch: String,
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
    /// Typed facts the execution environment must satisfy together.
    #[serde(
        default,
        skip_serializing_if = "crate::EnvironmentRequirements::is_empty",
        deserialize_with = "crate::EnvironmentRequirements::present"
    )]
    pub requirements: crate::EnvironmentRequirements,
    /// Scheduler project grouping.
    pub project: String,
    /// Filing standing, independent of project identity and visibility.
    #[serde(default, skip_serializing_if = "TaskFiling::is_filed")]
    pub filing: TaskFiling,
    /// Declared scheduling priority.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub priority: Option<TaskPriority>,
    /// Board-owned taste-review hold; absent while the statement is not held.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub taste: Option<TasteHold>,
    /// Slug of the task-document node a document apply placed this task
    /// under; a document row matches its task by project plus this slug.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub document_slug: Option<String>,
    /// Tasks that must complete first.
    pub dependencies: Vec<TaskStamp>,
    /// Declared Plan or Design intent, stable across every phase change.
    ///
    /// Only creation and an explicit [`TaskIntentEdit`] admitted through a
    /// statement correction establish it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub intent: Option<TaskIntent>,
    /// Explicit preparation context; omission declares an independent task.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub preparation: Option<TaskPreparation>,
    /// Creation provenance, immutable across statement revisions.
    pub provenance: crate::TaskProvenance,
    /// Creation-time transcript authority, immutable across statement revisions.
    pub creation: crate::body::TaskCreationContext,
    /// Fact that established this statement revision.
    pub revision: Seq,
}

impl TaskHead {
    /// Whether this task is the Design itself, currently waits on it, or
    /// retains it as the preparation controller after selection.
    #[must_use]
    pub fn is_associated_with_design(&self, design: &TaskStamp) -> bool {
        design_associated(
            &self.born,
            &self.dependencies,
            self.preparation.as_ref(),
            design,
        )
    }
}

pub(crate) fn design_associated(
    born: &TaskStamp,
    dependencies: &[TaskStamp],
    preparation: Option<&TaskPreparation>,
    design: &TaskStamp,
) -> bool {
    born == design
        || dependencies.contains(design)
        || preparation.is_some_and(|preparation| preparation.controller == *design)
}

/// Current disposition of the most recent winning claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClaimDisposition {
    /// The winning actor still owns the claim.
    Active,
    /// The winning actor released the claim.
    Freed,
    /// The winning actor's lease elapsed.
    Lapsed,
}

/// One current claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimHead {
    /// Actor that won possession.
    pub actor: Actor,
    /// Fact that established the current claim-head disposition.
    pub fact: Seq,
    /// Fact that established the most recent win, unchanged by its disposition.
    pub claimed: Seq,
    /// Project and phase occupied when the most recent win was minted.
    pub cell: ClaimCell,
    /// Lane bound to the claim.
    pub lane: CanonicalUuid,
    /// Occupant bound to the claim.
    pub occupant: CanonicalUuid,
    /// Resident generation under which the claim was won.
    pub epoch: CanonicalUuid,
    /// Dirty-handoff transaction whose replay this claim authorizes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub recovery: Option<String>,
    /// Whether the win remains active or how it ended.
    pub disposition: ClaimDisposition,
    /// Task generation this claim has acknowledged.
    ///
    /// A win records the task's generation at possession, assigned by the
    /// journal at commit. Automatic renewal and resident carry preserve it.
    /// An explicit reclaim advances it, and so does a mutation its own current
    /// holder commits; a claim that trails stays behind until reclaimed.
    /// Completion and review refuse while it trails the task generation.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub acknowledged: u64,
}

const fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl ClaimHead {
    /// Whether this claim trails a task mutation it has not acknowledged.
    #[must_use]
    pub const fn is_stale(&self, workflow: &WorkflowHead) -> bool {
        self.acknowledged < workflow.generation
    }

    /// Whether a lane still holds this claim; ended claims keep their head.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(self.disposition, ClaimDisposition::Active)
    }
}

/// One current claim lease.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LeaseHead {
    /// Instant at which the lease began.
    pub start: Instant,
    /// Most recent renewal instant.
    pub renewal: Instant,
    /// Maximum instant beyond which renewal may not extend possession.
    pub maximum: Instant,
    /// Current expiry instant.
    pub expiry: Instant,
    /// Resident generation permitted to advance this lease.
    pub epoch: CanonicalUuid,
}

/// Current repository OOPS generation.
///
/// No stored head means revision zero and vacancy. Every later head is folded
/// from one [`crate::body::OopsGenerationAdvanced`] fact.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OopsGenerationHead {
    /// Monotone transition revision.
    pub revision: u64,
    /// Sole current open opportunistic Design, absent between generations.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub design: Option<TaskStamp>,
}

/// Closed classes of authoritative witness heads.
///
/// This classifies seven stored predicate-head rows. Command preconditions use
/// the closed [`crate::witness::WitnessKind`] vocabulary instead. They
/// overlap on assignment, directive, consent, budget, and transition, but
/// neither contains the other: only heads name review and document classes,
/// while only commands name task, workflow, claim, and other concrete subjects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WitnessClass {
    /// Allocation or reviewer assignment.
    Assignment,
    /// Review authorship or verdict state.
    Review,
    /// Directive disposition or lineage.
    Directive,
    /// Consent request disposition.
    Consent,
    /// Named budget state.
    Budget,
    /// Normalized document barrier.
    Document,
    /// Subject transition precondition.
    Transition,
}

/// One named predicate later commands must re-prove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WitnessHead {
    /// Stable predicate name.
    pub name: String,
    /// Predicate class from WIRE's closed authority list.
    pub kind: WitnessClass,
    /// Canonical predicate statement.
    pub predicate: String,
    /// Fact that established the current predicate.
    pub head: Seq,
}

/// Approval state shared by durable trust heads, command witnesses, and answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustState {
    /// Changed bytes await an operator decision.
    Pended,
    /// The operator approved this exact digest.
    Granted,
    /// The operator withdrew approval from this exact digest.
    Revoked,
}

/// One executable digest's current approval state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustHead {
    /// Digest of the executable surface.
    pub digest: Digest,
    /// Current operator approval state.
    pub state: TrustState,
    /// Canonical worktree whose executable surface produced this digest.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub worktree: Option<String>,
}

/// Current posture of a gate finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FindingState {
    /// The finding still applies.
    Open,
    /// A later receipt cleared the finding.
    Cleared,
}

/// One current gate finding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindingHead {
    /// Stable finding identity.
    pub identity: String,
    /// Gate-defined severity.
    pub severity: String,
    /// Evidence location the finding cites.
    pub locus: String,
    /// Owner responsible for resolution.
    pub owner: String,
    /// Receipt that originated the current finding.
    pub receipt: Digest,
    /// Whether the finding remains open.
    pub state: FindingState,
}

/// One directive's complete current steering standing.
///
/// The immutable facts remain the audit authority. This row is their
/// transactionally maintained current head: keyed by both `directive` and
/// `key`, ordered by `created`, and sufficient to admit the next transition
/// without replaying unrelated journal history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteeringHead {
    /// Stable directive identity.
    pub directive: CanonicalUuid,
    /// Operator submit action shared by target-specific fanout directives.
    ///
    /// Absent on directives no operator submit fanned out: resident control
    /// directives, taste reminders and confirmations, and claim-lease warnings.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub fanout: Option<CanonicalUuid>,
    /// Human-sized key retained for the directive's complete lifetime.
    pub key: String,
    /// Lane the directive addresses.
    pub lane: CanonicalUuid,
    /// Original direction text.
    pub text: String,
    /// Seated lane that told this directive as a peer message.
    ///
    /// Absent on operator and resident direction, which is every fact written
    /// before peer messages existed. Present only when the creating fact's
    /// actor is exactly this lane.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub from: Option<CanonicalUuid>,
    /// Anchor task whose room context a peer route froze at publication.
    ///
    /// Absent on non-message direction and direct peer routes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub room: Option<crate::rooms::RoomAnchor>,
    /// Optional steering control driven by the directive.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub control: Option<String>,
    /// Opaque attachments carried by the original direction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<DirectiveAttachment>,
    /// Instant the creating fact was recorded.
    pub published: Instant,
    /// Current expiry, advanced by recovery.
    pub expires: Instant,
    /// Current presentation stage.
    pub stage: SteeringStage,
    /// Ordered presentation audit, bounded by the closed stage vocabulary.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub escalations: Vec<SteeringTransition>,
    /// Current directive disposition.
    pub disposition: Disposition,
    /// Causal origin retained from the creating fact.
    pub origin: Origin,
    /// Creating fact position retained as the cause of later transitions.
    pub created: Seq,
    /// Normalized Unix-microsecond moment for a key minted by a time codec.
    ///
    /// Legacy keys outside that codec leave this absent. The maximum present
    /// value lets new key allocation jump past all historical collisions in
    /// one indexed lookup.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub minted_micros: Option<i64>,
}

/// How far one logical transcript stream has been accounted.
///
/// The row key is [`Self::key`], recomposed from the three logical fields, so a
/// stored key that disagrees with its body is a head violation rather than a
/// second spelling of the same stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptSourceHead {
    /// Logical lane whose transcript this head accounts.
    pub lane: CanonicalUuid,
    /// Vendor that produced the transcript.
    pub vendor: Name,
    /// Logical stream within that lane and vendor.
    pub stream: String,
    /// Opaque durable source generation currently accounted.
    pub source: CanonicalUuid,
    /// Position the next batch must continue from.
    pub next: TranscriptPosition,
}

impl TranscriptSourceHead {
    /// Canonical row key for this logical stream.
    #[must_use]
    pub fn key(&self) -> TranscriptStreamKey {
        TranscriptStreamKey::new(&self.lane, &self.vendor, &self.stream)
    }
}

/// The row key addressing one logical transcript stream.
///
/// identity-law: non-crossing - recomposed from a head body on each side, never
/// encoded. The durable spelling is [`TranscriptSourceHead`] itself, which is
/// what makes a stored key disagreeing with its body a head violation.
///
/// A stream name alone does not identify a stream: the row is keyed by lane,
/// vendor and stream together, and the same stream name is expected under
/// several lanes and vendors. This carries the composed key so a reader cannot
/// supply one component and silently miss the row -- a miss that reads as *this
/// stream was never accounted*, which is the answer that makes a catch-up
/// re-ingest from the beginning and spend every sample twice.
///
/// The field is private and there is no conversion from a string, so the only
/// way to obtain one is to hold all three fields. An already-composed spelling
/// cannot be handed back in:
///
/// ```compile_fail
/// let _ = wire::TranscriptStreamKey("lane/vendor/stream".to_owned());
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TranscriptStreamKey(String);

impl TranscriptStreamKey {
    /// Compose the key from the three fields that together name a stream.
    #[must_use]
    pub fn new(lane: &CanonicalUuid, vendor: &Name, stream: &str) -> Self {
        Self(format!("{lane}/{vendor}/{stream}"))
    }

    /// The composed key as it is stored.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the key for storage.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

/// Resident lifecycle shared by durable heads, transition witnesses, intents, and answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResidentState {
    /// The resident holds authority and admits work.
    Seated,
    /// Admission stopped for graceful shutdown.
    Quiescing,
    /// Admitted work and occupants are ending.
    Draining,
    /// Grace elapsed and unfinished occupants are ending.
    Terminating,
    /// Bounded escalation is ending surviving occupants.
    Killing,
    /// Claims, seals, and residue are being verified.
    Verifying,
    /// Coordination authority released; detached lane supervisors may remain live.
    Dark,
}

/// Exact durable bytes that declared one vendor session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TapeLocus {
    /// Tape member holding the declaration.
    pub member: u64,
    /// Byte offset within the member.
    pub offset: u64,
    /// Number of declaration bytes.
    pub length: u64,
}

/// Current resumable session authority for one lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LaneSessionWire", into = "LaneSessionWire")]
pub struct LaneSession {
    /// Lane whose successor may resume this session.
    pub lane: CanonicalUuid,
    /// Occupant whose selected stream proved the identity.
    pub occupant: CanonicalUuid,
    /// Canonical worktree bound to the launch.
    pub worktree: String,
    /// Vendor dialect that admitted the declaration.
    pub vendor: crate::Name,
    /// Exact launch identity admitted for this session.
    pub launch: LaneLaunchIdentity,
    /// Resident generation that observed the declaration.
    pub epoch: CanonicalUuid,
    /// Selected child stream carrying the declaration.
    pub stream: String,
    /// Exact vendor session identity.
    pub identity: String,
    /// Durable bytes that declared the identity.
    pub locus: TapeLocus,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneSessionWire {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    worktree: String,
    vendor: crate::Name,
    model: String,
    effort: String,
    #[serde(default, skip_serializing_if = "is_false")]
    fast_mode: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    fast_refusal: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    git: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    rtk: Option<crate::body::RtkHealth>,
    epoch: CanonicalUuid,
    stream: String,
    identity: String,
    locus: TapeLocus,
}

impl TryFrom<LaneSessionWire> for LaneSession {
    type Error = &'static str;

    fn try_from(wire: LaneSessionWire) -> Result<Self, Self::Error> {
        let mut launch = LaneLaunchIdentity::from_parts(
            wire.model,
            wire.effort,
            wire.fast_mode,
            wire.git,
            wire.rtk,
        );
        launch.fast_refusal = wire.fast_refusal;
        Ok(Self {
            lane: wire.lane,
            occupant: wire.occupant,
            worktree: wire.worktree,
            vendor: wire.vendor,
            launch,
            epoch: wire.epoch,
            stream: wire.stream,
            identity: wire.identity,
            locus: wire.locus,
        })
    }
}

impl From<LaneSession> for LaneSessionWire {
    fn from(mut session: LaneSession) -> Self {
        let fast_refusal = session.launch.fast_refusal.take();
        let (model, effort, fast_mode, git, rtk) = LaneLaunchIdentity::into_parts(session.launch);
        Self {
            lane: session.lane,
            occupant: session.occupant,
            worktree: session.worktree,
            vendor: session.vendor,
            model,
            effort,
            fast_mode,
            fast_refusal,
            git,
            rtk,
            epoch: session.epoch,
            stream: session.stream,
            identity: session.identity,
            locus: session.locus,
        }
    }
}

/// Capability-free identity retained across one agent succession.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenewalIdentity {
    /// Occupant identity assigned by the resident.
    pub occupant: CanonicalUuid,
    /// Exact resumable vendor session identity.
    pub session: String,
    /// Selected transcript thread or stream identity.
    pub thread: String,
}

/// Closed durable stages of one agent renewal handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum LaneRenewalState {
    /// The operator requested renewal but the predecessor is still running.
    Requested {
        /// Current occupant being replaced.
        predecessor: RenewalIdentity,
        /// Original member position inside the app team.
        team_slot: u64,
    },
    /// The predecessor reached its stopping boundary and succession owns the slot.
    HandoffPending {
        /// Stopped occupant whose context the successor receives.
        predecessor: RenewalIdentity,
        /// Original member position inside the app team.
        team_slot: u64,
    },
    /// A fresh successor started and replaced the predecessor in the same slot.
    SuccessorStarted {
        /// Occupant that handed off the lane.
        predecessor: RenewalIdentity,
        /// Fresh occupant now owning the lane.
        successor: RenewalIdentity,
        /// Original member position inside the app team.
        team_slot: u64,
    },
    /// Successor launch refused after the predecessor reached handoff.
    LaunchRefused {
        /// Occupant whose handoff remains pending repair.
        predecessor: RenewalIdentity,
        /// Original member position inside the app team.
        team_slot: u64,
        /// Exact actionable launch refusal.
        reason: String,
    },
}

/// Current durable renewal state for one independently steerable lane member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneRenewalRecord {
    /// App team retaining the original team slot.
    pub team: CanonicalUuid,
    /// Independently steerable member undergoing succession.
    pub lane: CanonicalUuid,
    /// Topology revision that established this state.
    pub revision: u64,
    /// Current closed handoff stage and its stage-specific identity evidence.
    pub state: LaneRenewalState,
}

/// One lane's current place in the resident generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneResidency {
    /// Address of this independently steerable team member.
    pub lane: CanonicalUuid,
    /// App team whose lane contains this member.
    pub team: CanonicalUuid,
    /// Operator-configured agent display name, absent on pre-field seats.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub name: Option<String>,
    /// Canonical worktree permanently bound to the lane.
    pub worktree: String,
    /// Closed lane-life spelling governing re-seating.
    pub life: String,
    /// Vendor selected for each occupant in this lane.
    pub vendor: crate::Name,
    /// Model spelling selected for this seat, absent when the vendor's
    /// configured model governs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub model: Option<String>,
    /// Effort spelling selected for this seat, absent when the vendor's
    /// configured effort governs.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub effort: Option<String>,
    /// Whether this lane itself enables fast mode. Fast mode is additive: the
    /// lane runs fast when this, its team's flag, or the global setting is on.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fast_mode: bool,
    /// Current occupant, absent between seats.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub occupant: Option<CanonicalUuid>,
    /// Occupant ancestry, oldest first.
    pub lineage: Vec<CanonicalUuid>,
    /// Capabilities currently bound to this lane and occupant.
    pub capabilities: Vec<CanonicalUuid>,
    /// Exact session authority, absent only for legacy or never-started seats.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub session: Option<LaneSession>,
    /// Current lane disposition.
    pub disposition: String,
}

/// Durable task-allocation lifetime owned by one app team.
///
/// This policy controls which tasks a team may draw. It is deliberately
/// separate from [`LaneResidency::life`], which controls whether an individual
/// seat may be occupied again after its current process exits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSubscription {
    /// Stable app team identifying the subscribed team.
    pub team: CanonicalUuid,
    /// Commit instant when this identity most recently became [`teamed`].
    /// Absent exactly while the identity is unteamed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formed_at: Option<Instant>,
    /// Authoritative task-selection policy for the entire team.
    pub policy: crate::intent::TeamLife,
    /// Whether this team enables fast mode for every member it holds.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fast_mode: bool,
}

impl TeamSubscription {
    /// Whether this effective policy admits one immutable task-creator team.
    #[must_use]
    pub fn admits_creator_team(&self, creator: Option<CanonicalUuid>) -> bool {
        self.policy.admits_creator_team(creator, self.team)
    }
}

/// Maximum membership admitted when forming or growing an app team: the one
/// [grouping limit](crate::grouping::MEMBER_LIMIT).
///
/// Existing larger teams may be restated or reduced, but never enlarged.
pub const TEAM_MEMBER_LIMIT: usize = crate::grouping::MEMBER_LIMIT;

/// Whether one app team is a formation rather than a lone seat.
///
/// A fresh seat sits in a team carrying its own lane identity, so a team whose
/// only member is that identity is merely a defined agent, not a team. This is
/// the one definition of teamed, and it is a rendering distinction: the fleet
/// renders a surface only for a teamed row and the global menu lists an
/// unteamed agent for placement. Allocation and wake never ask it; a lane
/// follows its team's [`TeamSubscription::policy`] whether or not the team is
/// a formation.
#[must_use]
pub fn teamed(team: CanonicalUuid, members: impl IntoIterator<Item = CanonicalUuid>) -> bool {
    let mut members = members.into_iter();
    match (members.next(), members.next()) {
        (Some(only), None) => only != team,
        (Some(_), Some(_)) => true,
        (None, _) => false,
    }
}

/// Current resident and lane state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResidencyHead {
    /// Current resident generation.
    pub epoch: CanonicalUuid,
    /// Durable resident life.
    pub state: ResidentState,
    /// Monotonic authority for app team membership, order, and policy.
    ///
    /// Zero is the legacy and freshly seated baseline. Every transaction that
    /// changes topology advances this value exactly once, while lifecycle-only
    /// residency writes preserve it.
    pub topology_revision: u64,
    /// Current lanes, occupants, lineage, capabilities, and dispositions.
    pub lanes: Vec<LaneResidency>,
    /// One durable task-allocation lifetime per app team, in team order.
    pub subscriptions: Vec<TeamSubscription>,
    /// Current agent-renewal handoffs, sorted by team and member identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renewals: Vec<LaneRenewalRecord>,
}

impl ResidencyHead {
    /// Lane identities seated in one team, in canonical seat order.
    pub fn team_members(&self, team: CanonicalUuid) -> impl Iterator<Item = CanonicalUuid> + '_ {
        self.lanes
            .iter()
            .filter(move |row| row.team == team)
            .map(|row| row.lane)
    }

    /// Whether one lane's occupant launches in fast mode.
    ///
    /// Fast mode is additive across its three levels: the lane runs fast when
    /// its own flag, its team's flag, or the global setting enables it, and no
    /// level can switch it off for another.
    #[must_use]
    pub fn fast_mode(&self, row: &LaneResidency, global: bool) -> bool {
        global
            || row.fast_mode
            || self
                .subscriptions
                .iter()
                .any(|subscription| subscription.team == row.team && subscription.fast_mode)
    }

    /// This head with the fast-mode refusals of every `covered` lane
    /// forgotten, so their next launches try fast mode again; absent when
    /// none of them held one.
    #[must_use]
    pub fn forget_fast_refusals(&self, covered: impl Fn(&LaneResidency) -> bool) -> Option<Self> {
        let mut next = self.clone();
        let mut forgot = false;
        for row in next.lanes.iter_mut().filter(|row| covered(row)) {
            forgot |= row.forget_fast_refusal();
        }
        forgot.then_some(next)
    }
}

impl LaneResidency {
    /// Whether this seat's vendor refused fast mode for the model it runs, so
    /// its next launch runs without it.
    #[must_use]
    pub fn refuses_fast_mode(&self) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.launch.fast_refusal.is_some())
    }

    /// Forget this seat's fast-mode refusal so its next launch tries fast
    /// mode, saying whether one stood.
    pub fn forget_fast_refusal(&mut self) -> bool {
        self.session
            .as_mut()
            .and_then(|session| session.launch.fast_refusal.take())
            .is_some()
    }
}

/// Current journal and wake positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CursorHead {
    /// Last committed journal position, absent before the first fact.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub seq: Option<Seq>,
    /// Durable wake position, absent before the first consumed commit.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub wake: Option<Seq>,
}

const fn is_false(value: &bool) -> bool {
    !*value
}
