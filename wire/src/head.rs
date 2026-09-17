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

/// One allocator locality coordinate captured when a claim is won.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimCell {
    /// Project occupied by the claimed task.
    pub project: String,
    /// Workflow phase occupied by the claimed task.
    pub phase: Phase,
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
    /// Authored task labels.
    pub tags: Vec<String>,
    /// Tasks that must complete first.
    pub dependencies: Vec<TaskStamp>,
    /// Creation provenance, immutable across statement revisions.
    pub provenance: Origin,
    /// Creation-time transcript authority, immutable across statement revisions.
    #[serde(
        default = "crate::body::TaskCreationContext::legacy_unavailable",
        skip_serializing_if = "crate::body::TaskCreationContext::is_legacy_unavailable"
    )]
    pub creation: crate::body::TaskCreationContext,
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
    /// Absent only on authority created before fanout provenance was durable.
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
    /// Optional control channel driven by the directive.
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
    #[serde(alias = "minted_millis")]
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
        Ok(Self {
            lane: wire.lane,
            occupant: wire.occupant,
            worktree: wire.worktree,
            vendor: wire.vendor,
            launch: LaneLaunchIdentity::from_parts(wire.model, wire.effort, wire.git, wire.rtk)?,
            epoch: wire.epoch,
            stream: wire.stream,
            identity: wire.identity,
            locus: wire.locus,
        })
    }
}

impl From<LaneSession> for LaneSessionWire {
    fn from(session: LaneSession) -> Self {
        let (model, effort, git, rtk) = LaneLaunchIdentity::into_parts(session.launch);
        Self {
            lane: session.lane,
            occupant: session.occupant,
            worktree: session.worktree,
            vendor: session.vendor,
            model,
            effort,
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
        /// Original member position inside the Canvas team.
        team_slot: u64,
    },
    /// The predecessor reached its stopping boundary and succession owns the slot.
    HandoffPending {
        /// Stopped occupant whose context the successor receives.
        predecessor: RenewalIdentity,
        /// Original member position inside the Canvas team.
        team_slot: u64,
    },
    /// A fresh successor started and replaced the predecessor in the same slot.
    SuccessorStarted {
        /// Occupant that handed off the lane.
        predecessor: RenewalIdentity,
        /// Fresh occupant now owning the lane.
        successor: RenewalIdentity,
        /// Original member position inside the Canvas team.
        team_slot: u64,
    },
    /// Successor launch refused after the predecessor reached handoff.
    LaunchRefused {
        /// Occupant whose handoff remains pending repair.
        predecessor: RenewalIdentity,
        /// Original member position inside the Canvas team.
        team_slot: u64,
        /// Exact actionable launch refusal.
        reason: String,
    },
}

/// Current durable renewal state for one independently steerable lane member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneRenewalRecord {
    /// Canvas team retaining the original team slot.
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
    /// Canvas team whose lane contains this member.
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

/// Durable task-allocation lifetime owned by one Canvas team.
///
/// This policy controls which tasks a team may draw. It is deliberately
/// separate from [`LaneResidency::life`], which controls whether an individual
/// seat may be occupied again after its current process exits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TeamSubscription {
    /// Stable Canvas team identifying the subscribed team.
    pub team: CanonicalUuid,
    /// Authoritative task-selection policy for the entire team.
    pub policy: crate::intent::TeamLife,
}

impl TeamSubscription {
    /// Whether this effective policy admits one immutable task-creator team.
    #[must_use]
    pub fn admits_creator_team(&self, creator: Option<CanonicalUuid>) -> bool {
        self.policy.admits_creator_team(creator, self.team)
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
    /// Monotonic authority for Canvas team membership, order, and policy.
    ///
    /// Zero is the legacy and freshly seated baseline. Every transaction that
    /// changes topology advances this value exactly once, while lifecycle-only
    /// residency writes preserve it.
    pub topology_revision: u64,
    /// Current lanes, occupants, lineage, capabilities, and dispositions.
    pub lanes: Vec<LaneResidency>,
    /// One durable task-allocation lifetime per Canvas team.
    ///
    /// Absence is admitted only for journal heads written before this field
    /// existed; resident reads derive that legacy team's initial policy
    /// until its next seating or configuration transaction persists it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subscriptions: Vec<TeamSubscription>,
    /// Current agent-renewal handoffs, sorted by team and member identity.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renewals: Vec<LaneRenewalRecord>,
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
