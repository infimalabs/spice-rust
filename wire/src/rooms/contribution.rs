//! Typed problem-space contributions to an open Design room.
//!
//! A contribution is one immutable, lane-authored record: its kind, its
//! author, its text, and the exact sources it cites. It is evidence for the
//! operator's Design judgment, never authority: recording or reading one moves
//! no task, claim, working group, candidate, decision, or authorization.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use super::{RoomAnchor, RoomAuthorityStamp, RoomSource};
use crate::CanonicalUuid;

/// Maximum bytes of one contribution's text.
pub const ROOM_CONTRIBUTION_TEXT_LIMIT: usize = 4_096;
/// Maximum sources one contribution may cite.
pub const ROOM_CONTRIBUTION_SOURCE_LIMIT: usize = 8;

/// What a contribution offers the Design.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoomContributionKind {
    /// A reason to take the direction under consideration.
    ReasonFor,
    /// A reason against the direction under consideration.
    ReasonAgainst,
    /// An observation that bears on the problem.
    Evidence,
    /// An open question the Design has not answered.
    Question,
}

impl RoomContributionKind {
    /// Every kind, in the order reads group them.
    pub const ALL: [Self; 4] = [
        Self::ReasonFor,
        Self::ReasonAgainst,
        Self::Evidence,
        Self::Question,
    ];

    /// The kind as it is spelled on the wire and the command line.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReasonFor => "reason-for",
            Self::ReasonAgainst => "reason-against",
            Self::Evidence => "evidence",
            Self::Question => "question",
        }
    }
}

impl fmt::Display for RoomContributionKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// A spelling outside the closed contribution vocabulary.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid contribution kind {0:?}; expected reason-for, reason-against, evidence, or question"
)]
pub struct RoomContributionKindError(String);

impl FromStr for RoomContributionKind {
    type Err = RoomContributionKindError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or_else(|| RoomContributionKindError(value.to_owned()))
    }
}

/// A lane contributed one typed record to an open Design room.
///
/// The fact owns the text; each source stays a link to the authority that
/// owns its bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomContributed {
    /// The Design anchor; this is the room identity.
    pub room: RoomAnchor,
    /// Native moment identity of this publication.
    pub publication: RoomAuthorityStamp,
    /// The logical lane that authored the contribution.
    pub lane: CanonicalUuid,
    /// What the contribution offers.
    pub contribution: RoomContributionKind,
    /// The lane's complete contributed text.
    pub text: String,
    /// Exact sources cited, in the author's order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<RoomSource>,
}

/// Why a contribution's own shape is inadmissible.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum RoomContributionError {
    /// The text is empty or only whitespace.
    #[error("contribution text is empty")]
    Empty,
    /// The text exceeds [`ROOM_CONTRIBUTION_TEXT_LIMIT`] bytes.
    #[error("contribution text exceeds {ROOM_CONTRIBUTION_TEXT_LIMIT} bytes")]
    TextTooLong,
    /// More than [`ROOM_CONTRIBUTION_SOURCE_LIMIT`] sources are cited.
    #[error("contribution cites more than {ROOM_CONTRIBUTION_SOURCE_LIMIT} sources")]
    TooManySources,
    /// One source is cited twice.
    #[error("contribution cites one source twice")]
    DuplicateSource,
}

impl RoomContributed {
    /// Refuse a contribution whose text or source list is out of bounds.
    ///
    /// # Errors
    ///
    /// Returns the first bound the contribution violates.
    pub fn admit(text: &str, sources: &[RoomSource]) -> Result<(), RoomContributionError> {
        if text.trim().is_empty() {
            return Err(RoomContributionError::Empty);
        }
        if text.len() > ROOM_CONTRIBUTION_TEXT_LIMIT {
            return Err(RoomContributionError::TextTooLong);
        }
        if sources.len() > ROOM_CONTRIBUTION_SOURCE_LIMIT {
            return Err(RoomContributionError::TooManySources);
        }
        if sources
            .iter()
            .enumerate()
            .any(|(index, source)| sources[..index].contains(source))
        {
            return Err(RoomContributionError::DuplicateSource);
        }
        Ok(())
    }
}
