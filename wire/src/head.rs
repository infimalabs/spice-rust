//! Current authoritative content owned by the journal head tables.
//!
//! These are wire DTOs, not alternate authorities. The journal updates them
//! in the same transaction as the facts that justify the new current state.

use serde::{Deserialize, Serialize};

use crate::Name;
use crate::body::{LaneLaunchIdentity, TranscriptPosition};
use crate::envelope::{Actor, Origin, Seq};
use crate::ident::{CanonicalUuid, Digest, Instant};
use crate::witness::{Phase, TaskHandle};

/// One current task statement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskHead {
    /// Public task name.
    pub handle: TaskHandle,
    /// Stable task identity.
    pub uuid: CanonicalUuid,
    /// Authored title.
    pub title: String,
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
    pub dependencies: Vec<TaskHandle>,
    /// Creation provenance, immutable across statement revisions.
    pub provenance: Origin,
    /// Fact that established this statement revision.
    pub revision: Seq,
}

/// One current task workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowHead {
    /// Ordered phases the task walks.
    pub flow: Vec<Phase>,
    /// Current phase.
    pub phase: Phase,
    /// Current workflow status.
    pub status: String,
    /// Current review state.
    pub review: String,
    /// Actor assigned to the current phase, when assignment applies.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub assignment: Option<Actor>,
    /// Wakeable deferral deadline.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub deferral: Option<Instant>,
    /// Completion instant.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub completion: Option<Instant>,
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
    /// Fact that established the win.
    pub fact: Seq,
    /// Lane bound to the claim.
    pub lane: CanonicalUuid,
    /// Occupant bound to the claim.
    pub occupant: CanonicalUuid,
    /// Resident generation under which the claim was won.
    pub epoch: CanonicalUuid,
    /// Whether the win remains active or how it ended.
    pub disposition: ClaimDisposition,
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

/// Closed classes of authoritative witness heads.
///
/// This classifies seven stored predicate-head rows. Command preconditions use
/// the 23-variant [`crate::witness::WitnessKind`] vocabulary instead. They
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
    /// Terminal state committed before authority release.
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
    /// Launch identity, absent when the declaration predates its recording.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub launch: Option<LaneLaunchIdentity>,
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
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    model: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    effort: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    git: Option<String>,
    epoch: CanonicalUuid,
    stream: String,
    identity: String,
    locus: TapeLocus,
}

impl TryFrom<LaneSessionWire> for LaneSession {
    type Error = &'static str;

    fn try_from(wire: LaneSessionWire) -> Result<Self, Self::Error> {
        Ok(Self {
            lane: wire.lane,
            occupant: wire.occupant,
            worktree: wire.worktree,
            vendor: wire.vendor,
            launch: LaneLaunchIdentity::from_parts(wire.model, wire.effort, wire.git)?,
            epoch: wire.epoch,
            stream: wire.stream,
            identity: wire.identity,
            locus: wire.locus,
        })
    }
}

impl From<LaneSession> for LaneSessionWire {
    fn from(session: LaneSession) -> Self {
        let (model, effort, git) = LaneLaunchIdentity::into_parts(session.launch);
        Self {
            lane: session.lane,
            occupant: session.occupant,
            worktree: session.worktree,
            vendor: session.vendor,
            model,
            effort,
            git,
            epoch: session.epoch,
            stream: session.stream,
            identity: session.identity,
            locus: session.locus,
        }
    }
}

/// One lane's current place in the resident generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneResidency {
    /// Address of this independently steerable lineage member.
    pub lane: CanonicalUuid,
    /// Canvas lineage whose lane contains this member.
    pub lineage_id: CanonicalUuid,
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

/// Durable task-subscription policy owned by one Canvas lineage.
///
/// This policy controls which tasks a team may draw. It is deliberately
/// separate from [`LaneResidency::life`], which controls whether an individual
/// seat may be occupied again after its current process exits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LineageSubscription {
    /// Stable Canvas lineage identifying the subscribed team.
    pub lineage_id: CanonicalUuid,
    /// Authoritative task-selection policy for the entire lineage.
    pub policy: crate::intent::LineageLife,
    /// Operator-pinned public projects retained across policy changes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_projects: Vec<String>,
    /// Server-projected public projects replayed from durable create/claim facts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_projects: Vec<String>,
}

impl LineageSubscription {
    /// Whether this effective policy admits one public project to `task next`.
    #[must_use]
    pub fn admits_project(&self, project: &str) -> bool {
        match self.policy.scope() {
            crate::intent::TaskSubscriptionScope::Manual => self
                .manual_projects
                .iter()
                .any(|candidate| candidate == project),
            crate::intent::TaskSubscriptionScope::ManualAndAutoProject => self
                .manual_projects
                .iter()
                .chain(&self.auto_projects)
                .any(|candidate| candidate == project),
            crate::intent::TaskSubscriptionScope::AllAssignable => true,
        }
    }

    /// Project one server-learned public project in deterministic lexical order.
    pub fn learn_project(&mut self, project: &str) {
        if !self
            .auto_projects
            .iter()
            .any(|candidate| candidate == project)
        {
            self.auto_projects.push(project.to_owned());
            self.auto_projects.sort();
        }
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
    /// Current lanes, occupants, lineage, capabilities, and dispositions.
    pub lanes: Vec<LaneResidency>,
    /// One durable task-subscription policy per Canvas lineage.
    ///
    /// Absence is admitted only for journal heads written before this field
    /// existed; resident reads derive that legacy lineage's initial policy
    /// until its next seating or configuration transaction persists it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<LineageSubscription>,
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
