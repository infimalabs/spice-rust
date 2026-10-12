//! Shadow relevance observations of room activity and their operator
//! assessments.

use serde::{Deserialize, Serialize};

use super::{RoomAnchor, RoomAuthorityStamp, RoomSourceView};
use crate::SequenceCode;

/// Maximum source records one relevance observation may cite.
pub const ROOM_RELEVANCE_SOURCE_LIMIT: usize = 8;
/// Maximum bytes in one relevance explanation, exception note, or uncertainty note.
pub const ROOM_RELEVANCE_TEXT_LIMIT: usize = 4_096;

/// Shadow classification of one source-backed room signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoomRelevanceClass {
    /// Evidence-bearing content advanced an unresolved criterion.
    RelevantContribution,
    /// A named participant explicitly declined the requested contribution.
    Declined,
    /// The signal repeated already covered content.
    Duplicate,
    /// Activity aged out without leaving a live duty.
    StaleActivity,
    /// A bounded request or evidence opportunity ended without useful content.
    MissedOpportunity,
    /// Repeated agreement added no independent evidence.
    Agreement,
    /// Message volume was observed without evidence-bearing content.
    MessageCount,
    /// Silence was observed without an explicit duty outcome.
    Silence,
    /// Time elapsed without an evidence-bearing state transition.
    ElapsedTime,
    /// Available authority cannot support a stronger classification.
    Unknown,
}

impl RoomRelevanceClass {
    /// Stable kebab-case spelling, equal to the wire encoding.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::RelevantContribution => "relevant-contribution",
            Self::Declined => "declined",
            Self::Duplicate => "duplicate",
            Self::StaleActivity => "stale-activity",
            Self::MissedOpportunity => "missed-opportunity",
            Self::Agreement => "agreement",
            Self::MessageCount => "message-count",
            Self::Silence => "silence",
            Self::ElapsedTime => "elapsed-time",
            Self::Unknown => "unknown",
        }
    }
}

/// Whether a shadow observation says useful progress occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoomProgressStanding {
    /// Source-backed content advanced a declared unresolved criterion.
    Advanced,
    /// The observation explicitly establishes no progress.
    NoProgress,
    /// Available authority cannot decide progress.
    Unknown,
}

impl RoomProgressStanding {
    /// Stable kebab-case spelling, equal to the wire encoding.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Advanced => "advanced",
            Self::NoProgress => "no-progress",
            Self::Unknown => "unknown",
        }
    }
}

/// One bounded, source-backed shadow observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomRelevanceObservation {
    /// Stable identity of this append-only observation.
    pub observation: RoomAuthorityStamp,
    /// Anchor task whose room was observed.
    pub room: RoomAnchor,
    /// Greatest journal position the observation declares complete.
    pub through: SequenceCode,
    /// Exact available or explicitly unavailable source coverage.
    pub sources: Vec<RoomSourceView>,
    /// Shadow classification, never room or task authority.
    pub classification: RoomRelevanceClass,
    /// Whether the classified signal advanced a declared criterion.
    pub progress: RoomProgressStanding,
    /// Why the sources and horizon support this classification.
    pub explanation: String,
    /// Known exceptions to the classification.
    pub exceptions: String,
    /// Consequential uncertainty not resolved by the cited sources.
    pub uncertainty: String,
}

/// The resident recorded one shadow relevance observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomRelevanceObserved {
    /// Complete append-only observation.
    pub observation: RoomRelevanceObservation,
}

/// Operator assessment used to measure shadow-mode quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RoomRelevanceVerdict {
    /// The observation's classification is useful and supported.
    Confirmed,
    /// The observation is a false positive.
    FalsePositive,
    /// Available authority cannot adjudicate the observation.
    Unknown,
}

impl RoomRelevanceVerdict {
    /// Stable kebab-case spelling, equal to the wire encoding.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::FalsePositive => "false-positive",
            Self::Unknown => "unknown",
        }
    }
}

impl std::str::FromStr for RoomRelevanceVerdict {
    type Err = &'static str;

    fn from_str(written: &str) -> Result<Self, Self::Err> {
        [Self::Confirmed, Self::FalsePositive, Self::Unknown]
            .into_iter()
            .find(|verdict| verdict.name() == written)
            .ok_or("a verdict is confirmed, false-positive, or unknown")
    }
}

/// One attributable operator assessment of a shadow observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomRelevanceAssessment {
    /// Observation being assessed.
    pub observation: RoomAuthorityStamp,
    /// Operator's bounded shadow-quality verdict.
    pub verdict: RoomRelevanceVerdict,
    /// Why this verdict follows from current authority.
    pub explanation: String,
}

/// The operator assessed one shadow relevance observation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoomRelevanceAssessed {
    /// Complete append-only assessment.
    pub assessment: RoomRelevanceAssessment,
}
