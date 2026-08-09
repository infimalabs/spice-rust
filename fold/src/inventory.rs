use std::fmt;
use std::str::FromStr;

use crate::{Beyond, FactPlane, Source};

/// One fold's static contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Definition {
    pub fold: FoldId,
    pub answer: &'static str,
    pub sources: &'static [Source],
    pub beyond: Beyond,
}

impl Definition {
    /// Exact command that heals this disposable fold.
    #[must_use]
    pub fn recovery_command(self) -> String {
        format!("spice journal replay {}", self.fold)
    }
}

macro_rules! source {
    (Facts($variant:ident => $name:literal)) => {
        Source::Facts(FactPlane::$variant)
    };
}

macro_rules! fold_registry {
    ($(
        $variant:ident => $name:literal {
            answer: $answer:literal;
            sources: [$($source_kind:ident $source_args:tt),* $(,)?];
            beyond: $beyond:ident;
        }
    )+) => {
        /// One registered disposable materialization.
        ///
        /// Declaration order is the stable rebuild order.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum FoldId {
            $($variant),+
        }

        impl FoldId {
            /// Every fold in stable rebuild order.
            pub const ALL: &[Self] = &[$(Self::$variant),+];

            /// The fold's wire name.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),+
                }
            }
        }

        impl FromStr for FoldId {
            type Err = FoldIdError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value {
                    $($name => Ok(Self::$variant),)+
                    _ => Err(FoldIdError(value.to_owned())),
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
                sources: &[
                    $(source!($source_kind $source_args)),*
                ],
                beyond: Beyond::$beyond,
            }),+
        ];
    };
}

/// A fold name outside the closed inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FoldIdError(String);

impl fmt::Display for FoldIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "unknown fold {:?}", self.0)
    }
}

impl std::error::Error for FoldIdError {}

include!("registry.rs");

/// The closed operational fold registry.
#[derive(Clone, Copy, Debug)]
pub struct Inventory;

/// The one inventory value.
pub const INVENTORY: Inventory = Inventory;

impl Inventory {
    /// Every fold in stable rebuild order.
    #[must_use]
    pub const fn definitions(self) -> &'static [Definition] {
        DEFINITIONS
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
