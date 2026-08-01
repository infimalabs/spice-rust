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
    Accepted, Actor, Authority, CanonicalUuid, Command, CommandKind, Fault, FaultClass, FoldName,
    LawError, Outcome, Overflow, Response, Seq, WireVersion, Witness,
};

use crate::socket::{Admitted, SessionScope};
use crate::state::{Answer, BrowserState};

/// Reason named when an answer does not belong to the command it was paired
/// with.
const ANSWER_MISMATCH: &str = "pane::answer-mismatch";

/// Who this session acts as, in the generation it acts in.
///
/// An actor is bound to the unrendered session scope that admitted it, so the
/// two travel together here as well. There is no constructor from text, from
/// a bare [`Actor`], or from anything the app rendered. The only door is
/// [`Principal::admitted`], which needs a frame the resident sent and
/// [`Session::admit_accepted`] let through, because the resident derives actor
/// and rights when it accepts the connection and caller fields may describe
/// provenance but cannot grant it.
///
/// [`Session::admit_accepted`]: crate::Session::admit_accepted
#[derive(Clone, PartialEq, Eq)]
pub struct Principal {
    actor: Actor,
    scope: SessionScope,
}

impl fmt::Debug for Principal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Principal")
            .field("actor", &self.actor)
            .field("epoch", &self.epoch())
            .finish()
    }
}

impl Principal {
    /// Adopt the principal the resident derived for this session.
    #[must_use]
    pub fn admitted(accepted: &Admitted<Accepted>) -> Self {
        let scope = accepted.scope();
        let accepted = accepted.get();
        Self {
            actor: accepted.actor.clone(),
            scope,
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
        self.scope.epoch()
    }

    pub(crate) const fn scope(&self) -> SessionScope {
        self.scope
    }

    /// True when this principal carries the operator capability.
    #[must_use]
    pub const fn is_operator(&self) -> bool {
        matches!(self.actor, Actor::Operator)
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
        (principal.scope() == state.scope()).then_some(Self { principal, state })
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

    /// The highest position applied from the stream, or `None` before the first
    /// frame.
    ///
    /// This is the browser's own progress, which is not the claim
    /// [`ReadPane::generation_cursor`] makes. Two subjects walking one journal
    /// keep two cursors, so they are read through two names and never merged.
    #[must_use]
    pub const fn cursor(&self) -> Option<Seq> {
        self.state.cursor()
    }

    /// The fold that produced the current answer, or `None` when a head-direct
    /// read did.
    ///
    /// A fold-backed read arrives wrapped in [`wire::Derived`] and can say
    /// which generation answered; a head-direct read is a bare
    /// [`wire::Packed`] with nowhere to hold one. The pane reports the absence
    /// rather than inventing a fold, because there is no field here to invent
    /// it from.
    #[must_use]
    pub fn fold(&self) -> Option<&'a FoldName> {
        self.state.answer().and_then(Answer::fold)
    }

    /// The generation of that fold, or `None` when a head-direct read answered.
    #[must_use]
    pub fn generation(&self) -> Option<u64> {
        self.state.answer().and_then(Answer::generation)
    }

    /// How far that generation had applied the journal, or `None` when a
    /// head-direct read answered.
    ///
    /// Distinct from [`ReadPane::horizon`], which names the source completeness
    /// boundary the answer declared rather than the fold's own progress.
    #[must_use]
    pub fn generation_cursor(&self) -> Option<Seq> {
        self.state.answer().and_then(Answer::cursor)
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
    ///
    /// The requirement is read from the command's registry row rather than
    /// restated here, so this pane cannot come to disagree with the law the
    /// resident will apply.
    #[must_use]
    pub const fn offers(&self, kind: CommandKind) -> bool {
        match kind.law().authority {
            Authority::Connected => true,
            Authority::Operator => self.principal.is_operator(),
        }
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
    ///
    /// The set is derived from the registry, so a command whose row is given
    /// operator authority appears here in the same edit rather than in a later
    /// one that remembers to add it.
    pub fn decisions() -> impl Iterator<Item = CommandKind> {
        CommandKind::ALL
            .iter()
            .copied()
            .filter(|kind| kind.law().authority == Authority::Operator)
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
