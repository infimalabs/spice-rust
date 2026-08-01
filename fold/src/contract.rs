use std::fmt;

/// Declare a closed vocabulary and the slice that enumerates it.
///
/// The variants are written once and `ALL` is generated from that list, so a
/// variant cannot be declared without joining the enumeration and the two can
/// never disagree. Only the enumeration needed generating: every `match` over
/// one of these enums is already exhaustive by the compiler, so a hand-written
/// `ALL` was the crate's one unlinked copy of the variant list.
macro_rules! closed_vocabulary {
    (
        $(#[$vocabulary_doc:meta])*
        pub enum $name:ident {
            $($(#[$variant_doc:meta])* $variant:ident,)+
        }
    ) => {
        $(#[$vocabulary_doc])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum $name {
            $($(#[$variant_doc])* $variant,)+
        }

        impl $name {
            /// Every variant, in declaration order.
            pub const ALL: &[Self] = &[$(Self::$variant),+];
        }
    };
}

closed_vocabulary! {
    /// One registered disposable materialization.
    ///
    /// Declaration order is dependency-safe rebuild order: a fold appears after
    /// every fold it names as a source.
    pub enum FoldId {
        SteerInbox,
        ConsentQueue,
        LaneCanary,
        TasteActive,
        CanvasVerdicts,
        CanvasMagnitude,
        CanvasFlow,
        CanvasTopology,
        CanvasChronology,
        ReleaseHistory,
    }
}

impl FoldId {
    /// The fold's wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SteerInbox => "steer::inbox",
            Self::ConsentQueue => "consent::queue",
            Self::LaneCanary => "lane::canary",
            Self::TasteActive => "taste::active",
            Self::CanvasVerdicts => "canvas::verdicts",
            Self::CanvasMagnitude => "canvas::magnitude",
            Self::CanvasFlow => "canvas::flow",
            Self::CanvasTopology => "canvas::topology",
            Self::CanvasChronology => "canvas::chronology",
            Self::ReleaseHistory => "release::history",
        }
    }
}

impl fmt::Display for FoldId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

closed_vocabulary! {
    /// A closed fact-plane source.
    pub enum FactPlane {
        Task,
        Claim,
        Document,
        Steering,
        Consent,
        Lane,
        Taste,
        Canary,
        Vendor,
        Trust,
        Commit,
        Gate,
        Journal,
        Resident,
        Proof,
        Release,
    }
}

impl FactPlane {
    /// True when the fact kind belongs to this declared source.
    #[must_use]
    pub const fn contains(self, kind: wire::FactKind) -> bool {
        match self {
            Self::Task => is_task(kind),
            Self::Claim => is_claim(kind),
            Self::Document => matches!(kind, wire::FactKind::DocApplied),
            Self::Steering => is_steering(kind),
            Self::Consent => is_consent(kind),
            Self::Lane => is_lane(kind),
            Self::Taste => is_taste(kind),
            Self::Canary => is_canary(kind),
            Self::Vendor => matches!(kind, wire::FactKind::VendorArmed),
            Self::Trust => is_trust(kind),
            Self::Commit => matches!(kind, wire::FactKind::CommitLanded),
            Self::Gate => is_gate(kind),
            Self::Journal => is_journal(kind),
            Self::Resident => is_resident(kind),
            Self::Proof => matches!(kind, wire::FactKind::ProofSealed),
            Self::Release => matches!(kind, wire::FactKind::ReleasePublished),
        }
    }
}

const fn is_task(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::TaskAdded
            | wire::FactKind::TaskPhased
            | wire::FactKind::TaskDone
            | wire::FactKind::TaskReviewed
            | wire::FactKind::TaskAssigned
            | wire::FactKind::TaskNoted
            | wire::FactKind::TaskDeferred
            | wire::FactKind::TaskWoken
            | wire::FactKind::TaskEdited
    )
}

const fn is_claim(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::ClaimWon
            | wire::FactKind::ClaimCarried
            | wire::FactKind::ClaimFreed
            | wire::FactKind::ClaimLapsed
            | wire::FactKind::ClaimRenewed
    )
}

const fn is_steering(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::SteerSent
            | wire::FactKind::SteerResent
            | wire::FactKind::SteerAcked
            | wire::FactKind::SteerDeclined
            | wire::FactKind::SteerMissed
            | wire::FactKind::SteerLapsed
            | wire::FactKind::SteerCanceled
    )
}

const fn is_consent(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::ConsentAsked
            | wire::FactKind::ConsentGranted
            | wire::FactKind::ConsentDenied
            | wire::FactKind::ConsentQueued
            | wire::FactKind::ConsentFloored
    )
}

const fn is_lane(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::LaneSeated
            | wire::FactKind::LaneStarted
            | wire::FactKind::LaneExited
            | wire::FactKind::LaneCompacted
            | wire::FactKind::LaneRenewed
            | wire::FactKind::LaneDrained
            | wire::FactKind::LaneDegraded
            | wire::FactKind::LaneHeld
            | wire::FactKind::LaneResumed
    )
}

const fn is_taste(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::TasteConfirmed | wire::FactKind::TasteCleared
    )
}

const fn is_canary(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::CanaryAlarmed | wire::FactKind::CanaryContradicted
    )
}

const fn is_trust(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::TrustPended | wire::FactKind::TrustGranted | wire::FactKind::TrustRevoked
    )
}

const fn is_gate(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::GateFound | wire::FactKind::GateCleared
    )
}

const fn is_journal(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::JournalMigrated
            | wire::FactKind::JournalBacked
            | wire::FactKind::JournalRestored
    )
}

const fn is_resident(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::UpSeated
            | wire::FactKind::UpQuiescing
            | wire::FactKind::UpDraining
            | wire::FactKind::UpTerminating
            | wire::FactKind::UpKilling
            | wire::FactKind::UpVerifying
            | wire::FactKind::UpDark
    )
}

closed_vocabulary! {
    /// One authoritative head table admitted as a fold source.
    pub enum HeadSource {
        Task,
        Workflow,
        Claim,
        Lease,
        Witness,
        Finding,
        Residency,
    }
}

closed_vocabulary! {
    /// One bounded non-replayable ring source.
    pub enum RingId {
        BusRefusals,
    }
}

impl RingId {
    /// The ring's wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BusRefusals => "bus::refusals",
        }
    }
}

closed_vocabulary! {
    /// Tracked configuration whose digest is an explicit fold input.
    pub enum ConfigSource {
        Consent,
        Taste,
    }
}

closed_vocabulary! {
    /// A bounded tape input.
    pub enum TapeSource {
        DeclaredExtents,
    }
}

/// One complete, typed input declaration.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Source {
    Fact(wire::FactKind),
    Facts(FactPlane),
    Fold(FoldId),
    Head(HeadSource),
    Ring(RingId),
    Config(ConfigSource),
    Tape(TapeSource),
    CommandResults,
}

/// Policy for a requested position beyond a fold's complete horizon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Beyond {
    Wait,
    Answer,
}

/// Whether a requested position is complete, marked, or must wait.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Availability {
    Complete,
    Marked(Horizon),
    Wait,
}

/// Greatest journal position applied by a generation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, PartialOrd, Ord)]
pub struct Cursor(Option<wire::Seq>);

impl Cursor {
    #[must_use]
    pub const fn empty() -> Self {
        Self(None)
    }

    #[must_use]
    pub const fn at(position: wire::Seq) -> Self {
        Self(Some(position))
    }

    #[must_use]
    pub const fn position(self) -> Option<wire::Seq> {
        self.0
    }
}

/// Greatest position through which every declared input is complete.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, PartialOrd, Ord)]
pub struct Horizon(Option<wire::Seq>);

impl Horizon {
    #[must_use]
    pub const fn empty() -> Self {
        Self(None)
    }

    #[must_use]
    pub const fn at(position: wire::Seq) -> Self {
        Self(Some(position))
    }

    #[must_use]
    pub const fn position(self) -> Option<wire::Seq> {
        self.0
    }

    #[must_use]
    pub fn availability(self, requested: wire::Seq, beyond: Beyond) -> Availability {
        if self.0.is_some_and(|horizon| requested <= horizon) {
            Availability::Complete
        } else {
            match beyond {
                Beyond::Wait => Availability::Wait,
                Beyond::Answer => Availability::Marked(self),
            }
        }
    }
}

/// Exact healing posture for one inventory entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Recovery {
    Replay,
    None,
}
