//! Resident records of session-targeted requests for open Designs.
//!
//! Each record durably chooses one request. It is not a room publication; it
//! keeps the asked lane listening to the Design's room until that lane leaves,
//! and adds no participation, steering, claim, assignment, or wake.

use std::fmt;

use serde::{Deserialize, Serialize};

use super::{DecisionRef, DesignItemState, RoomQuestion, RoomQuestionItem};
use crate::{CanonicalUuid, Digest, Name, Seq, SequenceCode, TaskStamp};

/// Most bytes of completion validation retained for Design request matching.
pub const COMPLETION_VALIDATION_BYTES: usize = 4 * 1024;

/// Most matched terms one Design awareness record carries.
pub const DESIGN_NOTICE_TERMS: usize = 12;

/// Most matched terms any surface shows for one notice, however many its
/// record keeps.
pub const DESIGN_NOTICE_SHOWN_TERMS: usize = 6;

/// How widely one Design request was cast, judged from the Design's maturity
/// when it was selected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesignReach {
    /// An unsteeped Design invites at the lower relevance floor.
    Wide,
    /// A steeped or OOPS Design invites only at the higher floor.
    Narrow,
}

impl fmt::Display for DesignReach {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Wide => "wide",
            Self::Narrow => "narrow",
        })
    }
}

/// Most characters of a Design request's supporting-work excerpt.
pub const DESIGN_EXCERPT_CHARS: usize = 240;

/// Supporting-work `text` as one line of at most [`DESIGN_EXCERPT_CHARS`]
/// characters. An admitted excerpt is non-empty and its own excerpt.
#[must_use]
pub fn design_excerpt(text: &str) -> String {
    one_line(text, DESIGN_EXCERPT_CHARS)
}

/// `text` with whitespace runs collapsed, cut to `chars` characters with an
/// ellipsis when longer.
#[must_use]
pub fn one_line(text: &str, chars: usize) -> String {
    let mut line = String::with_capacity(text.len().min(chars));
    for word in text.split_whitespace() {
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if line.chars().count() <= chars {
        return line;
    }
    let mut cut = line
        .chars()
        .take(chars.saturating_sub(1))
        .collect::<String>();
    cut.push('…');
    cut
}

/// What the task work supporting a Design request did, named from the phase
/// its claim held so the request does not overstate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DesignSupport {
    /// An implementing phase changed the product.
    Implemented,
    /// A plan or Design phase diagnosed or planned the work.
    Planned,
    /// A review phase inspected another lane's work.
    Reviewed,
}

impl DesignSupport {
    /// The kind of the supporting work, from the phase its claim held.
    #[must_use]
    pub fn for_phase(phase: &crate::Phase) -> Self {
        match phase.as_str() {
            "review" => Self::Reviewed,
            "plan" | "design" => Self::Planned,
            _ => Self::Implemented,
        }
    }

    /// The past-tense clause a request uses for this work.
    #[must_use]
    pub const fn verb(self) -> &'static str {
        match self {
            Self::Implemented => "implemented",
            Self::Planned => "diagnosed or planned",
            Self::Reviewed => "reviewed",
        }
    }
}

/// The matched terms a surface shows: each distinct term once, in record
/// order, at most [`DESIGN_NOTICE_SHOWN_TERMS`].
#[must_use]
pub fn shown_notice_terms(terms: &[String]) -> Vec<&str> {
    let mut shown: Vec<&str> = Vec::with_capacity(DESIGN_NOTICE_SHOWN_TERMS);
    for term in terms {
        if shown.len() == DESIGN_NOTICE_SHOWN_TERMS {
            break;
        }
        if !shown.contains(&term.as_str()) {
            shown.push(term);
        }
    }
    shown
}

/// One exact continuing vendor session invited to an open Design.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignSession {
    /// Vendor whose declaration admitted the identity.
    pub vendor: Name,
    /// Exact resumable vendor session identity.
    pub identity: String,
}

/// Exact settled conversation source used by an opportunity or as support.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignConversationEvidence {
    /// Durable transcript record containing the assistant-authored prose.
    pub locus: crate::body::TranscriptLocus,
    /// Identity of the exact cleaned prose retained by the session profile.
    pub digest: Digest,
}

/// Event at which one session profile was evaluated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DesignNoticeOpportunity {
    /// A successful task completion settled.
    TaskCompletion { task: TaskStamp, completed: Seq },
    /// A retained task selection under this exact claim. `terminal` names the
    /// terminal task operation when one preceded the selection record; its
    /// absence does not pretend that the task completed.
    TaskClaim {
        task: TaskStamp,
        claim: SequenceCode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal: Option<Seq>,
    },
    /// One assistant-authored conversation message settled.
    Conversation { source: DesignConversationEvidence },
    /// The selected Design was filed or revised.
    DesignStatement,
    /// A bounded profile warm finished, optionally through its newest journal
    /// fact when the profile has task history.
    ProfileReady { horizon: Option<Seq> },
    /// The same continuing session resumed at this declaration.
    SessionResumed { declaration: Seq },
}

/// Exact retained card that supported one Design match.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum DesignNoticeEvidence {
    /// Successful task evidence at the statement observed with its completion,
    /// classified by the phase the completed claim held.
    Task {
        task: TaskStamp,
        statement: Seq,
        completed: Seq,
        support: DesignSupport,
    },
    /// Task evidence retained under an exact claim, classified by the phase
    /// that claim held. `terminal` is present only when a terminal task
    /// operation preceded the selection record; the claim remains the witness
    /// when no completion did.
    TaskClaim {
        task: TaskStamp,
        statement: Seq,
        claim: SequenceCode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        terminal: Option<Seq>,
        support: DesignSupport,
    },
    /// Assistant-authored conversation evidence.
    Conversation { source: DesignConversationEvidence },
}

/// One question item a Design request asks a lane about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignAsk {
    /// The item's 1-based position in the noticed statement's acceptance.
    pub question: RoomQuestion,
    /// The item's text identity, which keys its charge series.
    pub item: RoomQuestionItem,
    /// Where the item stood when asked. A contested item is asked only of a
    /// lane already in its series, for a strength-2 re-charge.
    pub state: DesignItemState,
}

/// One continuing session was invited to contribute to an open Design.
///
/// The opportunity and supporting evidence are independent: a new completion
/// can select a Design because of an older task or conversation card. The
/// record is request state only. It grants no participation, steering, claim,
/// assignment, permission to work, or wake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignNoticed {
    /// Open Design whose room accepts the answer.
    pub design: TaskStamp,
    /// Exact Design statement head evaluated for this request.
    pub statement: Seq,
    /// Logical lane carrying the continuing session when selected.
    pub lane: CanonicalUuid,
    /// Occupant carrying the continuing session when selected.
    pub occupant: CanonicalUuid,
    /// Continuing vendor session that owns the invitation.
    pub session: DesignSession,
    /// Newest compaction boundary of this session before selection. Admission
    /// refuses if it moves before recording; it does not reset request history.
    pub compaction: Option<Seq>,
    /// Event that gave the resident an opportunity to evaluate the profile.
    pub opportunity: DesignNoticeOpportunity,
    /// Retained card whose view selected the Design.
    pub evidence: DesignNoticeEvidence,
    /// The card's authored text as a [`design_excerpt`]: the completion
    /// validation, the cleaned assistant prose, or the task title when the
    /// supporting claim reached no terminal operation.
    pub excerpt: String,
    /// Matched terms in ascending order.
    pub terms: Vec<String>,
    /// Reach whose relevance floor admitted the match. Absent on records older
    /// than the field and on those a retired Passive group's floor admitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<DesignReach>,
    /// The question items asked, the best-scoring first and the rest in
    /// statement order. Empty asks about the whole Design: no current item
    /// was askable of the lane, or the record predates items.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub asks: Vec<DesignAsk>,
}

/// Scale of [`MatchScore`]: one unit is a ten-thousandth of cosine.
pub const MATCH_SCORE_UNIT: u16 = 10_000;

/// One cosine relevance score or floor in ten-thousandths, truncated, so
/// records compare exactly and a score below its floor stays below it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MatchScore(u16);

impl MatchScore {
    /// `cosine` truncated to ten-thousandths; `None` outside `[0, 1]` or when
    /// not finite. Truncation is monotonic, so two cosines keep their order.
    #[must_use]
    pub fn of_cosine(cosine: f64) -> Option<Self> {
        if !cosine.is_finite() || !(0.0..=1.0).contains(&cosine) {
            return None;
        }
        // Bounded to [0, 10000] by the range check, so the cast is exact.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the product lies in [0, 10000]"
        )]
        Some(Self((cosine * f64::from(MATCH_SCORE_UNIT)).floor() as u16))
    }

    /// The score as a cosine in `[0, 1]`.
    #[must_use]
    pub fn cosine(self) -> f64 {
        f64::from(self.0) / f64::from(MATCH_SCORE_UNIT)
    }
}

impl fmt::Display for MatchScore {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "0.{:04}", self.0)
    }
}

/// Identity of the frequency fit a score was weighted under: the lowercase
/// hex of the fit's generation hash.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FitGeneration(String);

impl FitGeneration {
    /// The generation whose hash is `bytes`.
    #[must_use]
    pub fn of_hash(bytes: &[u8; 32]) -> Self {
        Self(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    /// The generation's hex spelling.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FitGeneration {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// The best open Design one continuing session's cards selected at an
/// opportunity fell short of its reach floor, so no request was recorded.
///
/// The record is positional: it names the best candidate that did not clear,
/// under whatever floor its reach carried at the time, so a later floor change
/// leaves every earlier shadow self-describing. It delivers nothing, wakes
/// nothing, grants nothing, and opens no listening; it exists so the invites a
/// floor excluded can be compared with what the lane later did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignShadowed {
    /// Open Design the session's cards scored best against.
    pub design: TaskStamp,
    /// Exact Design statement head evaluated.
    pub statement: Seq,
    /// Logical lane carrying the continuing session when evaluated.
    pub lane: CanonicalUuid,
    /// Occupant carrying the continuing session when evaluated.
    pub occupant: CanonicalUuid,
    /// Continuing vendor session whose cards were scored.
    pub session: DesignSession,
    /// Event that gave the resident an opportunity to evaluate the profile.
    pub opportunity: DesignNoticeOpportunity,
    /// Retained card whose view scored best.
    pub evidence: DesignNoticeEvidence,
    /// Strongest shared terms in ascending order.
    pub terms: Vec<String>,
    /// Reach whose floor the match fell short of.
    pub reach: DesignReach,
    /// That reach's floor when evaluated.
    pub floor: MatchScore,
    /// The best cosine any card view scored, below `floor`.
    pub score: MatchScore,
    /// Frequency fit the score was weighted under.
    pub generation: FitGeneration,
}

/// The resident handed an active Design's holder the operator's newest return
/// decision as its next input.
///
/// One record consumes that decision: no later claimant is told it again, and
/// only a newer return becomes input. It authorizes and mutates nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DesignReturnDelivered {
    /// The return decision delivered; its controller is the Design.
    pub decision: DecisionRef,
    /// Logical lane holding the Design.
    pub lane: CanonicalUuid,
    /// Occupant told.
    pub occupant: CanonicalUuid,
    /// Fact that won the claim the delivery served.
    pub claim: SequenceCode,
}

#[cfg(test)]
mod tests {
    use super::{DESIGN_NOTICE_SHOWN_TERMS, shown_notice_terms};

    #[test]
    fn shown_terms_are_distinct_and_capped_in_record_order() {
        let terms = [
            "beta", "alpha", "beta", "gamma", "alpha", "delta", "eps", "zeta", "eta",
        ]
        .map(str::to_owned);
        assert_eq!(
            shown_notice_terms(&terms),
            ["beta", "alpha", "gamma", "delta", "eps", "zeta"]
        );
        let many = (0..20).map(|n| format!("term{n:02}")).collect::<Vec<_>>();
        assert_eq!(shown_notice_terms(&many).len(), DESIGN_NOTICE_SHOWN_TERMS);
        assert!(shown_notice_terms(&[]).is_empty());
    }
}
