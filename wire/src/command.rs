//! The command envelope: one typed, authenticated resident request crossing the socket.
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

use crate::envelope::{Actor, WireVersion};
use crate::hello::Accepted;
use crate::ident::CanonicalUuid;
use crate::intent::{CommandAnswer, Intent};
use crate::reply::{Fault, FaultClass, Outcome, Refusal, RefusalDiagnostic, Response};
use crate::witness::{Witness, WitnessKind};

/// Every admitted command discriminant.
///
/// Each variant is one row of the command registry [`Self::law`] states. The
/// discriminant selects the body DTO, witness schema, transition, and reply DTO
/// together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommandKind {
    /// Read the validated current board heads.
    BoardRead,
    /// Render one deterministic board graph.
    BoardGraph,
    /// Compose one deterministic board handout.
    BoardHandout,
    /// Emit one canonical authored-document ledger.
    DocLedger,
    /// Read reduced claim-to-terminal phase effort.
    EffortRead,
    /// Add one task as direct intent.
    TaskAdd,
    /// Add one task through the fixed server-owned `.oops` route.
    TaskOopsAdd,
    /// File one mined taste candidate through the fixed hidden proposal route.
    TasteProposalAdd,
    /// File gate findings as deferred board work.
    GateFindings,
    /// Replace an unsettled task statement.
    TaskEdit,
    /// Withdraw a task from the board with a recorded reason.
    TaskDelete,
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
    /// Read pending direction visible to the authenticated principal.
    SteerInbox,
    /// Rebuild one lane's inbox fold from current journal authority.
    SteerRepair,
    /// Re-deliver one live directive and catch up any due presentation stages.
    SteerResend,
    /// Restore one lapsed directive without changing its identity.
    SteerRecover,
    /// Retire a live directive with a typed disposition.
    SteerDispose,
    /// Withdraw a live directive as operator intent.
    SteerCancel,

    /// Read the pending consent queue.
    ConsentRead,
    /// Read the tracked consent policy.
    ConsentPolicy,
    /// Decide one pending consent request.
    ConsentDecide,

    /// Quarantine a changed executable surface digest.
    TrustPend,
    /// Approve the exact executable digest.
    TrustGrant,
    /// Withdraw approval from the exact executable digest.
    TrustRevoke,

    /// Read the current lane table at one resident barrier.
    LaneRead,
    /// Seat a lane in an operator-created worktree.
    LaneSeat,
    /// Place one seated member before another member.
    LaneArrange,
    /// Change a seated lane's state.
    LaneState,
    /// Change the shared task-subscription life of one Canvas lineage.
    LineageConfigure,
    /// Rearrange the order of every Canvas lineage and the lanes each holds.
    LineageArrange,
    /// Build one bounded continuity account from the current resident barrier.
    LaneRecall,

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

/// The capability a connection must carry to submit one command.
///
/// identity-law: non-crossing - registry metadata held in static process memory.
///
/// This is law rather than witness. A witness proves a fact about the board
/// and travels in the command's `expected` list; authority proves who is
/// asking, and the resident derives it from the authenticated capability that
/// opened the session. Nothing here is serializable for that reason: a client
/// that could author its own authority would be answering the question the
/// capability exists to answer.
///
/// A client cannot encode an authority into a request:
///
/// ```compile_fail
/// let _ = wire::encode(&wire::Authority::Connected);
/// ```
///
/// Nor can bytes crossing the socket manufacture one:
///
/// ```compile_fail
/// let _ = wire::decode::<wire::Authority>(b"null");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Authority {
    /// Any authenticated connection may submit, operator or lane alike.
    Connected,
    /// Only an operator connection may submit.
    Operator,
}

/// The law one command carries.
///
/// identity-law: non-crossing - registry metadata held in static process memory.
///
/// `required` must all appear in a command's `expected` list. `optional`
/// names predicates the row admits conditionally — an assignment witness on
/// the review path, a budget predicate when a rule spends one. Anything
/// outside the union is refused, because a command witnesses only the
/// subjects it relies on.
///
/// `authority` states the same row's capability requirement. It sits here
/// rather than beside a reader because a row that named its witnesses and left
/// its authority to prose would be restated by everyone who consumed it, and
/// restatements drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Law {
    /// Predicates every instance of this command must prove.
    pub required: &'static [WitnessKind],
    /// Predicates this command admits but does not always require.
    pub optional: &'static [WitnessKind],
    /// The capability a connection must carry to submit this command.
    pub authority: Authority,
}

impl Law {
    /// Authenticated read with no caller-proposed authority predicate.
    pub const READ: Self = Self {
        required: &[],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Operator-only authenticated read.
    pub const OPERATOR_READ: Self = Self {
        required: &[],
        optional: &[],
        authority: Authority::Operator,
    };
    /// Snapshot-only operator inspection carries no mutable predicate.
    pub const INSPECT: Self = Self {
        required: &[],
        optional: &[],
        authority: Authority::Operator,
    };
    /// True when this law admits `kind` at all.
    #[must_use]
    pub fn admits(&self, kind: WitnessKind) -> bool {
        self.required.contains(&kind) || self.optional.contains(&kind)
    }
}

/// The rows of the command registry, one constant apiece.
///
/// Each row is named for the move it governs rather than for a single
/// command, because several commands share one law: possession changes hands
/// under the same predicates whether it is carried, renewed, freed, or
/// lapsed. Every doc comment restates the row it implements.
impl Law {
    /// Intended inception stamp is absent and every dependency exists at its
    /// named workflow head.
    pub const ADD: Self = Self {
        required: &[WitnessKind::Absence],
        optional: &[WitnessKind::Workflow],
        authority: Authority::Connected,
    };
    /// Gate finding dispatch derives its task witnesses inside the resident
    /// transaction from one typed gate report fold.
    pub const GATE_FINDINGS: Self = Self {
        required: &[],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact task statement head.
    pub const EDIT: Self = Self {
        required: &[WitnessKind::Task],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact task statement and workflow heads plus the current claim
    /// disposition.
    ///
    /// The claim predicate is what makes the refusal honest rather than
    /// advisory. Withdrawal decides against possession, so reading the claim
    /// from a board snapshot and deciding afterwards would let a claim landing
    /// in between take the decision's basis away; naming the head the decision
    /// was made against moves that race into the admission door.
    pub const DELETE: Self = Self {
        required: &[WitnessKind::Task, WitnessKind::Workflow, WitnessKind::Claim],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Task identity; no statement revision witness, because notes append
    /// rather than replace.
    pub const NOTE: Self = Self {
        required: &[WitnessKind::Existence],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact workflow head and current defer disposition.
    pub const DEFER: Self = Self {
        required: &[WitnessKind::Workflow, WitnessKind::Deferral],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact workflow and defer disposition, plus the statement head when a
    /// parked wake also promotes the task into another project.
    pub const WAKE: Self = Self {
        required: &[WitnessKind::Workflow, WitnessKind::Deferral],
        optional: &[WitnessKind::Task],
        authority: Authority::Connected,
    };
    /// Exact task statement and workflow heads of both tasks and the absence
    /// of the edge; no board barrier.
    pub const DEPENDS: Self = Self {
        required: &[WitnessKind::Task, WitnessKind::Workflow, WitnessKind::Edge],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact task statement and workflow heads, winning claim head, live
    /// lease, assignment or review witness where required, bound lane and
    /// occupant, and resident epoch.
    pub const PROGRESS: Self = Self {
        required: &[
            WitnessKind::Task,
            WitnessKind::Workflow,
            WitnessKind::Claim,
            WitnessKind::Lease,
            WitnessKind::Residency,
        ],
        optional: &[WitnessKind::Assignment],
        authority: Authority::Connected,
    };
    /// The review path is the one that always carries the assignment, and the
    /// only one that may also carry follow-up tracking: the absence of each
    /// stamp it mints and of each edge it wires, exactly the predicates
    /// [`Self::ADD`] and [`Self::DEPENDS`] require of those moves alone.
    pub const REVIEW: Self = Self {
        required: &[
            WitnessKind::Task,
            WitnessKind::Workflow,
            WitnessKind::Claim,
            WitnessKind::Lease,
            WitnessKind::Residency,
            WitnessKind::Assignment,
        ],
        optional: &[WitnessKind::Absence, WitnessKind::Edge],
        authority: Authority::Connected,
    };
    /// Exact workflow and claim heads plus the anti-self-review witness.
    pub const ASSIGN: Self = Self {
        required: &[
            WitnessKind::Workflow,
            WitnessKind::Claim,
            WitnessKind::Assignment,
        ],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact task statement and workflow heads, current claim head,
    /// eligibility rule, lane occupancy, and resident epoch.
    pub const WIN: Self = Self {
        required: &[
            WitnessKind::Task,
            WitnessKind::Workflow,
            WitnessKind::Claim,
            WitnessKind::Eligibility,
            WitnessKind::Residency,
        ],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact claim head, lease head, bound lane and occupant, and resident
    /// epoch.
    pub const POSSESSION: Self = Self {
        required: &[
            WitnessKind::Claim,
            WitnessKind::Lease,
            WitnessKind::Residency,
        ],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact normalized preview digest and one named board barrier containing
    /// the task and workflow heads the preview read.
    pub const APPLY: Self = Self {
        required: &[WitnessKind::Preview, WitnessKind::Barrier],
        optional: &[],
        authority: Authority::Connected,
    };
    /// A new directive names the lane it lands on; there is no prior
    /// disposition to re-prove.
    pub const SEND: Self = Self {
        required: &[WitnessKind::Residency],
        optional: &[],
        authority: Authority::Operator,
    };
    /// Operator-owned repair of a disposable rendering from durable authority.
    pub const REPAIR: Self = Self {
        required: &[],
        optional: &[],
        authority: Authority::Operator,
    };
    /// Exact live directive disposition for a time-derived re-presentation.
    pub const RESEND: Self = Self {
        required: &[WitnessKind::Directive],
        optional: &[],
        authority: Authority::Operator,
    };
    /// Exact directive identity in either recoverable failure state.
    pub const RECOVER: Self = Self {
        required: &[WitnessKind::Recoverable],
        optional: &[],
        authority: Authority::Operator,
    };
    /// Exact directive disposition.
    pub const DISPOSE: Self = Self {
        required: &[],
        optional: &[WitnessKind::Directive],
        authority: Authority::Connected,
    };
    /// Exact directive disposition, withdrawn as operator intent.
    ///
    /// Cancel witnesses exactly what dispose witnesses and answers to a
    /// different capability, so it is its own row. Sharing one with dispose
    /// would leave the authority unsayable here and restated elsewhere.
    pub const CANCEL: Self = Self {
        required: &[WitnessKind::Directive],
        optional: &[],
        authority: Authority::Operator,
    };
    /// Exact request disposition, named rule, and budget predicate when a rule
    /// spends or renews a budget, decided as the operator.
    ///
    /// This row states the predicates without naming a capability. Consent is
    /// the operator's answer to a request raised on their behalf, and a lane
    /// deciding its own request would be granting itself the consent it was
    /// asked to wait for.
    pub const DECIDE: Self = Self {
        required: &[WitnessKind::Consent, WitnessKind::Rule],
        optional: &[WitnessKind::Budget],
        authority: Authority::Operator,
    };
    /// Exact executable digest and its current trust state.
    ///
    /// Pending quarantines a digest, which is a report that a surface changed
    /// rather than a judgment about it.
    pub const PEND: Self = Self {
        required: &[WitnessKind::Trust],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact executable digest and its current trust state, judged as the
    /// operator.
    ///
    /// Granting and revoking are the judgment pending asks for, so they carry
    /// the capability that can give it.
    pub const TRUST: Self = Self {
        required: &[WitnessKind::Trust],
        optional: &[],
        authority: Authority::Operator,
    };
    /// Operator-authored lane lifecycle intent. The command envelope binds the
    /// current resident epoch; the handler compares the named lane with the
    /// current residency head before changing it. A residency witness cannot
    /// represent either a lane that does not exist yet or a vacant lane.
    pub const LANE: Self = Self {
        required: &[],
        optional: &[],
        authority: Authority::Operator,
    };
    /// Read the lane table as the operator without authoring a residency
    /// witness for the state being observed.
    pub const LANE_READ: Self = Self {
        required: &[],
        optional: &[],
        authority: Authority::Operator,
    };
    /// Read one bounded continuity account as either the operator or the lane
    /// capability whose view the resident filters.
    pub const LANE_RECALL: Self = Self {
        required: &[],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact bag, evidence locus, and tracked taste configuration digest.
    pub const TASTE: Self = Self {
        required: &[WitnessKind::Taste],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Exact observation key and the current head that makes recognition
    /// idempotent.
    pub const RECOGNIZE: Self = Self {
        required: &[WitnessKind::Observation],
        optional: &[],
        authority: Authority::Connected,
    };
    /// Publication also witnesses the sealed proof and canonical manifest
    /// digest.
    pub const PUBLISH: Self = Self {
        required: &[WitnessKind::Observation, WitnessKind::Sealed],
        optional: &[],
        authority: Authority::Operator,
    };
    /// Current lifecycle edge. The command envelope separately binds the
    /// resident epoch, so an operator lifecycle request does not fabricate a
    /// lane-and-occupant residency witness.
    pub const UP: Self = Self {
        required: &[WitnessKind::Transition],
        optional: &[],
        authority: Authority::Operator,
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
        Self::BoardRead,
        Self::BoardGraph,
        Self::BoardHandout,
        Self::DocLedger,
        Self::EffortRead,
        Self::TaskAdd,
        Self::TaskOopsAdd,
        Self::TasteProposalAdd,
        Self::GateFindings,
        Self::TaskEdit,
        Self::TaskDelete,
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
        Self::SteerInbox,
        Self::SteerRepair,
        Self::SteerResend,
        Self::SteerRecover,
        Self::SteerDispose,
        Self::SteerCancel,
        Self::ConsentRead,
        Self::ConsentPolicy,
        Self::ConsentDecide,
        Self::TrustPend,
        Self::TrustGrant,
        Self::TrustRevoke,
        Self::LaneRead,
        Self::LaneSeat,
        Self::LaneArrange,
        Self::LaneState,
        Self::LineageConfigure,
        Self::LineageArrange,
        Self::LaneRecall,
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
    /// The outer partition is exhaustive, so a new variant must state which
    /// closed registry section owns its spelling. The section matches remain
    /// short enough to review as bounded tables.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BoardRead
            | Self::BoardGraph
            | Self::BoardHandout
            | Self::DocLedger
            | Self::EffortRead
            | Self::TaskAdd
            | Self::TaskOopsAdd
            | Self::TasteProposalAdd
            | Self::GateFindings
            | Self::TaskEdit
            | Self::TaskDelete
            | Self::TaskNote
            | Self::TaskDefer
            | Self::TaskWake
            | Self::TaskDepends
            | Self::TaskPhase
            | Self::TaskDone
            | Self::TaskReview
            | Self::TaskAssign => self.board_spelling(),
            Self::ClaimWin
            | Self::ClaimCarry
            | Self::ClaimRenew
            | Self::ClaimFree
            | Self::ClaimLapse
            | Self::DocApply
            | Self::SteerSend
            | Self::SteerInbox
            | Self::SteerRepair
            | Self::SteerResend
            | Self::SteerRecover
            | Self::SteerDispose
            | Self::SteerCancel
            | Self::ConsentRead
            | Self::ConsentPolicy
            | Self::ConsentDecide => self.coordination_spelling(),
            Self::TrustPend
            | Self::TrustGrant
            | Self::TrustRevoke
            | Self::LaneRead
            | Self::LaneSeat
            | Self::LaneArrange
            | Self::LaneState
            | Self::LineageConfigure
            | Self::LineageArrange
            | Self::LaneRecall
            | Self::TasteRecord
            | Self::CanaryRecognize
            | Self::VendorRecognize
            | Self::CommitRecognize
            | Self::ProofRecognize
            | Self::ReleasePublish
            | Self::UpChange => self.runtime_spelling(),
        }
    }

    const fn board_spelling(self) -> &'static str {
        match self {
            Self::BoardRead => "board::read",
            Self::BoardGraph => "board::graph",
            Self::BoardHandout => "board::handout",
            Self::DocLedger => "doc::ledger",
            Self::EffortRead => "effort::read",
            Self::TaskAdd => "task::add",
            Self::TaskOopsAdd => "task::oops-add",
            Self::TasteProposalAdd => "taste::proposal-add",
            Self::GateFindings => "gate::findings",
            Self::TaskEdit => "task::edit",
            Self::TaskDelete => "task::delete",
            Self::TaskNote => "task::note",
            Self::TaskDefer => "task::defer",
            Self::TaskWake => "task::wake",
            Self::TaskDepends => "task::depends",
            Self::TaskPhase => "task::phase",
            Self::TaskDone => "task::done",
            Self::TaskReview => "task::review",
            Self::TaskAssign => "task::assign",
            _ => panic!("command spelling partition is closed"),
        }
    }

    const fn coordination_spelling(self) -> &'static str {
        match self {
            Self::ClaimWin => "claim::win",
            Self::ClaimCarry => "claim::carry",
            Self::ClaimRenew => "claim::renew",
            Self::ClaimFree => "claim::free",
            Self::ClaimLapse => "claim::lapse",
            Self::DocApply => "doc::apply",
            Self::SteerSend => "steer::send",
            Self::SteerInbox => "steer::inbox",
            Self::SteerRepair => "steer::repair",
            Self::SteerResend => "steer::resend",
            Self::SteerRecover => "steer::recover",
            Self::SteerDispose => "steer::dispose",
            Self::SteerCancel => "steer::cancel",
            Self::ConsentRead => "consent::read",
            Self::ConsentPolicy => "consent::policy",
            Self::ConsentDecide => "consent::decide",
            _ => panic!("command spelling partition is closed"),
        }
    }

    const fn runtime_spelling(self) -> &'static str {
        match self {
            Self::TrustPend => "trust::pend",
            Self::TrustGrant => "trust::grant",
            Self::TrustRevoke => "trust::revoke",
            Self::LaneRead => "lane::read",
            Self::LaneSeat => "lane::seat",
            Self::LaneArrange => "lane::arrange",
            Self::LaneState => "lane::state",
            Self::LineageConfigure => "lineage::configure",
            Self::LineageArrange => "lineage::arrange",
            Self::LaneRecall => "lane::recall",
            Self::TasteRecord => "taste::record",
            Self::CanaryRecognize => "canary::recognize",
            Self::VendorRecognize => "vendor::recognize",
            Self::CommitRecognize => "commit::recognize",
            Self::ProofRecognize => "proof::recognize",
            Self::ReleasePublish => "release::publish",
            Self::UpChange => "up::change",
            _ => panic!("command spelling partition is closed"),
        }
    }

    /// The witness law this command carries.
    ///
    /// This is the registry. The match is exhaustive, so a command added
    /// without an explicit row fails to compile — the law cannot be forgotten
    /// or defaulted, only stated. Each row it names states its predicates in
    /// full rather than deriving them from a neighbour, so the law governing
    /// one command is read in one place.
    #[must_use]
    pub const fn law(self) -> Law {
        match self {
            Self::BoardRead | Self::BoardGraph | Self::DocLedger | Self::EffortRead => Law::READ,
            Self::BoardHandout => Law::OPERATOR_READ,
            Self::TaskAdd | Self::TaskOopsAdd | Self::TasteProposalAdd => Law::ADD,
            Self::GateFindings => Law::GATE_FINDINGS,
            Self::TaskEdit => Law::EDIT,
            Self::TaskDelete => Law::DELETE,
            Self::TaskNote => Law::NOTE,
            Self::TaskDefer => Law::DEFER,
            Self::TaskWake => Law::WAKE,
            Self::TaskDepends => Law::DEPENDS,
            Self::TaskPhase | Self::TaskDone => Law::PROGRESS,
            Self::TaskReview => Law::REVIEW,
            Self::TaskAssign => Law::ASSIGN,
            Self::ClaimWin => Law::WIN,
            Self::ClaimCarry | Self::ClaimRenew | Self::ClaimFree | Self::ClaimLapse => {
                Law::POSSESSION
            }
            _ => self.non_task_law(),
        }
    }

    fn required_for_intent(self, body: &Intent) -> &'static [WitnessKind] {
        if matches!(
            (self, body),
            (Self::TaskWake, Intent::TaskWake(wake)) if wake.into.is_some()
        ) {
            return &[
                WitnessKind::Workflow,
                WitnessKind::Deferral,
                WitnessKind::Task,
            ];
        }
        self.law().required
    }

    fn witnessed_intent(self, expected: &[Witness], body: &Intent) -> Result<(), LawError> {
        for required in self.required_for_intent(body) {
            if !expected.iter().any(|witness| witness.kind() == *required) {
                return Err(LawError::Missing {
                    command: self,
                    missing: *required,
                });
            }
        }
        Ok(())
    }

    const fn non_task_law(self) -> Law {
        match self {
            Self::DocApply => Law::APPLY,
            Self::SteerSend => Law::SEND,
            Self::SteerInbox => Law::READ,
            Self::SteerRepair => Law::REPAIR,
            Self::SteerResend => Law::RESEND,
            Self::SteerRecover => Law::RECOVER,
            Self::SteerDispose => Law::DISPOSE,
            Self::SteerCancel => Law::CANCEL,
            Self::ConsentRead | Self::ConsentPolicy => Law::INSPECT,
            Self::ConsentDecide => Law::DECIDE,
            Self::TrustPend => Law::PEND,
            Self::TrustGrant | Self::TrustRevoke => Law::TRUST,
            Self::LaneRead => Law::LANE_READ,
            Self::LaneSeat
            | Self::LaneArrange
            | Self::LaneState
            | Self::LineageConfigure
            | Self::LineageArrange => Law::LANE,
            Self::LaneRecall => Law::LANE_RECALL,
            Self::TasteRecord => Law::TASTE,
            Self::CanaryRecognize
            | Self::VendorRecognize
            | Self::CommitRecognize
            | Self::ProofRecognize => Law::RECOGNIZE,
            Self::ReleasePublish => Law::PUBLISH,
            Self::UpChange => Law::UP,
            _ => panic!("command law partition is closed"),
        }
    }

    /// Whether this request reads one snapshot and commits no authority fact.
    #[must_use]
    pub const fn is_read(self) -> bool {
        matches!(
            self,
            Self::BoardRead
                | Self::BoardGraph
                | Self::BoardHandout
                | Self::DocLedger
                | Self::EffortRead
                | Self::SteerInbox
                | Self::ConsentRead
                | Self::ConsentPolicy
                | Self::LaneRead
                | Self::LaneRecall
        )
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

/// A typed resident request in one envelope.
///
/// The body contains no caller-authored actor, lane, epoch, journal position,
/// result, or fact envelope. A mutating handler assigns those at the commit
/// door; a read handler answers at its snapshot cursor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command<B> {
    /// Current wire version.
    pub v: WireVersion,
    /// Client-minted canonical identity for this request.
    pub id: CanonicalUuid,
    /// Resident generation this request addresses.
    pub epoch: CanonicalUuid,
    /// Closed command discriminant.
    pub kind: CommandKind,
    /// Named predicates over only the subjects this command relies on.
    pub expected: Vec<Witness>,
    /// Kind-specific intent.
    pub body: B,
}

/// One command admitted by the resident's authenticated socket door.
///
/// The caller never sends an actor. Admission derives it from the accepted
/// capability, verifies the request's generation and registry law, and
/// decodes the body selected by the closed command discriminant before this
/// value can exist.
///
/// identity-law: non-crossing - authenticated process-only admission state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmittedCommand {
    id: CanonicalUuid,
    epoch: CanonicalUuid,
    actor: Actor,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: Intent,
}

impl AdmittedCommand {
    /// Client-minted identity bound to this request and its response.
    #[must_use]
    pub const fn id(&self) -> CanonicalUuid {
        self.id
    }

    /// Resident generation that admitted this request.
    #[must_use]
    pub const fn epoch(&self) -> CanonicalUuid {
        self.epoch
    }

    /// Principal derived from the capability that opened the session.
    #[must_use]
    pub const fn actor(&self) -> &Actor {
        &self.actor
    }

    /// Closed command discriminant selecting this body's law and answer.
    #[must_use]
    pub const fn kind(&self) -> CommandKind {
        self.kind
    }

    /// Exact predicates admitted with this request.
    #[must_use]
    pub fn expected(&self) -> &[Witness] {
        &self.expected
    }

    /// Kind-selected, fully decoded caller intent.
    #[must_use]
    pub const fn body(&self) -> &Intent {
        &self.body
    }

    /// Bind one terminal command outcome to this request and generation.
    #[must_use]
    pub fn response(&self, outcome: Outcome<CommandAnswer>) -> Response<CommandAnswer> {
        Response {
            v: WireVersion::CURRENT,
            id: self.id,
            epoch: self.epoch,
            outcome,
        }
    }
}

/// Why a syntactically decoded command did not reach business logic.
///
/// identity-law: non-crossing - converted into its typed crossing outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandRejection {
    /// The caller can retry after changing the request or capability.
    Refusal(Refusal),
    /// The request addressed an invalid generation or carried an invalid body.
    Fault(Fault),
}

impl CommandRejection {
    /// Convert this rejection into the wire outcome returned for the request.
    #[must_use]
    pub fn outcome(self) -> Outcome<CommandAnswer> {
        match self {
            Self::Refusal(refusal) => Outcome::Refusal(refusal),
            Self::Fault(fault) => Outcome::Fault(fault),
        }
    }
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
            && self.epoch == other.epoch
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

impl Command<serde_json::Value> {
    /// Admit a crossing command under one authenticated socket acceptance.
    ///
    /// Wire and epoch checks run before command law and body admission. The
    /// actor comes only from `accepted`; no caller-authored principal exists
    /// in [`Command`]. A successful result therefore carries a current
    /// generation, an authorized principal, exact admitted witnesses, and the
    /// body shape selected by its kind.
    ///
    /// # Errors
    ///
    /// Returns a refusal for stale wire versions, insufficient authority, or
    /// witness-law violations. Returns a protocol fault for a stale resident
    /// epoch or a malformed kind-selected body.
    pub fn admit(self, accepted: &Accepted) -> Result<AdmittedCommand, CommandRejection> {
        if !self.v.is_current() {
            return Err(CommandRejection::Refusal(Refusal::bare(
                RefusalDiagnostic::new(
                    "command::wire-mismatch",
                    None,
                    Some(WireVersion::CURRENT.to_string()),
                    Some(self.v.to_string()),
                ),
            )));
        }
        if self.epoch != accepted.epoch {
            return Err(CommandRejection::Fault(Fault {
                class: FaultClass::Protocol,
                reason: "command::epoch-mismatch".to_owned(),
                detail: format!(
                    "request addressed epoch {}; authenticated session is {}",
                    self.epoch, accepted.epoch
                ),
            }));
        }
        let law = self.kind.law();
        let authorized = match law.authority {
            Authority::Connected => matches!(accepted.actor, Actor::Operator | Actor::Lane(_)),
            Authority::Operator => matches!(accepted.actor, Actor::Operator),
        };
        if !authorized {
            let expected = match law.authority {
                Authority::Connected => "operator or lane",
                Authority::Operator => "operator",
            };
            return Err(CommandRejection::Refusal(Refusal::bare(
                RefusalDiagnostic::new(
                    "command::authority",
                    Some("principal".to_owned()),
                    Some(expected.to_owned()),
                    Some(accepted.actor.to_string()),
                ),
            )));
        }
        if let Err(error) = self.witnessed() {
            return Err(witness_law_rejection(error, law.required, &self.expected));
        }
        let encoded = serde_json::to_vec(&self.body).map_err(|error| {
            CommandRejection::Fault(Fault {
                class: FaultClass::Protocol,
                reason: "command::body-unencodable".to_owned(),
                detail: error.to_string(),
            })
        })?;
        let body = self.kind.binding().body.decode(&encoded).map_err(|error| {
            CommandRejection::Fault(Fault {
                class: FaultClass::Protocol,
                reason: "command::body-undecodable".to_owned(),
                detail: error.to_string(),
            })
        })?;
        if let Err(error) = self.kind.witnessed_intent(&self.expected, &body) {
            return Err(witness_law_rejection(
                error,
                self.kind.required_for_intent(&body),
                &self.expected,
            ));
        }
        Ok(AdmittedCommand {
            id: self.id,
            epoch: self.epoch,
            actor: accepted.actor.clone(),
            kind: self.kind,
            expected: self.expected,
            body,
        })
    }
}

fn witness_law_rejection(
    error: LawError,
    required: &[WitnessKind],
    expected: &[Witness],
) -> CommandRejection {
    CommandRejection::Refusal(Refusal::bare(RefusalDiagnostic::new(
        "command::witness-law",
        Some(error.to_string()),
        Some(format!("{required:?}")),
        Some(format!(
            "{:?}",
            expected.iter().map(Witness::kind).collect::<Vec<_>>()
        )),
    )))
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
