use std::fmt;

use crate::{
    AnswerContent, Beyond, ConfigSource, FactPlane, HeadSource, Recovery, Source, TapeSource,
};

/// One fold's static contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Definition {
    pub fold: FoldId,
    pub answer: &'static str,
    pub answer_content: &'static [AnswerContent],
    pub sources: &'static [Source],
    pub beyond: Beyond,
    pub recovery: Recovery,
}

impl Definition {
    /// Exact command that heals this disposable fold.
    #[must_use]
    pub fn recovery_command(self) -> Option<String> {
        match self.recovery {
            Recovery::Replay => Some(format!("spice journal replay {}", self.fold)),
            Recovery::None => None,
        }
    }
}

/// One bounded ring that may be named as a non-replayable source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RingDefinition {
    pub ring: RingId,
    pub recovery: Recovery,
}

macro_rules! source {
    (Fact($variant:ident => $name:literal)) => {
        Source::Fact(wire::FactKind::$variant)
    };
    (Facts($variant:ident => $name:literal)) => {
        Source::Facts(FactPlane::$variant)
    };
    (Fold($variant:ident => $name:literal)) => {
        Source::Fold(FoldId::$variant)
    };
    (Head($variant:ident => $name:literal)) => {
        Source::Head(HeadSource::$variant)
    };
    (Ring($variant:ident => $name:literal)) => {
        Source::Ring(RingId::$variant)
    };
    (Config($variant:ident => $name:literal)) => {
        Source::Config(ConfigSource::$variant)
    };
    (Tape($variant:ident => $name:literal)) => {
        Source::Tape(TapeSource::$variant)
    };
    (CommandResults($variant:ident => $name:literal)) => {
        Source::CommandResults
    };
}

macro_rules! answer_content {
    (Fact($variant:ident => $name:literal)) => {
        AnswerContent::Fact(wire::FactKind::$variant)
    };
    (Facts($variant:ident => $name:literal)) => {
        AnswerContent::Facts(FactPlane::$variant)
    };
    (Head($variant:ident => $name:literal)) => {
        AnswerContent::Head(HeadSource::$variant)
    };
}

macro_rules! fold_registry {
    ($(
        $variant:ident => $name:literal {
            answer: $answer:literal;
            content: [$($content_kind:ident($content_variant:ident => $content_name:literal)),* $(,)?];
            sources: [$($source_kind:ident($source_variant:ident => $source_name:literal)),* $(,)?];
            beyond: $beyond:ident;
        }
    )+) => {
        /// One registered disposable materialization.
        ///
        /// Declaration order is dependency-safe rebuild order: a fold appears
        /// after every fold it names as a source.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum FoldId {
            $($variant),+
        }

        impl FoldId {
            /// Every fold in dependency-safe rebuild order.
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            /// The fold's wire name.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),+
                }
            }
        }

        impl fmt::Display for FoldId {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        const DEFINITIONS: &[Definition] = &[
            $(Definition {
                fold: FoldId::$variant,
                answer: $answer,
                answer_content: &[
                    $(answer_content!($content_kind($content_variant => $content_name))),*
                ],
                sources: &[
                    $(source!($source_kind($source_variant => $source_name))),*
                ],
                beyond: Beyond::$beyond,
                recovery: Recovery::Replay,
            }),+
        ];
    };
}

macro_rules! ring_registry {
    ($(
        $variant:ident => $name:literal {
            recovery: $recovery:ident;
        }
    )+) => {
        /// One bounded non-replayable ring source.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum RingId {
            $($variant),+
        }

        impl RingId {
            /// Every bounded ring in registry order.
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            /// The ring's wire name.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),+
                }
            }
        }

        const RINGS: &[RingDefinition] = &[
            $(RingDefinition {
                ring: RingId::$variant,
                recovery: Recovery::$recovery,
            }),+
        ];
    };
}

include!("registry.rs");

/// The closed fold and ring registry.
#[derive(Clone, Copy, Debug)]
pub struct Inventory;

/// The one inventory value.
pub const INVENTORY: Inventory = Inventory;

impl Inventory {
    /// Every fold in dependency-safe rebuild order.
    #[must_use]
    pub const fn definitions(self) -> &'static [Definition] {
        DEFINITIONS
    }

    /// Every admitted bounded ring source.
    #[must_use]
    pub const fn rings(self) -> &'static [RingDefinition] {
        RINGS
    }

    /// Looks up the one contract for a fold.
    #[must_use]
    pub fn definition(self, fold: FoldId) -> &'static Definition {
        DEFINITIONS
            .iter()
            .find(|definition| definition.fold == fold)
            .expect("FoldId::ALL and the fold inventory are generated together")
    }
}
