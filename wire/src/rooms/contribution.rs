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
use crate::{CanonicalUuid, Digest, Domain};

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

/// Canonical immutable authority behind one room-source link. A transcript's
/// moving accounted boundary is deliberately absent.
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum RoomSourceAuthority<'a> {
    Transcript {
        lane: &'a CanonicalUuid,
        vendor: &'a crate::Name,
        stream: &'a str,
        source: &'a CanonicalUuid,
        locus: &'a crate::MessageLocus,
        text: &'a super::TextRange,
    },
    Journal {
        fact: &'a crate::SequenceCode,
    },
    Artifact {
        digest: &'a Digest,
    },
    Contribution {
        room: &'a RoomAnchor,
        publication: &'a RoomAuthorityStamp,
    },
}

impl FromStr for RoomContributionKind {
    type Err = RoomContributionKindError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or_else(|| RoomContributionKindError(value.to_owned()))
    }
}
impl RoomSource {
    /// Whether two links name the same immutable source authority.
    ///
    /// A transcript's accounted boundary may advance without changing the
    /// event bytes its locus and text range identify.
    #[must_use]
    pub fn same_authority(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Transcript(left), Self::Transcript(right)) => left.same_bytes(right),
            (Self::Journal { fact: left }, Self::Journal { fact: right }) => left == right,
            (Self::Artifact { digest: left }, Self::Artifact { digest: right }) => left == right,
            (Self::Contribution { .. }, Self::Contribution { .. }) => self == other,
            _ => false,
        }
    }

    /// Digest of the exact immutable authority this link names.
    #[must_use]
    pub fn authority_digest(&self) -> Digest {
        let authority = match self {
            Self::Transcript(hunk) => RoomSourceAuthority::Transcript {
                lane: &hunk.head.lane,
                vendor: &hunk.head.vendor,
                stream: &hunk.head.stream,
                source: &hunk.head.source,
                locus: &hunk.locus,
                text: &hunk.text,
            },
            Self::Journal { fact } => RoomSourceAuthority::Journal { fact },
            Self::Artifact { digest } => RoomSourceAuthority::Artifact { digest },
            Self::Contribution { room, publication } => {
                RoomSourceAuthority::Contribution { room, publication }
            }
        };
        let canonical = crate::encode(&authority).expect("a typed room source authority encodes");
        Digest::compute(Domain::RoomSource, &canonical)
    }
}

/// Whether source witnesses name each cited immutable authority exactly once.
///
/// Non-source witnesses are ignored. The bounded comparison stays on the
/// stack because every admitted contribution has at most eight citations.
#[must_use]
pub fn source_witnesses_match(sources: &[RoomSource], expected: &[crate::Witness]) -> bool {
    if sources.len() > ROOM_CONTRIBUTION_SOURCE_LIMIT {
        return false;
    }
    let mut cited: [Option<Digest>; ROOM_CONTRIBUTION_SOURCE_LIMIT] = std::array::from_fn(|_| None);
    for (slot, source) in cited.iter_mut().zip(sources) {
        *slot = Some(source.authority_digest());
    }
    let mut matched = [false; ROOM_CONTRIBUTION_SOURCE_LIMIT];
    let mut source_count = 0_usize;
    for witness in expected {
        let crate::Witness::Source { source } = witness else {
            continue;
        };
        source_count += 1;
        let Some(index) = cited[..sources.len()]
            .iter()
            .position(|cited| cited.as_ref() == Some(source))
        else {
            return false;
        };
        if std::mem::replace(&mut matched[index], true) {
            return false;
        }
    }
    source_count == sources.len() && matched[..sources.len()].iter().all(|matched| *matched)
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
    /// The stored text retains leading or trailing whitespace.
    #[error("contribution text is not trimmed")]
    Untrimmed,
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
        if text != text.trim() {
            return Err(RoomContributionError::Untrimmed);
        }
        if text.len() > ROOM_CONTRIBUTION_TEXT_LIMIT {
            return Err(RoomContributionError::TextTooLong);
        }
        if sources.len() > ROOM_CONTRIBUTION_SOURCE_LIMIT {
            return Err(RoomContributionError::TooManySources);
        }
        if sources.iter().enumerate().any(|(index, source)| {
            sources[..index]
                .iter()
                .any(|earlier| source.same_authority(earlier))
        }) {
            return Err(RoomContributionError::DuplicateSource);
        }
        Ok(())
    }

    /// Whether one successful command receipt proves this exact record.
    ///
    /// The receipt binds the anchor heads, every cited source authority, the
    /// submitted body, and the returned publication. A contribution fact
    /// without this correspondence did not cross resident admission.
    #[must_use]
    pub fn matches_command(
        &self,
        actor: &crate::Actor,
        request: &crate::intent::RoomContribute,
        expected: &[crate::Witness],
        answer: &crate::intent::RoomContributeAnswer,
    ) -> bool {
        let mut tasks = expected.iter().filter_map(|witness| match witness {
            crate::Witness::Task { task, .. } => Some(task),
            _ => None,
        });
        let task_matches =
            tasks.next().is_some_and(|task| task == &self.room) && tasks.next().is_none();
        let mut workflows = expected.iter().filter_map(|witness| match witness {
            crate::Witness::Workflow { task, .. } => Some(task),
            _ => None,
        });
        let workflow_matches =
            workflows.next().is_some_and(|task| task == &self.room) && workflows.next().is_none();
        request.room.born().is_ok_and(|room| room == self.room)
            && *actor == crate::Actor::Lane(self.lane)
            && task_matches
            && workflow_matches
            && source_witnesses_match(&self.sources, expected)
            && request.kind == self.contribution
            && request.text.trim() == self.text
            && request.sources == self.sources
            && answer.room == request.room
            && answer.publication == self.publication
            && answer.kind == self.contribution
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::TranscriptPosition;
    use crate::rooms::{TextRange, TranscriptHunk};
    use crate::{MessageLocus, TranscriptSourceHead};

    fn transcript(accounted_offset: u64) -> RoomSource {
        RoomSource::Transcript(TranscriptHunk {
            head: TranscriptSourceHead {
                lane: "018f82d4-6f59-7d06-92e3-bce913116302"
                    .parse()
                    .expect("fixture lane"),
                vendor: "omp".parse().expect("fixture vendor"),
                stream: "session".to_owned(),
                source: "018f82d4-6f59-7d06-92e3-bce913116303"
                    .parse()
                    .expect("fixture source"),
                next: TranscriptPosition {
                    member: 1,
                    offset: accounted_offset,
                },
            },
            locus: MessageLocus {
                member: 1,
                offset: 20,
                length: 30,
                event: 2,
            },
            text: TextRange { start: 4, end: 12 },
        })
    }

    #[test]
    fn canonical_contributions_use_exact_source_authority() {
        assert_eq!(
            RoomContributed::admit(" evidence ", &[]),
            Err(RoomContributionError::Untrimmed)
        );
        let first = transcript(50);
        let advanced = transcript(90);
        assert!(first.same_authority(&advanced));
        assert_eq!(first.authority_digest(), advanced.authority_digest());
        assert_eq!(
            RoomContributed::admit("Evidence.", &[first, advanced]),
            Err(RoomContributionError::DuplicateSource)
        );
    }

    #[test]
    fn a_contribution_answer_carries_the_source_that_cites_it() {
        let stamp = "1kKRRsDR0".parse().expect("room stamp");
        let room = crate::TaskHandle::for_project("rooms.test", &stamp);
        let publication: RoomAuthorityStamp = "1kKRRsDR2".parse().expect("publication");
        let answer = crate::intent::RoomContributeAnswer::new(
            room,
            publication.clone(),
            RoomContributionKind::Evidence,
        )
        .expect("answer");
        let own = RoomSource::Contribution {
            room: stamp,
            publication,
        };
        assert_eq!(answer.source, own);
        let encoded = serde_json::to_value(&answer).expect("answer encodes");
        assert_eq!(
            serde_json::from_value::<RoomSource>(encoded["source"].clone()).expect("source"),
            own
        );
        let mut stored = encoded.clone();
        stored
            .as_object_mut()
            .expect("answer object")
            .remove("source");
        assert_eq!(
            serde_json::from_value::<crate::intent::RoomContributeAnswer>(stored)
                .expect("a receipt written before sources decodes"),
            answer
        );
        let mut forged = encoded;
        forged["source"]["publication"] = "1kKRRsDR3".into();
        assert!(serde_json::from_value::<crate::intent::RoomContributeAnswer>(forged).is_err());
    }
}
