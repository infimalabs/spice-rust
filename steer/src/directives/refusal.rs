//! Why steering facts refuse to fold, and why a media reference refuses to
//! resolve.
//!
//! Every refusal names the directive, key, or reference it answers for and
//! spells dispositions in the wire's own words, so an operator reading one
//! sees exactly which law the stream broke.

use std::error::Error;
use std::fmt;

use wire::body::SteeringStage;
use wire::{CanonicalUuid, Disposition, Instant};

use crate::ack::keys::Key;

/// Why a steering fact could not be folded into standing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// A field that must spell a key does not.
    Malformed {
        /// Field the text arrived in.
        field: &'static str,
        /// Text exactly as it arrived.
        text: String,
    },
    /// A durable directive carried media outside the shared admission law.
    InvalidMedia {
        /// Directive whose attachment set was refused.
        directive: CanonicalUuid,
        /// Exact shared media admission error.
        error: wire::MediaError,
    },
    /// A directive already carries this key.
    Duplicate {
        /// Key that is already taken.
        key: Key,
    },
    /// A directive identity was already used by another publication.
    IdentityDuplicate {
        /// Identity that is already taken.
        directive: CanonicalUuid,
    },
    /// A fact paired a key with another durable directive identity.
    Identity {
        /// Key whose identity was contradicted.
        key: Key,
        /// Durable identity the directive carries.
        expected: CanonicalUuid,
        /// Durable identity the fact claimed.
        observed: CanonicalUuid,
    },
    /// No directive carries this key.
    Unknown {
        /// Key nothing answers to.
        key: Key,
    },
    /// The directive already retired.
    Retired {
        /// Key naming the retired directive.
        key: Key,
        /// State it retired into.
        disposition: Disposition,
    },
    /// A presentation transition skipped, repeated, or moved backward.
    Stage {
        /// Stable key whose presentation was moved.
        key: Key,
        /// Only state admissible next.
        expected: SteeringStage,
        /// State the fact claimed.
        observed: SteeringStage,
    },
    /// An overdue directive has no later pre-expiry presentation state.
    FinalStage {
        /// Stable key already at the final state.
        key: Key,
    },
    /// A presentation transition predates publication or the preceding transition.
    Chronology {
        /// Stable key whose audit would move backward in time.
        key: Key,
        /// Publication or preceding-transition instant.
        previous: Instant,
        /// Instant the new transition claimed.
        observed: Instant,
    },
    /// A steering transition addressed a different lane than its directive.
    Lane {
        /// Stable key the transition carries.
        key: Key,
        /// Lane the directive is addressed to.
        expected: CanonicalUuid,
        /// Lane the fact claimed.
        observed: CanonicalUuid,
    },
    /// A lapse named an expiry the directive does not carry.
    Expiry {
        /// Key naming the directive.
        key: Key,
        /// Expiry the directive carries.
        expected: Instant,
        /// Expiry the fact claimed.
        observed: Instant,
    },
    /// Recovery did not establish an expiry later than the lapse it reopens.
    RecoveryExpiry {
        /// Stable key naming the recovered directive.
        key: Key,
        /// Expiry at which the directive lapsed.
        lapsed_at: Instant,
        /// New expiry the recovery claimed.
        expires: Instant,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { field, text } => {
                write!(formatter, "{field} does not spell a steering key: {text:?}")
            }
            Self::InvalidMedia { directive, error } => {
                write!(
                    formatter,
                    "directive {directive} carries invalid media: {error}"
                )
            }
            Self::Duplicate { key } => write!(formatter, "a directive already carries {key}"),
            Self::IdentityDuplicate { directive } => {
                write!(
                    formatter,
                    "a directive already carries identity {directive}"
                )
            }
            Self::Identity {
                key,
                expected,
                observed,
            } => write!(
                formatter,
                "directive {key} carries identity {expected}, not {observed}"
            ),
            Self::Unknown { key } => write!(formatter, "no directive carries {key}"),
            Self::Retired { key, disposition } => {
                let word = disposition_word(*disposition);
                write!(
                    formatter,
                    "the directive at {key} already retired as {word}"
                )
            }
            Self::Stage {
                key,
                expected,
                observed,
            } => write!(
                formatter,
                "{key} moved to {observed:?} where {expected:?} was next"
            ),
            Self::FinalStage { key } => {
                write!(
                    formatter,
                    "{key} is already at the final overdue presentation"
                )
            }
            Self::Chronology {
                key,
                previous,
                observed,
            } => write!(
                formatter,
                "{key} moved at {observed} before its preceding audit instant {previous}"
            ),
            Self::Lane {
                key,
                expected,
                observed,
            } => write!(
                formatter,
                "{key} addressed lane {observed} where its directive addresses {expected}"
            ),
            Self::Expiry {
                key,
                expected,
                observed,
            } => write!(
                formatter,
                "{key} lapsed at {observed} where its directive expires at {expected}"
            ),
            Self::RecoveryExpiry {
                key,
                lapsed_at,
                expires,
            } => write!(
                formatter,
                "{key} recovered with expiry {expires}, which does not follow lapse {lapsed_at}"
            ),
        }
    }
}

impl Error for Refusal {}

/// Why a compact agent media reference could not resolve against steering authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaResolutionError {
    /// The reference itself violates its bounded crossing contract.
    Malformed,
    /// No durable directive carries the referenced identity.
    Unknown {
        /// Identity no directive answers to.
        directive: CanonicalUuid,
    },
    /// The authenticated lane does not own the referenced directive.
    WrongLane {
        /// Referenced directive.
        directive: CanonicalUuid,
        /// Lane retained by authority.
        expected: CanonicalUuid,
        /// Authenticated requesting lane.
        observed: CanonicalUuid,
    },
    /// The directive exists but is no longer live.
    Retired {
        /// Referenced directive.
        directive: CanonicalUuid,
        /// Durable retirement state.
        disposition: Disposition,
    },
    /// The live directive no longer matches the reference generation.
    Stale {
        /// Referenced directive.
        directive: CanonicalUuid,
        /// Referenced source ordinal.
        ordinal: u8,
    },
    /// The live directive has no attachment at the referenced source ordinal.
    Missing {
        /// Referenced directive.
        directive: CanonicalUuid,
        /// Missing source ordinal.
        ordinal: u8,
    },
    /// Reference metadata does not match the current authoritative attachment.
    Mismatched {
        /// Referenced directive.
        directive: CanonicalUuid,
        /// Referenced source ordinal.
        ordinal: u8,
        /// Metadata field that differed.
        field: &'static str,
    },
    /// Current durable media could not be decoded under the shared media law.
    Undecodable {
        /// Referenced directive.
        directive: CanonicalUuid,
        /// Referenced source ordinal.
        ordinal: u8,
        /// Exact media admission failure, containing no media bytes.
        error: wire::MediaError,
    },
}

impl fmt::Display for MediaResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => formatter.write_str("media reference is malformed"),
            Self::Unknown { directive } => {
                write!(formatter, "media directive {directive} is unknown")
            }
            Self::WrongLane {
                directive,
                expected,
                observed,
            } => write!(
                formatter,
                "media directive {directive} belongs to lane {expected}, not authenticated lane {observed}",
            ),
            Self::Retired {
                directive,
                disposition,
            } => write!(
                formatter,
                "media directive {directive} retired as {}",
                disposition_word(*disposition),
            ),
            Self::Stale { directive, ordinal } => write!(
                formatter,
                "media reference {directive} attachment {ordinal} is stale",
            ),
            Self::Missing { directive, ordinal } => write!(
                formatter,
                "media directive {directive} has no attachment {ordinal}",
            ),
            Self::Mismatched {
                directive,
                ordinal,
                field,
            } => write!(
                formatter,
                "media reference {directive} attachment {ordinal} mismatches {field}",
            ),
            Self::Undecodable {
                directive,
                ordinal,
                error,
            } => write!(
                formatter,
                "media authority {directive} attachment {ordinal} is undecodable: {error}",
            ),
        }
    }
}

impl Error for MediaResolutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Undecodable { error, .. } => Some(error),
            _ => None,
        }
    }
}

/// The word a disposition is reported under, spelled once for refusals.
pub(super) fn disposition_word(disposition: Disposition) -> &'static str {
    crate::ack::keys::DISPOSITIONS
        .iter()
        .find(|(_, admitted)| *admitted == disposition)
        .map_or("live", |(word, _)| word)
}

/// A durable steering stream that cannot be reduced consistently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayError {
    pub(super) seq: wire::Seq,
    pub(super) kind: wire::FactKind,
    pub(super) detail: String,
}

impl fmt::Display for ReplayError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "steering fact {} at {} is invalid: {}",
            self.kind, self.seq, self.detail
        )
    }
}

impl Error for ReplayError {}
