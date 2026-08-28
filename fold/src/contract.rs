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
    /// A closed fact-plane source.
    pub enum FactPlane {
        Task,
        Claim,
        Lane,
        Lineage,
        Journal,
    }
}

impl FactPlane {
    /// True when the fact kind belongs to this declared source.
    #[must_use]
    pub const fn contains(self, kind: wire::FactKind) -> bool {
        match self {
            Self::Task => is_task(kind),
            Self::Claim => is_claim(kind),
            Self::Lane => is_lane(kind),
            Self::Lineage => is_lineage(kind),
            Self::Journal => is_journal(kind),
        }
    }
}

const fn is_task(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::TaskAdded
            | wire::FactKind::TaskPhased
            | wire::FactKind::TaskDeleted
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

const fn is_lane(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::LaneSeated
            | wire::FactKind::LaneArranged
            | wire::FactKind::LaneImported
            | wire::FactKind::LaneRemoved
            | wire::FactKind::LaneSessionDeclared
            | wire::FactKind::LaneStarted
            | wire::FactKind::LaneExited
            | wire::FactKind::LaneKilled
            | wire::FactKind::LaneCompacted
            | wire::FactKind::LaneRenewed
            | wire::FactKind::LaneDrained
            | wire::FactKind::LaneDegraded
            | wire::FactKind::LaneHeld
            | wire::FactKind::LaneResumed
    )
}

const fn is_lineage(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::LineageConfigured | wire::FactKind::LineageArranged
    )
}

const fn is_journal(kind: wire::FactKind) -> bool {
    matches!(
        kind,
        wire::FactKind::JournalMigrated
            | wire::FactKind::JournalBacked
            | wire::FactKind::JournalRestored
            | wire::FactKind::JournalIngested
    )
}

/// One complete, typed input declaration.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Source {
    Facts(FactPlane),
}

impl Source {
    /// Stable inventory spelling used to explain one materialization's inputs.
    #[must_use]
    pub fn provenance(self) -> String {
        match self {
            Self::Facts(plane) => format!("{}::*", plane.as_str()),
        }
    }
}

impl FactPlane {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Claim => "claim",
            Self::Lane => "lane",
            Self::Lineage => "lineage",
            Self::Journal => "journal",
        }
    }
}

/// Policy for a requested position beyond a fold's complete horizon.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Beyond {
    Wait,
    Answer,
}

/// Whether an answer is complete, marked with its horizon, or must wait.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Availability<T> {
    Complete(T),
    Marked { answer: T, horizon: Horizon },
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

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn availability<T>(
        self,
        requested: wire::Seq,
        beyond: Beyond,
        answer: T,
    ) -> Availability<T> {
        if self.0.is_some_and(|horizon| requested <= horizon) {
            Availability::Complete(answer)
        } else {
            match beyond {
                Beyond::Wait => Availability::Wait,
                Beyond::Answer => Availability::Marked {
                    answer,
                    horizon: self,
                },
            }
        }
    }
}
