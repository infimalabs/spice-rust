//! The command envelope: one typed, authenticated resident request crossing the socket.
//!
//! The set of commands is closed, and the registry below carries the witness
//! law beside the command. Both the spelling and the law are stated as
//! exhaustive matches over [`CommandKind`], so a new command that does not
//! carry an explicit registry row does not compile.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

mod kind;

use crate::envelope::{Actor, WireVersion};
use crate::hello::Accepted;
use crate::ident::CanonicalUuid;
use crate::intent::{CommandAnswer, Intent};
use crate::principal::{PrincipalRole, PrincipalRoles};
use crate::reply::{Fault, FaultClass, Outcome, Refusal, RefusalDiagnostic, Response};
use crate::witness::{Witness, WitnessKind};
mod law;

pub use law::{Authority, Law, LawError};

/// Every admitted command discriminant.
///
/// Each variant is one row of the command registry [`Self::law`] states. The
/// discriminant selects the body DTO, witness schema, transition, and reply DTO
/// together.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CommandKind {
    /// Read the validated current board heads.
    BoardRead,
    /// Read one bounded historical evidence page for the development exporter.
    ReportRead,
    /// Read one task's current evidence and durable note history.
    TaskDetail,
    /// Read a bounded typed relationship graph centered on selected tasks.
    TaskRelations,
    /// Page the task-workflow transitions a set of lanes caused.
    TaskTimeline,
    /// Render one deterministic board graph.
    BoardGraph,
    /// Compose one deterministic board handout.
    BoardHandout,
    /// Emit one canonical authored-document ledger.
    DocLedger,
    /// Publish one authored observation to an anchor task's room.
    RoomPost,
    /// Read one conversation publication's text from its owning source.
    RoomConversationText,
    /// A lane declines the Design request it owes in one anchor task's room.
    RoomLeave,
    /// A lane records one typed problem-space contribution in an open Design.
    RoomContribute,
    /// Resolve one Design contribution's cited sources at its own position
    /// and the question item it names.
    RoomContributionRead,
    /// Operator ACK, NAK, reply, recovery, or assessment of room authority.
    RoomControl,
    /// Read the current working-group partition.
    WorkingGroupRead,
    /// Move one design into a group, founding it, or out of every group, at
    /// the observed partition revision.
    WorkingGroupMove,
    /// Restate the whole working-group partition at the observed revision.
    WorkingGroupArrange,
    /// End one working group at the observed partition revision.
    WorkingGroupDisband,
    /// Set one working group's fast-mode flag at the observed revision.
    WorkingGroupFast,
    /// Set one working group's shared activity stance at the observed revision.
    WorkingGroupStance,
    /// Record one exact completed pass proposed by the active Design holder.
    DesignPropose,
    /// Record one source-backed critique of a Design pass proposal.
    CandidateCritiqueRecord,
    /// Record one source-backed review coverage statement.
    CandidateCoverageRecord,
    /// Record one exact operator disposition; selection also authorizes its subset.
    PreparationDecide,
    /// Withdraw one exact undecided candidate its author retained.
    CandidateWithdraw,
    /// Adopt pending existing work under preparation control.
    OutputAdopt,
    /// Ask one open Design to take its next pass.
    DesignNudge,
    /// Read durable global app settings.
    GlobalSettingsRead,
    /// Replace the durable global fast-mode setting.
    GlobalSettingsSet,
    /// Read the vendor choices this resident generation can seat.
    VendorCatalog,
    /// Add one task as direct intent.
    TaskAdd,
    /// Add one line-item Plan to the repository OOPS Design.
    TaskOopsAdd,
    /// Replace an unsettled task statement.
    TaskEdit,
    /// Withdraw a task from the board with a recorded reason.
    TaskDelete,
    /// Close a task as completed on the operator's word with a validation.
    TaskClose,
    /// Conclude an open, unheld Design done or collapsed on the operator's word.
    TaskConclude,
    /// Append an observation without replacing the statement.
    TaskNote,
    /// Set a wakeable deferral.
    TaskDefer,
    /// Clear the current deferral.
    TaskWake,
    /// Move one unfiled Draft into ordinary filed work.
    TaskFile,
    /// Add or remove dependency edges between existing tasks.
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
    /// Page operator-authored direction history, one fanout group per row.
    SteerHistory,
    /// Re-deliver one live directive and catch up any due presentation stages.
    SteerResend,
    /// Restore one lapsed directive without changing its identity.
    SteerRecover,
    /// Retire a live directive with a typed disposition.
    SteerDispose,
    /// Withdraw a live directive as operator intent.
    SteerCancel,
    /// Resolve one quote target against durable tape and journal authority.
    QuoteRead,

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

    /// Read every principal's display identity and this principal's roles.
    PrincipalDirectoryRead,
    /// Read every born human principal.
    PrincipalRead,
    /// Bear one new human principal.
    PrincipalAdmit,
    /// Change one principal's recorded profile.
    PrincipalEdit,
    /// Grant one role to one principal.
    PrincipalGrant,
    /// Revoke one role from one principal.
    PrincipalRevoke,
    /// Retire the capability one principal's admission issued.
    PrincipalDeauthorize,

    /// Read the current lane table at one resident barrier.
    LaneRead,
    /// Read the operator job catalog without claiming delivery.
    LaneJobsRead,
    /// Inspect one lane's keeper job without claiming its agent delivery.
    LaneJobRead,
    /// Ask the keeper to terminate one exact lane job.
    LaneJobStop,
    /// Seat a lane in an operator-created worktree.
    LaneSeat,
    /// Place one seated member before another member.
    LaneArrange,
    /// Change a seated lane's state.
    LaneState,
    /// Request or clear agent succession for one exact lane member.
    LaneRenewal,
    /// Change the model spelling one seated lane launches with.
    LaneRetune,
    /// Change the effort spelling one seated lane launches with.
    LaneEffortRetune,
    /// Change whether one seated lane launches in fast mode.
    LaneFastRetune,
    /// Change the shared task-allocation lifetime of one app team.
    TeamConfigure,
    /// Rearrange the order of every app team and the lanes each holds.
    TeamArrange,
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
    roles: BTreeSet<PrincipalRole>,
    kind: CommandKind,
    expected: Vec<Witness>,
    body: Intent,
    canonical_request: Vec<u8>,
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

    /// Whether the principal this command's actor acts as held `role` when
    /// the command was admitted. A lane or the resident holds none.
    #[must_use]
    pub fn holds(&self, role: PrincipalRole) -> bool {
        self.roles.contains(&role)
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

    /// Canonical crossing bytes whose client identity this admission binds.
    #[must_use]
    pub fn canonical_request(&self) -> &[u8] {
        &self.canonical_request
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
    /// Admit a crossing command under one authenticated socket acceptance
    /// and the principal roles current when it arrives.
    ///
    /// Wire and epoch checks run before command law and body admission. The
    /// actor comes only from `accepted`; no caller-authored principal exists
    /// in [`Command`]. Authority is decided by `roles` alone: an operator
    /// command needs an actor whose principal holds the operator role, and a
    /// connected command needs a lane or an actor acting as a born principal.
    /// A successful result therefore carries a current generation, an
    /// authorized principal and the roles it held, exact admitted witnesses,
    /// and the body shape selected by its kind.
    ///
    /// # Errors
    ///
    /// Returns a refusal for stale wire versions, insufficient authority, or
    /// witness-law violations. Returns a protocol fault for a stale resident
    /// epoch or a malformed kind-selected body.
    pub fn admit(
        self,
        accepted: &Accepted,
        roles: &PrincipalRoles,
    ) -> Result<AdmittedCommand, CommandRejection> {
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
        let held = roles.roles(&accepted.actor);
        let authorized = match law.authority {
            Authority::Connected => matches!(accepted.actor, Actor::Lane(_)) || held.is_some(),
            Authority::Operator => held
                .as_ref()
                .is_some_and(|held| held.contains(&PrincipalRole::Operator)),
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
        let canonical_request = self
            .canonical_json()
            .map(String::into_bytes)
            .map_err(|error| {
                CommandRejection::Fault(Fault {
                    class: FaultClass::Protocol,
                    reason: "command::canonicalization-failed".to_owned(),
                    detail: error.to_string(),
                })
            })?;
        Ok(AdmittedCommand {
            id: self.id,
            epoch: self.epoch,
            actor: accepted.actor.clone(),
            roles: held.unwrap_or_default(),
            kind: self.kind,
            expected: self.expected,
            body,
            canonical_request,
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
