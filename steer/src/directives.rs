//! Directives as durable state: publication, presentation, disposition, and expiry.
//!
//! Operator steering is state, not a message that scrolled past. A directive is
//! appended once and then retires only through a typed disposition, so what a
//! lane still owes is a question about facts rather than about whether a
//! terminal happened to be watched.
//!
//! Silence changes the presentation of the published directive without changing
//! its identity. Each named transition is retained in the directive's audit,
//! while acknowledgment, refusal, cancellation, lapse, and recovery address the
//! same UUID and human key throughout.
//!
//! Everything here reduces facts into standing. It appends nothing: the command
//! door mints directives, the scanner mints retirements, and this decides what
//! those facts mean together — which is why replaying the same facts always
//! produces the same pending set.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;

use wire::body::{
    ClaimLeaseWarned, DirectiveAttachment, SteerAcked, SteerCanceled, SteerDeclined,
    SteerEscalated, SteerLapsed, SteerMissed, SteerRecovered, SteerSent, SteeringStage,
    SteeringTransition, TasteDecision, TasteJudged, TasteReminded,
};
use wire::steering::SteerDeadLettered;
use wire::{AgentMediaReference, CanonicalUuid, Digest, Disposition, Instant};

use crate::ack::keys::Key;
use crate::ack::scan::Standing;

/// One stable directive, as the facts left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    directive: CanonicalUuid,
    fanout: Option<CanonicalUuid>,
    key: Key,
    lane: CanonicalUuid,
    text: String,
    control: Option<String>,
    attachments: Vec<DirectiveAttachment>,
    expires: Instant,
    published: Option<Instant>,
    stage: SteeringStage,
    escalations: Vec<SteeringTransition>,
    disposition: Disposition,
}

impl Directive {
    /// Durable identity of this directive.
    #[must_use]
    pub const fn directive(&self) -> CanonicalUuid {
        self.directive
    }

    /// Operator submit action shared by target-specific fanout directives.
    #[must_use]
    pub const fn fanout(&self) -> Option<CanonicalUuid> {
        self.fanout
    }

    /// The key this directive is quoted by.
    #[must_use]
    pub const fn key(&self) -> &Key {
        &self.key
    }

    /// The lane this directive is addressed to.
    #[must_use]
    pub const fn lane(&self) -> CanonicalUuid {
        self.lane
    }

    /// The direction itself.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The control word the operator attached, if any.
    #[must_use]
    pub fn control(&self) -> Option<&str> {
        self.control.as_deref()
    }

    /// Exact typed attachments retained with this directive.
    #[must_use]
    pub fn attachments(&self) -> &[DirectiveAttachment] {
        &self.attachments
    }

    /// Compact agent-facing references to these attachments in source order.
    ///
    /// # Errors
    ///
    /// Returns the shared reference error if retained authority is malformed.
    pub fn media_references(
        &self,
    ) -> Result<Vec<AgentMediaReference>, wire::media::AgentMediaReferenceError> {
        wire::media::agent_media_references(self.directive, &self.attachments)
    }

    /// When this directive lapses if nothing retires it first.
    #[must_use]
    pub const fn expires(&self) -> Instant {
        self.expires
    }

    /// Original publication instant, present on directives replayed from a journal.
    #[must_use]
    pub const fn published(&self) -> Option<Instant> {
        self.published
    }

    /// Current named presentation state.
    #[must_use]
    pub const fn stage(&self) -> SteeringStage {
        self.stage
    }

    /// Complete ordered presentation-transition audit.
    #[must_use]
    pub fn escalations(&self) -> &[SteeringTransition] {
        &self.escalations
    }

    /// The state this directive currently sits in.
    #[must_use]
    pub const fn disposition(&self) -> Disposition {
        self.disposition
    }

    fn from_head(head: &wire::SteeringHead) -> Result<Self, Refusal> {
        let key = read("key", &head.key)?;
        admit_attachments(head.directive, &head.attachments)?;
        Ok(Self {
            directive: head.directive,
            fanout: head.fanout,
            key,
            lane: head.lane,
            text: head.text.clone(),
            control: head.control.clone(),
            attachments: head.attachments.clone(),
            expires: head.expires,
            published: Some(head.published),
            stage: head.stage,
            escalations: head.escalations.clone(),
            disposition: head.disposition,
        })
    }

    fn to_head(&self, origin: wire::Origin, created: wire::Seq) -> wire::SteeringHead {
        wire::SteeringHead {
            directive: self.directive,
            fanout: self.fanout,
            key: self.key.to_string(),
            lane: self.lane,
            text: self.text.clone(),
            control: self.control.clone(),
            attachments: self.attachments.clone(),
            published: self
                .published
                .expect("an authoritative directive has a publication instant"),
            expires: self.expires,
            stage: self.stage,
            escalations: self.escalations.clone(),
            disposition: self.disposition,
            origin,
            created,
            minted_micros: self.key.minted_micros(),
        }
    }
}

/// One steering fact, as the reducer reads it.
#[derive(Debug, Clone, Copy)]
pub enum Steering<'a> {
    /// A directive was appended.
    Sent(&'a SteerSent),
    /// A live directive crossed one named silence horizon.
    Escalated(&'a SteerEscalated),
    /// A lapsed directive was restored to live standing.
    Recovered(&'a SteerRecovered),
    /// A directive was heard.
    Acked(&'a SteerAcked),
    /// A directive was refused with a reason.
    Declined(&'a SteerDeclined),
    /// An acknowledgment did not land.
    Missed(&'a SteerMissed),
    /// A directive reached its expiry unheard.
    Lapsed(&'a SteerLapsed),
    /// A directive was parked because its lane could not start.
    DeadLettered(&'a SteerDeadLettered),
    /// The operator withdrew a directive.
    Canceled(&'a SteerCanceled),
    /// A taste fired on authored prose and opened a directive.
    ///
    /// It reads as a steering fact because it is one. The taste evidence it
    /// also carries is read by the taste plane from the same row, and never by
    /// anything here.
    Reminded(&'a TasteReminded),
    /// A claim lease crossed its warning horizon and opened a directive.
    ClaimWarned(&'a ClaimLeaseWarned),
    /// A judge-confirmed taste candidate opened a directive.
    ///
    /// Cleared judgments are also replayed through this arm but deliberately
    /// alter no steering standing.
    Judged(&'a TasteJudged),
}

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
fn disposition_word(disposition: Disposition) -> &'static str {
    crate::ack::keys::DISPOSITIONS
        .iter()
        .find(|(_, admitted)| *admitted == disposition)
        .map_or("live", |(word, _)| word)
}

/// What every steering fact so far leaves standing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ledger {
    order: Vec<Key>,
    directives: BTreeMap<Key, Directive>,
}

/// One canonical replay projected into the bounded journal steering index.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub struct AuthorityIndex {
    heads: Vec<journal::JournalHead<wire::SteeringHead>>,
    misses: Vec<journal::JournalHead<SteerMissed>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl AuthorityIndex {
    /// Current directive heads in immutable publication order.
    #[must_use]
    pub fn heads(&self) -> &[journal::JournalHead<wire::SteeringHead>] {
        &self.heads
    }

    /// Canonical missed-recognition evidence indexed during replay.
    #[must_use]
    pub fn misses(&self) -> &[journal::JournalHead<SteerMissed>] {
        &self.misses
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct AuthorityPosition {
    origin: wire::Origin,
    created: wire::Seq,
    head: wire::Seq,
}

#[cfg(not(target_arch = "wasm32"))]
type AuthorityReplay = (
    Ledger,
    BTreeMap<CanonicalUuid, AuthorityPosition>,
    Vec<journal::JournalHead<SteerMissed>>,
);

impl Ledger {
    /// An empty ledger, before any fact.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Materialize a bounded ledger from authoritative current heads.
    ///
    /// Callers normally pass one directive or one lane's live set. This keeps
    /// existing pure rendering and recognition logic while ensuring retired
    /// history outside the requested keyspace is never loaded.
    pub fn from_authority_heads<'a>(
        heads: impl IntoIterator<Item = &'a wire::SteeringHead>,
    ) -> Result<Self, Refusal> {
        let mut ledger = Self::new();
        for head in heads {
            ledger.open(Directive::from_head(head)?)?;
        }
        Ok(ledger)
    }

    /// Rebuild standing from one canonical journal snapshot.
    ///
    /// # Errors
    ///
    /// Returns the exact fact position whose steering transition contradicts
    /// the durable stream. A body cannot be the reason: the snapshot holds only
    /// bodies its registered DTO already accepted.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_snapshot(snapshot: &journal::FactSnapshot) -> Result<Self, ReplayError> {
        Self::from_facts(snapshot.facts())
    }

    /// Rebuild standing from one canonical ordered fact iterator.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_facts<'a>(
        facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
    ) -> Result<Self, ReplayError> {
        replay(facts).map(|(ledger, _, _)| ledger)
    }

    /// Advance standing from one ordered suffix of authoritative facts.
    ///
    /// Non-steering facts are ignored. This is the incremental counterpart to
    /// [`Self::from_facts`]: a resident can retain one ledger and apply only
    /// facts newer than its admitted cursor. A refused fact rolls back every
    /// steering transition earlier in the same suffix.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn advance<'a>(
        &mut self,
        facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
    ) -> Result<(), ReplayError> {
        let order_len = self.order.len();
        let mut originals = BTreeMap::<Key, Option<Directive>>::new();
        for envelope in facts {
            if let Some(key) = transaction_key(envelope) {
                originals
                    .entry(key.clone())
                    .or_insert_with(|| self.directives.get(&key).cloned());
            }
            if let Err(error) = self.apply_envelope(envelope) {
                self.order.truncate(order_len);
                for (key, original) in originals {
                    match original {
                        Some(directive) => {
                            self.directives.insert(key, directive);
                        }
                        None => {
                            self.directives.remove(&key);
                        }
                    }
                }
                return Err(error);
            }
        }
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn apply_envelope(
        &mut self,
        envelope: &wire::Envelope<wire::AnyFact>,
    ) -> Result<Option<AppliedSteering>, ReplayError> {
        let fact = &envelope.fact;
        let (result, affected, created, missed) = if let Some(body) = fact.read::<SteerSent>() {
            (
                self.absorb_recorded(Steering::Sent(&body), Some(envelope.at)),
                Some(body.directive),
                true,
                None,
            )
        } else if let Some(body) = fact.read::<TasteReminded>() {
            (
                self.absorb_recorded(Steering::Reminded(&body), Some(envelope.at)),
                Some(body.directive),
                true,
                None,
            )
        } else if let Some(body) = fact.read::<ClaimLeaseWarned>() {
            (
                self.absorb_recorded(Steering::ClaimWarned(&body), Some(envelope.at)),
                Some(body.directive),
                true,
                None,
            )
        } else if let Some(body) = fact.read::<TasteJudged>() {
            let affected = match &body.decision {
                TasteDecision::Confirmed { directive, .. } => Some(*directive),
                TasteDecision::Cleared => None,
            };
            (
                self.absorb_recorded(Steering::Judged(&body), Some(envelope.at)),
                affected,
                affected.is_some(),
                None,
            )
        } else if let Some(body) = fact.read::<SteerEscalated>() {
            (
                self.absorb_recorded(Steering::Escalated(&body), Some(envelope.at)),
                Some(body.directive),
                false,
                None,
            )
        } else if let Some(body) = fact.read::<SteerRecovered>() {
            (
                self.absorb_recorded(Steering::Recovered(&body), Some(envelope.at)),
                Some(body.directive),
                false,
                None,
            )
        } else if let Some(body) = fact.read::<SteerAcked>() {
            (
                self.absorb_recorded(Steering::Acked(&body), Some(envelope.at)),
                Some(body.directive),
                false,
                None,
            )
        } else if let Some(body) = fact.read::<SteerDeclined>() {
            (
                self.absorb_recorded(Steering::Declined(&body), Some(envelope.at)),
                Some(body.directive),
                false,
                None,
            )
        } else if let Some(body) = fact.read::<SteerMissed>() {
            (
                self.absorb_recorded(Steering::Missed(&body), Some(envelope.at)),
                None,
                false,
                Some(body),
            )
        } else if let Some(body) = fact.read::<SteerLapsed>() {
            let affected = body
                .key
                .parse::<Key>()
                .ok()
                .and_then(|key| self.directives.get(&key))
                .map(|directive| directive.directive);
            (
                self.absorb_recorded(Steering::Lapsed(&body), Some(envelope.at)),
                affected,
                false,
                None,
            )
        } else if let Some(body) = fact.read::<SteerDeadLettered>() {
            (
                self.absorb_recorded(Steering::DeadLettered(&body), Some(envelope.at)),
                Some(body.directive),
                false,
                None,
            )
        } else if let Some(body) = fact.read::<SteerCanceled>() {
            let affected = body
                .key
                .parse::<Key>()
                .ok()
                .and_then(|key| self.directives.get(&key))
                .map(|directive| directive.directive);
            (
                self.absorb_recorded(Steering::Canceled(&body), Some(envelope.at)),
                affected,
                false,
                None,
            )
        } else {
            return Ok(None);
        };
        result.map_err(|error| ReplayError {
            seq: envelope.seq,
            kind: fact.kind(),
            detail: error.to_string(),
        })?;
        Ok(Some(AppliedSteering {
            affected,
            created,
            missed,
        }))
    }

    /// Rebuild the singular bounded steering authority from immutable facts.
    ///
    /// # Errors
    ///
    /// Returns the same exact contradictory fact refusal as [`Self::from_snapshot`].
    #[cfg(not(target_arch = "wasm32"))]
    pub fn authority_index(
        snapshot: &journal::FactSnapshot,
    ) -> Result<AuthorityIndex, ReplayError> {
        Self::authority_index_from_facts(snapshot.facts())
    }

    /// Rebuild the singular bounded steering authority from selected facts.
    ///
    /// Facts retain their global journal positions but need not include facts
    /// that cannot affect steering authority.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn authority_index_from_facts<'a>(
        facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
    ) -> Result<AuthorityIndex, ReplayError> {
        let (ledger, positions, misses) = replay(facts)?;
        let heads = ledger
            .order
            .iter()
            .filter_map(|key| ledger.directives.get(key))
            .map(|directive| {
                let position = positions
                    .get(&directive.directive)
                    .expect("every replayed directive has authority positions");
                journal::JournalHead::new(
                    position.head,
                    directive.to_head(position.origin.clone(), position.created),
                )
            })
            .collect();
        Ok(AuthorityIndex { heads, misses })
    }

    /// Build one new operator directive head without consulting other history.
    pub fn sent_head(
        body: &SteerSent,
        origin: wire::Origin,
        published: Instant,
        created: wire::Seq,
    ) -> Result<wire::SteeringHead, Refusal> {
        let mut ledger = Self::new();
        ledger.absorb_recorded(Steering::Sent(body), Some(published))?;
        Ok(ledger
            .directive(body.directive)
            .expect("sent fact created its directive")
            .to_head(origin, created))
    }

    /// Build one new taste-reminder directive head without unrelated history.
    pub fn reminded_head(
        body: &TasteReminded,
        origin: wire::Origin,
        published: Instant,
        created: wire::Seq,
    ) -> Result<wire::SteeringHead, Refusal> {
        let mut ledger = Self::new();
        ledger.absorb_recorded(Steering::Reminded(body), Some(published))?;
        Ok(ledger
            .directive(body.directive)
            .expect("reminder fact created its directive")
            .to_head(origin, created))
    }

    /// Build one new claim-warning directive head without unrelated history.
    pub fn claim_warned_head(
        body: &ClaimLeaseWarned,
        origin: wire::Origin,
        published: Instant,
        created: wire::Seq,
    ) -> Result<wire::SteeringHead, Refusal> {
        let mut ledger = Self::new();
        ledger.absorb_recorded(Steering::ClaimWarned(body), Some(published))?;
        Ok(ledger
            .directive(body.directive)
            .expect("claim warning fact created its directive")
            .to_head(origin, created))
    }

    /// Build one judge-confirmed taste directive without unrelated history.
    pub fn judged_head(
        body: &TasteJudged,
        origin: wire::Origin,
        published: Instant,
        created: wire::Seq,
    ) -> Result<wire::SteeringHead, Refusal> {
        let TasteDecision::Confirmed { directive, .. } = &body.decision else {
            panic!("a cleared taste judgment does not create a steering head");
        };
        let mut ledger = Self::new();
        ledger.absorb_recorded(Steering::Judged(body), Some(published))?;
        Ok(ledger
            .directive(*directive)
            .expect("confirmed judgment created its directive")
            .to_head(origin, created))
    }

    /// Apply one transition to one indexed head without replaying other facts.
    pub fn transition_head(
        current: &wire::SteeringHead,
        transition: Steering<'_>,
        at: Instant,
    ) -> Result<wire::SteeringHead, Refusal> {
        let mut ledger = Self::new();
        ledger.place(Directive::from_head(current)?);
        ledger.absorb_recorded(transition, Some(at))?;
        Ok(ledger
            .directive(current.directive)
            .expect("transition retains the stable directive")
            .to_head(current.origin.clone(), current.created))
    }

    /// Folds one steering fact into standing.
    ///
    /// # Errors
    ///
    /// Returns the [`Refusal`] naming what the fact contradicts. A refusal
    /// leaves standing exactly as it was, so a rejected fact cannot half-apply.
    pub fn absorb(&mut self, steering: Steering<'_>) -> Result<(), Refusal> {
        self.absorb_recorded(steering, None)
    }

    fn absorb_recorded(
        &mut self,
        steering: Steering<'_>,
        at: Option<Instant>,
    ) -> Result<(), Refusal> {
        match steering {
            Steering::Sent(body) => self.send(body, at),
            Steering::Reminded(body) => self.remind(body, at),
            Steering::ClaimWarned(body) => self.claim_warn(body, at),
            Steering::Judged(body) => self.judged(body, at),
            Steering::Escalated(body) => self.escalate(body, at),
            Steering::Recovered(body) => self.recover(body),
            Steering::Acked(body) => {
                self.dispose(&body.key, body.directive, body.lane, Disposition::Acked)
            }
            Steering::Declined(body) => {
                self.dispose(&body.key, body.directive, body.lane, Disposition::Declined)
            }
            Steering::Canceled(body) => {
                self.retire(&read("key", &body.key)?, Disposition::Canceled)
            }
            Steering::Lapsed(body) => self.lapse(body),
            Steering::DeadLettered(body) => self.dead_letter(body),
            // A missed acknowledgment alters nothing, by law. Its key is not
            // even read: the fact exists precisely because the key was unknown,
            // malformed, quoted, or historical, so refusing it here would
            // refuse the diagnostic instead of recording it.
            Steering::Missed(_) => Ok(()),
        }
    }

    /// The directives a lane still owes an answer for, in journal order.
    ///
    /// Each publication answers as exactly one directive at every urgency.
    #[must_use]
    pub fn pending(&self, lane: CanonicalUuid) -> Vec<&Directive> {
        self.order
            .iter()
            .filter_map(|key| self.directives.get(key))
            .filter(|directive| {
                directive.lane == lane && directive.disposition == Disposition::Live
            })
            .collect()
    }

    /// Every directive in journal order, whatever it retired into.
    #[must_use]
    pub fn directives(&self) -> Vec<&Directive> {
        self.order
            .iter()
            .filter_map(|key| self.directives.get(key))
            .collect()
    }

    /// One directive by durable identity, if it exists.
    #[must_use]
    pub fn directive(&self, id: CanonicalUuid) -> Option<&Directive> {
        self.directives
            .values()
            .find(|directive| directive.directive == id)
    }

    /// Resolve one bounded encoded media reference against this live lane authority.
    ///
    /// This is the untrusted crossing boundary. It bounds encoded bytes before
    /// decoding the private-field reference, then checks the authenticated lane
    /// before any attachment metadata is exposed. Resolution requires live
    /// pending standing, validates the ordinal and authority bytes, and
    /// recomputes both digest and byte count.
    ///
    /// # Errors
    ///
    /// Returns one typed refusal for malformed, unknown, wrong-lane, retired,
    /// stale, missing, mismatched, or undecodable authority.
    pub fn resolve_media(
        &self,
        lane: CanonicalUuid,
        encoded_reference: &[u8],
    ) -> Result<Vec<u8>, MediaResolutionError> {
        if encoded_reference.len() > wire::media::MAX_AGENT_MEDIA_REFERENCE_BYTES {
            return Err(MediaResolutionError::Malformed);
        }
        let reference: AgentMediaReference =
            wire::decode(encoded_reference).map_err(|_| MediaResolutionError::Malformed)?;
        self.resolve_media_reference(lane, &reference)
    }

    /// Resolve one already-admitted private-field reference.
    fn resolve_media_reference(
        &self,
        lane: CanonicalUuid,
        reference: &AgentMediaReference,
    ) -> Result<Vec<u8>, MediaResolutionError> {
        let directive =
            self.directive(reference.directive())
                .ok_or(MediaResolutionError::Unknown {
                    directive: reference.directive(),
                })?;
        if directive.lane != lane {
            return Err(MediaResolutionError::WrongLane {
                directive: reference.directive(),
                expected: directive.lane,
                observed: lane,
            });
        }
        if directive.disposition != Disposition::Live {
            return Err(MediaResolutionError::Retired {
                directive: reference.directive(),
                disposition: directive.disposition,
            });
        }
        if !self
            .pending(lane)
            .iter()
            .any(|pending| pending.directive == reference.directive())
        {
            return Err(MediaResolutionError::Stale {
                directive: reference.directive(),
                ordinal: reference.ordinal(),
            });
        }
        let attachment = directive
            .attachments
            .get(usize::from(reference.ordinal()))
            .ok_or(MediaResolutionError::Missing {
                directive: reference.directive(),
                ordinal: reference.ordinal(),
            })?;
        let bytes =
            attachment
                .decoded_bytes()
                .map_err(|error| MediaResolutionError::Undecodable {
                    directive: reference.directive(),
                    ordinal: reference.ordinal(),
                    error,
                })?;
        if attachment.name != reference.name() {
            return Err(MediaResolutionError::Mismatched {
                directive: reference.directive(),
                ordinal: reference.ordinal(),
                field: "name",
            });
        }
        if attachment.content_type != reference.content_type() {
            return Err(MediaResolutionError::Mismatched {
                directive: reference.directive(),
                ordinal: reference.ordinal(),
                field: "content_type",
            });
        }
        let length = u64::try_from(bytes.len()).expect("admitted media length fits u64");
        let digest = Digest::compute(wire::Domain::MediaContent, &bytes);
        if length != reference.bytes() || &digest != reference.digest() {
            return Err(MediaResolutionError::Stale {
                directive: reference.directive(),
                ordinal: reference.ordinal(),
            });
        }
        Ok(bytes)
    }

    /// Open standing for one newly published directive.
    ///
    /// Both publication paths land here rather than each stating the admission
    /// for itself. The two refusals are the whole publication identity law -- a
    /// key already standing, and an identity already spent -- so
    /// sharing them is what stops one origin from drifting into accepting a
    /// duplicate the other rejects.
    fn open(&mut self, opened: Directive) -> Result<(), Refusal> {
        if self.directives.contains_key(&opened.key) {
            return Err(Refusal::Duplicate { key: opened.key });
        }
        if self.directive(opened.directive).is_some() {
            return Err(Refusal::IdentityDuplicate {
                directive: opened.directive,
            });
        }
        self.place(opened);
        Ok(())
    }

    fn send(&mut self, body: &SteerSent, published: Option<Instant>) -> Result<(), Refusal> {
        let key = read("key", &body.key)?;
        admit_attachments(body.directive, &body.attachments)?;
        self.open(Directive {
            directive: body.directive,
            fanout: body.fanout,
            key,
            lane: body.lane,
            text: body.text.clone(),
            control: body.control.clone(),
            attachments: body.attachments.clone(),
            expires: body.expires,
            published,
            stage: SteeringStage::Published,
            escalations: Vec::new(),
            disposition: Disposition::Live,
        })
    }

    /// A taste reminder opens standing exactly as an operator send does.
    ///
    /// Everything a directive is asked afterwards -- whether it is live, what
    /// retires it, when it lapses, how it resends, what the inbox renders -- is
    /// answered from the standing this places, so none of those readers needs to
    /// know a taste produced it. That is the point of routing it here: a
    /// taste-only branch downstream is a second definition of "live", and two
    /// definitions are what let a reminder become unretirable.
    ///
    /// A reminder drives no control channel and carries no attachments. It is
    /// prose addressed to a lane, which is what an operator's plainest send is.
    fn remind(&mut self, body: &TasteReminded, published: Option<Instant>) -> Result<(), Refusal> {
        let key = read("key", &body.key)?;
        self.open(Directive {
            directive: body.directive,
            fanout: None,
            key,
            lane: body.lane,
            text: body.text.clone(),
            control: None,
            attachments: Vec::new(),
            expires: body.expires,
            published,
            stage: SteeringStage::Published,
            escalations: Vec::new(),
            disposition: Disposition::Live,
        })
    }

    fn claim_warn(
        &mut self,
        body: &ClaimLeaseWarned,
        published: Option<Instant>,
    ) -> Result<(), Refusal> {
        let key = read("key", &body.key)?;
        self.open(Directive {
            directive: body.directive,
            fanout: None,
            key,
            lane: body.lane,
            text: body.text.clone(),
            control: None,
            attachments: Vec::new(),
            expires: body.expiry,
            published,
            stage: SteeringStage::Overdue,
            escalations: Vec::new(),
            disposition: Disposition::Live,
        })
    }

    /// Open only the confirmed half of canonical adjudication evidence.
    fn judged(&mut self, body: &TasteJudged, published: Option<Instant>) -> Result<(), Refusal> {
        let TasteDecision::Confirmed {
            directive,
            key,
            text,
            expires,
        } = &body.decision
        else {
            return Ok(());
        };
        self.open(Directive {
            directive: *directive,
            fanout: None,
            key: read("key", key)?,
            lane: body.lane,
            text: text.clone(),
            control: None,
            attachments: Vec::new(),
            expires: *expires,
            published,
            stage: SteeringStage::Published,
            escalations: Vec::new(),
            disposition: Disposition::Live,
        })
    }

    fn escalate(&mut self, body: &SteerEscalated, at: Option<Instant>) -> Result<(), Refusal> {
        let key = read("key", &body.key)?;
        let directive = self
            .directives
            .get_mut(&key)
            .ok_or_else(|| Refusal::Unknown { key: key.clone() })?;
        if directive.directive != body.directive {
            return Err(Refusal::Identity {
                key,
                expected: directive.directive,
                observed: body.directive,
            });
        }
        if directive.disposition != Disposition::Live {
            return Err(Refusal::Retired {
                key,
                disposition: directive.disposition,
            });
        }
        if directive.lane != body.lane {
            return Err(Refusal::Lane {
                key,
                expected: directive.lane,
                observed: body.lane,
            });
        }
        let Some(expected) = directive.stage.next() else {
            return Err(Refusal::FinalStage { key });
        };
        if body.stage != expected {
            return Err(Refusal::Stage {
                key,
                expected,
                observed: body.stage,
            });
        }
        if let Some(at) = at {
            let previous = directive
                .escalations
                .last()
                .map(|transition| transition.at)
                .or(directive.published);
            if let Some(previous) = previous
                && at < previous
            {
                return Err(Refusal::Chronology {
                    key,
                    previous,
                    observed: at,
                });
            }
            directive.escalations.push(SteeringTransition {
                stage: body.stage,
                at,
            });
        }
        directive.stage = body.stage;
        Ok(())
    }

    fn recover(&mut self, body: &SteerRecovered) -> Result<(), Refusal> {
        let key = read("key", &body.key)?;
        let directive = self
            .directives
            .get(&key)
            .ok_or_else(|| Refusal::Unknown { key: key.clone() })?;
        if directive.directive != body.directive {
            return Err(Refusal::Identity {
                key,
                expected: directive.directive,
                observed: body.directive,
            });
        }
        if directive.lane != body.lane {
            return Err(Refusal::Lane {
                key,
                expected: directive.lane,
                observed: body.lane,
            });
        }
        if !matches!(
            directive.disposition,
            Disposition::Lapsed | Disposition::DeadLettered
        ) {
            return Err(Refusal::Retired {
                key,
                disposition: directive.disposition,
            });
        }
        if directive.expires != body.lapsed_at {
            return Err(Refusal::Expiry {
                key,
                expected: directive.expires,
                observed: body.lapsed_at,
            });
        }
        if body.expires <= body.lapsed_at {
            return Err(Refusal::RecoveryExpiry {
                key,
                lapsed_at: body.lapsed_at,
                expires: body.expires,
            });
        }
        let directive = self
            .directives
            .get_mut(&key)
            .expect("validated directive remains present");
        directive.disposition = Disposition::Live;
        directive.expires = body.expires;
        Ok(())
    }

    fn dispose(
        &mut self,
        text: &str,
        identity: CanonicalUuid,
        lane: CanonicalUuid,
        disposition: Disposition,
    ) -> Result<(), Refusal> {
        let key = read("key", text)?;
        let directive = self
            .directives
            .get(&key)
            .ok_or_else(|| Refusal::Unknown { key: key.clone() })?;
        if directive.directive != identity {
            return Err(Refusal::Identity {
                key,
                expected: directive.directive,
                observed: identity,
            });
        }
        if directive.lane != lane {
            return Err(Refusal::Lane {
                key,
                expected: directive.lane,
                observed: lane,
            });
        }
        self.retire(&key, disposition)
    }

    fn lapse(&mut self, body: &SteerLapsed) -> Result<(), Refusal> {
        let key = read("key", &body.key)?;
        let expected = self
            .directives
            .get(&key)
            .ok_or_else(|| Refusal::Unknown { key: key.clone() })?
            .expires;
        if expected != body.expires {
            return Err(Refusal::Expiry {
                key,
                expected,
                observed: body.expires,
            });
        }
        self.retire(&key, Disposition::Lapsed)
    }

    fn dead_letter(&mut self, body: &SteerDeadLettered) -> Result<(), Refusal> {
        self.dispose(
            &body.key,
            body.directive,
            body.lane,
            Disposition::DeadLettered,
        )
    }

    /// Retire the one stable directive addressed by this key.
    fn retire(&mut self, key: &Key, disposition: Disposition) -> Result<(), Refusal> {
        let directive = self
            .directives
            .get_mut(key)
            .ok_or_else(|| Refusal::Unknown { key: key.clone() })?;
        if directive.disposition != Disposition::Live {
            return Err(Refusal::Retired {
                key: key.clone(),
                disposition: directive.disposition,
            });
        }
        directive.disposition = disposition;
        Ok(())
    }

    fn place(&mut self, directive: Directive) {
        self.order.push(directive.key.clone());
        self.directives.insert(directive.key.clone(), directive);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn transaction_key(envelope: &wire::Envelope<wire::AnyFact>) -> Option<Key> {
    let fact = &envelope.fact;
    match fact.kind() {
        wire::FactKind::SteerSent => fact
            .read::<SteerSent>()
            .and_then(|body| body.key.parse().ok()),
        wire::FactKind::TasteReminded => fact
            .read::<TasteReminded>()
            .and_then(|body| body.key.parse().ok()),
        wire::FactKind::ClaimLeaseWarned => fact
            .read::<ClaimLeaseWarned>()
            .and_then(|body| body.key.parse().ok()),
        wire::FactKind::TasteJudged => fact.read::<TasteJudged>().and_then(|body| {
            let TasteDecision::Confirmed { key, .. } = &body.decision else {
                return None;
            };
            key.parse().ok()
        }),
        wire::FactKind::SteerEscalated => fact
            .read::<SteerEscalated>()
            .and_then(|body| body.key.parse().ok()),
        wire::FactKind::SteerRecovered => fact
            .read::<SteerRecovered>()
            .and_then(|body| body.key.parse().ok()),
        wire::FactKind::SteerAcked => fact
            .read::<SteerAcked>()
            .and_then(|body| body.key.parse().ok()),
        wire::FactKind::SteerDeclined => fact
            .read::<SteerDeclined>()
            .and_then(|body| body.key.parse().ok()),
        wire::FactKind::SteerLapsed => fact
            .read::<SteerLapsed>()
            .and_then(|body| body.key.parse().ok()),
        wire::FactKind::SteerDeadLettered => fact
            .read::<SteerDeadLettered>()
            .and_then(|body| body.key.parse().ok()),
        wire::FactKind::SteerCanceled => fact
            .read::<SteerCanceled>()
            .and_then(|body| body.key.parse().ok()),
        _ => None,
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct AppliedSteering {
    affected: Option<CanonicalUuid>,
    created: bool,
    missed: Option<SteerMissed>,
}

#[cfg(not(target_arch = "wasm32"))]
fn replay<'a>(
    facts: impl IntoIterator<Item = &'a wire::Envelope<wire::AnyFact>>,
) -> Result<AuthorityReplay, ReplayError> {
    let mut ledger = Ledger::new();
    let mut positions = BTreeMap::new();
    let mut misses = Vec::new();
    for envelope in facts {
        let Some(applied) = ledger.apply_envelope(envelope)? else {
            continue;
        };
        if let Some(missed) = applied.missed {
            misses.push(journal::JournalHead::new(envelope.seq, missed));
        }
        if let Some(directive) = applied.affected {
            if applied.created {
                positions.insert(
                    directive,
                    AuthorityPosition {
                        origin: envelope.origin.clone(),
                        created: envelope.seq,
                        head: envelope.seq,
                    },
                );
            } else {
                positions
                    .get_mut(&directive)
                    .expect("a transition follows one creating fact")
                    .head = envelope.seq;
            }
        }
    }
    Ok((ledger, positions, misses))
}

/// A durable steering stream that cannot be reduced consistently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplayError {
    seq: wire::Seq,
    kind: wire::FactKind,
    detail: String,
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

impl Standing for Ledger {
    fn disposition(&self, key: &Key) -> Option<Disposition> {
        self.directives.get(key).map(Directive::disposition)
    }
}

/// Reads a key out of a wire field, naming the field when it does not spell one.
fn read(field: &'static str, text: &str) -> Result<Key, Refusal> {
    text.parse().map_err(|_| Refusal::Malformed {
        field,
        text: text.to_owned(),
    })
}

fn admit_attachments(
    directive: CanonicalUuid,
    attachments: &[DirectiveAttachment],
) -> Result<(), Refusal> {
    wire::media::admit_media(attachments)
        .map_err(|error| Refusal::InvalidMedia { directive, error })
}

#[cfg(test)]
mod tests;
