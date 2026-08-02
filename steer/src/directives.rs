//! Directives as durable state: send, resend lineage, escalation, cancel, and
//! expiry.
//!
//! Operator steering is state, not a message that scrolled past. A directive is
//! appended once and then retires only through a typed disposition, so what a
//! lane still owes is a question about facts rather than about whether a
//! terminal happened to be watched.
//!
//! Silence escalates through lineage rather than through anonymous repetition.
//! A resend is a new rung naming the rung below it, which is what lets an
//! acknowledgment of any live rung retire the whole ladder once, and what lets
//! an exhausted ladder be named as unheard instead of escalating forever.
//!
//! Everything here reduces facts into standing. It appends nothing: the command
//! door mints directives, the scanner mints retirements, and this decides what
//! those facts mean together — which is why replaying the same facts always
//! produces the same pending set.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

#[cfg(not(target_arch = "wasm32"))]
use serde::de::DeserializeOwned;
use wire::body::{
    DirectiveAttachment, SteerAcked, SteerCanceled, SteerDeclined, SteerLapsed, SteerMissed,
    SteerRecovered, SteerResent, SteerSent,
};
use wire::{CanonicalUuid, Disposition, Instant};

use crate::ack::keys::Key;
use crate::ack::scan::Standing;

/// Rungs one lineage may climb before its ladder is exhausted.
///
/// The bound exists so a deaf channel alarms by name instead of escalating
/// forever; past the top rung the directive stops being resent and lapses on
/// its own expiry.
pub const LADDER: u32 = 3;

/// One directive in a lineage, as the facts left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Directive {
    directive: CanonicalUuid,
    key: Key,
    lane: CanonicalUuid,
    text: String,
    control: Option<String>,
    attachments: Vec<DirectiveAttachment>,
    expires: Instant,
    escalation: u32,
    ancestor: Option<Key>,
    disposition: Disposition,
}

impl Directive {
    /// Durable identity of this directive rung.
    #[must_use]
    pub const fn directive(&self) -> CanonicalUuid {
        self.directive
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

    /// When this directive lapses if nothing retires it first.
    #[must_use]
    pub const fn expires(&self) -> Instant {
        self.expires
    }

    /// Which rung of its ladder this directive is.
    #[must_use]
    pub const fn escalation(&self) -> u32 {
        self.escalation
    }

    /// The rung below this one, absent on the rung a lineage started at.
    #[must_use]
    pub const fn ancestor(&self) -> Option<&Key> {
        self.ancestor.as_ref()
    }

    /// The state this directive currently sits in.
    #[must_use]
    pub const fn disposition(&self) -> Disposition {
        self.disposition
    }
}

/// One steering fact, as the reducer reads it.
#[derive(Debug, Clone, Copy)]
pub enum Steering<'a> {
    /// A directive was appended.
    Sent(&'a SteerSent),
    /// A live directive climbed a rung.
    Resent(&'a SteerResent),
    /// A lapsed directive was restored to live standing.
    Recovered(&'a SteerRecovered),
    /// A lineage was heard.
    Acked(&'a SteerAcked),
    /// A lineage was refused with a reason.
    Declined(&'a SteerDeclined),
    /// An acknowledgment did not land.
    Missed(&'a SteerMissed),
    /// A lineage reached its expiry unheard.
    Lapsed(&'a SteerLapsed),
    /// The operator withdrew a lineage.
    Canceled(&'a SteerCanceled),
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
    /// A directive already carries this key.
    Duplicate {
        /// Key that is already taken.
        key: Key,
    },
    /// A directive identity was already used by another rung.
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
    /// The lineage already retired.
    Retired {
        /// Key naming the retired lineage.
        key: Key,
        /// State it retired into.
        disposition: Disposition,
    },
    /// A resend skipped or repeated a rung.
    Rung {
        /// Key the resend minted.
        key: Key,
        /// Rung the ladder was standing on next.
        expected: u32,
        /// Rung the fact claimed.
        observed: u32,
    },
    /// The ladder has no rung left to climb.
    Exhausted {
        /// Key the resend minted.
        key: Key,
        /// Rungs the ladder carries.
        ladder: u32,
    },
    /// A resend addressed a different lane than the lineage it descends from.
    Lane {
        /// Key the resend minted.
        key: Key,
        /// Lane the lineage is addressed to.
        expected: CanonicalUuid,
        /// Lane the fact claimed.
        observed: CanonicalUuid,
    },
    /// A lapse named an expiry the lineage does not carry.
    Expiry {
        /// Key naming the lineage.
        key: Key,
        /// Expiry the lineage carries.
        expected: Instant,
        /// Expiry the fact claimed.
        observed: Instant,
    },
}

impl fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed { field, text } => {
                write!(formatter, "{field} does not spell a steering key: {text:?}")
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
                write!(formatter, "the lineage at {key} already retired as {word}")
            }
            Self::Rung {
                key,
                expected,
                observed,
            } => write!(
                formatter,
                "{key} climbed to rung {observed} where rung {expected} was next"
            ),
            Self::Exhausted { key, ladder } => write!(
                formatter,
                "{key} has no rung left: the ladder carries {ladder}"
            ),
            Self::Lane {
                key,
                expected,
                observed,
            } => write!(
                formatter,
                "{key} addressed lane {observed} where its lineage addresses {expected}"
            ),
            Self::Expiry {
                key,
                expected,
                observed,
            } => write!(
                formatter,
                "{key} lapsed at {observed} where its lineage expires at {expected}"
            ),
        }
    }
}

impl Error for Refusal {}

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

impl Ledger {
    /// An empty ledger, before any fact.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Rebuild standing from one canonical journal snapshot.
    ///
    /// # Errors
    ///
    /// Returns the exact fact position whose registered body or steering
    /// transition contradicts the durable stream.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_snapshot(snapshot: &journal::FactSnapshot) -> Result<Self, ReplayError> {
        let mut ledger = Self::new();
        for envelope in snapshot.facts() {
            let kind = envelope.fact.kind();
            let result = match kind {
                wire::FactKind::SteerSent => {
                    let body = decode_body::<SteerSent>(envelope)?;
                    ledger.absorb(Steering::Sent(&body))
                }
                wire::FactKind::SteerResent => {
                    let body = decode_body::<SteerResent>(envelope)?;
                    ledger.absorb(Steering::Resent(&body))
                }
                wire::FactKind::SteerRecovered => {
                    let body = decode_body::<SteerRecovered>(envelope)?;
                    ledger.absorb(Steering::Recovered(&body))
                }
                wire::FactKind::SteerAcked => {
                    let body = decode_body::<SteerAcked>(envelope)?;
                    ledger.absorb(Steering::Acked(&body))
                }
                wire::FactKind::SteerDeclined => {
                    let body = decode_body::<SteerDeclined>(envelope)?;
                    ledger.absorb(Steering::Declined(&body))
                }
                wire::FactKind::SteerMissed => {
                    let body = decode_body::<SteerMissed>(envelope)?;
                    ledger.absorb(Steering::Missed(&body))
                }
                wire::FactKind::SteerLapsed => {
                    let body = decode_body::<SteerLapsed>(envelope)?;
                    ledger.absorb(Steering::Lapsed(&body))
                }
                wire::FactKind::SteerCanceled => {
                    let body = decode_body::<SteerCanceled>(envelope)?;
                    ledger.absorb(Steering::Canceled(&body))
                }
                _ => continue,
            };
            result.map_err(|error| ReplayError {
                seq: envelope.seq,
                kind,
                detail: error.to_string(),
            })?;
        }
        Ok(ledger)
    }

    /// Folds one steering fact into standing.
    ///
    /// # Errors
    ///
    /// Returns the [`Refusal`] naming what the fact contradicts. A refusal
    /// leaves standing exactly as it was, so a rejected fact cannot half-apply.
    pub fn absorb(&mut self, steering: Steering<'_>) -> Result<(), Refusal> {
        match steering {
            Steering::Sent(body) => self.send(body),
            Steering::Resent(body) => self.resend(body),
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
            // A missed acknowledgment alters nothing, by law. Its key is not
            // even read: the fact exists precisely because the key was unknown,
            // malformed, quoted, or historical, so refusing it here would
            // refuse the diagnostic instead of recording it.
            Steering::Missed(_) => Ok(()),
        }
    }

    /// The directives a lane still owes an answer for, in journal order.
    ///
    /// A lineage answers as one directive, so only its top rung is pending: the
    /// rungs below are the same direction said again, and rendering them all
    /// would ask a lane to acknowledge one directive several times.
    #[must_use]
    pub fn pending(&self, lane: CanonicalUuid) -> Vec<&Directive> {
        let climbed: BTreeSet<&Key> = self
            .directives
            .values()
            .filter_map(|directive| directive.ancestor.as_ref())
            .collect();
        self.order
            .iter()
            .filter_map(|key| self.directives.get(key))
            .filter(|directive| {
                directive.lane == lane
                    && directive.disposition == Disposition::Live
                    && !climbed.contains(&directive.key)
            })
            .collect()
    }

    /// One lineage from its first rung upward, empty when nothing carries the
    /// key.
    #[must_use]
    pub fn lineage(&self, key: &Key) -> Vec<&Directive> {
        let Some(root) = self.root(key) else {
            return Vec::new();
        };
        let mut members: Vec<&Directive> = self
            .directives
            .values()
            .filter(|directive| self.root(&directive.key).as_ref() == Some(&root))
            .collect();
        members.sort_by_key(|directive| directive.escalation);
        members
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

    fn send(&mut self, body: &SteerSent) -> Result<(), Refusal> {
        let key = read("key", &body.key)?;
        if self.directives.contains_key(&key) {
            return Err(Refusal::Duplicate { key });
        }
        if self.directive(body.directive).is_some() {
            return Err(Refusal::IdentityDuplicate {
                directive: body.directive,
            });
        }
        self.place(Directive {
            directive: body.directive,
            key,
            lane: body.lane,
            text: body.text.clone(),
            control: body.control.clone(),
            attachments: body.attachments.clone(),
            expires: body.expires,
            escalation: 0,
            ancestor: None,
            disposition: Disposition::Live,
        });
        Ok(())
    }

    fn resend(&mut self, body: &SteerResent) -> Result<(), Refusal> {
        let key = read("key", &body.key)?;
        if self.directive(body.directive).is_some() {
            return Err(Refusal::IdentityDuplicate {
                directive: body.directive,
            });
        }
        let ancestor = read("ancestor", &body.ancestor)?;
        let below = self
            .directives
            .get(&ancestor)
            .ok_or_else(|| Refusal::Unknown {
                key: ancestor.clone(),
            })?
            .clone();

        if below.disposition != Disposition::Live {
            return Err(Refusal::Retired {
                key: ancestor,
                disposition: below.disposition,
            });
        }
        if below.lane != body.lane {
            return Err(Refusal::Lane {
                key,
                expected: below.lane,
                observed: body.lane,
            });
        }
        let next = below.escalation + 1;
        if body.escalation != next {
            return Err(Refusal::Rung {
                key,
                expected: next,
                observed: body.escalation,
            });
        }
        if next > LADDER {
            return Err(Refusal::Exhausted {
                key,
                ladder: LADDER,
            });
        }
        if self.directives.contains_key(&key) {
            return Err(Refusal::Duplicate { key });
        }

        self.place(Directive {
            directive: body.directive,
            key,
            lane: below.lane,
            text: below.text,
            control: below.control,
            attachments: below.attachments,
            expires: below.expires,
            escalation: next,
            ancestor: Some(ancestor),
            disposition: Disposition::Live,
        });
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
        if directive.disposition != Disposition::Lapsed {
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
        let root = self
            .root(&key)
            .ok_or_else(|| Refusal::Unknown { key: key.clone() })?;
        let members = self
            .directives
            .values()
            .filter(|member| self.root(&member.key).as_ref() == Some(&root))
            .map(|member| member.key.clone())
            .collect::<Vec<_>>();
        for member in &members {
            let directive = self
                .directives
                .get(member)
                .expect("lineage member came from the directive map");
            if directive.disposition != Disposition::Lapsed {
                return Err(Refusal::Retired {
                    key: member.clone(),
                    disposition: directive.disposition,
                });
            }
            if directive.expires != body.lapsed_at {
                return Err(Refusal::Expiry {
                    key: member.clone(),
                    expected: directive.expires,
                    observed: body.lapsed_at,
                });
            }
        }
        for member in members {
            let directive = self
                .directives
                .get_mut(&member)
                .expect("validated lineage member remains present");
            directive.disposition = Disposition::Live;
            directive.expires = body.expires;
        }
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

    /// Retires a whole lineage at once.
    ///
    /// Any live rung answers for the ladder, so acknowledging one cancels the
    /// escalation of the rest rather than leaving them to climb.
    fn retire(&mut self, key: &Key, disposition: Disposition) -> Result<(), Refusal> {
        let root = self
            .root(key)
            .ok_or_else(|| Refusal::Unknown { key: key.clone() })?;
        let members: Vec<Key> = self
            .directives
            .values()
            .filter(|directive| self.root(&directive.key).as_ref() == Some(&root))
            .map(|directive| directive.key.clone())
            .collect();

        if let Some(standing) = self.directives.get(&root).map(Directive::disposition)
            && standing != Disposition::Live
        {
            return Err(Refusal::Retired {
                key: key.clone(),
                disposition: standing,
            });
        }
        for member in members {
            if let Some(directive) = self.directives.get_mut(&member) {
                directive.disposition = disposition;
            }
        }
        Ok(())
    }

    /// The first rung of the lineage this key belongs to.
    ///
    /// The walk is bounded by the ledger's own size, so a lineage that somehow
    /// named itself cannot spin here.
    fn root(&self, key: &Key) -> Option<Key> {
        let mut current = self.directives.get(key)?;
        for _ in 0..self.directives.len() {
            let Some(ancestor) = current.ancestor.as_ref() else {
                return Some(current.key.clone());
            };
            current = self.directives.get(ancestor)?;
        }
        None
    }

    fn place(&mut self, directive: Directive) {
        self.order.push(directive.key.clone());
        self.directives.insert(directive.key.clone(), directive);
    }
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

#[cfg(not(target_arch = "wasm32"))]
fn decode_body<T: DeserializeOwned>(
    envelope: &wire::Envelope<wire::AnyFact>,
) -> Result<T, ReplayError> {
    let bytes = wire::encode(envelope.fact.body()).map_err(|error| ReplayError {
        seq: envelope.seq,
        kind: envelope.fact.kind(),
        detail: error.to_string(),
    })?;
    wire::decode(&bytes).map_err(|error| ReplayError {
        seq: envelope.seq,
        kind: envelope.fact.kind(),
        detail: error.to_string(),
    })
}

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

#[cfg(test)]
mod tests {
    use super::{LADDER, Ledger, Refusal, Steering};
    use crate::ack::keys::Key;
    use crate::ack::scan::Standing;
    use wire::body::{
        AcknowledgmentMiss, DirectiveAttachment, SteerAcked, SteerCanceled, SteerDeclined,
        SteerLapsed, SteerMissed, SteerRecovered, SteerResent, SteerSent, TranscriptLocus,
    };
    use wire::{CanonicalUuid, Disposition, Instant};

    const ROOT: &str = "1kJK6N3R";
    const SECOND: &str = "1kJK6M0V";
    const THIRD: &str = "1kJK7Sqy";
    const FOURTH: &str = "1kJK8KcW";
    const LANE: &str = "018f82d4-6f59-7d06-92e3-bce913116302";
    const ELSEWHERE: &str = "018f82d4-6f59-7d06-92e3-bce913116303";
    const EXPIRES: &str = "2026-08-01T07:07:19.000Z";

    #[test]
    fn a_sent_directive_stands_pending_until_something_retires_it() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

        let pending = ledger.pending(lane());
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].key(), &key(ROOT));
        assert_eq!(pending[0].text(), "hold the line");
        assert_eq!(pending[0].control(), Some("drive-drain-queue"));
        assert_eq!(pending[0].escalation(), 0);
        assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Live));
    }

    #[test]
    fn a_lane_hears_only_its_own_direction() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

        assert!(ledger.pending(elsewhere()).is_empty());
    }

    #[test]
    fn a_repeated_key_is_refused() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

        assert_eq!(
            ledger.absorb(Steering::Sent(&sent(ROOT))),
            Err(Refusal::Duplicate { key: key(ROOT) })
        );
    }

    /// A ladder is one directive said again, so the lane is asked once.
    #[test]
    fn a_resend_climbs_one_rung_and_leaves_the_lineage_pending_once() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        ledger
            .absorb(Steering::Resent(&resent(SECOND, ROOT, 1)))
            .expect("resent");

        let pending = ledger.pending(lane());
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].key(), &key(SECOND));
        assert_eq!(pending[0].escalation(), 1);
        assert_eq!(pending[0].ancestor(), Some(&key(ROOT)));
        assert_eq!(pending[0].text(), "hold the line");
        assert_eq!(ledger.lineage(&key(ROOT)).len(), 2);
    }

    #[test]
    fn a_resend_that_skips_or_repeats_a_rung_is_refused() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

        assert_eq!(
            ledger.absorb(Steering::Resent(&resent(SECOND, ROOT, 2))),
            Err(Refusal::Rung {
                key: key(SECOND),
                expected: 1,
                observed: 2,
            })
        );
        assert_eq!(
            ledger.absorb(Steering::Resent(&resent(SECOND, ROOT, 0))),
            Err(Refusal::Rung {
                key: key(SECOND),
                expected: 1,
                observed: 0,
            })
        );
    }

    /// Past the top rung the ladder stops rather than escalating forever.
    #[test]
    fn an_exhausted_ladder_is_refused() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        let rungs = [SECOND, THIRD, FOURTH];
        for (index, minted) in rungs.iter().enumerate() {
            let below = if index == 0 { ROOT } else { rungs[index - 1] };
            let climbed = u32::try_from(index + 1).expect("rung fits");
            ledger
                .absorb(Steering::Resent(&resent(minted, below, climbed)))
                .expect("rung within the ladder");
        }

        let over = "1kJK8yRS";
        assert_eq!(
            ledger.absorb(Steering::Resent(&resent(over, FOURTH, LADDER + 1))),
            Err(Refusal::Exhausted {
                key: key(over),
                ladder: LADDER,
            })
        );
    }

    #[test]
    fn a_resend_addressing_another_lane_is_refused() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

        let mut wandered = resent(SECOND, ROOT, 1);
        wandered.lane = elsewhere();
        assert_eq!(
            ledger.absorb(Steering::Resent(&wandered)),
            Err(Refusal::Lane {
                key: key(SECOND),
                expected: lane(),
                observed: elsewhere(),
            })
        );
    }

    /// Acknowledging the rung the lane was first asked at answers the ladder
    /// climbed above it, which is what stops later escalation.
    #[test]
    fn acknowledging_any_live_rung_retires_the_whole_lineage() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        ledger
            .absorb(Steering::Resent(&resent(SECOND, ROOT, 1)))
            .expect("resent");
        ledger
            .absorb(Steering::Acked(&acked(ROOT)))
            .expect("acked at the first rung");

        for rung in [ROOT, SECOND] {
            assert_eq!(
                ledger.disposition(&key(rung)),
                Some(Disposition::Acked),
                "{rung} did not retire with its lineage"
            );
        }
        assert!(ledger.pending(lane()).is_empty());
    }

    #[test]
    fn a_retired_lineage_refuses_a_second_disposition() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        ledger.absorb(Steering::Acked(&acked(ROOT))).expect("acked");

        assert_eq!(
            ledger.absorb(Steering::Declined(&declined(ROOT))),
            Err(Refusal::Retired {
                key: key(ROOT),
                disposition: Disposition::Acked,
            })
        );
        assert_eq!(
            ledger.absorb(Steering::Resent(&resent(SECOND, ROOT, 1))),
            Err(Refusal::Retired {
                key: key(ROOT),
                disposition: Disposition::Acked,
            })
        );
    }

    #[test]
    fn a_declined_directive_retires_as_declined() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        ledger
            .absorb(Steering::Declined(&declined(ROOT)))
            .expect("declined");

        assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Declined));
    }

    #[test]
    fn an_acknowledgment_binds_the_directive_identity_and_lane() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        let before = ledger.clone();

        let mut wrong_identity = acked(ROOT);
        wrong_identity.directive = directive(SECOND);
        assert!(matches!(
            ledger.absorb(Steering::Acked(&wrong_identity)),
            Err(Refusal::Identity { .. })
        ));
        assert_eq!(ledger, before);

        let mut wrong_lane = acked(ROOT);
        wrong_lane.lane = "018f82d4-6f59-7d06-92e3-bce913116399"
            .parse()
            .expect("other lane");
        assert!(matches!(
            ledger.absorb(Steering::Acked(&wrong_lane)),
            Err(Refusal::Lane { .. })
        ));
        assert_eq!(ledger, before);
    }

    #[test]
    fn an_operator_cancel_retires_a_live_lineage() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        ledger
            .absorb(Steering::Canceled(&SteerCanceled {
                key: ROOT.to_owned(),
                lane: lane(),
            }))
            .expect("canceled");

        assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Canceled));
        assert!(ledger.pending(lane()).is_empty());
    }

    #[test]
    fn a_lapse_must_name_the_expiry_the_lineage_carries() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");

        let early: Instant = "2026-08-01T06:00:00.000Z".parse().expect("instant");
        assert_eq!(
            ledger.absorb(Steering::Lapsed(&SteerLapsed {
                key: ROOT.to_owned(),
                lane: lane(),
                expires: early,
            })),
            Err(Refusal::Expiry {
                key: key(ROOT),
                expected: expires(),
                observed: early,
            })
        );

        ledger
            .absorb(Steering::Lapsed(&SteerLapsed {
                key: ROOT.to_owned(),
                lane: lane(),
                expires: expires(),
            }))
            .expect("lapsed at its own expiry");
        assert_eq!(ledger.disposition(&key(ROOT)), Some(Disposition::Lapsed));
    }

    #[test]
    fn recovery_restores_only_the_exact_lapsed_directive_without_losing_payload() {
        let mut ledger = Ledger::new();
        let original = sent(ROOT);
        ledger.absorb(Steering::Sent(&original)).expect("sent");
        ledger
            .absorb(Steering::Resent(&resent(SECOND, ROOT, 1)))
            .expect("resent");
        ledger
            .absorb(Steering::Lapsed(&SteerLapsed {
                key: SECOND.to_owned(),
                lane: lane(),
                expires: expires(),
            }))
            .expect("lapsed at its own expiry");
        let recovered_expiry: Instant = "2026-08-01T08:07:19.000Z"
            .parse()
            .expect("recovery instant");
        let recovered = SteerRecovered {
            directive: directive(SECOND),
            key: SECOND.to_owned(),
            lane: lane(),
            lapsed_at: expires(),
            expires: recovered_expiry,
        };

        ledger
            .absorb(Steering::Recovered(&recovered))
            .expect("exact lapsed directive recovers");

        let restored = ledger
            .directive(directive(SECOND))
            .expect("recovered directive retains identity");
        assert_eq!(restored.text(), original.text);
        assert_eq!(restored.control(), original.control.as_deref());
        assert_eq!(restored.attachments(), original.attachments);
        assert_eq!(restored.expires(), recovered_expiry);
        assert_eq!(restored.disposition(), Disposition::Live);
        assert_eq!(ledger.pending(lane()), vec![restored]);
        for member in ledger.lineage(&key(SECOND)) {
            assert_eq!(member.disposition(), Disposition::Live);
            assert_eq!(member.expires(), recovered_expiry);
        }
        assert_eq!(
            ledger.absorb(Steering::Recovered(&recovered)),
            Err(Refusal::Retired {
                key: key(SECOND),
                disposition: Disposition::Live,
            }),
            "the same failed directive cannot be recovered twice",
        );
        ledger
            .absorb(Steering::Acked(&acked(SECOND)))
            .expect("the recovered lineage can retire normally");
        for member in ledger.lineage(&key(SECOND)) {
            assert_eq!(member.disposition(), Disposition::Acked);
        }
    }

    /// A missed acknowledgment is a diagnostic, never a retirement. Its key is
    /// not even read, because the fact exists precisely when the key was
    /// unknown or malformed.
    #[test]
    fn a_missed_acknowledgment_alters_nothing() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        let before = ledger.clone();

        for named in [ROOT, "nothing", ""] {
            ledger
                .absorb(Steering::Missed(&SteerMissed {
                    key: named.to_owned(),
                    lane: lane(),
                    epoch: epoch(),
                    locus: transcript_locus(12, 1),
                    miss: AcknowledgmentMiss::Unknown,
                }))
                .expect("a missed acknowledgment is always absorbed");
        }

        assert_eq!(ledger, before, "a missed acknowledgment altered standing");
    }

    #[test]
    fn a_key_that_is_not_spelled_as_one_names_the_field() {
        let mut ledger = Ledger::new();
        let mut malformed = sent(ROOT);
        malformed.key = "1kJK6N3".to_owned();

        assert_eq!(
            ledger.absorb(Steering::Sent(&malformed)),
            Err(Refusal::Malformed {
                field: "key",
                text: "1kJK6N3".to_owned(),
            })
        );

        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        let mut wandering = resent(SECOND, ROOT, 1);
        wandering.ancestor = "nope".to_owned();
        assert_eq!(
            ledger.absorb(Steering::Resent(&wandering)),
            Err(Refusal::Malformed {
                field: "ancestor",
                text: "nope".to_owned(),
            })
        );
    }

    #[test]
    fn an_acknowledgment_of_nothing_is_refused() {
        let mut ledger = Ledger::new();

        assert_eq!(
            ledger.absorb(Steering::Acked(&acked(ROOT))),
            Err(Refusal::Unknown { key: key(ROOT) })
        );
    }

    /// A refusal is not a half-applied fact.
    #[test]
    fn a_refused_fact_leaves_standing_untouched() {
        let mut ledger = Ledger::new();
        ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
        let before = ledger.clone();

        for refused in [
            Steering::Sent(&sent(ROOT)),
            Steering::Resent(&resent(SECOND, ROOT, 2)),
            Steering::Acked(&acked(SECOND)),
        ] {
            assert!(ledger.absorb(refused).is_err(), "fact was admitted");
            assert_eq!(ledger, before, "a refused fact altered standing");
        }
    }

    /// Replay is the same reduction, so standing does not depend on when the
    /// facts were read.
    #[test]
    fn replaying_the_same_facts_produces_the_same_standing() {
        let build = || {
            let mut ledger = Ledger::new();
            ledger.absorb(Steering::Sent(&sent(ROOT))).expect("sent");
            ledger
                .absorb(Steering::Resent(&resent(SECOND, ROOT, 1)))
                .expect("resent");
            ledger
                .absorb(Steering::Acked(&acked(SECOND)))
                .expect("acked");
            ledger
        };

        assert_eq!(build(), build());
    }

    fn key(text: &str) -> Key {
        text.parse().expect("test key")
    }

    fn lane() -> CanonicalUuid {
        LANE.parse().expect("test lane")
    }

    fn elsewhere() -> CanonicalUuid {
        ELSEWHERE.parse().expect("other lane")
    }

    fn expires() -> Instant {
        EXPIRES.parse().expect("test instant")
    }

    fn sent(key: &str) -> SteerSent {
        SteerSent {
            directive: directive(key),
            key: key.to_owned(),
            lane: lane(),
            text: "hold the line".to_owned(),
            control: Some("drive-drain-queue".to_owned()),
            attachments: vec![DirectiveAttachment {
                name: "operator-note.txt".to_owned(),
                content_type: "text/plain".to_owned(),
                data_url: "data:text/plain;base64,aG9sZCB0aGUgbGluZQ==".to_owned(),
            }],
            expires: expires(),
        }
    }

    fn resent(key: &str, ancestor: &str, escalation: u32) -> SteerResent {
        SteerResent {
            directive: directive(key),
            key: key.to_owned(),
            ancestor: ancestor.to_owned(),
            lane: lane(),
            escalation,
        }
    }

    fn acked(key: &str) -> SteerAcked {
        SteerAcked {
            directive: directive(key),
            key: key.to_owned(),
            lane: lane(),
            epoch: epoch(),
            locus: transcript_locus(3, 1),
        }
    }

    fn declined(key: &str) -> SteerDeclined {
        SteerDeclined {
            directive: directive(key),
            key: key.to_owned(),
            lane: lane(),
            epoch: epoch(),
            locus: transcript_locus(3, 1),
            reason: "the tree is red".to_owned(),
        }
    }

    fn transcript_locus(line: u32, column: u32) -> TranscriptLocus {
        TranscriptLocus {
            member: 1,
            offset: 40,
            length: 80,
            event: 0,
            line,
            column,
        }
    }

    fn epoch() -> CanonicalUuid {
        "018f82d4-6f59-7d06-92e3-bce913116303"
            .parse()
            .expect("fixture epoch")
    }

    fn directive(key: &str) -> CanonicalUuid {
        let suffix = key.bytes().fold(0_u64, |value, byte| {
            value.wrapping_mul(131).wrapping_add(u64::from(byte))
        });
        format!("00000000-0000-4000-8000-{:012x}", suffix & 0xffff_ffff_ffff)
            .parse()
            .expect("fixture directive UUID")
    }
}
