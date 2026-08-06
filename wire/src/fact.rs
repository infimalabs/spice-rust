//! The closed fact vocabulary.
//!
//! Each kind is a plane-crossing verb. A kind is a path of single words and
//! always appears as a field, never as an object key. The set is closed: a
//! spelling absent from this enum is not a fact.
//!
//! The vocabulary is declared once, as a table. The discriminant, its wire
//! spelling, its body DTO, and the producer class permitted to mint it all come
//! from the same row, so a kind cannot be added without a body and a producer:
//! there is no arm of the table to write one in.

use std::fmt;

use serde::de::{Deserializer, Error as DeError};
use serde::ser::{Error as SerError, Serializer};
use serde::{Deserialize, Serialize};

/// A closed class of appenders. Every kind names exactly one.
///
/// A kind with no producer class is as dead as a kind with no fold: a bus timer
/// that never fires is a generated-check failure, not a quiet stall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Producer {
    /// The command door, admitting a client-proposed transition.
    CommandDoor,
    /// A bus timer firing on a deadline the heads carry.
    BusTimer,
    /// The scanner that reads dispositions out of transcripts.
    AckScanner,
    /// The resident recognizing an artifact or an observed lifecycle change.
    ResidentRecognition,
    /// The judge ruling on a taste at an evidence locus.
    Judge,
}

impl Producer {
    /// Every producer class, in the order WIRE.md enumerates them.
    ///
    /// The set is closed, so a caller sweeping it is sweeping the whole of
    /// what can mint a fact rather than a list it restated by hand.
    pub const ALL: &[Self] = &[
        Self::CommandDoor,
        Self::BusTimer,
        Self::AckScanner,
        Self::ResidentRecognition,
        Self::Judge,
    ];

    /// The producer class as it is spelled on the wire.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CommandDoor => "command-door",
            Self::BusTimer => "bus-timer",
            Self::AckScanner => "ack-scanner",
            Self::ResidentRecognition => "resident-recognition",
            Self::Judge => "judge",
        }
    }
}

impl fmt::Display for Producer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

mod sealed {
    /// Closes [`super::Body`] to the registered vocabulary.
    pub trait Registered {}
}

/// A body DTO registered against exactly one kind.
///
/// The trait is sealed: the vocabulary is closed, so a type outside this crate
/// cannot register itself as a body and no kind can acquire a second one.
pub trait Body: sealed::Registered + Serialize + serde::de::DeserializeOwned {
    /// The one kind this body is registered against.
    const KIND: FactKind;
}

/// Declare the vocabulary.
///
/// Each row binds a discriminant to its wire spelling, to the body DTO of the
/// same name in [`crate::body`], and to its producer class.
macro_rules! vocabulary {
    ($(
        $(#[doc = $doc:literal])+
        $variant:ident = $spelling:literal by $producer:ident;
    )+) => {
        /// Every admitted fact discriminant.
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        pub enum FactKind {
            $(
                $(#[doc = $doc])+
                #[serde(rename = $spelling)]
                $variant,
            )+
        }

        impl FactKind {
            /// Every kind, in vocabulary order.
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            /// The kind as it is spelled on the wire.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $spelling,)+
                }
            }

            /// The producer class permitted to mint this kind.
            #[must_use]
            pub const fn producer(self) -> Producer {
                match self {
                    $(Self::$variant => Producer::$producer,)+
                }
            }

            /// Validate an opaque body through the DTO registered for this kind.
            pub(crate) fn validate_body(
                self,
                body: &serde_json::Map<String, serde_json::Value>,
            ) -> Result<(), serde_json::Error> {
                match self {
                    $(
                        Self::$variant => {
                            decode_registered_body::<crate::body::$variant>(body).map(drop)
                        }
                    )+
                }
            }
        }

        $(
            impl sealed::Registered for crate::body::$variant {}

            impl Body for crate::body::$variant {
                const KIND: FactKind = FactKind::$variant;
            }
        )+
    };
}

vocabulary! {
    /// A task statement and initial workflow head were created.
    TaskAdded = "task::added" by CommandDoor;
    /// A claimed phase completed and workflow advanced.
    TaskPhased = "task::phased" by CommandDoor;
    /// A task completed under a re-proven active claim.
    TaskDone = "task::done" by CommandDoor;
    /// A review verdict landed with reviewer and assignment witnesses.
    TaskReviewed = "task::reviewed" by CommandDoor;
    /// The allocator seated a reviewer under the anti-self-review witness.
    TaskAssigned = "task::assigned" by CommandDoor;
    /// A durable task note appended.
    TaskNoted = "task::noted" by CommandDoor;
    /// A task received a wakeable deferral.
    TaskDeferred = "task::deferred" by CommandDoor;
    /// A task's deferral cleared.
    TaskWoken = "task::woken" by CommandDoor;
    /// An unsettled task statement changed.
    TaskEdited = "task::edited" by CommandDoor;

    /// Exclusive possession was minted from current eligibility and claim heads.
    ClaimWon = "claim::won" by CommandDoor;
    /// Possession passed to a successor with ancestor lineage preserved.
    ClaimCarried = "claim::carried" by ResidentRecognition;
    /// Possession was released and the task became eligible again.
    ClaimFreed = "claim::freed" by CommandDoor;
    /// The lease deadline passed and the resident expired possession.
    ClaimLapsed = "claim::lapsed" by BusTimer;
    /// The resident extended a live, epoch-bound lease within its maximum.
    ClaimRenewed = "claim::renewed" by BusTimer;

    /// The exact normalized plan digest landed at its witnessed board barrier.
    DocApplied = "doc::applied" by CommandDoor;

    /// A directive was created.
    SteerSent = "steer::sent" by CommandDoor;
    /// The same intent was sent again with lineage and escalation carried.
    SteerResent = "steer::resent" by CommandDoor;
    /// A lapsed directive returned to live standing under operator recovery.
    SteerRecovered = "steer::recovered" by CommandDoor;
    /// A live directive was acknowledged at a transcript locus.
    SteerAcked = "steer::acked" by AckScanner;
    /// A live directive was declined at a transcript locus.
    SteerDeclined = "steer::declined" by AckScanner;
    /// An ACK named no live directive; nothing retired.
    SteerMissed = "steer::missed" by AckScanner;
    /// A directive reached expiry without a valid disposition.
    SteerLapsed = "steer::lapsed" by BusTimer;
    /// The operator withdrew a live directive.
    SteerCanceled = "steer::canceled" by CommandDoor;

    /// An ACP permission request arrived, bound to its cause.
    ConsentAsked = "consent::asked" by CommandDoor;
    /// A named rule or operator allowed the request.
    ConsentGranted = "consent::granted" by CommandDoor;
    /// A named rule or operator denied the request.
    ConsentDenied = "consent::denied" by CommandDoor;
    /// The request pended to the operator while reversible work continued.
    ConsentQueued = "consent::queued" by CommandDoor;
    /// A non-ACP lane's consent floor was recorded once for its occupancy.
    ConsentFloored = "consent::floored" by CommandDoor;

    /// A lane and its capability were created.
    LaneSeated = "lane::seated" by ResidentRecognition;
    /// One Canvas lineage adopted a shared task-subscription life.
    LineageConfigured = "lineage::configured" by ResidentRecognition;
    /// A selected vendor record declared the exact resumable lane session.
    LaneSessionDeclared = "lane::declared" by ResidentRecognition;
    /// An occupant became live under the lane.
    LaneStarted = "lane::started" by ResidentRecognition;
    /// A vacant lane seat moved to another linked worktree.
    LaneImported = "lane::imported" by ResidentRecognition;
    /// The resident observed the occupant end.
    LaneExited = "lane::exited" by ResidentRecognition;
    /// The occupant crossed a context compaction boundary.
    LaneCompacted = "lane::compacted" by ResidentRecognition;
    /// A successor occupant was seated with its ancestor named.
    LaneRenewed = "lane::renewed" by ResidentRecognition;
    /// A graceful occupant end completed.
    LaneDrained = "lane::drained" by ResidentRecognition;
    /// Work run without the resident left a marker recognized once.
    LaneDegraded = "lane::degraded" by ResidentRecognition;
    /// Repeated young occupant deaths paused automatic re-seating.
    LaneHeld = "lane::held" by BusTimer;
    /// A held lane returned to seating eligibility.
    LaneResumed = "lane::resumed" by BusTimer;

    /// The judge confirmed a taste at an evidence locus.
    TasteConfirmed = "taste::confirmed" by Judge;
    /// The judge cleared a taste at an evidence locus.
    TasteCleared = "taste::cleared" by Judge;

    /// An expected listening event remained absent past its armed window.
    CanaryAlarmed = "canary::alarmed" by ResidentRecognition;
    /// Listening meters disagreed: activity existed without decoded context.
    CanaryContradicted = "canary::contradicted" by ResidentRecognition;

    /// An unproven vendor version first ran with residue measurement armed.
    VendorArmed = "vendor::armed" by ResidentRecognition;

    /// An executable surface digest changed and entered quarantine.
    TrustPended = "trust::pended" by CommandDoor;
    /// The operator approved the exact executable digest.
    TrustGranted = "trust::granted" by CommandDoor;
    /// The operator withdrew approval from the exact executable digest.
    TrustRevoked = "trust::revoked" by CommandDoor;

    /// The resident recognized a repository commit and bound its receipt verdict.
    CommitLanded = "commit::landed" by ResidentRecognition;

    /// The resident recognized a new gate finding; the Finding head opened.
    GateFound = "gate::found" by ResidentRecognition;
    /// A standing finding resolved at its evidence; the Finding head closed.
    GateCleared = "gate::cleared" by ResidentRecognition;

    /// The one forward schema migration finalized before epoch mint.
    JournalMigrated = "journal::migrated" by ResidentRecognition;
    /// A quiesced backup bundle was produced with its enumerated tape manifest.
    JournalBacked = "journal::backed" by ResidentRecognition;
    /// A restore completed and the new epoch recognized the exchanged authority.
    JournalRestored = "journal::restored" by ResidentRecognition;

    /// The resident acquired the kernel writer lock and minted its epoch.
    UpSeated = "up::seated" by CommandDoor;
    /// The resident stopped admission and began a graceful end.
    UpQuiescing = "up::quiescing" by CommandDoor;
    /// The resident began preserving admitted work and ending occupants.
    UpDraining = "up::draining" by CommandDoor;
    /// Grace expired and the resident began ending unfinished occupants.
    UpTerminating = "up::terminating" by CommandDoor;
    /// Bounded escalation began against occupants that survived termination.
    UpKilling = "up::killing" by CommandDoor;
    /// Occupants ended; verification of claims, seals, and residue began.
    UpVerifying = "up::verifying" by CommandDoor;
    /// The verified terminal state committed before releasing authority.
    UpDark = "up::dark" by CommandDoor;

    /// The resident recognized a valid evidence bundle.
    ProofSealed = "proof::sealed" by ResidentRecognition;

    /// The sealed manifest and each target artifact digest published.
    ReleasePublished = "release::published" by ResidentRecognition;
}

impl fmt::Display for FactKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// An internally tagged fact: its discriminant beside its kind-specific body.
///
/// The body is a type parameter bounded by [`Body`], so the pair is fixed at
/// compile time: `F` names the kind, and there is no way to spell a fact whose
/// body is an untyped map.
///
/// The discriminant is derived from the registered body type and cannot be
/// supplied independently:
///
/// ```compile_fail
/// let fact = wire::Fact {
///     kind: wire::FactKind::TaskDone,
///     body: wire::body::TaskWoken {
///         task: "WIRE-aBcD".parse().expect("task handle"),
///     },
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact<F: Body> {
    /// Kind-specific body, flattened beside the discriminant.
    pub body: F,
}

#[derive(Deserialize)]
struct UncheckedFact {
    kind: FactKind,
    #[serde(flatten)]
    body: serde_json::Map<String, serde_json::Value>,
}

fn decode_registered_body<F: Body>(
    body: &serde_json::Map<String, serde_json::Value>,
) -> Result<F, serde_json::Error> {
    let supplied = serde_json::Value::Object(body.clone());
    let decoded: F = serde_json::from_value(supplied.clone())?;
    let registered = serde_json::to_value(&decoded)?;
    if supplied == registered {
        Ok(decoded)
    } else {
        Err(<serde_json::Error as DeError>::custom(
            "body fields do not exactly match the registered DTO",
        ))
    }
}

impl<F: Body> Serialize for Fact<F> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = match serde_json::to_value(&self.body).map_err(S::Error::custom)? {
            serde_json::Value::Object(fields) => fields,
            _ => return Err(S::Error::custom("a registered body must be an object")),
        };
        fields.insert(
            "kind".to_owned(),
            serde_json::to_value(F::KIND).map_err(S::Error::custom)?,
        );
        fields.serialize(serializer)
    }
}

impl<'de, F: Body> Deserialize<'de> for Fact<F> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let unchecked = UncheckedFact::deserialize(deserializer)?;
        if unchecked.kind != F::KIND {
            return Err(D::Error::custom(format!(
                "kind {} is not the kind this body is registered against, {}",
                unchecked.kind,
                F::KIND
            )));
        }
        let body = decode_registered_body(&unchecked.body).map_err(D::Error::custom)?;
        Ok(Self { body })
    }
}

/// A fact read back without naming the type of its body.
///
/// [`Fact`] ties a body to the one kind it is registered against, which is what
/// keeps an invented fact off the socket. That tie needs a body type, and a
/// reader sweeping rows it did not write does not have one: a journal verifier
/// or a migration learns the kind from the bytes in front of it. It reads the
/// discriminant, which is not part of the body, and leaves the body as the
/// fields it arrived as.
///
/// This gives up nothing the registration was protecting. The vocabulary is
/// enforced where a fact is minted and where a typed consumer decodes one; a
/// reader validates the body through that kind's registered DTO before keeping
/// its fields opaque. It can re-encode a stored row for canonical verification,
/// but its private fields prevent callers from inventing or mutating an opaque
/// fact and sending it as a typed one.
///
/// An unchecked opaque fact cannot be constructed:
///
/// ```compile_fail
/// let fact = wire::AnyFact {
///     kind: wire::FactKind::TaskDone,
///     body: Default::default(),
/// };
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnyFact {
    /// Discriminant from the closed vocabulary, refused if it is not one.
    kind: FactKind,
    /// The remaining fields, validated once and then uninterpreted.
    #[serde(flatten)]
    body: serde_json::Map<String, serde_json::Value>,
}

impl<'de> Deserialize<'de> for AnyFact {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let unchecked = UncheckedFact::deserialize(deserializer)?;
        unchecked
            .kind
            .validate_body(&unchecked.body)
            .map_err(D::Error::custom)?;
        Ok(Self {
            kind: unchecked.kind,
            body: unchecked.body,
        })
    }
}

impl AnyFact {
    /// The registered discriminant decoded for this body.
    #[must_use]
    pub const fn kind(&self) -> FactKind {
        self.kind
    }

    /// The validated body fields as they appeared on the wire.
    #[must_use]
    pub const fn body(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.body
    }
}

impl<F: Body> Fact<F> {
    /// Pair a body with the one discriminant it is registered against.
    #[must_use]
    pub const fn new(body: F) -> Self {
        Self { body }
    }

    /// The discriminant registered for this body's type.
    #[must_use]
    pub const fn kind(&self) -> FactKind {
        F::KIND
    }

    /// The producer class permitted to mint this fact.
    #[must_use]
    pub const fn producer(&self) -> Producer {
        F::KIND.producer()
    }
}
