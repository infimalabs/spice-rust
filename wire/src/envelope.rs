//! The fact envelope: version, position, instant, attribution, and cause.

use std::cmp::Ordering;
use std::fmt;
use std::num::NonZeroU64;
use std::str::FromStr;

use serde::de::{Deserializer, Error as DeError};
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::ident::{CanonicalUuid, Instant};

/// The wire schema version writers emit and readers admit.
///
/// This is a transparent number rather than a type-level constant because a
/// mismatch must be able to name the version it observed. A version that
/// refused to decode could not be reported back to the peer that sent it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireVersion(pub u32);

impl WireVersion {
    /// The one version this build speaks.
    pub const CURRENT: Self = Self(1);

    /// True when this is the version this build speaks.
    #[must_use]
    pub const fn is_current(self) -> bool {
        self.0 == Self::CURRENT.0
    }
}

impl fmt::Display for WireVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// A monotone journal position assigned by the committing transaction.
///
/// Positions are non-zero so that the "zero and null sentinels are invalid"
/// rule for `cause` is carried by the type rather than re-checked by callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Seq(pub NonZeroU64);

impl Seq {
    /// Build a position, refusing the zero sentinel.
    #[must_use]
    pub const fn new(value: u64) -> Option<Self> {
        match NonZeroU64::new(value) {
            Some(inner) => Some(Self(inner)),
            None => None,
        }
    }

    /// The position as a plain number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

impl fmt::Display for Seq {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// The principal that caused an append.
///
/// The resident derives this from the accepted capability; client text cannot
/// choose it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Actor {
    /// The operator acting directly.
    Operator,
    /// The resident acting on its own behalf.
    Spice,
    /// A seated lane.
    Lane(CanonicalUuid),
}

impl Actor {
    /// Prefix introducing a lane principal.
    const LANE: &'static str = "lane:";
}

impl Ord for Actor {
    /// Rank follows the crossing spelling, not the order the variants are
    /// declared in.
    ///
    /// An actor crosses as the text [`fmt::Display`] writes, and deriving this
    /// would rank by variant position instead: `Operator` is declared first
    /// but `lane:` sorts before `operator`, so a derived order disagrees with
    /// the bytes for every lane-versus-operator pair. Two orders for one value
    /// is the same defect as two spellings for one value, and a ranked list
    /// built from either one would not match the other.
    ///
    /// Comparing the rendered spelling costs an allocation. That is deliberate:
    /// the invariant is stated here rather than re-derived from the knowledge
    /// that variant order happens to match lexical order, which is exactly the
    /// assumption that was already false.
    fn cmp(&self, other: &Self) -> Ordering {
        self.to_string().cmp(&other.to_string())
    }
}

impl PartialOrd for Actor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Text that does not name a principal.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("actor must be `operator`, `spice`, or `lane:<canonical-uuid>`: {0:?}")]
pub struct ActorError(String);

impl FromStr for Actor {
    type Err = ActorError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "operator" => Ok(Self::Operator),
            "spice" => Ok(Self::Spice),
            _ => text
                .strip_prefix(Self::LANE)
                .and_then(|lane| lane.parse().ok())
                .map(Self::Lane)
                .ok_or_else(|| ActorError(text.to_owned())),
        }
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Operator => formatter.write_str("operator"),
            Self::Spice => formatter.write_str("spice"),
            Self::Lane(lane) => write!(formatter, "{}{lane}", Self::LANE),
        }
    }
}

impl Serialize for Actor {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Actor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// Root attribution carried through the work.
///
/// WIRE names ACK keys, task handles, lane ids, and repository revisions as
/// examples rather than a closed set, so the shape law is the one thing
/// enforced here: a non-empty class, a colon, and a non-empty value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Origin(String);

/// Text that does not name a root attribution.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("origin must be `<class>:<value>` with both parts non-empty: {0:?}")]
pub struct OriginError(String);

impl Origin {
    /// The attribution as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Origin {
    type Err = OriginError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.split_once(':') {
            Some((class, value)) if !class.is_empty() && !value.is_empty() => {
                Ok(Self(text.to_owned()))
            }
            _ => Err(OriginError(text.to_owned())),
        }
    }
}

impl fmt::Display for Origin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Serialize for Origin {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Origin {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(D::Error::custom)
    }
}

/// A fact as it rests in the journal and crosses the socket.
///
/// `seq`, `at`, `actor`, and any inherited `origin` are assigned inside the
/// commit door. A client may propose domain data but cannot mint journal
/// position, time, attribution, or causality.
///
/// The parameter is the fact, not the body, because the two readers of a stored
/// row want different things from it. A consumer that knows which kind it asked
/// for names [`crate::Fact`] and gets the body typed and the discriminant checked
/// against it. A consumer sweeping rows it did not write learns the kind at
/// runtime and names [`crate::AnyFact`], which keeps the header meaningful without
/// pretending to a body type it cannot know. Both spell the same header once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope<T> {
    /// Current wire version.
    pub v: WireVersion,
    /// Durable observation cursor for this fact.
    pub seq: Seq,
    /// Commit instant.
    pub at: Instant,
    /// Principal that caused the append.
    pub actor: Actor,
    /// Root attribution carried through the work.
    pub origin: Origin,
    /// Position of the immediately provoking fact; absent at a chain root.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::optional::present"
    )]
    pub cause: Option<Seq>,
    /// Discriminant and body, typed or opaque per the parameter.
    pub fact: T,
}

impl<T> Envelope<T> {
    /// True when this envelope begins a causal chain.
    #[must_use]
    pub const fn is_root(&self) -> bool {
        self.cause.is_none()
    }
}
