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

use wire::body::{
    DirectiveAttachment, SteerAcked, SteerCanceled, SteerDeadLettered, SteerDeclined, SteerLapsed,
    SteerMissed, SteerRecovered, SteerResent, SteerSent, TasteReminded,
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
    /// A lapsed or dead-lettered directive was restored to live standing.
    Recovered(&'a SteerRecovered),
    /// A lineage was heard.
    Acked(&'a SteerAcked),
    /// A lineage was refused with a reason.
    Declined(&'a SteerDeclined),
    /// An acknowledgment did not land.
    Missed(&'a SteerMissed),
    /// A lineage reached its expiry unheard.
    Lapsed(&'a SteerLapsed),
    /// A lineage was parked because its lane could not start.
    DeadLettered(&'a SteerDeadLettered),
    /// The operator withdrew a lineage.
    Canceled(&'a SteerCanceled),
    /// A taste fired on authored prose and opened a directive.
    ///
    /// It reads as a steering fact because it is one. The taste evidence it
    /// also carries is read by the taste plane from the same row, and never by
    /// anything here.
    Reminded(&'a TasteReminded),
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
    /// Returns the exact fact position whose steering transition contradicts
    /// the durable stream. A body cannot be the reason: the snapshot holds only
    /// bodies its registered DTO already accepted.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_snapshot(snapshot: &journal::FactSnapshot) -> Result<Self, ReplayError> {
        let mut ledger = Self::new();
        for envelope in snapshot.facts() {
            // Each read is the kind test and the decode at once, so a kind is
            // named here exactly once, by the body type registered against it.
            let fact = &envelope.fact;
            let result = if let Some(body) = fact.read::<SteerSent>() {
                ledger.absorb(Steering::Sent(&body))
            } else if let Some(body) = fact.read::<TasteReminded>() {
                ledger.absorb(Steering::Reminded(&body))
            } else if let Some(body) = fact.read::<SteerResent>() {
                ledger.absorb(Steering::Resent(&body))
            } else if let Some(body) = fact.read::<SteerRecovered>() {
                ledger.absorb(Steering::Recovered(&body))
            } else if let Some(body) = fact.read::<SteerAcked>() {
                ledger.absorb(Steering::Acked(&body))
            } else if let Some(body) = fact.read::<SteerDeclined>() {
                ledger.absorb(Steering::Declined(&body))
            } else if let Some(body) = fact.read::<SteerMissed>() {
                ledger.absorb(Steering::Missed(&body))
            } else if let Some(body) = fact.read::<SteerLapsed>() {
                ledger.absorb(Steering::Lapsed(&body))
            } else if let Some(body) = fact.read::<SteerDeadLettered>() {
                ledger.absorb(Steering::DeadLettered(&body))
            } else if let Some(body) = fact.read::<SteerCanceled>() {
                ledger.absorb(Steering::Canceled(&body))
            } else {
                continue;
            };
            result.map_err(|error| ReplayError {
                seq: envelope.seq,
                kind: fact.kind(),
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
            Steering::Reminded(body) => self.remind(body),
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

    /// Open standing for a directive that descends from nothing.
    ///
    /// Both ways a lineage can begin land here rather than each stating the
    /// admission for itself. The two refusals are the whole of what "first rung"
    /// means -- a key already standing, and an identity already spent -- so
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

    fn send(&mut self, body: &SteerSent) -> Result<(), Refusal> {
        let key = read("key", &body.key)?;
        self.open(Directive {
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
    fn remind(&mut self, body: &TasteReminded) -> Result<(), Refusal> {
        let key = read("key", &body.key)?;
        self.open(Directive {
            directive: body.directive,
            key,
            lane: body.lane,
            text: body.text.clone(),
            control: None,
            attachments: Vec::new(),
            expires: body.expires,
            escalation: 0,
            ancestor: None,
            disposition: Disposition::Live,
        })
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
            if !matches!(
                directive.disposition,
                Disposition::Lapsed | Disposition::DeadLettered
            ) {
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

    fn dead_letter(&mut self, body: &SteerDeadLettered) -> Result<(), Refusal> {
        self.dispose(
            &body.key,
            body.directive,
            body.lane,
            Disposition::DeadLettered,
        )
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
mod tests;
