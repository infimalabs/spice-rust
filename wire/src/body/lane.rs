use super::{Release, Severity, SourceBoundary, TranscriptLocus, Verdict};
use crate::Name;
use crate::envelope::Actor;
use crate::ident::{CanonicalUuid, Digest, Instant};
use serde::{Deserialize, Serialize};

/// A lane and its capability were created.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneSeated {
    /// Created independently steerable member seat.
    pub lane: CanonicalUuid,
    /// Canvas lineage whose lane contains the member.
    pub lineage_id: CanonicalUuid,
    /// Capability minted with the lane.
    pub capability: CanonicalUuid,
    /// Operator-configured agent display name, absent on pre-field facts.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub name: Option<String>,
    /// Worktree the lane is bound to.
    pub worktree: String,
    /// Closed lane-life spelling selected for the seat.
    pub life: String,
    /// Vendor selected for occupants of this seat.
    pub vendor: Name,
}

/// One Canvas lineage adopted a task-subscription policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageConfigured {
    /// Stable Canvas lineage identifying the configured group.
    pub lineage_id: CanonicalUuid,
    /// Member moved into the configured lineage, absent for policy-only changes.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub member: Option<CanonicalUuid>,
    /// Lineage the moved member occupied before this transaction.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub from_lineage: Option<CanonicalUuid>,
    /// Replacement identity assigned to a source team when its host left.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub source_rekeyed_to: Option<CanonicalUuid>,
    /// Prior task-subscription policy before this transaction.
    pub from: crate::intent::LineageLife,
    /// Authoritative task-subscription policy after this transaction.
    pub life: crate::intent::LineageLife,
    /// Prior operator-pinned project set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub from_manual_projects: Vec<String>,
    /// Authoritative operator-pinned project set after this transaction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub manual_projects: Vec<String>,
    /// Boundary for the moved member, when one moved and is actively claimed.
    ///
    /// Empty for a policy-only change, which moves no member and so crosses no
    /// lane's attribution.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<LaneSourceBoundary>,
}

/// One Canvas lineage and the lanes it holds after an arrangement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageStanding {
    /// Stable Canvas lineage identifying the group.
    pub lineage_id: CanonicalUuid,
    /// Every lane this lineage holds, in the order the operator arranged them.
    pub lanes: Vec<CanonicalUuid>,
}

/// One lane's accounted transcript boundary at a membership transition.
///
/// A membership transition names the lanes it moves rather than carrying a
/// single lane of its own, so the boundary travels beside the lane it belongs
/// to. This is what lets a reducer split attribution across the move: the spend
/// before this locus belongs to the lineage the lane left, and the spend after
/// it to the lineage the lane joined.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneSourceBoundary {
    /// Lane whose membership the transition changed.
    pub lane: CanonicalUuid,
    /// Accounted transcript that lane's membership changed at.
    pub source: SourceBoundary,
}

/// The operator rearranged the Canvas lineages.
///
/// Both sides are recorded because the gesture is not recoverable from the
/// result alone: a reorder and a join leave different `from` standings behind,
/// and a reader that only saw the outcome could not tell which one the
/// operator performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineageArranged {
    /// Lineage standings before this transaction, in their prior order.
    pub from: Vec<LineageStanding>,
    /// Lineage standings after this transaction, in the arranged order.
    pub lineages: Vec<LineageStanding>,
    /// Boundaries for the actively claimed lanes this arrangement moved.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<LaneSourceBoundary>,
}

/// One seated member moved to a durable position, possibly in another lineage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneArranged {
    /// Stable member identity that moved.
    pub lane: CanonicalUuid,
    /// Lineage the member occupied before the move.
    pub from_lineage_id: CanonicalUuid,
    /// Lineage owned by the target member after the move.
    pub lineage_id: CanonicalUuid,
    /// Stable member identity immediately following the moved member.
    pub before: CanonicalUuid,
    /// Boundary for the moved lane, when it is actively claimed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<LaneSourceBoundary>,
}

/// Exact model, effort, and observable Git outcome for one vendor launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaneLaunchIdentity {
    /// Concrete model supplied to the vendor launch.
    pub model: String,
    /// Reasoning effort supplied to the vendor launch.
    pub effort: String,
    /// GIT-04 outcome observed before this launch read repository configuration.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub git: Option<String>,
}

impl LaneLaunchIdentity {
    /// Build one complete recorded launch identity.
    pub fn new(model: impl Into<String>, effort: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            effort: effort.into(),
            git: None,
        }
    }

    /// Attach the observable GIT-04 launch outcome to a current launch.
    #[must_use]
    pub fn with_git(mut self, git: impl Into<String>) -> Self {
        self.git = Some(git.into());
        self
    }

    pub(crate) fn from_parts(
        model: Option<String>,
        effort: Option<String>,
        git: Option<String>,
    ) -> Result<Option<Self>, &'static str> {
        match (model, effort, git) {
            (Some(model), Some(effort), git) => Ok(Some(Self { model, effort, git })),
            (None, None, None) => Ok(None),
            (Some(_), None, _) => Err("lane launch identity is missing effort"),
            (None, Some(_), _) => Err("lane launch identity is missing model"),
            (None, None, Some(_)) => Err("lane launch identity is missing model and effort"),
        }
    }

    pub(crate) fn into_parts(
        identity: Option<Self>,
    ) -> (Option<String>, Option<String>, Option<String>) {
        identity.map_or((None, None, None), |identity| {
            (Some(identity.model), Some(identity.effort), identity.git)
        })
    }
}

/// A selected vendor record declared the exact resumable lane session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LaneSessionDeclaredWire", into = "LaneSessionDeclaredWire")]
pub struct LaneSessionDeclared {
    /// Lane whose session was declared.
    pub lane: CanonicalUuid,
    /// Occupant whose selected stream emitted the declaration.
    pub occupant: CanonicalUuid,
    /// Canonical worktree bound to the launch.
    pub worktree: String,
    /// Vendor dialect that admitted the declaration.
    pub vendor: Name,
    /// Launch identity, absent when the declaration predates its recording.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub launch: Option<LaneLaunchIdentity>,
    /// Resident generation that observed the declaration.
    pub epoch: CanonicalUuid,
    /// Selected child stream carrying records.
    pub stream: String,
    /// Exact resumable vendor identity.
    pub identity: String,
    /// Durable tape member holding the declaration.
    pub member: u64,
    /// Declaration offset within the member.
    pub offset: u64,
    /// Number of declaration bytes.
    pub length: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LaneSessionDeclaredWire {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    worktree: String,
    vendor: Name,
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
    member: u64,
    offset: u64,
    length: u64,
}

impl TryFrom<LaneSessionDeclaredWire> for LaneSessionDeclared {
    type Error = &'static str;

    fn try_from(wire: LaneSessionDeclaredWire) -> Result<Self, Self::Error> {
        Ok(Self {
            lane: wire.lane,
            occupant: wire.occupant,
            worktree: wire.worktree,
            vendor: wire.vendor,
            launch: LaneLaunchIdentity::from_parts(wire.model, wire.effort, wire.git)?,
            epoch: wire.epoch,
            stream: wire.stream,
            identity: wire.identity,
            member: wire.member,
            offset: wire.offset,
            length: wire.length,
        })
    }
}

impl From<LaneSessionDeclared> for LaneSessionDeclaredWire {
    fn from(declaration: LaneSessionDeclared) -> Self {
        let (model, effort, git) = LaneLaunchIdentity::into_parts(declaration.launch);
        Self {
            lane: declaration.lane,
            occupant: declaration.occupant,
            worktree: declaration.worktree,
            vendor: declaration.vendor,
            model,
            effort,
            git,
            epoch: declaration.epoch,
            stream: declaration.stream,
            identity: declaration.identity,
            member: declaration.member,
            offset: declaration.offset,
            length: declaration.length,
        }
    }
}

/// An occupant became live under the lane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneStarted {
    /// Occupied lane.
    pub lane: CanonicalUuid,
    /// Occupant that became live.
    pub occupant: CanonicalUuid,
    /// Resident epoch that observed the start.
    pub epoch: CanonicalUuid,
    /// Process-group identity returned by the sole process door.
    pub group: u32,
}

/// A vacant lane seat moved to a fresh linked worktree before exact-session seating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneImported {
    /// Lane whose lineage seat is retained.
    pub lane: CanonicalUuid,
    /// Latest occupant in the retained lineage.
    pub ancestor: CanonicalUuid,
    /// Worktree the seat was authoritatively bound to.
    pub from_worktree: String,
    /// Fresh linked worktree receiving the seat.
    pub worktree: String,
    /// Driver dialect retained by the transfer.
    pub vendor: Name,
    /// Exact resumable identity proved before the transfer.
    pub identity: String,
    /// Resident generation that admitted the transfer.
    pub epoch: CanonicalUuid,
}

/// The resident observed the occupant end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneExited {
    /// Vacated lane.
    pub lane: CanonicalUuid,
    /// Occupant that ended.
    pub occupant: CanonicalUuid,
    /// Exit status the resident observed.
    pub code: i32,
}

/// The occupant crossed a context compaction boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneCompacted {
    /// Lane the occupant runs under.
    pub lane: CanonicalUuid,
    /// Occupant that compacted.
    pub occupant: CanonicalUuid,
    /// Selected child stream carrying the boundary record.
    pub stream: String,
    /// Durable tape member holding the boundary record.
    pub member: u64,
    /// Boundary-record offset within the member.
    pub offset: u64,
    /// Number of bytes in the boundary record.
    pub length: u64,
}

/// A successor occupant was seated with its ancestor named.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneRenewed {
    /// Reseated lane.
    pub lane: CanonicalUuid,
    /// Successor occupant.
    pub occupant: CanonicalUuid,
    /// Occupant the successor descends from.
    pub ancestor: CanonicalUuid,
    /// Resident epoch that observed the successor start.
    pub epoch: CanonicalUuid,
    /// Process-group identity returned by the sole process door.
    pub group: u32,
}

/// A graceful occupant end completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneDrained {
    /// Drained lane.
    pub lane: CanonicalUuid,
    /// Occupant that ended gracefully.
    pub occupant: CanonicalUuid,
}

/// A signal this resident sent ended an occupant.
///
/// Separate from [`LaneExited`] because being signalled is a different ending,
/// not an exit status the occupant chose. A signalled process has no status of
/// its own to report, so recording one would mean publishing the resident's
/// substitute for a missing number as though the occupant had returned it. This
/// kind carries no code for the same reason.
///
/// Both signalling stages record this, not only the kill. An occupant that never
/// trapped the termination signal dies from that signal rather than outliving
/// the grace, and this resident is the reason for the ending either way. What
/// this kind separates is whose decision the ending was, not which signal
/// carried it; the stage is already durable in the shutdown ladder facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneKilled {
    /// Vacated lane.
    pub lane: CanonicalUuid,
    /// Occupant the resident ended.
    pub occupant: CanonicalUuid,
}

/// Work run without the resident left a marker that the next resident recognized once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneDegraded {
    /// Lane the daemonless work ran in.
    pub lane: CanonicalUuid,
    /// Marker the next resident recognized.
    pub marker: String,
}

/// Repeated young occupant deaths paused automatic re-seating.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneHeld {
    /// Held lane.
    pub lane: CanonicalUuid,
    /// Evidence the hold names.
    pub evidence: String,
    /// Deadline the hold names.
    pub until: Instant,
}

/// A held lane returned to seating eligibility by operator wake or expired hold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaneResumed {
    /// Resumed lane.
    pub lane: CanonicalUuid,
    /// What released the hold.
    pub release: Release,
}

/// The judge confirmed a taste at an evidence locus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasteConfirmed {
    /// Confirmed taste.
    pub taste: String,
    /// Evidence locus the judge ruled at.
    pub locus: String,
}

/// The judge cleared a taste at an evidence locus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasteCleared {
    /// Cleared taste.
    pub taste: String,
    /// Evidence locus the judge ruled at.
    pub locus: String,
}

/// A taste fired on authored prose and became one live directive.
///
/// This is one fact, not a fire recorded here and a directive raised somewhere
/// else. It carries the complete initial directive alongside the evidence that
/// justified it, so the interval in which a fire exists but is not yet
/// retirable does not exist: whatever reduces the steering plane sees a live
/// directive the moment the fact lands, and whatever reduces the taste plane
/// sees the evidence from the same row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TasteReminded {
    /// Durable directive identity minted by resident recognition.
    pub directive: CanonicalUuid,
    /// Directive key, spelled exactly as an operator-sent key is.
    pub key: String,
    /// Lane the reminder addresses.
    pub lane: CanonicalUuid,
    /// Taste that fired.
    pub taste: String,
    /// Identity of the taste name and normalized prose the taste fired on.
    ///
    /// Domain tagged, so the same bytes read as a taste trigger and as anything
    /// else can never collide into one identity.
    pub content: Digest,
    /// Occupant whose context the prose was authored in.
    pub occupant: CanonicalUuid,
    /// Journal position opening that occupant's current context epoch.
    ///
    /// This is the latest matching `lane::started`, `lane::renewed`, or
    /// subsequent `lane::compacted` fact. Paired with `content` it makes one
    /// reminder per compaction epoch decidable from the durable stream alone,
    /// rather than from a separate memory of what has fired.
    pub compaction: u64,
    /// Exact transcript locus of the prose that fired the taste.
    pub locus: TranscriptLocus,
    /// Reminder text, carried as ordinary directive text.
    pub text: String,
    /// Instant the reminder expires without a disposition.
    pub expires: Instant,
}

/// An expected listening event remained absent past its armed window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryAlarmed {
    /// Lane the window was armed for.
    pub lane: CanonicalUuid,
    /// Listening event that stayed absent.
    pub expected: String,
    /// End of the armed window.
    pub window: Instant,
}

/// Listening meters disagreed: activity existed without decoded context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanaryContradicted {
    /// Lane the meters cover.
    pub lane: CanonicalUuid,
    /// Activity the meters saw.
    pub activity: u64,
    /// Context the decoder produced from it.
    pub decoded: u64,
}

/// An unproven vendor version first ran with residue measurement armed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VendorArmed {
    /// Vendor that ran.
    pub vendor: String,
    /// Unproven version.
    pub version: String,
}

/// An executable surface digest changed and entered quarantine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustPended {
    /// Executable surface.
    pub surface: String,
    /// Digest that entered quarantine.
    pub digest: Digest,
    /// Digest it replaced; absent the first time the surface is seen.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub previous: Option<Digest>,
}

/// The operator approved the exact executable digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustGranted {
    /// Executable surface.
    pub surface: String,
    /// Exact approved digest.
    pub digest: Digest,
}

/// The operator withdrew approval from the exact executable digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustRevoked {
    /// Executable surface.
    pub surface: String,
    /// Exact digest approval was withdrawn from.
    pub digest: Digest,
}

/// The resident recognized a repository commit and bound its receipt verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitLanded {
    /// Recognized repository commit.
    pub commit: String,
    /// Receipt the commit published.
    pub receipt: Digest,
    /// Verdict bound to the receipt.
    pub verdict: Verdict,
}

/// The resident recognized a new gate finding; the Finding head opened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateFound {
    /// Stable finding identity.
    pub identity: String,
    /// Finding severity.
    pub severity: Severity,
    /// Evidence locus the finding stands at.
    pub locus: String,
    /// Owner the finding is charged to.
    pub owner: Actor,
    /// Receipt the finding originated from.
    pub receipt: Digest,
}

/// A standing finding resolved at its evidence; the Finding head closed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCleared {
    /// Stable finding identity.
    pub identity: String,
    /// Evidence locus the resolution stands at.
    pub locus: String,
}

/// A requested gate finding dispatch completed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateFindingsFiled {
    /// Digest of the complete typed battery report that was dispatched.
    pub report: Digest,
    /// Number of projected findings consumed.
    pub findings: u32,
    /// Number of deferred board tasks created.
    pub created: u32,
    /// Number of open board tasks reused.
    pub reused: u32,
}

/// The one forward schema migration finalized before epoch mint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalMigrated {
    /// Schema the journal left.
    pub from: u32,
    /// Schema the journal reached.
    pub to: u32,
    /// Epoch minted after the migration finalized.
    pub epoch: CanonicalUuid,
}

/// A quiesced backup bundle was produced with its enumerated tape manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalBacked {
    /// Produced backup bundle.
    pub bundle: String,
    /// Enumerated tape manifest carried with the bundle.
    pub manifest: Vec<String>,
}

/// A restore protocol completed and the new epoch recognized the exchanged authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalRestored {
    /// Bundle the restore read.
    pub bundle: String,
    /// New epoch that recognized the exchange.
    pub epoch: CanonicalUuid,
    /// Authority the restore exchanged.
    pub authority: Digest,
}

/// Position of the next complete record in one transcript source generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptPosition {
    /// Member holding the next record's first byte.
    pub member: u64,
    /// Offset of that byte within the member.
    pub offset: u64,
}

/// One durable byte span a transcript record was decoded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptSpan {
    /// Member the bytes live in.
    pub member: u64,
    /// First byte within the member.
    pub offset: u64,
    /// Number of bytes.
    pub length: u64,
}

/// One normalized usage sample beside the exact bytes it was decoded from.
///
/// Every counter is required: a batch that omits one is refused rather than
/// accounted as zero, so an unreported counter can never be spent as free.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptUsage {
    /// Completed vendor turns this sample represents.
    pub turns: u64,
    /// Uncached input tokens.
    pub input_tokens: u64,
    /// Cached input tokens.
    pub cached_input_tokens: u64,
    /// Input tokens written to a cache.
    pub cache_write_input_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Reasoning output tokens, absent when the dialect does not report them.
    ///
    /// Absence is not zero: a dialect that reports no reasoning counter omits
    /// the field, while a dialect reporting none spent sends an explicit zero.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub reasoning_output_tokens: Option<u64>,
    /// Exact bytes the sample was decoded from, in source order.
    pub spans: Vec<TranscriptSpan>,
    /// Position of this event within the record that carried it.
    pub ordinal: u64,
}

/// One accounted batch of transcript records advanced a transcript source head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalIngested {
    /// Logical lane whose transcript was accounted.
    pub lane: CanonicalUuid,
    /// Vendor that produced the transcript.
    pub vendor: Name,
    /// Logical stream within that lane and vendor.
    pub stream: String,
    /// Opaque durable source generation the batch was read from.
    pub source: CanonicalUuid,
    /// Generation this batch retired, present only when [`Self::source`] is a
    /// new generation restarting the stream from its origin.
    ///
    /// A reader cannot infer supersession from a source that merely differs:
    /// that reading turns a batch written against a stale head, or a batch that
    /// named the wrong source, into a silent generation change that abandons
    /// the accounted position. Naming the retired generation makes the claim
    /// explicit and lets the guard refuse every batch that does not make it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub supersedes: Option<CanonicalUuid>,
    /// Position the batch continued from.
    pub from: TranscriptPosition,
    /// Position the batch left the source head at.
    pub to: TranscriptPosition,
    /// Every normalized usage sample the batch accounted.
    pub usage: Vec<TranscriptUsage>,
    /// Complete records carrying no usage that the batch accounted.
    pub non_usage_records: u64,
    /// Residue records the batch accounted.
    pub residue_records: u64,
}

/// The resident acquired the kernel writer lock and minted its epoch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpSeated {
    /// Minted resident epoch.
    pub epoch: CanonicalUuid,
}

/// The resident stopped admission and began a graceful end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpQuiescing {
    /// Resident epoch ending.
    pub epoch: CanonicalUuid,
}

/// The resident began preserving or finishing admitted work and ending occupants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpDraining {
    /// Resident epoch ending.
    pub epoch: CanonicalUuid,
    /// Occupants still live at the transition.
    pub occupants: u32,
}

/// Grace expired and the resident began ending occupants that had not finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpTerminating {
    /// Resident epoch ending.
    pub epoch: CanonicalUuid,
    /// Occupants that had not finished.
    pub occupants: u32,
}

/// Bounded escalation began against occupants that survived termination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpKilling {
    /// Resident epoch ending.
    pub epoch: CanonicalUuid,
    /// Occupants that survived termination.
    pub occupants: u32,
}

/// Occupants ended; the resident began verifying claims, tape seals, and residue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpVerifying {
    /// Resident epoch ending.
    pub epoch: CanonicalUuid,
}

/// The resident committed the verified terminal state immediately before releasing authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpDark {
    /// Resident epoch releasing authority.
    pub epoch: CanonicalUuid,
}

/// The resident recognized a valid evidence bundle bound to commit, tree, and manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProofSealed {
    /// Commit the bundle is bound to.
    pub commit: String,
    /// Tree the bundle is bound to.
    pub tree: String,
    /// Manifest the bundle is bound to.
    pub manifest: Digest,
}

/// One published target entry and its detached proof receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedTarget {
    /// Target the entry names.
    pub target: String,
    /// Artifact digest published for the target.
    pub artifact: Digest,
    /// Detached proof receipt the artifact published under.
    pub receipt: Digest,
}

/// The sealed release manifest, notes, and each target artifact digest published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleasePublished {
    /// Sealed release manifest digest.
    pub manifest: Digest,
    /// Identity of the exact curated release-note bytes published.
    ///
    /// `HistoricalAbsent` is written only by the journal schema-4-to-5
    /// migration for a publication whose pre-digest bytes cannot be
    /// reconstructed. The field remains required on the wire, so unmigrated
    /// schema-4 JSON does not enter the current decoder.
    pub notes: crate::ReleaseNotesIdentity,
    /// Each target entry published under its detached proof receipt.
    pub targets: Vec<PublishedTarget>,
}
