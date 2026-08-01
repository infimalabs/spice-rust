//! The command envelope: one mutating request crossing the socket.
//!
//! The set of commands is closed, and the registry below carries the witness
//! law beside the command. Both the spelling and the law are stated as
//! exhaustive matches over [`CommandKind`], so a new command that does not
//! carry an explicit registry row does not compile.

use std::fmt;
use std::str::FromStr;

use serde::de::{Deserializer, Error as DeError};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::envelope::WireVersion;
use crate::ident::CanonicalUuid;
use crate::witness::{Witness, WitnessKind, WitnessKind as W};

/// Every admitted command discriminant.
///
/// Each variant is one row of WIRE.md's command registry. The discriminant
/// selects the body DTO, witness schema, transition, and reply DTO together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommandKind {
    /// Add one task as direct intent.
    TaskAdd,
    /// Replace an unsettled task statement.
    TaskEdit,
    /// Append an observation without replacing the statement.
    TaskNote,
    /// Set a wakeable deferral.
    TaskDefer,
    /// Clear the current deferral.
    TaskWake,
    /// Wire a dependency edge between two existing tasks.
    TaskDepends,
    /// Advance the claimed phase.
    TaskPhase,
    /// Complete the phase held by the caller's active claim.
    TaskDone,
    /// Record an assigned review verdict.
    TaskReview,
    /// Seat a reviewer under the anti-self-review rule.
    TaskAssign,

    /// Run the eligibility query and atomically win its claim.
    ClaimWin,
    /// Pass possession to a successor with ancestor lineage preserved.
    ClaimCarry,
    /// Extend a live, epoch-bound lease.
    ClaimRenew,
    /// Release the winning claim.
    ClaimFree,
    /// Expire possession whose lease deadline passed.
    ClaimLapse,

    /// Commit the exact normalized preview at its witnessed board barrier.
    DocApply,

    /// Append one durable directive to a lane.
    SteerSend,
    /// Send the same intent again with lineage and escalation carried.
    SteerResend,
    /// Retire a live directive with a typed disposition.
    SteerDispose,
    /// Withdraw a live directive as operator intent.
    SteerCancel,

    /// Decide one pending consent request.
    ConsentDecide,

    /// Quarantine a changed executable surface digest.
    TrustPend,
    /// Approve the exact executable digest.
    TrustGrant,
    /// Withdraw approval from the exact executable digest.
    TrustRevoke,

    /// Seat a lane in an operator-created worktree.
    LaneSeat,
    /// Change a seated lane's state.
    LaneState,

    /// Record a taste verdict at its evidence locus.
    TasteRecord,

    /// Recognize canary evidence.
    CanaryRecognize,
    /// Recognize vendor evidence.
    VendorRecognize,
    /// Recognize a repository commit.
    CommitRecognize,
    /// Recognize a sealed proof bundle.
    ProofRecognize,
    /// Publish the sealed manifest and each target artifact digest.
    ReleasePublish,

    /// Drive the resident to the requested lifecycle state.
    UpChange,
}

/// Text that does not spell a command discriminant.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("not a command in the closed registry: {0:?}")]
pub struct CommandKindError(String);

/// The witness law one command carries.
///
/// `required` must all appear in a command's `expected` list. `optional`
/// names predicates the row admits conditionally — an assignment witness on
/// the review path, a budget predicate when a rule spends one. Anything
/// outside the union is refused, because a command witnesses only the
/// subjects it relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Law {
    /// Predicates every instance of this command must prove.
    pub required: &'static [WitnessKind],
    /// Predicates this command admits but does not always require.
    pub optional: &'static [WitnessKind],
}

impl Law {
    /// True when this law admits `kind` at all.
    #[must_use]
    pub fn admits(&self, kind: WitnessKind) -> bool {
        self.required.contains(&kind) || self.optional.contains(&kind)
    }
}

/// The rows of WIRE.md's command registry, one constant apiece.
///
/// Each row is named for the move it governs rather than for a single
/// command, because several commands share one law: possession changes hands
/// under the same predicates whether it is carried, renewed, freed, or
/// lapsed. Every doc comment restates the row it implements.
impl Law {
    /// Intended handle is absent and every dependency handle exists at its
    /// named workflow head.
    pub const ADD: Self = Self {
        required: &[W::Absence],
        optional: &[W::Workflow],
    };
    /// Exact task statement head.
    pub const EDIT: Self = Self {
        required: &[W::Task],
        optional: &[],
    };
    /// Task identity; no statement revision witness, because notes append
    /// rather than replace.
    pub const NOTE: Self = Self {
        required: &[W::Existence],
        optional: &[],
    };
    /// Exact workflow head and current defer disposition.
    pub const DEFER: Self = Self {
        required: &[W::Workflow, W::Deferral],
        optional: &[],
    };
    /// Exact task statement and workflow heads of both tasks and the absence
    /// of the edge; no board barrier.
    pub const DEPENDS: Self = Self {
        required: &[W::Task, W::Workflow, W::Edge],
        optional: &[],
    };
    /// Exact task statement and workflow heads, winning claim head, live
    /// lease, assignment or review witness where required, bound lane and
    /// occupant, and resident epoch.
    pub const PROGRESS: Self = Self {
        required: &[W::Task, W::Workflow, W::Claim, W::Lease, W::Residency],
        optional: &[W::Assignment],
    };
    /// The review path is the one that always carries the assignment.
    pub const REVIEW: Self = Self {
        required: &[
            W::Task,
            W::Workflow,
            W::Claim,
            W::Lease,
            W::Residency,
            W::Assignment,
        ],
        optional: &[],
    };
    /// Exact workflow and claim heads plus the anti-self-review witness.
    pub const ASSIGN: Self = Self {
        required: &[W::Workflow, W::Claim, W::Assignment],
        optional: &[],
    };
    /// Exact task statement and workflow heads, current claim head,
    /// eligibility rule, lane occupancy, and resident epoch.
    pub const WIN: Self = Self {
        required: &[W::Task, W::Workflow, W::Claim, W::Eligibility, W::Residency],
        optional: &[],
    };
    /// Exact claim head, lease head, bound lane and occupant, and resident
    /// epoch.
    pub const POSSESSION: Self = Self {
        required: &[W::Claim, W::Lease, W::Residency],
        optional: &[],
    };
    /// Exact normalized preview digest and one named board barrier containing
    /// the task and workflow heads the preview read.
    pub const APPLY: Self = Self {
        required: &[W::Preview, W::Barrier],
        optional: &[],
    };
    /// A new directive names the lane it lands on; there is no prior
    /// disposition to re-prove.
    pub const SEND: Self = Self {
        required: &[W::Residency],
        optional: &[],
    };
    /// Exact directive disposition; resend also names the preceding directive
    /// in its lineage.
    pub const RESEND: Self = Self {
        required: &[W::Directive, W::Lineage],
        optional: &[],
    };
    /// Exact directive disposition. Cancel additionally requires operator
    /// capability, which the connection carries rather than the witness list.
    pub const DISPOSE: Self = Self {
        required: &[W::Directive],
        optional: &[],
    };
    /// Exact request disposition, named rule, and budget predicate when a rule
    /// spends or renews a budget.
    pub const DECIDE: Self = Self {
        required: &[W::Consent, W::Rule],
        optional: &[W::Budget],
    };
    /// Exact executable digest and its current trust state. Grant and revoke
    /// additionally require operator capability.
    pub const TRUST: Self = Self {
        required: &[W::Trust],
        optional: &[],
    };
    /// Exact residency head, lane and occupant identities when present, and
    /// resident epoch.
    pub const LANE: Self = Self {
        required: &[W::Residency],
        optional: &[],
    };
    /// Exact bag, evidence locus, and tracked taste configuration digest.
    pub const TASTE: Self = Self {
        required: &[W::Taste],
        optional: &[],
    };
    /// Exact observation key and the current head that makes recognition
    /// idempotent.
    pub const RECOGNIZE: Self = Self {
        required: &[W::Observation],
        optional: &[],
    };
    /// Publication also witnesses the sealed proof and canonical manifest
    /// digest.
    pub const PUBLISH: Self = Self {
        required: &[W::Observation, W::Sealed],
        optional: &[],
    };
    /// Exact residency head, current epoch, and the deadline or completion
    /// predicate admitting the requested edge.
    pub const UP: Self = Self {
        required: &[W::Residency, W::Transition],
        optional: &[],
    };
}

/// A command whose `expected` list does not satisfy its registry row.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LawError {
    /// A required predicate is absent from `expected`.
    #[error("{command} requires a {missing:?} witness")]
    Missing {
        /// Command whose law was violated.
        command: CommandKind,
        /// Predicate class the row requires.
        missing: WitnessKind,
    },
    /// `expected` carries a predicate the row does not admit.
    #[error("{command} does not witness {extra:?}")]
    Unadmitted {
        /// Command whose law was violated.
        command: CommandKind,
        /// Predicate class the row does not name.
        extra: WitnessKind,
    },
}

impl CommandKind {
    /// Every command in the closed registry, in declaration order.
    pub const ALL: &'static [Self] = &[
        Self::TaskAdd,
        Self::TaskEdit,
        Self::TaskNote,
        Self::TaskDefer,
        Self::TaskWake,
        Self::TaskDepends,
        Self::TaskPhase,
        Self::TaskDone,
        Self::TaskReview,
        Self::TaskAssign,
        Self::ClaimWin,
        Self::ClaimCarry,
        Self::ClaimRenew,
        Self::ClaimFree,
        Self::ClaimLapse,
        Self::DocApply,
        Self::SteerSend,
        Self::SteerResend,
        Self::SteerDispose,
        Self::SteerCancel,
        Self::ConsentDecide,
        Self::TrustPend,
        Self::TrustGrant,
        Self::TrustRevoke,
        Self::LaneSeat,
        Self::LaneState,
        Self::TasteRecord,
        Self::CanaryRecognize,
        Self::VendorRecognize,
        Self::CommitRecognize,
        Self::ProofRecognize,
        Self::ReleasePublish,
        Self::UpChange,
    ];

    /// The wire spelling of this command.
    ///
    /// This match is exhaustive, so a new variant must state its spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TaskAdd => "task::add",
            Self::TaskEdit => "task::edit",
            Self::TaskNote => "task::note",
            Self::TaskDefer => "task::defer",
            Self::TaskWake => "task::wake",
            Self::TaskDepends => "task::depends",
            Self::TaskPhase => "task::phase",
            Self::TaskDone => "task::done",
            Self::TaskReview => "task::review",
            Self::TaskAssign => "task::assign",
            Self::ClaimWin => "claim::win",
            Self::ClaimCarry => "claim::carry",
            Self::ClaimRenew => "claim::renew",
            Self::ClaimFree => "claim::free",
            Self::ClaimLapse => "claim::lapse",
            Self::DocApply => "doc::apply",
            Self::SteerSend => "steer::send",
            Self::SteerResend => "steer::resend",
            Self::SteerDispose => "steer::dispose",
            Self::SteerCancel => "steer::cancel",
            Self::ConsentDecide => "consent::decide",
            Self::TrustPend => "trust::pend",
            Self::TrustGrant => "trust::grant",
            Self::TrustRevoke => "trust::revoke",
            Self::LaneSeat => "lane::seat",
            Self::LaneState => "lane::state",
            Self::TasteRecord => "taste::record",
            Self::CanaryRecognize => "canary::recognize",
            Self::VendorRecognize => "vendor::recognize",
            Self::CommitRecognize => "commit::recognize",
            Self::ProofRecognize => "proof::recognize",
            Self::ReleasePublish => "release::publish",
            Self::UpChange => "up::change",
        }
    }

    /// The witness law this command carries.
    ///
    /// This is the registry. The match is exhaustive, so a command added
    /// without an explicit row fails to compile — the law cannot be forgotten
    /// or defaulted, only stated. Each row it names states, verbatim, the
    /// predicates the matching WIRE.md registry row demands.
    #[must_use]
    pub const fn law(self) -> Law {
        match self {
            Self::TaskAdd => Law::ADD,
            Self::TaskEdit => Law::EDIT,
            Self::TaskNote => Law::NOTE,
            Self::TaskDefer | Self::TaskWake => Law::DEFER,
            Self::TaskDepends => Law::DEPENDS,
            Self::TaskPhase | Self::TaskDone => Law::PROGRESS,
            Self::TaskReview => Law::REVIEW,
            Self::TaskAssign => Law::ASSIGN,
            Self::ClaimWin => Law::WIN,
            Self::ClaimCarry | Self::ClaimRenew | Self::ClaimFree | Self::ClaimLapse => {
                Law::POSSESSION
            }
            Self::DocApply => Law::APPLY,
            Self::SteerSend => Law::SEND,
            Self::SteerResend => Law::RESEND,
            Self::SteerDispose | Self::SteerCancel => Law::DISPOSE,
            Self::ConsentDecide => Law::DECIDE,
            Self::TrustPend | Self::TrustGrant | Self::TrustRevoke => Law::TRUST,
            Self::LaneSeat | Self::LaneState => Law::LANE,
            Self::TasteRecord => Law::TASTE,
            Self::CanaryRecognize
            | Self::VendorRecognize
            | Self::CommitRecognize
            | Self::ProofRecognize => Law::RECOGNIZE,
            Self::ReleasePublish => Law::PUBLISH,
            Self::UpChange => Law::UP,
        }
    }
}

impl FromStr for CommandKind {
    type Err = CommandKindError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .find(|kind| kind.as_str() == text)
            .copied()
            .ok_or_else(|| CommandKindError(text.to_owned()))
    }
}

impl fmt::Display for CommandKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for CommandKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CommandKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// A mutating request in one envelope.
///
/// The body contains no caller-authored actor, lane, epoch, journal position,
/// result, or fact envelope; the commit door assigns all of those.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command<B> {
    /// Current wire version.
    pub v: WireVersion,
    /// Client-minted canonical identity for this request.
    pub id: CanonicalUuid,
    /// Closed command discriminant.
    pub kind: CommandKind,
    /// Named predicates over only the subjects this command relies on.
    pub expected: Vec<Witness>,
    /// Kind-specific intent.
    pub body: B,
}

/// The expected predicates in a settled order, so two lists can be compared
/// as the sets they represent.
///
/// This ranks by [`Witness`]'s own order while [`Command::canonical_json`]
/// ranks by rendered text, and the two disagree about which predicate comes
/// first. That is harmless and the difference is not worth erasing: sorting by
/// any total order yields one representative per multiset, so both agree about
/// which lists are the same list, which is the only question either is asked.
///
/// Sorting borrows rather than clones because the caller only needs the two
/// sequences side by side for one comparison.
fn ranked(expected: &[Witness]) -> Vec<&Witness> {
    let mut ranked: Vec<&Witness> = expected.iter().collect();
    ranked.sort_unstable();
    ranked
}

impl<B: PartialEq> PartialEq for Command<B> {
    /// Two requests are the same request when they canonicalize alike.
    ///
    /// `expected` is a set of predicates carried in a list, so the order a
    /// client typed them in is not part of what it asked for.
    /// [`Command::canonical_json`] already sorts that order away, and deriving
    /// this would compare the vector position-wise and call two byte-identical
    /// requests different. One value would then have two identities, which is
    /// the same defect as one value having two spellings: a de-duplicating
    /// caller and the canonical bytes would disagree about what it had already
    /// seen.
    ///
    /// Multiplicity is kept rather than collapsed. Canonicalization sorts and
    /// does not deduplicate, so a predicate named twice survives into the
    /// bytes, and equality has to see what the bytes see.
    fn eq(&self, other: &Self) -> bool {
        self.v == other.v
            && self.id == other.id
            && self.kind == other.kind
            && self.body == other.body
            && ranked(&self.expected) == ranked(&other.expected)
    }
}

impl<B: Eq> Eq for Command<B> {}

impl<B> Command<B> {
    /// Check `expected` against this command's registry row.
    ///
    /// Every required predicate must appear, and no predicate outside the row
    /// may. A command that over-witnesses would lose races it does not depend
    /// on, so the law is refused from both sides.
    ///
    /// # Errors
    ///
    /// Returns the first missing required predicate, then the first predicate
    /// the row does not admit.
    pub fn witnessed(&self) -> Result<(), LawError> {
        let law = self.kind.law();
        for required in law.required {
            if !self
                .expected
                .iter()
                .any(|witness| witness.kind() == *required)
            {
                return Err(LawError::Missing {
                    command: self.kind,
                    missing: *required,
                });
            }
        }
        for witness in &self.expected {
            if !law.admits(witness.kind()) {
                return Err(LawError::Unadmitted {
                    command: self.kind,
                    extra: witness.kind(),
                });
            }
        }
        Ok(())
    }
}

impl<B: Serialize> Command<B> {
    /// Render the canonical encoding this command's identity binds.
    ///
    /// Witness order carries no meaning, so witnesses are sorted; object keys
    /// are emitted in sorted order by the encoder. The same request therefore
    /// yields the same bytes regardless of how a client ordered its input.
    ///
    /// # Errors
    ///
    /// Returns the encoder error when the body cannot be represented as JSON.
    pub fn canonical_json(&self) -> Result<String, serde_json::Error> {
        let mut sorted = serde_json::to_value(self)?;
        if let Some(expected) = sorted.get_mut("expected").and_then(|e| e.as_array_mut()) {
            expected.sort_by_key(std::string::ToString::to_string);
        }
        serde_json::to_string(&sorted)
    }
}
