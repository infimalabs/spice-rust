//! Capability-scoped panes: what a principal sees and what it is offered.
//!
//! A pane is an affordance, not an authority. The resident authenticates every
//! command it receives and refuses one this layer would happily have built, so
//! nothing here is a security boundary. What a pane prevents is a misleading
//! control: an occupant is never shown a button whose only possible answer is
//! a refusal.
//!
//! Three panes divide the surface. A [`ReadPane`] shows one packed answer and
//! holds its state by shared reference, so no append path exists to call. A
//! [`SteerPane`] builds ordinary commands and keeps [`Outcome`] three-valued.
//! A [`GrantPane`] opens for the operator alone and presents the commands
//! whose wire law rows additionally require the operator capability.
//!
//! Every pane borrows the [`Principal`] it was opened for, so no pane outlives
//! the acceptance that produced it, and a [`Principal`] exists only by way of
//! a frame the resident sent and this session admitted.

use std::error::Error;
use std::fmt;

use wire::{
    Accepted, Actor, CanonicalUuid, Command, CommandKind, Fault, FaultClass, LawError, Outcome,
    Overflow, Response, Seq, WireVersion, Witness,
};

use crate::socket::Admitted;
use crate::state::BrowserState;

/// Reason named when an answer does not belong to the command it was paired
/// with.
const ANSWER_MISMATCH: &str = "pane::answer-mismatch";

/// Who this session acts as, in the generation it acts in.
///
/// A capability is bound to its principal and its epoch together, so the two
/// travel together here as well. There is no constructor from text, from a
/// bare [`Actor`], or from anything the app rendered. The only door is
/// [`Principal::admitted`], which needs a frame the resident sent and
/// [`Session::admit_accepted`] let through, because the resident derives actor
/// and rights when it accepts the connection and caller fields may describe
/// provenance but cannot grant it.
///
/// [`Session::admit_accepted`]: crate::Session::admit_accepted
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    actor: Actor,
    epoch: CanonicalUuid,
}

impl Principal {
    /// Adopt the principal the resident derived for this session.
    #[must_use]
    pub fn admitted(accepted: &Admitted<Accepted>) -> Self {
        let accepted = accepted.get();
        Self {
            actor: accepted.actor.clone(),
            epoch: accepted.epoch,
        }
    }

    /// The actor this session acts as.
    #[must_use]
    pub const fn actor(&self) -> &Actor {
        &self.actor
    }

    /// The generation this principal was accepted into.
    #[must_use]
    pub const fn epoch(&self) -> CanonicalUuid {
        self.epoch
    }

    /// True when this principal carries the operator capability.
    #[must_use]
    pub const fn is_operator(&self) -> bool {
        matches!(self.actor, Actor::Operator)
    }
}

/// True when this command's wire law row additionally requires the operator
/// capability.
///
/// A wire law row carries that requirement in prose rather than in its witness
/// list, because the capability is carried by the connection and not proved by
/// a predicate. This table restates those rows and decides nothing on its own;
/// `panes_restate_the_wire_law_rows` reads the rows and fails when the two
/// disagree in either direction.
///
/// The match is exhaustive, so a command added to the registry must state its
/// class here rather than defaulting into the lane-visible set.
const fn requires_operator(kind: CommandKind) -> bool {
    match kind {
        CommandKind::SteerCancel | CommandKind::TrustGrant | CommandKind::TrustRevoke => true,
        CommandKind::TaskAdd
        | CommandKind::TaskEdit
        | CommandKind::TaskNote
        | CommandKind::TaskDefer
        | CommandKind::TaskWake
        | CommandKind::TaskDepends
        | CommandKind::TaskPhase
        | CommandKind::TaskDone
        | CommandKind::TaskReview
        | CommandKind::TaskAssign
        | CommandKind::ClaimWin
        | CommandKind::ClaimCarry
        | CommandKind::ClaimRenew
        | CommandKind::ClaimFree
        | CommandKind::ClaimLapse
        | CommandKind::DocApply
        | CommandKind::SteerSend
        | CommandKind::SteerResend
        | CommandKind::SteerDispose
        | CommandKind::ConsentDecide
        | CommandKind::TrustPend
        | CommandKind::LaneSeat
        | CommandKind::LaneState
        | CommandKind::TasteRecord
        | CommandKind::CanaryRecognize
        | CommandKind::VendorRecognize
        | CommandKind::CommitRecognize
        | CommandKind::ProofRecognize
        | CommandKind::ReleasePublish
        | CommandKind::UpChange => false,
    }
}

/// Why a pane did not build the command it was asked for.
///
/// This is not a [`wire::Refusal`]. A refusal answers a command the resident
/// evaluated; nothing named here ever crossed the socket, so reporting it as
/// a refusal would put words in the resident's mouth.
#[derive(Debug, PartialEq, Eq)]
pub enum Declined {
    /// The command requires the operator capability and this principal is
    /// lane-scoped.
    Authority {
        /// Command that was asked for.
        kind: CommandKind,
        /// Principal that asked for it.
        actor: Actor,
    },
    /// The witnesses do not satisfy the command's law.
    Law(LawError),
}

impl fmt::Display for Declined {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authority { kind, actor } => write!(
                formatter,
                "{kind} requires the operator capability; this session acts as {actor}"
            ),
            Self::Law(error) => error.fmt(formatter),
        }
    }
}

impl Error for Declined {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Authority { .. } => None,
            Self::Law(error) => Some(error),
        }
    }
}

/// A pane that shows one packed answer and can append nothing.
///
/// The state is held by shared reference. [`BrowserState::apply`] and
/// [`BrowserState::replace`] both take `&mut self`, so a read pane does not
/// merely decline to append: no append path exists to call through it, and the
/// borrow checker rather than a convention is what says so.
#[derive(Debug)]
pub struct ReadPane<'a, T> {
    principal: &'a Principal,
    state: &'a BrowserState<T>,
}

impl<'a, T> ReadPane<'a, T> {
    /// Open a read pane over state from this principal's own generation.
    ///
    /// State from another generation is refused rather than rendered. Cursors
    /// and horizons mean something only inside the generation that assigned
    /// them, so a pane that mixed two would present one authority's answer
    /// under another authority's name.
    #[must_use]
    pub fn over(principal: &'a Principal, state: &'a BrowserState<T>) -> Option<Self> {
        (principal.epoch() == state.epoch()).then_some(Self { principal, state })
    }

    /// The principal this pane renders for.
    #[must_use]
    pub const fn principal(&self) -> &'a Principal {
        self.principal
    }

    /// Ranked rows of the current answer, empty before one arrives.
    #[must_use]
    pub fn rows(&self) -> &'a [T] {
        self.state.rows()
    }

    /// What the current answer left out, when it left anything out.
    #[must_use]
    pub fn overflow(&self) -> Option<Overflow> {
        self.state.overflow()
    }

    /// The completeness boundary the current answer declared.
    #[must_use]
    pub fn horizon(&self) -> Option<Seq> {
        self.state.horizon()
    }

    /// True when the current answer omitted ranked rows.
    #[must_use]
    pub fn is_overflowing(&self) -> bool {
        self.state.is_overflowing()
    }

    /// The highest position applied, or `None` before the first frame.
    #[must_use]
    pub const fn cursor(&self) -> Option<Seq> {
        self.state.cursor()
    }
}

/// A pane that builds ordinary commands for one principal.
#[derive(Debug)]
pub struct SteerPane<'a> {
    principal: &'a Principal,
}

impl<'a> SteerPane<'a> {
    /// Open a steer pane for this principal.
    #[must_use]
    pub const fn over(principal: &'a Principal) -> Self {
        Self { principal }
    }

    /// The principal this pane submits as.
    #[must_use]
    pub const fn principal(&self) -> &'a Principal {
        self.principal
    }

    /// True when this pane offers a control for `kind`.
    ///
    /// Hiding a control this principal cannot exercise removes a misleading
    /// affordance. It does not remove the command: only the resident decides
    /// that, and it decides the same way whether or not a control was drawn.
    #[must_use]
    pub const fn offers(&self, kind: CommandKind) -> bool {
        !requires_operator(kind) || self.principal.is_operator()
    }

    /// Build one command at the version this build speaks.
    ///
    /// The command is witnessed before it is returned, so a caller cannot hold
    /// one this pane built that would lose a race it does not depend on.
    ///
    /// # Errors
    ///
    /// Returns [`Declined::Authority`] when this pane offers no control for
    /// the command, and [`Declined::Law`] when the witnesses do not satisfy
    /// the command's registry row.
    pub fn command<B>(
        &self,
        id: CanonicalUuid,
        kind: CommandKind,
        expected: Vec<Witness>,
        body: B,
    ) -> Result<Command<B>, Declined> {
        if !self.offers(kind) {
            return Err(Declined::Authority {
                kind,
                actor: self.principal.actor().clone(),
            });
        }
        let command = Command {
            v: WireVersion::CURRENT,
            id,
            kind,
            expected,
            body,
        };
        command.witnessed().map_err(Declined::Law)?;
        Ok(command)
    }
}

/// A pane that presents the decisions the operator capability carries.
///
/// It opens for the operator and for nobody else. A lane-scoped principal has
/// no path to one: [`GrantPane::over`] is the only constructor, the field is
/// private, and a [`Principal`] can only come from an admitted acceptance, so
/// neither caller text nor anything the app rendered can produce one.
///
/// The pane presents; the [`SteerPane`] it wraps submits. Keeping those
/// separate means there is exactly one way to build a command, and reading a
/// grant control does not quietly become a second one.
#[derive(Debug)]
pub struct GrantPane<'a> {
    steer: SteerPane<'a>,
}

impl<'a> GrantPane<'a> {
    /// Open a grant pane, or `None` for a principal without the capability.
    #[must_use]
    pub const fn over(principal: &'a Principal) -> Option<Self> {
        if principal.is_operator() {
            Some(Self {
                steer: SteerPane::over(principal),
            })
        } else {
            None
        }
    }

    /// The principal this pane decides as.
    #[must_use]
    pub const fn principal(&self) -> &'a Principal {
        self.steer.principal()
    }

    /// The pane that submits what this one presents.
    #[must_use]
    pub const fn steer(&self) -> &SteerPane<'a> {
        &self.steer
    }

    /// The decisions this pane presents, in registry order.
    pub fn decisions() -> impl Iterator<Item = CommandKind> {
        CommandKind::ALL
            .iter()
            .copied()
            .filter(|kind| requires_operator(*kind))
    }
}

/// Pair one admitted answer with the command it answers.
///
/// The outcome is returned whole. [`Outcome`] has three variants and this
/// narrows none of them: a refusal is not an error and a fault is not an empty
/// reply, and collapsing either into the other loses exactly the distinction
/// the operator reads outcomes for.
///
/// # Errors
///
/// Returns a protocol fault when the answer names a different command. A
/// browser holding two submissions would otherwise be free to show one
/// command's outcome under the other one's control.
pub fn settled<'a, B, R>(
    command: &Command<B>,
    admitted: &'a Admitted<Response<R>>,
) -> Result<&'a Outcome<R>, Fault> {
    let response = admitted.get();
    if response.id == command.id {
        return Ok(&response.outcome);
    }
    Err(Fault {
        class: FaultClass::Protocol,
        reason: ANSWER_MISMATCH.to_owned(),
        detail: format!(
            "answer names command {}; this control submitted {}",
            response.id, command.id
        ),
    })
}
