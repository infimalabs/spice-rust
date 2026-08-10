//! The result triad: one reply names the command and carries one outcome.

use serde::{Deserialize, Serialize};

use crate::envelope::{Seq, WireVersion};
use crate::fact::AnyFact;
use crate::ident::{CanonicalUuid, Digest};
use crate::manifest::ReleaseTarget;
use crate::{Envelope, Packed};

/// An inclusive range of committed journal positions.
///
/// Decoding refuses a reversed range: WIRE names this an inclusive range, and
/// a `last` below `first` does not describe one. In-process construction is
/// the resident's own business, so the fields stay public and
/// [`is_ordered`](Self::is_ordered) remains available to check one before it
/// is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "SeqRangeShape")]
pub struct SeqRange {
    /// First position committed by the transaction.
    pub first: Seq,
    /// Last position committed by the transaction.
    pub last: Seq,
}

impl SeqRange {
    /// True when the range is ordered.
    #[must_use]
    pub const fn is_ordered(self) -> bool {
        self.first.get() <= self.last.get()
    }
}

/// The decoded shape of a [`SeqRange`], before its ordering is checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SeqRangeShape {
    first: Seq,
    last: Seq,
}

/// A decoded range whose `last` sits below its `first`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("committed seq range is inclusive, so last must not precede first: {first}..{last}")]
pub struct SeqRangeError {
    /// First position as decoded.
    pub first: Seq,
    /// Last position as decoded.
    pub last: Seq,
}

impl TryFrom<SeqRangeShape> for SeqRange {
    type Error = SeqRangeError;

    fn try_from(shape: SeqRangeShape) -> Result<Self, Self::Error> {
        let range = Self {
            first: shape.first,
            last: shape.last,
        };
        if range.is_ordered() {
            Ok(range)
        } else {
            Err(SeqRangeError {
                first: shape.first,
                last: shape.last,
            })
        }
    }
}

/// Where a successful result sits relative to the journal.
///
/// A mutating command commits a range; a read-only command answers from a
/// snapshot instead; and a daemonless command states directly that no journal
/// position exists. The three are distinct keys rather than one overloaded
/// field, so a reader never guesses which it received and no caller needs to
/// fabricate a sequence for a journal-free answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "ReplyPositionShape")]
pub enum ReplyPosition {
    /// Inclusive range committed by a mutating transaction.
    #[serde(rename = "seq")]
    Committed(SeqRange),
    /// Snapshot cursor answered by a read-only command.
    #[serde(rename = "cursor")]
    Snapshot(Seq),
    /// Direct daemonless answer carrying no journal position.
    ///
    /// The struct variant deliberately encodes as `"direct": {}`. An empty
    /// object is the one explicit absence marker; `null` and objects carrying
    /// sequence-shaped or unknown fields are not alternate spellings.
    #[serde(rename = "direct")]
    Direct {},
}

/// The three optional keys are decoded before their exclusivity is checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReplyPositionShape {
    #[serde(default, deserialize_with = "crate::optional::present")]
    seq: Option<SeqRange>,
    #[serde(default, deserialize_with = "crate::optional::present")]
    cursor: Option<Seq>,
    #[serde(default, deserialize_with = "crate::optional::present")]
    direct: Option<DirectShape>,
}

/// The only payload admitted beside the `direct` key.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectShape {}

/// A reply position that names none or more than one journal relationship.
#[derive(Debug, thiserror::Error)]
#[error("reply position must carry exactly one of seq, cursor, or direct")]
struct ReplyPositionShapeError;

impl TryFrom<ReplyPositionShape> for ReplyPosition {
    type Error = ReplyPositionShapeError;

    fn try_from(shape: ReplyPositionShape) -> Result<Self, Self::Error> {
        match (shape.seq, shape.cursor, shape.direct) {
            (Some(range), None, None) => Ok(Self::Committed(range)),
            (None, Some(cursor), None) => Ok(Self::Snapshot(cursor)),
            (None, None, Some(DirectShape {})) => Ok(Self::Direct {}),
            _ => Err(ReplyPositionShapeError),
        }
    }
}

/// A typed result and its relationship to the journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reply<B> {
    /// Committed range, read snapshot, or explicit journal-free position.
    #[serde(flatten)]
    pub position: ReplyPosition,
    /// Kind-specific result.
    pub body: B,
}

/// Compiled identities reported by the daemonless `version` leaf.
///
/// These four values are one machine document rather than independent prose:
/// the native artifact and lane-built app identify what is running, while the
/// wire and journal versions identify the only crossing and authority schemas
/// that build admits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Version {
    /// Digest of the native executable for its release target.
    pub binary: Digest,
    /// The one wire schema this build speaks.
    pub wire: WireVersion,
    /// Finalized journal schema this build observed without migrating it.
    pub journal: u32,
    /// Digest of the app bytes embedded in the native executable.
    pub app: Digest,
}

/// Receipt for opening the current resident's published browser surface.
///
/// The operator capability is intentionally absent. It is a socket credential,
/// not browser state, and must never cross the native command's output boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasOpen {
    /// Exact loopback HTTP origin read from owner-only discovery.
    pub endpoint: String,
    /// Resident generation whose discovery was still current at launch.
    pub epoch: CanonicalUuid,
}

/// Deterministic environment and lifecycle receipt returned by `spice demo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DemoAnswer {
    /// Canonical disposable repository root.
    pub root: String,
    /// Canned transcript seeded beneath the repository root.
    pub transcript: String,
    /// Whether this invocation only seeded or also ran the canvas.
    pub mode: String,
    /// Whether the root remains available or was removed after clean canvas shutdown.
    pub lifecycle: String,
    /// Exact loopback origin admitted for canvas mode.
    ///
    /// Present whenever the canvas ran, including the default run that opens no
    /// browser, so a deliberate open is always one copy away.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub endpoint: Option<String>,
    /// Shell-safe command that opens this exact seeded root.
    pub launch_command: String,
    /// Shell-safe command that removes this exact disposable root.
    pub remove_command: String,
}

/// Authenticated identities and replay counts for one offline restore bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreVerification {
    /// Domain-separated identity of the canonical backup manifest.
    pub bundle: Digest,
    /// Crossing schema admitted by the canonical manifest.
    pub wire: WireVersion,
    /// Finalized schema admitted from the authority member.
    pub journal_schema: u32,
    /// Domain-separated identity of the SQLite authority bytes.
    pub authority: Digest,
    /// Number of authenticated regular-file members.
    pub members: u64,
    /// Facts whose complete replay closure verified.
    pub facts: u64,
    /// Command receipts whose request/answer bindings verified.
    pub receipts: u64,
    /// Current authority heads whose canonical bodies verified.
    pub heads: u64,
    /// Origin/cause thread-index rows whose closure verified.
    pub threads: u64,
}

/// One consistent read of finalized journal identity and extent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalReport {
    /// Finalized journal schema admitted by the reader.
    pub schema: u32,
    /// Greatest committed position, absent for an empty authority.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub cursor: Option<Seq>,
    /// Number of canonical fact envelopes in the snapshot.
    pub facts: u64,
    /// Bounded canonical immutable envelopes in ascending sequence order.
    pub entries: Packed<Envelope<AnyFact>>,
}

/// Counts proved by a complete read-only authority verification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalVerification {
    /// Finalized journal schema that was verified.
    pub schema: u32,
    /// Canonical fact envelopes verified.
    pub facts: u64,
    /// Command receipts verified.
    pub receipts: u64,
    /// Current authority heads verified.
    pub heads: u64,
    /// Derived thread-index rows verified.
    pub threads: u64,
}

/// Counts returned after rebuilding the disposable thread index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalReindex {
    /// Immutable fact envelopes used as the rebuild source.
    pub facts: u64,
    /// Thread rows published by the transaction.
    pub threads: u64,
}

/// Receipt for one complete, authenticated backup bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalBackup {
    /// Operator-selected bundle destination.
    pub output: String,
    /// Authentication and replay proof for the published bundle.
    pub verification: RestoreVerification,
}

/// Digest-pinned preview or completed authority replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalRestore {
    /// Domain-separated identity of this bundle, destination, and predecessor.
    pub plan: Digest,
    /// Repository authority path the plan names.
    pub destination: String,
    /// Identity of the authority present when the plan was made, if any.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub previous: Option<Digest>,
    /// Complete verification of the proposed replacement generation.
    pub verification: RestoreVerification,
    /// True only after the digest-pinned replacement and reopen verification.
    pub applied: bool,
}

/// One registered fold's current materialization state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoldState {
    /// Closed-inventory fold name.
    pub fold: String,
    /// Published generation, absent before first replay.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub generation: Option<u64>,
    /// Reducer semantic version carried by that generation.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub reducer_version: Option<u32>,
    /// Greatest journal fact applied by the generation.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub cursor: Option<Seq>,
    /// Greatest position through which its answer is complete.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub horizon: Option<Seq>,
    /// Exact command that recreates the fold.
    pub recovery: String,
    /// Closed-inventory inputs that bound and explain this answer.
    pub sources: Vec<String>,
    /// Deterministic derived answer, absent before first replay.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub payload: Option<serde_json::Value>,
}

/// Complete closed-inventory fold report or replay receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoldReport {
    /// Fold states in dependency-safe inventory order.
    pub folds: Vec<FoldState>,
}

/// One verified cause thread reconstructed from immutable envelopes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadReport {
    /// Exact sequence or origin reference supplied by the caller.
    pub reference: String,
    /// Root fact of the resolved thread.
    pub root: Seq,
    /// Bounded canonical envelopes ordered from root through descendants.
    pub entries: Packed<Envelope<AnyFact>>,
}

/// One task-level change derived from first-parent landing history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseNotesEntry {
    /// Project declared by the landing commit, or `general` when absent.
    pub project: String,
    /// Latest landing wording after structural subject cleanup.
    pub subject: String,
    /// Stable task identity shared by repeated phase landings, when declared.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub task: Option<String>,
    /// Every first-parent landing reference contributing to this entry.
    pub references: Vec<String>,
}

/// Deterministic release-note draft or publication document derived from history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseNotes {
    /// Sole release-note document schema emitted by this build.
    pub version: u16,
    /// Domain-separated identity of the exact rendered note bytes.
    pub identity: Digest,
    /// Version the release page names.
    pub release_version: String,
    /// Fully resolved commit whose first-parent ledger was folded.
    pub release_commit: String,
    /// Highest-versioned reachable prior release tag, when one exists.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub previous_tag: Option<String>,
    /// True when absence of a prior tag forced the explicit recent-history window.
    pub bounded_recent_window: bool,
    /// Task-level entries in first-landing order.
    pub entries: Vec<ReleaseNotesEntry>,
    /// Exact Markdown bytes rendered for review or publication.
    pub document: String,
}

/// Verified identities returned by both daemonless proof leaves.
///
/// This body has no standing or success flag: its existence is the clean
/// outcome. Proof verification admits only clean evidence before producing
/// this document, while a refused replay travels as [`Outcome::Refusal`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProofEvidence {
    /// Release tuple covered by the payload, replay, and receipt.
    pub target: ReleaseTarget,
    /// Canonical identity of the receipt-free target payload.
    pub payload: Digest,
    /// Canonical identity of the detached proof receipt.
    pub receipt: Digest,
    /// Canonical identity of the ordered proof battery.
    pub battery: Digest,
    /// Canonical identity of the complete replay verdict.
    pub replay: Digest,
}

/// Exact registry observation rendered by a recovery preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRegistryEvidence {
    /// Canonical semantic version observed at the package authority.
    pub version: crate::manifest::ProductVersion,
    /// Exact release commit named by the package authority.
    pub release_commit: String,
    /// Canonical package identity made visible by that commit.
    pub package: Digest,
}

/// Complete no-effect observations resolved before a release plan is sealed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePlanObservations {
    /// Fully resolved HEAD commit.
    pub head: String,
    /// Whether tracked worktree state is clean.
    pub clean: bool,
    /// Whether any version transition belongs to the selected release commit.
    pub bump_claimed: bool,
    /// Number of commits after the selected release commit.
    pub unrecorded_local_commits: u64,
    /// Product version compiled into the source executable.
    pub source_version: crate::manifest::ProductVersion,
    /// Product version read from the selected release tree.
    pub release_version: crate::manifest::ProductVersion,
    /// Whether the configured remote accepted a no-effect authenticated read.
    pub credentials_valid: bool,
    /// Unexpected source-ref object, absent when unpublished or converged.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub source_ref: Option<String>,
    /// Unexpected package-ref object, absent when unpublished or converged.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub package_ref: Option<String>,
    /// Unexpected tag object, absent when unpublished or converged.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub tag_ref: Option<String>,
    /// Unexpected announcement object, absent when unpublished or converged.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub announcement_ref: Option<String>,
    /// Exact package visibility required by tag-and-announce recovery.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub registry: Option<ReleaseRegistryEvidence>,
}

/// One operator-previewed publication of a complete sealed release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleasePlan {
    /// Digest the apply command must re-present.
    pub key: Digest,
    /// Identity of the repository authority observed by the planner.
    pub repository: Digest,
    /// Closed release action: `publish` or `tag-and-announce`.
    pub action: String,
    /// Canonical semantic version requested by the operator.
    pub requested_version: crate::manifest::ProductVersion,
    /// Immutable source commit from which release preparation began.
    pub source_commit: String,
    /// Exact release commit all external refs must name.
    pub release_commit: String,
    /// Canonical identity of the sealed two-target manifest.
    pub manifest: Digest,
    /// Source identity shared by both target payloads.
    pub source: Digest,
    /// Identity of the exact curated release-note bytes approved by this plan.
    pub notes: Digest,
    /// Exact curated bytes, not merely a digest reconstructed at apply time.
    pub notes_document: String,
    /// Canonical identity of the complete installable package closure.
    pub package: Digest,
    /// Exact Git tree object published at the immutable package ref.
    pub package_object: String,
    /// Exact independently installed executable identity the probe must return.
    pub installed_artifact: Digest,
    /// Exact target payload and detached-receipt identities to publish.
    pub targets: Vec<crate::body::PublishedTarget>,
    /// Sole ordered operation inventory shared by preview and execution.
    pub operations: Vec<String>,
    /// Exact no-effect preconditions and authority observations bound by `key`.
    pub observations: ReleasePlanObservations,
}

/// Structured diagnostic shared by refusal forms that report a failed predicate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RefusalDiagnostic {
    /// Stable reason, suitable for matching rather than display alone.
    reason: String,
    /// Failed predicate name, when a witness is what failed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    witness: Option<String>,
    /// Value the predicate required.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    expected: Option<String>,
    /// Value the predicate observed.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    observed: Option<String>,
    /// Human diagnostic detail that is not itself a recovery command.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    detail: Option<String>,
}

impl RefusalDiagnostic {
    /// Build one diagnostic from the exact predicate evidence available.
    #[must_use]
    pub fn new(
        reason: impl Into<String>,
        witness: Option<String>,
        expected: Option<String>,
        observed: Option<String>,
    ) -> Self {
        Self {
            reason: reason.into(),
            witness,
            expected,
            observed,
            detail: None,
        }
    }

    /// Attach human detail without claiming it is a runnable recovery.
    #[must_use]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Stable machine reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Failed predicate name, when present.
    #[must_use]
    pub fn witness(&self) -> Option<&str> {
        self.witness.as_deref()
    }

    /// Required predicate value, when present.
    #[must_use]
    pub fn expected(&self) -> Option<&str> {
        self.expected.as_deref()
    }

    /// Observed predicate value, when present.
    #[must_use]
    pub fn observed(&self) -> Option<&str> {
        self.observed.as_deref()
    }

    /// Human diagnostic detail, when present.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Replace the observed predicate value while retaining the same refusal form.
    pub fn replace_observed(&mut self, observed: Option<String>) -> Option<String> {
        std::mem::replace(&mut self.observed, observed)
    }
}

/// One runnable Spice recovery command.
///
/// The executable is structural rather than caller-authored: every value runs
/// `spice` with these exact arguments, and construction requires one concrete
/// top-level subcommand rather than an absent or prose-only remedy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct SpiceCommand {
    arguments: Vec<String>,
}

impl SpiceCommand {
    /// Bind a required top-level command and exact trailing arguments to the
    /// one Spice executable.
    ///
    /// # Errors
    ///
    /// Refuses an empty or option-shaped top-level command. This keeps a
    /// repair arm from carrying `spice` alone or a dummy global option.
    pub fn new<I, S, T>(command: S, arguments: I) -> Result<Self, &'static str>
    where
        I: IntoIterator<Item = T>,
        S: Into<String>,
        T: Into<String>,
    {
        let arguments: Vec<String> = std::iter::once(command.into())
            .chain(arguments.into_iter().map(Into::into))
            .collect();
        Self::try_from(arguments)
    }

    /// Exact arguments following the `spice` executable.
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.arguments
    }
}

impl TryFrom<Vec<String>> for SpiceCommand {
    type Error = &'static str;

    fn try_from(arguments: Vec<String>) -> Result<Self, Self::Error> {
        let Some(command) = arguments.first() else {
            return Err("a recovery command requires one Spice subcommand");
        };
        if command.is_empty()
            || command.starts_with('-')
            || command.chars().any(char::is_whitespace)
        {
            return Err("a recovery command requires one non-option Spice subcommand token");
        }
        if arguments.iter().any(|argument| argument.contains('\0')) {
            return Err("a recovery command cannot contain a NUL byte");
        }
        Ok(Self { arguments })
    }
}

impl From<SpiceCommand> for Vec<String> {
    fn from(command: SpiceCommand) -> Self {
        command.arguments
    }
}

/// An answer about an event that did not occur.
///
/// A refusal is never a fact. It causes no journal or head mutation; the bus
/// writes a bounded telemetry row instead. Its semantic form is explicit, so
/// absence of a runnable recovery cannot be encoded as an empty remedy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "form", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Refusal {
    /// A caller can repair the refusal by running one exact Spice command.
    Repairable {
        /// Runnable command rendered before the diagnostic.
        command: SpiceCommand,
        /// Predicate that refused the operation.
        diagnostic: Box<RefusalDiagnostic>,
    },
    /// No runnable recovery exists; render only the diagnostic.
    Bare {
        /// Predicate that refused the operation.
        diagnostic: Box<RefusalDiagnostic>,
    },
    /// Invocation syntax itself was invalid; render its one contract statement.
    Invocation {
        /// Stable machine reason.
        reason: String,
        /// Complete invocation-contract statement.
        contract: String,
    },
    /// A gate refused with authoritative evidence that must precede its summary.
    Evidence {
        /// Stable machine reason.
        reason: String,
        /// Complete gate evidence.
        evidence: String,
        /// Human summary following the evidence.
        summary: String,
    },
}

impl Refusal {
    /// Construct a repairable refusal with one structurally runnable command.
    #[must_use]
    pub fn repairable(command: SpiceCommand, diagnostic: RefusalDiagnostic) -> Self {
        Self::Repairable {
            command,
            diagnostic: Box::new(diagnostic),
        }
    }

    /// Construct a refusal for which no runnable recovery exists.
    #[must_use]
    pub fn bare(diagnostic: RefusalDiagnostic) -> Self {
        Self::Bare {
            diagnostic: Box::new(diagnostic),
        }
    }

    /// Construct one invocation-contract refusal statement.
    #[must_use]
    pub fn invocation(reason: impl Into<String>, contract: impl Into<String>) -> Self {
        Self::Invocation {
            reason: reason.into(),
            contract: contract.into(),
        }
    }

    /// Construct one evidence-first gate refusal.
    #[must_use]
    pub fn evidence(
        reason: impl Into<String>,
        evidence: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self::Evidence {
            reason: reason.into(),
            evidence: evidence.into(),
            summary: summary.into(),
        }
    }

    /// Stable machine reason shared by every refusal form.
    #[must_use]
    pub fn reason(&self) -> &str {
        match self {
            Self::Repairable { diagnostic, .. } | Self::Bare { diagnostic } => diagnostic.reason(),
            Self::Invocation { reason, .. } | Self::Evidence { reason, .. } => reason,
        }
    }

    /// Human explanation suitable for non-CLI surfaces that retain reason and
    /// detail as separate fields.
    #[must_use]
    pub fn explanation(&self) -> &str {
        match self {
            Self::Repairable { diagnostic, .. } | Self::Bare { diagnostic } => diagnostic
                .detail()
                .or_else(|| diagnostic.observed())
                .or_else(|| diagnostic.expected())
                .unwrap_or_else(|| diagnostic.reason()),
            Self::Invocation { contract, .. } => contract,
            Self::Evidence { summary, .. } => summary,
        }
    }

    /// Predicate diagnostic carried by repairable and bare forms.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&RefusalDiagnostic> {
        match self {
            Self::Repairable { diagnostic, .. } | Self::Bare { diagnostic } => {
                Some(diagnostic.as_ref())
            }
            Self::Invocation { .. } | Self::Evidence { .. } => None,
        }
    }

    /// Mutably narrow to the predicate diagnostic carried by diagnostic forms.
    #[must_use]
    pub fn diagnostic_mut(&mut self) -> Option<&mut RefusalDiagnostic> {
        match self {
            Self::Repairable { diagnostic, .. } | Self::Bare { diagnostic } => {
                Some(diagnostic.as_mut())
            }
            Self::Invocation { .. } | Self::Evidence { .. } => None,
        }
    }
}

/// The plane in which a defect was detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FaultClass {
    /// A wire, epoch, or identity defect.
    Protocol,
    /// A journal or folds-store defect.
    Store,
    /// A defect outside the process boundary.
    Environment,
}

/// A named protocol, store, or environment defect.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fault {
    /// Plane in which the defect was detected.
    pub class: FaultClass,
    /// Stable reason, suitable for matching rather than display alone.
    pub reason: String,
    /// What was observed, in enough detail to act on.
    pub detail: String,
}

/// One tagged outcome carried by a reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Outcome<B> {
    /// The command succeeded.
    Reply(Reply<B>),
    /// The command was refused; nothing was committed.
    Refusal(Refusal),
    /// A defect prevented an answer.
    Fault(Fault),
}

/// The reply frame: the command it answers, the epoch that answered, and one
/// tagged outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response<B> {
    /// Current wire version.
    pub v: WireVersion,
    /// Identity of the command being answered.
    pub id: CanonicalUuid,
    /// Resident epoch that produced the answer.
    pub epoch: CanonicalUuid,
    /// The one outcome.
    pub outcome: Outcome<B>,
}
